use std::{
    fs,
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use pitui::{
    app::{
        Action, App, BranchTreeNode, ChangeGroup, ChangesTreeNode, DiffViewMode, FocusKind,
        FocusRole, GlobalMode, PanelId, RepositoryId, ViewId,
    },
    config::KeyStroke,
    domain::{
        BranchName, CommitHash, DiffHunk, DiffLine, DiffLineKind, FileChangeKind, FileDiff,
        GitPath, WorkingTreeDiffKind,
    },
    git::{GitCommandBus, GitRequest, GitResponse, ResetMode, execute_request},
};
use tempfile::TempDir;

static NEXT_TEST_LOG: AtomicU64 = AtomicU64::new(1);

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("Git must be installed for integration tests");
    assert!(
        output.status.success(),
        "git {} failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_may_fail(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("Git must be installed for integration tests")
        .status
        .success()
}

fn repository() -> TempDir {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "user.name", "Pitui Test"]);
    git(
        directory.path(),
        &["config", "user.email", "pitui@example.invalid"],
    );
    directory
}

fn commit_all(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn wait_until_idle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        app.poll_git();
        if app.state.pending_jobs.is_empty() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "Git worker did not become idle: {:?}",
        app.state.pending_jobs
    );
}

fn app_for(paths: &[&Path]) -> App {
    let log_path = std::env::temp_dir()
        .join("pitui-integration-logs")
        .join(format!(
            "{}-{}.jsonl",
            std::process::id(),
            NEXT_TEST_LOG.fetch_add(1, Ordering::Relaxed)
        ));
    App::new(
        GitCommandBus::spawn_with_log_path(log_path).unwrap(),
        paths.iter().map(|path| path.to_path_buf()).collect(),
    )
}

fn open_commit_copy_chord(app: &mut App) {
    app.dispatch(Action::BeginChord(vec![
        KeyStroke::parse("Ctrl+C").unwrap(),
    ]));
}

fn branch_tree_index(app: &App, repository_index: usize, name: &str) -> usize {
    app.state
        .visible_tree_nodes()
        .iter()
        .position(|node| match *node {
            BranchTreeNode::Branch {
                repository_index: index,
                branch_index,
            } if index == repository_index => app
                .state
                .repository_branch(index, branch_index)
                .is_some_and(|branch| branch.name.0 == name),
            _ => false,
        })
        .unwrap_or_else(|| panic!("branch {name} is absent from repository {repository_index}"))
}

fn repository_tree_index(app: &App, repository_index: usize) -> usize {
    app.state
        .visible_tree_nodes()
        .iter()
        .position(|node| *node == BranchTreeNode::Repository { repository_index })
        .unwrap()
}

#[test]
fn loads_repository_branches_commits_details_and_diffs() {
    let repo = repository();
    fs::write(repo.path().join("alpha.txt"), "one\n").unwrap();
    let root_commit = commit_all(repo.path(), "initial");

    fs::write(repo.path().join("alpha.txt"), "one\ntwo\n").unwrap();
    fs::write(repo.path().join("beta.txt"), "beta\n").unwrap();
    let second_commit = commit_all(repo.path(), "add second line");
    git(repo.path(), &["branch", "feature/read-only"]);

    match execute_request(repo.path(), GitRequest::LoadRepositoryStatus) {
        GitResponse::RepositoryStatusLoaded(repository) => {
            assert_eq!(repository.current_branch.unwrap().0, "main");
            assert_eq!(repository.head.0, &second_commit[..8]);
            assert!(repository.status.is_clean());
        }
        response => panic!("unexpected repository response: {response:?}"),
    }

    match execute_request(repo.path(), GitRequest::LoadBranches) {
        GitResponse::BranchesLoaded(branches) => {
            assert_eq!(branches.len(), 2);
            assert!(branches.iter().any(|branch| branch.name.0 == "main"));
            assert!(
                branches
                    .iter()
                    .any(|branch| branch.name.0 == "feature/read-only")
            );
            assert_eq!(
                branches.iter().filter(|branch| branch.is_current).count(),
                1
            );
        }
        response => panic!("unexpected branch response: {response:?}"),
    }

    let commits = match execute_request(
        repo.path(),
        GitRequest::LoadCommits {
            branch: BranchName("main".into()),
            limit: 300,
        },
    ) {
        GitResponse::CommitsLoaded { branch, commits } => {
            assert_eq!(branch.0, "main");
            commits
        }
        response => panic!("unexpected commit response: {response:?}"),
    };
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].hash.0, second_commit);
    assert_eq!(commits[1].hash.0, root_commit);

    let detail = match execute_request(
        repo.path(),
        GitRequest::LoadCommitDetail {
            commit: CommitHash(second_commit.clone()),
        },
    ) {
        GitResponse::CommitDetailLoaded(detail) => detail,
        response => panic!("unexpected detail response: {response:?}"),
    };
    assert_eq!(detail.commit.subject, "add second line");
    assert_eq!(detail.files.len(), 2);
    let alpha = detail
        .files
        .iter()
        .find(|file| file.path.as_str() == "alpha.txt")
        .unwrap();
    assert_eq!(alpha.additions, Some(1));
    assert_eq!(alpha.deletions, Some(0));
    assert_eq!(alpha.hunks.len(), 1);

    match execute_request(
        repo.path(),
        GitRequest::LoadFileDiff {
            commit: CommitHash(second_commit),
            path: GitPath::from("alpha.txt"),
            old_path: None,
        },
    ) {
        GitResponse::FileDiffLoaded(diff) => {
            assert_eq!(diff.hunks.len(), 1);
            assert!(diff.hunks[0].lines.iter().any(|line| line.text == "two"));
        }
        response => panic!("unexpected diff response: {response:?}"),
    }
}

#[test]
fn loads_root_commit_and_rename() {
    let repo = repository();
    fs::write(
        repo.path().join("old-name.txt"),
        "content that remains identical\n",
    )
    .unwrap();
    let root = commit_all(repo.path(), "root");

    let root_detail = match execute_request(
        repo.path(),
        GitRequest::LoadCommitDetail {
            commit: CommitHash(root),
        },
    ) {
        GitResponse::CommitDetailLoaded(detail) => detail,
        response => panic!("unexpected root detail response: {response:?}"),
    };
    assert_eq!(root_detail.files.len(), 1);
    assert!(matches!(root_detail.files[0].kind, FileChangeKind::Added));

    git(repo.path(), &["mv", "old-name.txt", "new-name.txt"]);
    let rename = commit_all(repo.path(), "rename");
    let detail = match execute_request(
        repo.path(),
        GitRequest::LoadCommitDetail {
            commit: CommitHash(rename.clone()),
        },
    ) {
        GitResponse::CommitDetailLoaded(detail) => detail,
        response => panic!("unexpected rename response: {response:?}"),
    };
    assert_eq!(detail.files.len(), 1);
    assert!(matches!(
        detail.files[0].kind,
        FileChangeKind::Renamed { .. }
    ));
    assert_eq!(detail.files[0].path.as_str(), "new-name.txt");
    assert_eq!(
        detail.files[0].old_path.as_ref().unwrap().as_str(),
        "old-name.txt"
    );

    match execute_request(
        repo.path(),
        GitRequest::LoadFileDiff {
            commit: CommitHash(rename),
            path: GitPath::from("new-name.txt"),
            old_path: Some(GitPath::from("old-name.txt")),
        },
    ) {
        GitResponse::FileDiffLoaded(diff) => {
            assert_eq!(diff.path.as_str(), "new-name.txt");
            assert_eq!(diff.old_path.unwrap().as_str(), "old-name.txt");
        }
        response => panic!("unexpected rename diff response: {response:?}"),
    }
}

#[test]
fn counts_staged_modified_untracked_and_conflicted_entries() {
    let repo = repository();
    fs::write(repo.path().join("conflict.txt"), "base\n").unwrap();
    fs::write(repo.path().join("modified.txt"), "clean\n").unwrap();
    commit_all(repo.path(), "base");

    fs::write(repo.path().join("modified.txt"), "dirty\n").unwrap();
    fs::write(repo.path().join("staged.txt"), "staged\n").unwrap();
    git(repo.path(), &["add", "staged.txt"]);
    fs::write(repo.path().join("untracked.txt"), "untracked\n").unwrap();
    match execute_request(repo.path(), GitRequest::LoadRepositoryStatus) {
        GitResponse::RepositoryStatusLoaded(repository) => {
            assert_eq!(repository.status.staged, 1);
            assert_eq!(repository.status.modified, 1);
            assert_eq!(repository.status.untracked, 1);
            assert_eq!(repository.status.conflicted, 0);
        }
        response => panic!("unexpected status response: {response:?}"),
    }

    git(repo.path(), &["reset", "--hard", "HEAD"]);
    let _ = fs::remove_file(repo.path().join("untracked.txt"));
    git(repo.path(), &["switch", "-c", "other"]);
    fs::write(repo.path().join("conflict.txt"), "other\n").unwrap();
    commit_all(repo.path(), "other change");
    git(repo.path(), &["switch", "main"]);
    fs::write(repo.path().join("conflict.txt"), "main\n").unwrap();
    commit_all(repo.path(), "main change");
    assert!(!git_may_fail(repo.path(), &["merge", "other"]));

    match execute_request(repo.path(), GitRequest::LoadRepositoryStatus) {
        GitResponse::RepositoryStatusLoaded(repository) => {
            assert_eq!(repository.status.conflicted, 1);
        }
        response => panic!("unexpected conflict status response: {response:?}"),
    }

    let conflict = match execute_request(repo.path(), GitRequest::LoadWorkingTree) {
        GitResponse::WorkingTreeLoaded(changes) => changes
            .into_iter()
            .find(|change| change.path.as_str() == "conflict.txt")
            .unwrap(),
        response => panic!("unexpected conflicted worktree response: {response:?}"),
    };
    assert!(conflict.is_conflicted());
    let include_staged = conflict.has_staged_changes();
    let include_worktree = conflict.has_worktree_changes();
    match execute_request(
        repo.path(),
        GitRequest::LoadWorkingTreeDiff {
            path: conflict.path,
            old_path: conflict.old_path,
            include_staged,
            include_worktree,
            untracked: false,
        },
    ) {
        GitResponse::WorkingTreeDiffLoaded(diff) => {
            assert!(!diff.sections.is_empty());
            assert!(
                diff.sections
                    .iter()
                    .any(|section| !section.lines.is_empty())
            );
        }
        response => panic!("unexpected conflicted worktree diff response: {response:?}"),
    }
}

