use std::{fs, path::Path, process::Command};

use pitui_core::{BranchName, CommitHash, GitPath};
use pitui_data::{
    ChangeBoundary, CommitMetadata, DatasetChildren, DatasetCollection, DatasetIdentity,
    DatasetIndex, DatasetKind, DatasetRevision, DatasetTemplate, DatasetTemplateId,
    FileChangesMetadata, HasSnapshot, RepositoryKey, RepositoryMetadata,
    WorkingTreeFileChangesMetadata,
};
use pitui_ecs::{
    DatasetRuntime, GitCommandData, GitExecutionFailures, GitLoadKey, GitLoadStatus, GitLoadTarget,
    GitLoadTracker, GitRequestId,
};
use pitui_git::GitCommand;

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

fn register_default(runtime: &mut DatasetRuntime, id: &str, kind: DatasetKind) {
    runtime
        .register_default_template(DatasetTemplate {
            id: DatasetTemplateId::from(id),
            kind,
            collection: pitui_data::CollectionManagerSpec::default(),
            views: Vec::new(),
            operations: Vec::new(),
            hotkeys: Default::default(),
            render_proxies: Vec::new(),
        })
        .unwrap();
}

fn enqueue(
    runtime: &mut DatasetRuntime,
    repository_dataset: bevy_ecs::prelude::Entity,
    cwd: &Path,
    command: GitCommand,
) -> GitRequestId {
    let request_id = runtime
        .enqueue_git_command(GitCommandData {
            repository_dataset,
            cwd: cwd.to_path_buf(),
            command,
        })
        .unwrap();
    runtime.run_schedule();
    request_id
}

