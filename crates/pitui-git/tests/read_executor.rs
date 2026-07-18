use std::{fs, path::Path, process::Command};

use pitui_core::{BranchName, CommitHash, GitPath};
use pitui_git::{CliGitExecutor, GitCommand, GitExecutor, ParsedGitPayload};

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn synchronous_executor_loads_the_read_only_vertical_chain() {
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "-b", "main"]);
    git(repository.path(), &["config", "user.name", "Pitui Test"]);
    git(
        repository.path(),
        &["config", "user.email", "pitui@example.invalid"],
    );
    fs::write(repository.path().join("file.txt"), "one\n").unwrap();
    git(repository.path(), &["add", "file.txt"]);
    git(repository.path(), &["commit", "-m", "initial"]);
    fs::write(repository.path().join("file.txt"), "one\ntwo\n").unwrap();
    git(repository.path(), &["add", "file.txt"]);
    git(repository.path(), &["commit", "-m", "second"]);
    let head = CommitHash(git(repository.path(), &["rev-parse", "HEAD"]));

    let executor = CliGitExecutor;
    let root = executor
        .execute(repository.path(), &GitCommand::LoadRepository)
        .unwrap();
    assert!(matches!(
        root,
        ParsedGitPayload::Repository(repository) if repository.current_branch == Some(BranchName("main".into()))
    ));

    let branches = executor
        .execute(repository.path(), &GitCommand::LoadBranches)
        .unwrap();
    assert!(matches!(
        branches,
        ParsedGitPayload::Branches(branches) if branches.iter().any(|branch| branch.is_current)
    ));

    let commits = executor
        .execute(
            repository.path(),
            &GitCommand::LoadCommits {
                branch: BranchName("main".into()),
                limit: 50,
            },
        )
        .unwrap();
    assert!(matches!(
        commits,
        ParsedGitPayload::Commits { commits, .. } if commits.len() == 2
    ));

    let reflog = executor
        .execute(repository.path(), &GitCommand::LoadReflog { limit: 50 })
        .unwrap();
    assert!(matches!(
        reflog,
        ParsedGitPayload::Reflog(entries)
            if entries.len() >= 2
                && entries[0].selector.ends_with("@{0}")
                && !entries[0].authored_at.is_empty()
    ));

    let detail = executor
        .execute(
            repository.path(),
            &GitCommand::LoadCommitDetail {
                commit: head.clone(),
            },
        )
        .unwrap();
    assert!(matches!(
        detail,
        ParsedGitPayload::CommitDetail(detail)
            if detail.commit.hash == head && detail.files.len() == 1
    ));

    let diff = executor
        .execute(
            repository.path(),
            &GitCommand::LoadFileDiff {
                commit: head,
                path: GitPath::from("file.txt"),
                old_path: None,
            },
        )
        .unwrap();
    assert!(matches!(
        diff,
        ParsedGitPayload::FileDiff(diff) if !diff.hunks.is_empty()
    ));

    fs::write(repository.path().join("file.txt"), "one\ntwo\nstaged\n").unwrap();
    git(repository.path(), &["add", "file.txt"]);
    fs::write(
        repository.path().join("file.txt"),
        "one\ntwo\nstaged\nunstaged\n",
    )
    .unwrap();
    fs::write(repository.path().join("new.txt"), "untracked\n").unwrap();
    let changes = executor
        .execute(repository.path(), &GitCommand::LoadWorkingTree)
        .unwrap();
    assert!(matches!(
        changes,
        ParsedGitPayload::WorkingTree(changes)
            if changes.iter().any(|change| change.has_staged_changes() && change.has_worktree_changes())
                && changes.iter().any(|change| change.is_untracked())
    ));

    for (include_staged, include_worktree, untracked, path, expected) in [
        (true, false, false, "file.txt", "staged"),
        (false, true, false, "file.txt", "unstaged"),
        (false, false, true, "new.txt", "untracked"),
    ] {
        let payload = executor
            .execute(
                repository.path(),
                &GitCommand::LoadWorkingTreeDiff {
                    path: GitPath::from(path),
                    old_path: None,
                    include_staged,
                    include_worktree,
                    untracked,
                },
            )
            .unwrap();
        assert!(matches!(
            payload,
            ParsedGitPayload::WorkingTreeDiff(diff)
                if diff.sections.len() == 1
                    && diff.sections[0].lines.iter().any(|line| line.contains(expected))
        ));
    }
}