#[test]
fn loads_working_tree_files_and_index_worktree_untracked_diffs() {
    let repo = repository();
    fs::write(repo.path().join("staged.txt"), "base\n").unwrap();
    fs::write(repo.path().join("modified.txt"), "base\n").unwrap();
    fs::write(repo.path().join("both.txt"), "base\n").unwrap();
    commit_all(repo.path(), "base");

    fs::write(repo.path().join("staged.txt"), "base\nstaged line\n").unwrap();
    git(repo.path(), &["add", "staged.txt"]);
    fs::write(
        repo.path().join("modified.txt"),
        "base\nworking tree line\n",
    )
    .unwrap();
    fs::write(repo.path().join("untracked.txt"), "untracked line\n").unwrap();
    fs::write(repo.path().join("both.txt"), "base\nindex line\n").unwrap();
    git(repo.path(), &["add", "both.txt"]);
    fs::write(
        repo.path().join("both.txt"),
        "base\nindex line\nworktree line\n",
    )
    .unwrap();

    let changes = match execute_request(repo.path(), GitRequest::LoadWorkingTree) {
        GitResponse::WorkingTreeLoaded(changes) => changes,
        response => panic!("unexpected working tree response: {response:?}"),
    };
    assert_eq!(changes.len(), 4);
    let staged = changes
        .iter()
        .find(|change| change.path.as_str() == "staged.txt")
        .unwrap();
    assert!(staged.has_staged_changes());
    assert!(!staged.has_worktree_changes());
    let modified = changes
        .iter()
        .find(|change| change.path.as_str() == "modified.txt")
        .unwrap();
    assert!(!modified.has_staged_changes());
    assert!(modified.has_worktree_changes());
    let untracked = changes
        .iter()
        .find(|change| change.path.as_str() == "untracked.txt")
        .unwrap();
    assert!(untracked.is_untracked());
    let both = changes
        .iter()
        .find(|change| change.path.as_str() == "both.txt")
        .unwrap();
    assert!(both.has_staged_changes());
    assert!(both.has_worktree_changes());

    for (change, expected_kind, expected_line) in [
        (staged, WorkingTreeDiffKind::Staged, "+staged line"),
        (
            modified,
            WorkingTreeDiffKind::Worktree,
            "+working tree line",
        ),
        (untracked, WorkingTreeDiffKind::Untracked, "+untracked line"),
    ] {
        match execute_request(
            repo.path(),
            GitRequest::LoadWorkingTreeDiff {
                path: change.path.clone(),
                old_path: change.old_path.clone(),
                include_staged: change.has_staged_changes(),
                include_worktree: change.has_worktree_changes(),
                untracked: change.is_untracked(),
            },
        ) {
            GitResponse::WorkingTreeDiffLoaded(diff) => {
                assert_eq!(diff.sections.len(), 1);
                assert_eq!(diff.sections[0].kind, expected_kind);
                assert!(
                    diff.sections[0]
                        .lines
                        .iter()
                        .any(|line| line == expected_line)
                );
            }
            response => panic!("unexpected working tree diff response: {response:?}"),
        }
    }

    match execute_request(
        repo.path(),
        GitRequest::LoadWorkingTreeDiff {
            path: both.path.clone(),
            old_path: None,
            include_staged: true,
            include_worktree: true,
            untracked: false,
        },
    ) {
        GitResponse::WorkingTreeDiffLoaded(diff) => {
            assert_eq!(diff.sections.len(), 2);
            assert_eq!(diff.sections[0].kind, WorkingTreeDiffKind::Staged);
            assert_eq!(diff.sections[1].kind, WorkingTreeDiffKind::Worktree);
            assert!(
                diff.sections[0]
                    .lines
                    .iter()
                    .any(|line| line == "+index line")
            );
            assert!(
                diff.sections[1]
                    .lines
                    .iter()
                    .any(|line| line == "+worktree line")
            );
        }
        response => panic!("unexpected two-section worktree diff: {response:?}"),
    }

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::ToggleChanges);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::Changes);
    assert_eq!(app.state.view_projection().focused, PanelId::Changes);
    assert_eq!(app.state.working_tree_changes().len(), 4);
    assert!(app.state.current_changes_diff.is_some());
    let nodes = app.state.visible_changes_nodes();
    assert_eq!(nodes[0], ChangesTreeNode::Root);
    assert_eq!(nodes[1], ChangesTreeNode::Group(ChangeGroup::Staged));
    assert_eq!(app.state.change_group_count(ChangeGroup::Staged), 2);
    assert_eq!(app.state.change_group_count(ChangeGroup::Unstaged), 3);
    assert_eq!(
        nodes
            .iter()
            .filter(|node| matches!(node, ChangesTreeNode::File { .. }))
            .count(),
        5
    );
    assert_eq!(
        app.state.current_changes_diff_group,
        Some(ChangeGroup::Staged)
    );
    let selected_diff = app.state.current_changes_diff.as_ref().unwrap();
    assert!(
        selected_diff
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .any(|line| line.text == "index line" || line.text == "staged line")
    );
    app.dispatch(Action::Back);
    assert_eq!(app.state.view_projection().view, ViewId::History);
}

#[test]
fn stages_unstages_and_commits_paths_without_touching_worktree_content() {
    let repo = repository();
    fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
    commit_all(repo.path(), "base");

    fs::write(repo.path().join("tracked.txt"), "base\nchanged\n").unwrap();
    fs::write(repo.path().join("odd -- file.txt"), "new\n").unwrap();

    match execute_request(
        repo.path(),
        GitRequest::StagePaths {
            paths: vec![GitPath::from("tracked.txt")],
        },
    ) {
        GitResponse::CommandSucceeded { .. } => {}
        response => panic!("unexpected stage response: {response:?}"),
    }
    assert_eq!(
        git(repo.path(), &["diff", "--cached", "--name-only"]),
        "tracked.txt"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "base\nchanged\n"
    );

    match execute_request(
        repo.path(),
        GitRequest::UnstagePaths {
            paths: vec![GitPath::from("tracked.txt")],
        },
    ) {
        GitResponse::CommandSucceeded { .. } => {}
        response => panic!("unexpected unstage response: {response:?}"),
    }
    assert!(git(repo.path(), &["diff", "--cached", "--name-only"]).is_empty());
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "base\nchanged\n"
    );

    match execute_request(
        repo.path(),
        GitRequest::StagePaths {
            paths: vec![
                GitPath::from("tracked.txt"),
                GitPath::from("odd -- file.txt"),
            ],
        },
    ) {
        GitResponse::CommandSucceeded { .. } => {}
        response => panic!("unexpected second stage response: {response:?}"),
    }
    match execute_request(
        repo.path(),
        GitRequest::Commit {
            message: "create from Changes".into(),
        },
    ) {
        GitResponse::CommandSucceeded { message } => assert_eq!(message, "Commit created"),
        response => panic!("unexpected commit response: {response:?}"),
    }
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]),
        "create from Changes"
    );
    assert!(git(repo.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn unstaging_is_safe_in_an_unborn_repository() {
    let repo = repository();
    fs::write(repo.path().join("first.txt"), "first\n").unwrap();
    let path = GitPath::from("first.txt");

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::StagePaths {
                paths: vec![path.clone()]
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert!(git(repo.path(), &["status", "--short"]).starts_with("A "));
    assert!(matches!(
        execute_request(repo.path(), GitRequest::UnstagePaths { paths: vec![path] }),
        GitResponse::CommandSucceeded { .. }
    ));
    assert!(git(repo.path(), &["status", "--short"]).starts_with("??"));
    assert_eq!(
        fs::read_to_string(repo.path().join("first.txt")).unwrap(),
        "first\n"
    );
}

#[test]
fn changes_controller_multiselects_stages_unstages_and_creates_a_commit() {
    let repo = repository();
    fs::write(repo.path().join("one.txt"), "one\n").unwrap();
    fs::write(repo.path().join("two.txt"), "two\n").unwrap();
    commit_all(repo.path(), "base");
    fs::write(repo.path().join("one.txt"), "one changed\n").unwrap();
    fs::write(repo.path().join("two.txt"), "two changed\n").unwrap();

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::ToggleChanges);
    wait_until_idle(&mut app);
    assert_eq!(app.state.change_group_count(ChangeGroup::Unstaged), 2);
    assert!(matches!(
        app.state.selected_changes_node(),
        Some(ChangesTreeNode::File {
            group: ChangeGroup::Unstaged,
            ..
        })
    ));

    // Selection and stage work while the reusable diff panel has focus.
    app.dispatch(Action::FocusNext);
    assert_eq!(app.state.view_projection().focused, PanelId::ChangesDiff);
    app.dispatch(Action::ToggleChangeSelection);
    assert_eq!(app.state.change_selection.len(), 1);
    app.dispatch(Action::StageSelectedChanges);
    wait_until_idle(&mut app);
    assert_eq!(app.state.change_group_count(ChangeGroup::Staged), 1);
    assert_eq!(app.state.change_group_count(ChangeGroup::Unstaged), 1);
    assert!(app.state.change_selection.is_empty());

    // With no explicit selection, unstage applies to the currently displayed
    // staged file and never changes its working-tree content.
    app.dispatch(Action::UnstageSelectedChanges);
    wait_until_idle(&mut app);
    assert_eq!(app.state.change_group_count(ChangeGroup::Staged), 0);
    assert_eq!(app.state.change_group_count(ChangeGroup::Unstaged), 2);

    // Space on a group selects every child, then one command stages all of it.
    let unstaged_group = app
        .state
        .visible_changes_nodes()
        .iter()
        .position(|node| *node == ChangesTreeNode::Group(ChangeGroup::Unstaged))
        .unwrap();
    app.state.selection.selected_changes_index = Some(unstaged_group);
    app.state
        .set_focus_layer(FocusKind::Changes, FocusRole::Entity);
    app.dispatch(Action::ToggleChangeSelection);
    assert_eq!(app.state.selected_change_count(ChangeGroup::Unstaged), 2);
    app.dispatch(Action::StageSelectedChanges);
    wait_until_idle(&mut app);
    assert_eq!(app.state.change_group_count(ChangeGroup::Staged), 2);
    assert_eq!(app.state.change_group_count(ChangeGroup::Unstaged), 0);

    app.dispatch(Action::OpenCommitDialog);
    assert!(matches!(
        app.state.mode,
        GlobalMode::EditingCommitMessage { .. }
    ));
    app.dispatch(Action::SubmitCommit);
    assert!(matches!(
        app.state.mode,
        GlobalMode::EditingCommitMessage {
            validation_error: Some(_),
            ..
        }
    ));
    app.dispatch(Action::UpdateCommitMessage(
        "commit created in Changes".into(),
    ));
    app.dispatch(Action::SubmitCommit);
    wait_until_idle(&mut app);

    assert_eq!(app.state.mode, GlobalMode::Normal);
    assert_eq!(app.state.view_projection().view, ViewId::Changes);
    assert_eq!(app.state.change_group_count(ChangeGroup::Staged), 0);
    assert_eq!(app.state.change_group_count(ChangeGroup::Unstaged), 0);
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]),
        "commit created in Changes"
    );
}