#[test]
fn git_messages_build_the_canonical_dataset_chain_and_keep_cache_on_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    git(&cwd, &["init", "-b", "main"]);
    git(&cwd, &["config", "user.name", "Pitui Test"]);
    git(&cwd, &["config", "user.email", "pitui@example.invalid"]);
    fs::write(cwd.join("file.txt"), "one\n").unwrap();
    git(&cwd, &["add", "file.txt"]);
    git(&cwd, &["commit", "-m", "initial"]);
    let shared_hash = CommitHash(git(&cwd, &["rev-parse", "HEAD"]));
    git(&cwd, &["branch", "feature"]);
    fs::write(cwd.join("file.txt"), "one\ntwo\n").unwrap();
    git(&cwd, &["add", "file.txt"]);
    git(&cwd, &["commit", "-m", "second"]);
    let head = CommitHash(git(&cwd, &["rev-parse", "HEAD"]));

    let mut runtime = DatasetRuntime::new();
    for (id, kind) in [
        ("repository", DatasetKind::Repository),
        ("branch", DatasetKind::Branch),
        ("commits", DatasetKind::Commits),
        ("commit", DatasetKind::Commit),
        ("commit-field", DatasetKind::CommitField),
        ("files", DatasetKind::Files),
        ("file-tree-directory", DatasetKind::FileTreeDirectory),
        ("file", DatasetKind::File),
        ("file-changes", DatasetKind::FileChanges),
    ] {
        register_default(&mut runtime, id, kind);
    }
    let repository = RepositoryKey::new(cwd.clone());
    let repository_dataset = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(repository.clone()),
            DatasetKind::Repository,
            DatasetTemplateId::from("repository"),
        )
        .unwrap();
    runtime.add_root(repository_dataset).unwrap();

    let repository_request = enqueue(
        &mut runtime,
        repository_dataset,
        &cwd,
        GitCommand::LoadRepository,
    );
    assert_eq!(
        runtime
            .world()
            .get::<RepositoryMetadata>(repository_dataset)
            .unwrap()
            .0
            .current_branch,
        Some(BranchName("main".into()))
    );
    assert!(
        runtime
            .world()
            .get::<HasSnapshot>(repository_dataset)
            .unwrap()
            .0
    );
    assert_eq!(
        runtime
            .world()
            .resource::<GitLoadTracker>()
            .get(&GitLoadKey {
                repository_dataset,
                target: GitLoadTarget::Repository,
            }),
        Some(&GitLoadStatus::Ready {
            request_id: repository_request,
        })
    );

    enqueue(
        &mut runtime,
        repository_dataset,
        &cwd,
        GitCommand::LoadBranches,
    );
    let main_commits_identity = DatasetIdentity::Commits {
        repository: repository.clone(),
        branch: BranchName("main".into()),
    };
    let feature_commits_identity = DatasetIdentity::Commits {
        repository: repository.clone(),
        branch: BranchName("feature".into()),
    };
    let main_commits = runtime
        .world()
        .resource::<DatasetIndex>()
        .get(&main_commits_identity)
        .unwrap();
    let feature_commits = runtime
        .world()
        .resource::<DatasetIndex>()
        .get(&feature_commits_identity)
        .unwrap();

    enqueue(
        &mut runtime,
        repository_dataset,
        &cwd,
        GitCommand::LoadCommits {
            branch: BranchName("main".into()),
            limit: 50,
        },
    );
    enqueue(
        &mut runtime,
        repository_dataset,
        &cwd,
        GitCommand::LoadCommits {
            branch: BranchName("feature".into()),
            limit: 50,
        },
    );

    let shared_commit = runtime
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::Commit {
            repository: repository.clone(),
            hash: shared_hash,
        })
        .unwrap();
    assert!(
        runtime
            .world()
            .get::<DatasetChildren>(main_commits)
            .unwrap()
            .0
            .contains(&shared_commit)
    );
    assert_eq!(
        runtime
            .world()
            .get::<DatasetChildren>(feature_commits)
            .unwrap()
            .0,
        vec![shared_commit]
    );

    enqueue(
        &mut runtime,
        repository_dataset,
        &cwd,
        GitCommand::LoadCommitDetail {
            commit: head.clone(),
        },
    );
    let head_dataset = runtime
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::Commit {
            repository: repository.clone(),
            hash: head.clone(),
        })
        .unwrap();
    assert_eq!(
        runtime
            .world()
            .get::<CommitMetadata>(head_dataset)
            .unwrap()
            .message
            .as_deref(),
        Some("second")
    );

    enqueue(
        &mut runtime,
        repository_dataset,
        &cwd,
        GitCommand::LoadFileDiff {
            commit: head.clone(),
            path: GitPath::from("file.txt"),
            old_path: None,
        },
    );
    let diff_dataset = runtime
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::FileChanges {
            repository: repository.clone(),
            commit: head,
            path: GitPath::from("file.txt"),
        })
        .unwrap();
    assert!(
        !runtime
            .world()
            .get::<FileChangesMetadata>(diff_dataset)
            .unwrap()
            .0
            .hunks
            .is_empty()
    );

    let children_before = runtime
        .world()
        .get::<DatasetChildren>(main_commits)
        .unwrap()
        .0
        .clone();
    let revision_before = runtime
        .world()
        .get::<DatasetRevision>(main_commits)
        .unwrap()
        .0;
    let failed_request = enqueue(
        &mut runtime,
        repository_dataset,
        &cwd.join("missing-directory"),
        GitCommand::LoadCommits {
            branch: BranchName("main".into()),
            limit: 50,
        },
    );
    assert_eq!(
        runtime
            .world()
            .get::<DatasetChildren>(main_commits)
            .unwrap()
            .0,
        children_before
    );
    assert_eq!(
        runtime
            .world()
            .get::<DatasetRevision>(main_commits)
            .unwrap()
            .0,
        revision_before
    );
    assert_eq!(
        runtime.world().resource::<GitExecutionFailures>().0.len(),
        1
    );
    assert!(matches!(
        runtime
            .world()
            .resource::<GitLoadTracker>()
            .get(&GitLoadKey {
                repository_dataset,
                target: GitLoadTarget::Commits {
                    branch: BranchName("main".into()),
                },
            }),
        Some(GitLoadStatus::Failed { request_id, .. }) if *request_id == failed_request
    ));
    assert!(runtime.validate().is_empty());
}

#[test]
fn repository_status_rekeys_a_subdirectory_seed_to_the_git_root() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    git(&root, &["init", "-b", "main"]);
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();

    let mut runtime = DatasetRuntime::new();
    register_default(&mut runtime, "repository", DatasetKind::Repository);
    register_default(&mut runtime, "branch", DatasetKind::Branch);
    register_default(&mut runtime, "commits", DatasetKind::Commits);
    let seed_key = RepositoryKey::new(nested.clone());
    let repository_dataset = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(seed_key.clone()),
            DatasetKind::Repository,
            DatasetTemplateId::from("repository"),
        )
        .unwrap();
    runtime.add_root(repository_dataset).unwrap();

    enqueue(
        &mut runtime,
        repository_dataset,
        &nested,
        GitCommand::LoadRepository,
    );

    {
        let index = runtime.world().resource::<DatasetIndex>();
        assert_eq!(
            index.get(&DatasetIdentity::Repository(RepositoryKey::new(
                root.clone()
            ))),
            Some(repository_dataset)
        );
        assert_eq!(index.get(&DatasetIdentity::Repository(seed_key)), None);
    }
    enqueue(
        &mut runtime,
        repository_dataset,
        &nested,
        GitCommand::LoadBranches,
    );
    assert!(
        runtime
            .world()
            .resource::<DatasetIndex>()
            .get(&DatasetIdentity::Branch {
                repository: RepositoryKey::new(root),
                name: BranchName("main".into()),
            })
            .is_some()
    );
    assert!(runtime.validate().is_empty());
}