#[test]
fn mutating_commands_are_executed_safely_in_a_temporary_repository() {
    let repo = repository();
    fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    let base = commit_all(repo.path(), "base");
    git(repo.path(), &["switch", "-c", "feature"]);
    fs::write(repo.path().join("feature.txt"), "feature\n").unwrap();
    let feature_commit = commit_all(repo.path(), "feature work");

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::SwitchBranch {
                branch: BranchName("main".into())
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert_eq!(git(repo.path(), &["branch", "--show-current"]), "main");

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::CherryPick {
                commits: vec![CommitHash(feature_commit)]
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert!(repo.path().join("feature.txt").exists());
    let cherry_picked = git(repo.path(), &["rev-parse", "HEAD"]);

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::Reset {
                commit: CommitHash(base.clone()),
                mode: ResetMode::Soft
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), base);
    assert_eq!(
        git(repo.path(), &["diff", "--cached", "--name-only"]),
        "feature.txt"
    );

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::Reset {
                commit: CommitHash(cherry_picked.clone()),
                mode: ResetMode::Mixed
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), cherry_picked);
    assert!(git(repo.path(), &["status", "--porcelain"]).is_empty());

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::Reset {
                commit: CommitHash(base.clone()),
                mode: ResetMode::Hard
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), base);
    assert!(!repo.path().join("feature.txt").exists());
}

#[test]
fn non_repository_returns_a_structured_error() {
    let directory = tempfile::tempdir().unwrap();
    match execute_request(directory.path(), GitRequest::LoadRepositoryStatus) {
        GitResponse::CommandFailed { command, stderr } => {
            assert!(command.contains("rev-parse"));
            assert!(!stderr.is_empty());
        }
        response => panic!("expected error, got {response:?}"),
    }
}

#[test]
fn unborn_repository_is_a_valid_empty_repository() {
    let repo = repository();
    match execute_request(repo.path(), GitRequest::LoadRepositoryStatus) {
        GitResponse::RepositoryStatusLoaded(repository) => {
            assert_eq!(repository.current_branch.unwrap().0, "main");
            assert!(repository.head.0.is_empty());
            assert!(repository.status.is_clean());
        }
        response => panic!("unexpected unborn repository response: {response:?}"),
    }

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    assert_eq!(app.state.visible_tree_nodes().len(), 2);
    assert_eq!(app.state.model.branch_summaries(RepositoryId(0)).len(), 1);
    assert_eq!(app.state.repository_branch(0, 0).unwrap().name.0, "main");
    assert_eq!(
        app.state.repository_branch(0, 0).unwrap().short_head,
        "unborn"
    );
    let main_index = branch_tree_index(&app, 0, "main");
    app.dispatch(Action::SelectBranch(main_index));
    app.dispatch(Action::LoadCommitsForSelectedBranch);
    wait_until_idle(&mut app);
    assert!(app.state.branch_commit_summaries().is_empty());
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    assert_eq!(app.state.mode, GlobalMode::Normal);
}

#[test]
fn detached_head_can_still_load_commits() {
    let repo = repository();
    fs::write(repo.path().join("detached.txt"), "content\n").unwrap();
    let head = commit_all(repo.path(), "detached target");
    git(repo.path(), &["switch", "--detach", "HEAD"]);

    match execute_request(repo.path(), GitRequest::LoadRepositoryStatus) {
        GitResponse::RepositoryStatusLoaded(repository) => {
            assert!(repository.current_branch.is_none());
            assert_eq!(repository.head.0, &head[..8]);
        }
        response => panic!("unexpected detached repository response: {response:?}"),
    }
    match execute_request(
        repo.path(),
        GitRequest::LoadCommits {
            branch: BranchName("HEAD".into()),
            limit: 300,
        },
    ) {
        GitResponse::CommitsLoaded { commits, .. } => {
            assert_eq!(commits.len(), 1);
            assert_eq!(commits[0].hash.0, head);
        }
        response => panic!("unexpected detached commit response: {response:?}"),
    }
}

#[test]
fn identifies_binary_file_changes() {
    let repo = repository();
    fs::write(repo.path().join("image.bin"), [0, 1, 2, 3]).unwrap();
    commit_all(repo.path(), "binary base");
    fs::write(repo.path().join("image.bin"), [0, 1, 9, 3]).unwrap();
    let commit = commit_all(repo.path(), "binary update");

    let detail = match execute_request(
        repo.path(),
        GitRequest::LoadCommitDetail {
            commit: CommitHash(commit.clone()),
        },
    ) {
        GitResponse::CommitDetailLoaded(detail) => detail,
        response => panic!("unexpected binary detail response: {response:?}"),
    };
    assert_eq!(detail.files.len(), 1);
    assert!(detail.files[0].is_binary);
    assert_eq!(detail.files[0].additions, None);
    assert_eq!(detail.files[0].deletions, None);

    match execute_request(
        repo.path(),
        GitRequest::LoadFileDiff {
            commit: CommitHash(commit),
            path: detail.files[0].path.clone(),
            old_path: None,
        },
    ) {
        GitResponse::FileDiffLoaded(diff) => assert!(diff.is_binary),
        response => panic!("unexpected binary diff response: {response:?}"),
    }
}

#[test]
fn application_controller_drives_the_three_view_workflow_and_modals() {
    let repo = repository();
    fs::write(repo.path().join("app.txt"), "first\n").unwrap();
    commit_all(repo.path(), "first");
    fs::write(repo.path().join("app.txt"), "first\nsecond\n").unwrap();
    commit_all(repo.path(), "second");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    assert!(app.state.active_repository().is_some());
    assert_eq!(app.state.model.branch_summaries(RepositoryId(0)).len(), 1);
    assert_eq!(app.state.branch_commit_summaries().len(), 2);
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );

    app.dispatch(Action::OpenCommandPrompt);
    assert!(matches!(
        app.state.mode,
        GlobalMode::CommandPrompt {
            ref input,
            validation_error: None
        } if input.is_empty()
    ));
    assert!(!matches!(app.state.mode, GlobalMode::Normal));
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );
    app.dispatch(Action::SubmitCommandPrompt);
    assert!(matches!(
        app.state.mode,
        GlobalMode::CommandPrompt {
            validation_error: Some(_),
            ..
        }
    ));
    app.dispatch(Action::UpdateCommandPrompt("help".into()));
    assert!(matches!(
        app.state.mode,
        GlobalMode::CommandPrompt {
            validation_error: None,
            ..
        }
    ));
    app.dispatch(Action::SubmitCommandPrompt);
    assert!(matches!(
        app.state.mode,
        GlobalMode::ShortcutHelp { scroll: 0 }
    ));
    assert!(!matches!(app.state.mode, GlobalMode::Normal));
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );
    app.dispatch(Action::PageDown);
    assert!(matches!(
        app.state.mode,
        GlobalMode::ShortcutHelp { scroll: 10 }
    ));
    app.dispatch(Action::Cancel);
    assert_eq!(app.state.mode, GlobalMode::Normal);
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );

    app.dispatch(Action::FocusNext);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    app.dispatch(Action::OpenCommitDetail);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    assert_eq!(app.state.current_commit_detail().unwrap().files.len(), 1);

    app.dispatch(Action::ToggleFileExpanded);
    assert_eq!(app.state.expansion.expanded_files.len(), 1);
    app.dispatch(Action::OpenSelectedFileDiff);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::FileDiff);
    assert_eq!(app.state.view_projection().focused, PanelId::FileDiff);
    assert!(app.state.current_file_diff().is_some());
    app.dispatch(Action::ToggleDiffMode);
    assert_eq!(app.state.diff_mode, DiffViewMode::SideBySide);
    app.dispatch(Action::ToggleWrap);
    assert!(app.state.wrap_diff);

    // Changes is a global destination, not a child of Branch Overview. It can
    // be opened from a commit diff and returns to that exact screen/focus.
    app.dispatch(Action::ToggleChanges);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::Changes);
    app.dispatch(Action::Back);
    assert_eq!(app.state.view_projection().view, ViewId::FileDiff);
    assert_eq!(app.state.view_projection().focused, PanelId::FileDiff);
    assert!(app.state.current_file_diff().is_some());

    app.dispatch(Action::Back);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    app.dispatch(Action::FocusNext);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    app.dispatch(Action::ToggleCommitSelection);
    app.dispatch(Action::MoveDown);
    app.dispatch(Action::ToggleCommitSelection);
    assert_eq!(app.state.commit_selection.len(), 2);
    let expected_cherry_pick = app
        .state
        .branch_commit_summaries()
        .iter()
        .rev()
        .filter(|commit| app.state.commit_selection.contains(&commit.hash))
        .map(|commit| commit.hash.clone())
        .collect::<Vec<_>>();
    app.dispatch(Action::OpenCherryPickSelectedDialog);
    assert!(matches!(
        &app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::CherryPickSelected { commits, .. }
        } if commits == &expected_cherry_pick
    ));
    assert!(!matches!(app.state.mode, GlobalMode::Normal));
    app.dispatch(Action::Cancel);
    assert_eq!(app.state.mode, GlobalMode::Normal);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);

    open_commit_copy_chord(&mut app);
    assert!(matches!(app.state.mode, GlobalMode::Chord { .. }));
    app.dispatch(Action::CopySelectedCommitHashes);
    assert_eq!(app.state.mode, GlobalMode::Normal);
    let expected_hashes = app
        .state
        .branch_commit_summaries()
        .iter()
        .map(|commit| commit.hash.0.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        app.take_clipboard_request().as_deref(),
        Some(expected_hashes.as_str())
    );
    open_commit_copy_chord(&mut app);
    app.dispatch(Action::CopyCurrentCommitInfo);
    let info = app.take_clipboard_request().unwrap();
    assert!(info.contains(&app.state.selected_commit().unwrap().hash.0));
    assert!(info.contains("Author:"));

    app.dispatch(Action::OpenResetDialog);
    assert!(matches!(app.state.mode, GlobalMode::Confirming { .. }));
    app.dispatch(Action::ChooseResetHard);
    assert!(matches!(app.state.mode, GlobalMode::Confirming { .. }));
    app.dispatch(Action::Confirm);
    assert!(matches!(
        app.state.mode,
        GlobalMode::TypingConfirmation { .. }
    ));
    app.dispatch(Action::UpdateTypedConfirmation("wrong".into()));
    app.dispatch(Action::ConfirmReset);
    assert!(matches!(
        app.state.mode,
        GlobalMode::TypingConfirmation {
            validation_error: Some(_),
            ..
        }
    ));
    app.dispatch(Action::Cancel);
    assert_eq!(app.state.mode, GlobalMode::Normal);

    app.dispatch(Action::Back);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    app.dispatch(Action::StartFilter);
    app.dispatch(Action::UpdateFilter("second".into()));
    assert_eq!(app.state.visible_commits().len(), 1);
    app.dispatch(Action::SubmitFilter);
    assert_eq!(app.state.commit_filter, "second");
}

#[test]
fn operation_palette_invokes_the_same_focus_scoped_operation_without_changing_focus() {
    let repo = repository();
    fs::write(repo.path().join("palette.txt"), "content\n").unwrap();
    commit_all(repo.path(), "palette operation");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::FocusNext);
    let focus_before = app.state.focus_context();

    app.dispatch(Action::OpenOperationPalette);
    assert_eq!(app.state.focus_context(), focus_before);
    app.dispatch(Action::UpdateOperationPalette(
        "commit.toggle_selection".into(),
    ));
    app.dispatch(Action::SubmitOperationPalette);

    assert_eq!(app.state.mode, GlobalMode::Normal);
    assert_eq!(app.state.focus_context(), focus_before);
    assert_eq!(app.state.commit_selection.len(), 1);
    assert!(
        app.state
            .commit_selection
            .contains(&app.state.selected_commit().unwrap().hash)
    );
}

#[test]
fn horizontal_navigation_slides_columns_through_branch_commit_file_and_diff_levels() {
    let repo = repository();
    fs::write(repo.path().join("app.txt"), "first\nsecond\n").unwrap();
    commit_all(repo.path(), "hierarchical navigation");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );

    // Branches | Commits: move into the right column, then slide it to the
    // left of Commits | Commit without wrapping back to Branches.
    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);

    // Returning before the worker answers must keep the older screen; a stale
    // response may not reopen the deeper level.
    app.dispatch(Action::MoveLeft);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);

    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    assert!(app.state.current_commit_detail().is_some());

    // Commits | Commit: enter the reused Commit column, then slide the whole
    // metadata + files column to the left of Commit | Diff.
    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().view, ViewId::FileDiff);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);

    app.dispatch(Action::MoveLeft);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);

    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().view, ViewId::FileDiff);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    assert!(app.state.current_file_diff().is_some());

    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().focused, PanelId::FileDiff);
    app.dispatch(Action::MoveRight);
    assert_eq!(app.state.view_projection().view, ViewId::FileDiff);
    assert_eq!(app.state.view_projection().focused, PanelId::FileDiff);

    // Left is the exact inverse: move within a pair, then slide the current
    // left column back into the previous screen's right column.
    app.dispatch(Action::MoveLeft);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    app.dispatch(Action::MoveLeft);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    app.dispatch(Action::MoveLeft);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    app.dispatch(Action::MoveLeft);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    app.dispatch(Action::MoveLeft);
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );
    app.dispatch(Action::MoveLeft);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );
}

#[test]
fn repository_status_waits_for_manual_refresh_instead_of_polling_on_tick() {
    let repo = repository();
    fs::write(repo.path().join("tracked.txt"), "clean\n").unwrap();
    commit_all(repo.path(), "initial");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    assert!(app.state.active_repository().unwrap().status.is_clean());

    fs::write(repo.path().join("tracked.txt"), "changed outside pitui\n").unwrap();
    thread::sleep(Duration::from_millis(2_100));

    let tick_before = app.state.tick_count;
    app.dispatch(Action::Tick);
    assert_eq!(app.state.tick_count, tick_before + 1);
    assert!(app.state.pending_jobs.is_empty());
    assert!(app.state.active_repository().unwrap().status.is_clean());

    app.dispatch(Action::RefreshRepository);
    assert!(!app.state.pending_jobs.is_empty());
    wait_until_idle(&mut app);
    assert_eq!(app.state.active_repository().unwrap().status.modified, 1);
}