#[test]
fn working_tree_snapshot_builds_three_level_changes_and_invalidates_cached_diff() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    git(&cwd, &["init", "-b", "main"]);
    git(&cwd, &["config", "user.name", "Pitui Test"]);
    git(&cwd, &["config", "user.email", "pitui@example.invalid"]);
    fs::write(cwd.join("file.txt"), "one\n").unwrap();
    git(&cwd, &["add", "file.txt"]);
    git(&cwd, &["commit", "-m", "initial"]);
    fs::write(cwd.join("file.txt"), "one\nstaged\n").unwrap();
    git(&cwd, &["add", "file.txt"]);
    fs::write(cwd.join("file.txt"), "one\nstaged\nunstaged\n").unwrap();
    fs::write(cwd.join("new.txt"), "new\n").unwrap();

    let mut runtime = DatasetRuntime::new();
    for template in pitui_config::builtin_dataset_templates() {
        runtime.register_default_template(template).unwrap();
    }
    let repository_key = RepositoryKey::new(cwd.clone());
    let repository = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(repository_key.clone()),
            DatasetKind::Repository,
            DatasetTemplateId::from("repository"),
        )
        .unwrap();
    let changes = runtime
        .ensure_dataset(
            DatasetIdentity::Changes(repository_key.clone()),
            DatasetKind::Changes,
            DatasetTemplateId::from("changes"),
        )
        .unwrap();
    runtime.add_root(repository).unwrap();
    runtime.add_root(changes).unwrap();
    enqueue(&mut runtime, repository, &cwd, GitCommand::LoadRepository);
    enqueue(&mut runtime, repository, &cwd, GitCommand::LoadWorkingTree);

    let children = &runtime.world().get::<DatasetChildren>(changes).unwrap().0;
    assert_eq!(children.len(), 2);
    let collection = &runtime.world().get::<DatasetCollection>(changes).unwrap().0;
    assert_eq!(collection.len(), 5, "two groups + MM twice + untracked");
    let staged_diff_identity = DatasetIdentity::WorkingTreeFileChanges {
        repository: repository_key.clone(),
        boundary: ChangeBoundary::Staged,
        path: GitPath::from("file.txt"),
    };
    let unstaged_file =
        runtime
            .world()
            .resource::<DatasetIndex>()
            .get(&DatasetIdentity::WorkingTreeFile {
                repository: repository_key.clone(),
                boundary: ChangeBoundary::Unstaged,
                path: GitPath::from("file.txt"),
            });
    assert!(unstaged_file.is_some(), "MM file must exist in both groups");

    enqueue(
        &mut runtime,
        repository,
        &cwd,
        GitCommand::LoadWorkingTreeDiff {
            path: GitPath::from("file.txt"),
            old_path: None,
            include_staged: true,
            include_worktree: false,
            untracked: false,
        },
    );
    let staged_diff = runtime
        .world()
        .resource::<DatasetIndex>()
        .get(&staged_diff_identity)
        .unwrap();
    assert!(
        runtime
            .world()
            .get::<WorkingTreeFileChangesMetadata>(staged_diff)
            .is_some()
    );
    assert!(runtime.world().get::<HasSnapshot>(staged_diff).unwrap().0);

    fs::write(cwd.join("newer.txt"), "newer\n").unwrap();
    enqueue(&mut runtime, repository, &cwd, GitCommand::LoadWorkingTree);
    assert!(
        runtime
            .world()
            .get::<WorkingTreeFileChangesMetadata>(staged_diff)
            .is_none(),
        "a working-tree refresh must invalidate stale file diff data"
    );
    assert!(!runtime.world().get::<HasSnapshot>(staged_diff).unwrap().0);
    assert!(runtime.validate().is_empty());
}