#[test]
fn every_file_detail_panel_supports_home_end_and_page_navigation() {
    let repo = repository();
    for index in 0..15 {
        fs::write(
            repo.path().join(format!("file-{index:02}.txt")),
            format!("line {index}\n"),
        )
        .unwrap();
    }
    commit_all(repo.path(), "add many files");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::FocusNext);
    app.dispatch(Action::OpenCommitDetail);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    assert_eq!(app.state.current_commit_detail().unwrap().files.len(), 15);

    app.dispatch(Action::PageDown);
    assert_eq!(app.state.selection.selected_file_index, Some(10));
    app.dispatch(Action::PageUp);
    assert_eq!(app.state.selection.selected_file_index, Some(0));
    app.dispatch(Action::End);
    assert_eq!(app.state.selection.selected_file_index, Some(14));
    app.dispatch(Action::Home);
    assert_eq!(app.state.selection.selected_file_index, Some(0));

    app.dispatch(Action::OpenSelectedFileDiff);
    wait_until_idle(&mut app);
    app.dispatch(Action::FocusNext);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    app.dispatch(Action::PageDown);
    assert_eq!(app.state.selection.selected_file_index, Some(10));
    assert_eq!(
        app.state.pending_jobs.len(),
        1,
        "paging the file list must load only the final file diff"
    );
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    app.dispatch(Action::PageUp);
    assert_eq!(app.state.selection.selected_file_index, Some(0));
    wait_until_idle(&mut app);
    app.dispatch(Action::End);
    assert_eq!(app.state.selection.selected_file_index, Some(14));
    wait_until_idle(&mut app);
    app.dispatch(Action::Home);
    assert_eq!(app.state.selection.selected_file_index, Some(0));
    wait_until_idle(&mut app);

    let long_diff = FileDiff {
        commit: CommitHash("synthetic".into()),
        path: GitPath::from("file.txt"),
        old_path: None,
        header: vec!["diff --git a/file.txt b/file.txt".into()],
        hunks: vec![DiffHunk {
            header: "@@ -1,40 +1,40 @@".into(),
            old_start: 1,
            old_count: 40,
            new_start: 1,
            new_count: 40,
            lines: (1..=40)
                .map(|line| DiffLine {
                    old_line_no: Some(line),
                    new_line_no: Some(line),
                    kind: DiffLineKind::Context,
                    text: format!("line {line}"),
                })
                .collect(),
        }],
        is_binary: false,
    };
    let last_line = 41;

    let mut long_diff = long_diff;
    long_diff.commit = app.state.current_commit_id().unwrap().hash;
    long_diff.path = app.state.selected_file().unwrap().path.clone();
    app.state
        .model
        .set_file_diff(RepositoryId(0), long_diff.clone());
    app.state
        .set_focus_layer(FocusKind::Diff, FocusRole::Content);
    app.state.selection.diff_scroll = 0;
    app.dispatch(Action::PageDown);
    assert_eq!(app.state.selection.diff_scroll, 10);
    app.dispatch(Action::End);
    assert_eq!(app.state.selection.diff_scroll, last_line);
    app.dispatch(Action::PageUp);
    assert_eq!(app.state.selection.diff_scroll, last_line - 10);
    app.dispatch(Action::Home);
    assert_eq!(app.state.selection.diff_scroll, 0);

    app.state.current_changes_diff = Some(long_diff);
    app.state
        .set_focus_layer(FocusKind::ChangesDiff, FocusRole::Content);
    app.state.selection.changes_diff_scroll = 0;
    app.dispatch(Action::PageDown);
    assert_eq!(app.state.selection.changes_diff_scroll, 10);
    app.dispatch(Action::End);
    assert_eq!(app.state.selection.changes_diff_scroll, last_line);
    app.dispatch(Action::PageUp);
    assert_eq!(app.state.selection.changes_diff_scroll, last_line - 10);
    app.dispatch(Action::Home);
    assert_eq!(app.state.selection.changes_diff_scroll, 0);
}

#[test]
fn file_diff_navigation_keeps_file_list_focus_and_copies_full_commit_message() {
    let repo = repository();
    fs::write(repo.path().join("alpha.txt"), "alpha\n").unwrap();
    fs::write(repo.path().join("beta.txt"), "beta\n").unwrap();
    git(repo.path(), &["add", "-A"]);
    git(
        repo.path(),
        &[
            "commit",
            "-m",
            "multi-file subject",
            "-m",
            "body line one\nbody line two",
        ],
    );
    let expected_message = "multi-file subject\n\nbody line one\nbody line two";

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::FocusNext);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);

    // Overview only has the subject, so message copying loads the full body
    // without navigating away from the current screen or focus.
    open_commit_copy_chord(&mut app);
    app.dispatch(Action::CopyCurrentCommitMessage);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::History);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    assert_eq!(
        app.take_clipboard_request().as_deref(),
        Some(expected_message)
    );

    app.dispatch(Action::OpenCommitDetail);
    wait_until_idle(&mut app);
    assert_eq!(app.state.current_commit_detail().unwrap().files.len(), 2);
    app.dispatch(Action::OpenSelectedFileDiff);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().focused, PanelId::FileDiff);

    app.dispatch(Action::FocusNext);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    app.dispatch(Action::MoveDown);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    assert_eq!(
        app.state.current_file_diff().unwrap().path,
        app.state.selected_file().unwrap().path
    );

    let selected_path = app.state.selected_file().unwrap().path.clone();
    open_commit_copy_chord(&mut app);
    app.dispatch(Action::CopySelectedFileName);
    assert_eq!(
        app.take_clipboard_request().as_deref(),
        Some(
            Path::new(selected_path.as_str())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
    );
    open_commit_copy_chord(&mut app);
    app.dispatch(Action::CopySelectedFileRelativePath);
    assert_eq!(
        app.take_clipboard_request().as_deref(),
        Some(selected_path.as_str())
    );
    open_commit_copy_chord(&mut app);
    app.dispatch(Action::CopySelectedFileAbsolutePath);
    assert_eq!(
        app.take_clipboard_request().as_deref(),
        Some(
            fs::canonicalize(repo.path())
                .unwrap()
                .join(selected_path.as_str())
                .to_string_lossy()
                .as_ref()
        )
    );

    app.dispatch(Action::MoveUp);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    assert_eq!(
        app.state.current_file_diff().unwrap().path,
        app.state.selected_file().unwrap().path
    );
}

#[test]
fn application_surfaces_and_dismisses_non_repository_errors() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = app_for(&[directory.path()]);
    wait_until_idle(&mut app);
    assert!(matches!(app.state.mode, GlobalMode::Error));
    assert!(!matches!(app.state.mode, GlobalMode::Normal));
    assert!(app.state.last_error.is_some());

    app.dispatch(Action::DismissError);
    assert_eq!(app.state.mode, GlobalMode::Normal);
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );
    assert!(app.state.last_error.is_none());
}

#[test]
fn latest_branch_request_wins_over_stale_worker_responses() {
    let repo = repository();
    fs::write(repo.path().join("branch.txt"), "content\n").unwrap();
    commit_all(repo.path(), "base");
    git(repo.path(), &["branch", "feature"]);

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    let main_index = branch_tree_index(&app, 0, "main");
    let feature_index = branch_tree_index(&app, 0, "feature");

    // Queue two reads without polling between them. The serial worker returns
    // both, but only the response associated with the latest id may be applied.
    app.dispatch(Action::SelectBranch(main_index));
    app.dispatch(Action::LoadCommitsForSelectedBranch);
    app.dispatch(Action::SelectBranch(feature_index));
    app.dispatch(Action::LoadCommitsForSelectedBranch);
    wait_until_idle(&mut app);

    assert_eq!(app.state.viewing_branch_id().unwrap().name.0, "feature");
}

#[test]
fn branch_arrow_navigation_automatically_previews_the_selected_branch() {
    let repo = repository();
    fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    commit_all(repo.path(), "base");
    git(repo.path(), &["switch", "-c", "feature"]);
    fs::write(repo.path().join("feature.txt"), "feature\n").unwrap();
    commit_all(repo.path(), "feature tip");
    git(repo.path(), &["switch", "main"]);
    fs::write(repo.path().join("main.txt"), "main\n").unwrap();
    commit_all(repo.path(), "main tip");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    let main_index = branch_tree_index(&app, 0, "main");
    let feature_index = branch_tree_index(&app, 0, "feature");
    assert_eq!(main_index.abs_diff(feature_index), 1);

    app.state
        .set_focus_layer(FocusKind::Branch, FocusRole::Entity);
    app.dispatch(Action::SelectBranch(main_index));
    wait_until_idle(&mut app);
    assert_eq!(app.state.viewing_branch_id().unwrap().name.0, "main");

    app.dispatch(if feature_index < main_index {
        Action::MoveUp
    } else {
        Action::MoveDown
    });
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );
    wait_until_idle(&mut app);

    assert_eq!(app.state.selected_branch().unwrap().name.0, "feature");
    assert_eq!(app.state.viewing_branch_id().unwrap().name.0, "feature");
    assert_eq!(app.state.selected_commit().unwrap().subject, "feature tip");
    assert_eq!(
        app.state.view_projection().focused,
        PanelId::RepositoryBranches
    );
}

#[test]
fn commit_arrow_navigation_automatically_previews_detail_without_stealing_focus() {
    let repo = repository();
    fs::write(repo.path().join("first.txt"), "first\n").unwrap();
    commit_all(repo.path(), "first commit");
    fs::write(repo.path().join("second.txt"), "second\n").unwrap();
    commit_all(repo.path(), "second commit");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.state
        .set_focus_layer(FocusKind::Commit, FocusRole::Collection);
    app.dispatch(Action::OpenCommitDetail);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::Commit);
    assert_eq!(app.state.view_projection().focused, PanelId::Commit);
    assert_eq!(
        app.state.current_commit_detail().unwrap().commit.subject,
        "second commit"
    );

    app.state
        .set_focus_layer(FocusKind::Commit, FocusRole::Entity);
    app.dispatch(Action::MoveDown);
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
    assert!(app.state.current_commit_detail().is_none());
    assert!(app.state.latest_commit_detail_job.is_some());
    wait_until_idle(&mut app);

    let selected = app.state.selected_commit().unwrap().hash.clone();
    assert_eq!(
        app.state.current_commit_detail().unwrap().commit.hash,
        selected
    );
    assert_eq!(
        app.state.current_commit_detail().unwrap().commit.subject,
        "first commit"
    );
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);

    // Queue two previews without polling. Only the final selection may update
    // the right pane when responses arrive.
    app.dispatch(Action::MoveUp);
    app.dispatch(Action::MoveDown);
    wait_until_idle(&mut app);
    let selected = app.state.selected_commit().unwrap().hash.clone();
    assert_eq!(
        app.state.current_commit_detail().unwrap().commit.hash,
        selected
    );
    assert_eq!(app.state.view_projection().focused, PanelId::Commits);
}

#[test]
fn application_confirms_switch_cherry_pick_and_typed_reset_end_to_end() {
    let repo = repository();
    fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    commit_all(repo.path(), "base");
    git(repo.path(), &["switch", "-c", "feature"]);
    fs::write(repo.path().join("feature.txt"), "feature\n").unwrap();
    let feature_commit = commit_all(repo.path(), "feature work");
    git(repo.path(), &["switch", "main"]);

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);

    let feature_index = branch_tree_index(&app, 0, "feature");
    app.dispatch(Action::SelectBranch(feature_index));
    app.dispatch(Action::OpenSwitchBranchDialog);
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);
    assert_eq!(git(repo.path(), &["branch", "--show-current"]), "feature");

    let main_index = branch_tree_index(&app, 0, "main");
    app.dispatch(Action::SelectBranch(main_index));
    app.dispatch(Action::OpenSwitchBranchDialog);
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);
    assert_eq!(git(repo.path(), &["branch", "--show-current"]), "main");

    let feature_index = branch_tree_index(&app, 0, "feature");
    app.dispatch(Action::SelectBranch(feature_index));
    app.dispatch(Action::LoadCommitsForSelectedBranch);
    wait_until_idle(&mut app);
    assert_eq!(app.state.selected_commit().unwrap().hash.0, feature_commit);

    app.dispatch(Action::OpenCherryPickSelectedDialog);
    assert_eq!(app.state.mode, GlobalMode::Normal);
    app.dispatch(Action::ToggleCommitSelection);
    app.dispatch(Action::OpenCherryPickSelectedDialog);
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);
    assert!(app.state.commit_selection.is_empty());
    assert!(repo.path().join("feature.txt").exists());
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]),
        "feature work"
    );

    let expected = app.state.selected_commit().unwrap().short_hash.clone();
    app.dispatch(Action::OpenResetDialog);
    app.dispatch(Action::ChooseResetHard);
    app.dispatch(Action::Confirm);
    app.dispatch(Action::UpdateTypedConfirmation(expected));
    app.dispatch(Action::ConfirmReset);
    wait_until_idle(&mut app);
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), feature_commit);
    assert_eq!(app.state.mode, GlobalMode::Normal);
    assert_eq!(app.state.view_projection().view, ViewId::History);
}

#[test]
fn application_loads_and_navigates_multiple_repository_trees() {
    let first = repository();
    fs::write(first.path().join("first.txt"), "first\n").unwrap();
    commit_all(first.path(), "first repository commit");
    git(first.path(), &["branch", "first-feature"]);

    let second = repository();
    fs::write(second.path().join("second.txt"), "second\n").unwrap();
    commit_all(second.path(), "second repository commit");
    git(second.path(), &["branch", "second-feature"]);

    let mut app = app_for(&[first.path(), second.path()]);
    wait_until_idle(&mut app);

    assert_eq!(app.state.repository_count(), 2);
    assert!((0..2).all(|index| app.state.repository(index).is_some()));
    assert!(
        (0..2).all(|index| { app.state.model.branch_summaries(RepositoryId(index)).len() == 2 })
    );
    assert_eq!(
        app.state
            .visible_tree_nodes()
            .iter()
            .filter(|node| matches!(node, BranchTreeNode::Repository { .. }))
            .count(),
        2
    );
    assert_eq!(
        app.state.branch_commit_summaries()[0].subject,
        "first repository commit"
    );

    let second_repository = repository_tree_index(&app, 1);
    app.dispatch(Action::SelectBranch(second_repository));
    wait_until_idle(&mut app);
    assert_eq!(app.state.active_repository_index, Some(1));
    assert_eq!(app.state.viewing_repository_index(), Some(1));
    assert_eq!(
        app.state.branch_commit_summaries()[0].subject,
        "second repository commit"
    );

    app.dispatch(Action::LoadCommitsForSelectedBranch);
    assert!(!app.state.repository_ui[1].expanded);
    assert_eq!(
        app.state
            .visible_tree_nodes()
            .iter()
            .filter(|node| node.repository_index() == 1)
            .count(),
        1
    );

    let first_feature = branch_tree_index(&app, 0, "first-feature");
    app.dispatch(Action::SelectBranch(first_feature));
    app.dispatch(Action::LoadCommitsForSelectedBranch);
    wait_until_idle(&mut app);
    assert_eq!(app.state.active_repository_index, Some(0));
    assert_eq!(
        app.state.viewing_branch_id().unwrap().name.0,
        "first-feature"
    );
    assert_eq!(
        app.state.branch_commit_summaries()[0].subject,
        "first repository commit"
    );
}

#[test]
fn selected_repository_fetches_its_remote_and_refreshes_tree() {
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-b", "main"]);

    let source = repository();
    fs::write(source.path().join("shared.txt"), "base\n").unwrap();
    commit_all(source.path(), "remote base");
    let remote_path = remote.path().to_string_lossy().into_owned();
    git(source.path(), &["remote", "add", "origin", &remote_path]);
    git(source.path(), &["push", "-u", "origin", "main"]);

    let clone_parent = tempfile::tempdir().unwrap();
    git(
        clone_parent.path(),
        &["clone", &remote_path, "fetch-target"],
    );
    let target = clone_parent.path().join("fetch-target");
    let before = git(&target, &["rev-parse", "refs/remotes/origin/main"]);

    fs::write(source.path().join("shared.txt"), "base\nnew\n").unwrap();
    let new_head = commit_all(source.path(), "remote update");
    git(source.path(), &["push", "origin", "main"]);
    assert_ne!(before, new_head);

    let unrelated = repository();
    fs::write(unrelated.path().join("local.txt"), "local\n").unwrap();
    commit_all(unrelated.path(), "unrelated");

    let mut app = app_for(&[unrelated.path(), &target]);
    wait_until_idle(&mut app);
    let target_repository = repository_tree_index(&app, 1);
    app.dispatch(Action::SelectBranch(target_repository));
    app.dispatch(Action::OpenFetchRepositoryDialog);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::FetchRepository {
                repository_index: 1
            }
        }
    ));
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    assert_eq!(
        git(&target, &["rev-parse", "refs/remotes/origin/main"]),
        new_head
    );
    assert_eq!(
        git(unrelated.path(), &["log", "-1", "--format=%s"]),
        "unrelated"
    );
    assert_eq!(app.state.last_message.as_deref(), Some("Fetch completed"));
    assert!(
        app.state
            .model
            .branch_summaries(RepositoryId(1))
            .iter()
            .any(|branch| branch.name.0 == "origin/main" && branch.head.0 == new_head)
    );
}

#[test]
fn remote_management_adds_a_shared_url_and_sets_branch_upstream() {
    let repo = repository();
    fs::write(repo.path().join("tracked.txt"), "content\n").unwrap();
    let local_head = commit_all(repo.path(), "initial");

    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-b", "main"]);
    let remote_url = remote.path().to_string_lossy().into_owned();

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::OpenRemotes);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::Remotes);
    assert_eq!(app.state.view_projection().focused, PanelId::Remotes);
    assert!(app.state.remotes().is_empty());

    app.dispatch(Action::OpenAddRemoteEditor);
    app.dispatch(Action::UpdateRemoteName("origin".into()));
    app.dispatch(Action::SubmitRemoteEditor);
    assert!(matches!(
        app.state.mode,
        GlobalMode::EditingRemote {
            field: pitui::app::RemoteInputField::Url,
            ..
        }
    ));
    app.dispatch(Action::UpdateRemoteUrl(remote_url.clone()));
    app.dispatch(Action::SubmitRemoteEditor);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::AddRemote {
                ref name,
                ref url,
                ..
            }
        } if name == "origin" && url == &remote_url
    ));
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    assert_eq!(app.state.remotes().len(), 1);
    let origin = &app.state.remotes()[0];
    assert_eq!(origin.name, "origin");
    assert_eq!(origin.fetch_urls, vec![remote_url.clone()]);
    assert_eq!(origin.push_urls, origin.fetch_urls);
    assert!(origin.urls_match());
    assert!(!origin.is_upstream);

    app.dispatch(Action::OpenSetUpstreamRemoteDialog);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::SetUpstreamRemote {
                ref name,
                ref branch,
                ..
            }
        } if name == "origin" && branch.0 == "main"
    ));
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    let origin = &app.state.remotes()[0];
    assert!(origin.is_upstream);
    assert!(origin.is_push_target);
    assert_eq!(
        git(repo.path(), &["config", "--get", "branch.main.remote"]),
        "origin"
    );
    assert_eq!(
        git(repo.path(), &["config", "--get", "branch.main.pushRemote"]),
        "origin"
    );
    assert_eq!(
        git(repo.path(), &["config", "--get", "branch.main.merge"]),
        "refs/heads/main"
    );

    app.dispatch(Action::Back);
    app.dispatch(Action::OpenPushDialog);
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);
    assert_eq!(
        git(remote.path(), &["rev-parse", "refs/heads/main"]),
        local_head
    );
}

#[test]
fn remote_policy_blocks_split_urls_and_split_branch_routing_until_repaired() {
    let repo = repository();
    fs::write(repo.path().join("tracked.txt"), "content\n").unwrap();
    let local_head = commit_all(repo.path(), "initial");

    let fetch_remote = tempfile::tempdir().unwrap();
    git(fetch_remote.path(), &["init", "--bare", "-b", "main"]);
    let push_remote = tempfile::tempdir().unwrap();
    git(push_remote.path(), &["init", "--bare", "-b", "main"]);
    let fetch_url = fetch_remote.path().to_string_lossy().into_owned();
    let push_url = push_remote.path().to_string_lossy().into_owned();
    git(repo.path(), &["remote", "add", "origin", &fetch_url]);
    git(
        repo.path(),
        &["config", "--add", "remote.origin.pushurl", &push_url],
    );

    let loaded = execute_request(repo.path(), GitRequest::LoadRemotes);
    let GitResponse::RemotesLoaded(remotes) = loaded else {
        panic!("unexpected remote response: {loaded:?}");
    };
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].fetch_urls, vec![fetch_url.clone()]);
    assert_eq!(remotes[0].push_urls, vec![push_url]);
    assert!(!remotes[0].urls_match());

    for request in [GitRequest::Fetch, GitRequest::PullRebase, GitRequest::Push] {
        match execute_request(repo.path(), request) {
            GitResponse::CommandFailed { stderr, .. } => {
                assert!(stderr.contains("identical fetch and push URLs"));
            }
            response => panic!("split URL operation should be blocked, got {response:?}"),
        }
    }
    assert!(!git_may_fail(
        push_remote.path(),
        &["rev-parse", "refs/heads/main"]
    ));

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::SetRemoteUrl {
                name: "origin".into(),
                url: fetch_url.clone(),
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert!(!git_may_fail(
        repo.path(),
        &["config", "--get-all", "remote.origin.pushurl"]
    ));
    let GitResponse::RemotesLoaded(remotes) = execute_request(repo.path(), GitRequest::LoadRemotes)
    else {
        panic!("repaired remote should load");
    };
    assert!(remotes[0].urls_match());

    git(repo.path(), &["remote", "add", "mirror", &fetch_url]);
    git(repo.path(), &["config", "branch.main.remote", "origin"]);
    git(
        repo.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    git(repo.path(), &["config", "branch.main.pushRemote", "mirror"]);
    match execute_request(repo.path(), GitRequest::Push) {
        GitResponse::CommandFailed { stderr, .. } => {
            assert!(stderr.contains("fetches from `origin` but pushes to `mirror`"));
        }
        response => panic!("split branch routing should be blocked, got {response:?}"),
    }

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::SetUpstreamRemote {
                name: "origin".into(),
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert!(matches!(
        execute_request(repo.path(), GitRequest::Push),
        GitResponse::CommandSucceeded { .. }
    ));
    assert_eq!(
        git(fetch_remote.path(), &["rev-parse", "refs/heads/main"]),
        local_head
    );
}

#[test]
fn repository_pull_rebases_local_commits_and_pushes_current_branch() {
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-b", "main"]);
    let remote_path = remote.path().to_string_lossy().into_owned();

    let source = repository();
    fs::write(source.path().join("base.txt"), "base\n").unwrap();
    commit_all(source.path(), "remote base");
    git(source.path(), &["remote", "add", "origin", &remote_path]);
    git(source.path(), &["push", "-u", "origin", "main"]);

    let clone_parent = tempfile::tempdir().unwrap();
    git(clone_parent.path(), &["clone", &remote_path, "sync-target"]);
    let target = clone_parent.path().join("sync-target");
    git(&target, &["config", "user.name", "Pitui Test"]);
    git(&target, &["config", "user.email", "pitui@example.invalid"]);

    fs::write(target.join("local.txt"), "local\n").unwrap();
    let local_before_rebase = commit_all(&target, "local ahead");
    fs::write(source.path().join("remote.txt"), "remote\n").unwrap();
    let remote_head = commit_all(source.path(), "remote ahead");
    git(source.path(), &["push", "origin", "main"]);

    let mut app = app_for(&[&target]);
    wait_until_idle(&mut app);
    app.dispatch(Action::OpenPullRebaseDialog);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::PullRebaseRepository {
                repository_index: 0,
                ref branch,
            }
        } if branch.0 == "main"
    ));
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    let rebased_head = git(&target, &["rev-parse", "HEAD"]);
    assert_ne!(rebased_head, local_before_rebase);
    assert_eq!(git(&target, &["rev-parse", "HEAD^"]), remote_head);
    assert_eq!(git(&target, &["log", "-1", "--format=%s"]), "local ahead");
    assert!(
        git(
            &target,
            &["rev-list", "--merges", &format!("{remote_head}..HEAD")]
        )
        .is_empty()
    );
    assert_eq!(app.state.mode, GlobalMode::Normal);

    let repository_index = repository_tree_index(&app, 0);
    app.dispatch(Action::SelectBranch(repository_index));
    app.dispatch(Action::OpenPushDialog);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::PushRepository {
                repository_index: 0,
                ref branch,
            }
        } if branch.0 == "main"
    ));
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    assert_eq!(
        git(remote.path(), &["rev-parse", "refs/heads/main"]),
        rebased_head
    );
    assert_eq!(app.state.last_message.as_deref(), Some("Push completed"));
}

#[test]
fn pull_rebase_refuses_dirty_state_before_contacting_a_remote() {
    let repo = repository();
    fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
    let head = commit_all(repo.path(), "base");
    fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();

    match execute_request(repo.path(), GitRequest::PullRebase) {
        GitResponse::CommandFailed { command, stderr } => {
            assert_eq!(command, "git pull --rebase");
            assert!(stderr.contains("clean working tree"));
        }
        response => panic!("dirty pull --rebase should be rejected, got {response:?}"),
    }

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::OpenPullRebaseDialog);
    assert!(matches!(app.state.mode, GlobalMode::Error));
    assert!(
        app.state
            .last_error
            .as_ref()
            .unwrap()
            .message
            .contains("clean working tree")
    );
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn pull_rebase_conflict_is_reported_and_automatically_aborted() {
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare", "-b", "main"]);
    let remote_path = remote.path().to_string_lossy().into_owned();

    let source = repository();
    fs::write(source.path().join("conflict.txt"), "base\n").unwrap();
    commit_all(source.path(), "remote base");
    git(source.path(), &["remote", "add", "origin", &remote_path]);
    git(source.path(), &["push", "-u", "origin", "main"]);

    let clone_parent = tempfile::tempdir().unwrap();
    git(
        clone_parent.path(),
        &["clone", &remote_path, "conflict-target"],
    );
    let target = clone_parent.path().join("conflict-target");
    git(&target, &["config", "user.name", "Pitui Test"]);
    git(&target, &["config", "user.email", "pitui@example.invalid"]);

    fs::write(target.join("conflict.txt"), "local\n").unwrap();
    let local_head = commit_all(&target, "local conflict");
    fs::write(source.path().join("conflict.txt"), "remote\n").unwrap();
    let remote_head = commit_all(source.path(), "remote conflict");
    git(source.path(), &["push", "origin", "main"]);

    let mut app = app_for(&[&target]);
    wait_until_idle(&mut app);
    app.dispatch(Action::OpenPullRebaseDialog);
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    assert!(matches!(app.state.mode, GlobalMode::Error));
    let error = app.state.last_error.as_ref().unwrap();
    assert_eq!(error.command, "git pull --rebase");
    assert!(error.message.contains("Pull --rebase stopped"));
    assert!(
        error
            .message
            .contains("automatically ran `git rebase --abort`")
    );
    assert_eq!(git(&target, &["rev-parse", "HEAD"]), local_head);
    assert_eq!(
        fs::read_to_string(target.join("conflict.txt")).unwrap(),
        "local\n"
    );
    assert!(git(&target, &["status", "--porcelain"]).is_empty());
    assert_eq!(
        git(&target, &["rev-parse", "refs/remotes/origin/main"]),
        remote_head
    );
    assert!(!target.join(".git/rebase-merge").exists());
    assert!(!target.join(".git/rebase-apply").exists());
}

#[test]
fn application_browses_reflog_and_resets_to_a_selected_entry() {
    let repo = repository();
    fs::write(repo.path().join("history.txt"), "one\n").unwrap();
    let first = commit_all(repo.path(), "first reflog target");
    fs::write(repo.path().join("history.txt"), "one\ntwo\n").unwrap();
    let second = commit_all(repo.path(), "second reflog target");

    let loaded = execute_request(repo.path(), GitRequest::LoadReflog { limit: 300 });
    match loaded {
        GitResponse::ReflogLoaded(entries) => {
            assert!(entries.iter().any(|entry| entry.hash.0 == first));
            assert!(entries.iter().any(|entry| entry.hash.0 == second));
            assert!(entries.iter().any(|entry| entry.action == "commit"));
        }
        response => panic!("unexpected reflog response: {response:?}"),
    }

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    app.dispatch(Action::OpenReflog);
    app.dispatch(Action::Back);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::History);

    app.dispatch(Action::OpenReflog);
    wait_until_idle(&mut app);
    assert_eq!(app.state.view_projection().view, ViewId::Reflog);
    assert_eq!(app.state.view_projection().focused, PanelId::Reflog);
    assert_eq!(app.state.reflog_repository_index, Some(0));

    let first_index = app
        .state
        .reflog_entries()
        .iter()
        .position(|entry| entry.hash.0 == first)
        .unwrap();
    app.state.selection.selected_reflog_index = Some(first_index);
    app.dispatch(Action::OpenResetDialog);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::ResetModeChoice { .. }
        }
    ));
    app.dispatch(Action::ChooseResetSoft);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::Reset {
                mode: ResetMode::Soft,
                ..
            }
        }
    ));
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), first);
    assert_eq!(
        git(repo.path(), &["diff", "--cached", "--name-only"]),
        "history.txt"
    );
    assert_eq!(app.state.view_projection().view, ViewId::History);
}

#[test]
fn safe_rebase_succeeds_when_clean() {
    let repo = repository();
    fs::write(repo.path().join("base.txt"), "base\n").unwrap();
    let base = commit_all(repo.path(), "base");

    git(repo.path(), &["switch", "-c", "upstream"]);
    fs::write(repo.path().join("upstream.txt"), "upstream\n").unwrap();
    let upstream_head = commit_all(repo.path(), "upstream work");

    git(repo.path(), &["switch", "-c", "feature", &base]);
    fs::write(repo.path().join("feature.txt"), "feature\n").unwrap();
    commit_all(repo.path(), "feature work");

    assert!(matches!(
        execute_request(
            repo.path(),
            GitRequest::Rebase {
                upstream: BranchName("upstream".into())
            }
        ),
        GitResponse::CommandSucceeded { .. }
    ));
    assert_eq!(git(repo.path(), &["branch", "--show-current"]), "feature");
    assert!(git_may_fail(
        repo.path(),
        &["merge-base", "--is-ancestor", &upstream_head, "HEAD"]
    ));
    assert_eq!(
        git(repo.path(), &["log", "-1", "--format=%s"]),
        "feature work"
    );
}

#[test]
fn safe_rebase_refuses_a_dirty_working_tree_before_running_git() {
    let repo = repository();
    fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
    let head = commit_all(repo.path(), "base");
    git(repo.path(), &["branch", "upstream"]);
    fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();

    match execute_request(
        repo.path(),
        GitRequest::Rebase {
            upstream: BranchName("upstream".into()),
        },
    ) {
        GitResponse::CommandFailed { stderr, .. } => {
            assert!(stderr.contains("clean working tree"));
        }
        response => panic!("dirty rebase should be rejected, got {response:?}"),
    }

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    let upstream_index = branch_tree_index(&app, 0, "upstream");
    app.dispatch(Action::SelectBranch(upstream_index));
    app.dispatch(Action::OpenRebaseDialog);

    assert!(matches!(app.state.mode, GlobalMode::Error));
    assert!(
        app.state
            .last_error
            .as_ref()
            .unwrap()
            .message
            .contains("clean working tree")
    );
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
        "dirty\n"
    );
}

#[test]
fn safe_rebase_conflict_is_reported_and_automatically_aborted() {
    let repo = repository();
    fs::write(repo.path().join("conflict.txt"), "base\n").unwrap();
    let base = commit_all(repo.path(), "base");

    git(repo.path(), &["switch", "-c", "upstream"]);
    fs::write(repo.path().join("conflict.txt"), "upstream\n").unwrap();
    commit_all(repo.path(), "upstream conflict");

    git(repo.path(), &["switch", "-c", "feature", &base]);
    fs::write(repo.path().join("conflict.txt"), "feature\n").unwrap();
    let feature_head = commit_all(repo.path(), "feature conflict");

    let mut app = app_for(&[repo.path()]);
    wait_until_idle(&mut app);
    let upstream_index = branch_tree_index(&app, 0, "upstream");
    app.dispatch(Action::SelectBranch(upstream_index));
    app.dispatch(Action::OpenRebaseDialog);
    assert!(matches!(
        app.state.mode,
        GlobalMode::Confirming {
            dialog: pitui::app::ConfirmDialog::Rebase { .. }
        }
    ));
    app.dispatch(Action::Confirm);
    wait_until_idle(&mut app);

    assert!(matches!(app.state.mode, GlobalMode::Error));
    assert!(
        app.state
            .last_error
            .as_ref()
            .unwrap()
            .message
            .contains("automatically ran `git rebase --abort`")
    );
    assert_eq!(git(repo.path(), &["branch", "--show-current"]), "feature");
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), feature_head);
    assert!(git(repo.path(), &["status", "--porcelain"]).is_empty());
    assert!(!repo.path().join(".git/rebase-merge").exists());
    assert!(!repo.path().join(".git/rebase-apply").exists());
}

#[test]
fn safe_rebase_never_aborts_a_preexisting_rebase() {
    let repo = repository();
    fs::write(repo.path().join("conflict.txt"), "base\n").unwrap();
    let base = commit_all(repo.path(), "base");

    git(repo.path(), &["switch", "-c", "upstream"]);
    fs::write(repo.path().join("conflict.txt"), "upstream\n").unwrap();
    commit_all(repo.path(), "upstream conflict");

    git(repo.path(), &["switch", "-c", "feature", &base]);
    fs::write(repo.path().join("conflict.txt"), "feature\n").unwrap();
    let feature_head = commit_all(repo.path(), "feature conflict");

    assert!(!git_may_fail(repo.path(), &["rebase", "upstream"]));
    assert!(
        repo.path().join(".git/rebase-merge").exists()
            || repo.path().join(".git/rebase-apply").exists()
    );

    match execute_request(
        repo.path(),
        GitRequest::Rebase {
            upstream: BranchName("upstream".into()),
        },
    ) {
        GitResponse::CommandFailed { stderr, .. } => {
            assert!(stderr.contains("already in progress"));
            assert!(stderr.contains("left it untouched"));
        }
        response => panic!("preexisting rebase should be preserved, got {response:?}"),
    }

    assert!(
        repo.path().join(".git/rebase-merge").exists()
            || repo.path().join(".git/rebase-apply").exists()
    );
    assert!(!git(repo.path(), &["diff", "--name-only", "--diff-filter=U"]).is_empty());

    git(repo.path(), &["rebase", "--abort"]);
    assert_eq!(git(repo.path(), &["branch", "--show-current"]), "feature");
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), feature_head);
}

#[test]
fn backend_jsonl_log_records_every_job_lifecycle_and_failure() {
    let repo = repository();
    let log_directory = tempfile::tempdir().unwrap();
    let log_path = log_directory.path().join("backend.jsonl");
    let mut bus = GitCommandBus::spawn_with_log_path(log_path.clone()).unwrap();

    let load_job = bus.submit(repo.path().to_path_buf(), GitRequest::LoadWorkingTree);
    let failed_job = bus.submit(
        repo.path().to_path_buf(),
        GitRequest::StagePaths { paths: Vec::new() },
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut responses = Vec::new();
    while responses.len() < 2 && Instant::now() < deadline {
        if let Ok(response) = bus.try_recv() {
            responses.push(response);
        } else {
            thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(responses.len(), 2);
    assert!(responses.iter().any(|response| {
        response.id == load_job && matches!(response.response, GitResponse::WorkingTreeLoaded(_))
    }));
    assert!(responses.iter().any(|response| {
        response.id == failed_job && matches!(response.response, GitResponse::CommandFailed { .. })
    }));

    // The worker writes and flushes the completion record before publishing
    // each response, so the log is safe to inspect while the bus is alive.
    let log = fs::read_to_string(log_path).unwrap();
    assert!(log.contains("\"event\":\"session_started\""));
    assert!(log.contains("\"operation\":\"load_working_tree\""));
    assert!(log.contains("\"operation\":\"stage_paths\""));
    assert!(log.contains("\"status\":\"success\""));
    assert!(log.contains("\"status\":\"failure\""));
    assert!(log.contains("No files were selected for staging"));

    for job_id in [load_job, failed_job] {
        for event in ["queued", "started", "completed"] {
            assert!(log.lines().any(|line| {
                line.contains(&format!("\"job_id\":{job_id}"))
                    && line.contains(&format!("\"event\":\"{event}\""))
            }));
        }
    }
}

// APFS rejects invalid UTF-8 names at creation time; Unix filesystems such as
// ext4 accept them and exercise the full argv round trip.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn preserves_non_utf8_git_paths_for_follow_up_diff_commands() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let repo = repository();
    let raw_name = b"non-utf8-\xff.txt".to_vec();
    let os_name = OsString::from_vec(raw_name.clone());
    fs::write(repo.path().join(&os_name), "content\n").unwrap();
    let commit = commit_all(repo.path(), "non utf8 path");

    let detail = match execute_request(
        repo.path(),
        GitRequest::LoadCommitDetail {
            commit: CommitHash(commit.clone()),
        },
    ) {
        GitResponse::CommitDetailLoaded(detail) => detail,
        response => panic!("unexpected non-UTF8 detail response: {response:?}"),
    };
    assert_eq!(detail.files.len(), 1);
    assert_eq!(detail.files[0].path.as_bytes(), raw_name);

    match execute_request(
        repo.path(),
        GitRequest::LoadFileDiff {
            commit: CommitHash(commit),
            path: detail.files[0].path.clone(),
            old_path: None,
        },
    ) {
        GitResponse::FileDiffLoaded(diff) => {
            assert_eq!(diff.path.as_bytes(), raw_name);
            assert_eq!(diff.hunks.len(), 1);
        }
        response => panic!("unexpected non-UTF8 diff response: {response:?}"),
    }
}
