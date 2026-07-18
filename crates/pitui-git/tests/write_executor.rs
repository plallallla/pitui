use std::{fs, path::Path, process::Command};

use pitui_core::{CommitHash, GitPath};
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
fn synchronous_write_commands_stage_unstage_and_commit_only_the_index() {
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "-b", "main"]);
    git(repository.path(), &["config", "user.name", "Pitui Test"]);
    git(
        repository.path(),
        &["config", "user.email", "pitui@example.invalid"],
    );
    fs::write(repository.path().join("staged.txt"), "staged\n").unwrap();
    fs::write(repository.path().join("unstaged.txt"), "unstaged\n").unwrap();

    let executor = CliGitExecutor;
    let staged = executor
        .execute(
            repository.path(),
            &GitCommand::StagePaths {
                paths: vec![GitPath::from("staged.txt")],
            },
        )
        .unwrap();
    assert!(matches!(
        staged,
        ParsedGitPayload::CommandSucceeded { message } if message == "Staged 1 path"
    ));
    assert!(
        git(repository.path(), &["status", "--porcelain=v1"])
            .lines()
            .any(|line| line == "A  staged.txt")
    );

    let unstaged = executor
        .execute(
            repository.path(),
            &GitCommand::UnstagePaths {
                paths: vec![GitPath::from("staged.txt")],
            },
        )
        .unwrap();
    assert!(matches!(
        unstaged,
        ParsedGitPayload::CommandSucceeded { message } if message == "Unstaged 1 path"
    ));
    assert!(
        git(repository.path(), &["status", "--porcelain=v1"])
            .lines()
            .any(|line| line == "?? staged.txt")
    );

    executor
        .execute(
            repository.path(),
            &GitCommand::StagePaths {
                paths: vec![GitPath::from("staged.txt")],
            },
        )
        .unwrap();
    let committed = executor
        .execute(
            repository.path(),
            &GitCommand::Commit {
                message: "created from Changes".into(),
            },
        )
        .unwrap();
    assert!(matches!(
        committed,
        ParsedGitPayload::CommandSucceeded { message } if message == "Commit created"
    ));
    assert_eq!(
        git(repository.path(), &["log", "-1", "--pretty=%s"]),
        "created from Changes"
    );
    assert_eq!(
        git(
            repository.path(),
            &["show", "--format=", "--name-only", "HEAD"]
        ),
        "staged.txt"
    );
    assert_eq!(
        git(repository.path(), &["status", "--porcelain=v1"]),
        "?? unstaged.txt"
    );
}

#[test]
fn commit_rejects_an_empty_message_without_mutating_the_repository() {
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "-b", "main"]);
    fs::write(repository.path().join("file.txt"), "content\n").unwrap();
    git(repository.path(), &["add", "file.txt"]);

    let failure = CliGitExecutor
        .execute(
            repository.path(),
            &GitCommand::Commit {
                message: "  \n  ".into(),
            },
        )
        .unwrap_err();
    assert_eq!(failure.stderr, "Commit message cannot be empty");
    assert!(
        git(repository.path(), &["status", "--porcelain=v1"])
            .lines()
            .any(|line| line == "A  file.txt")
    );
}

#[test]
fn safe_cherry_pick_replays_an_explicit_commit_sequence() {
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "-b", "main"]);
    git(repository.path(), &["config", "user.name", "Pitui Test"]);
    git(
        repository.path(),
        &["config", "user.email", "pitui@example.invalid"],
    );
    fs::write(repository.path().join("base.txt"), "base\n").unwrap();
    git(repository.path(), &["add", "base.txt"]);
    git(repository.path(), &["commit", "-m", "base"]);
    git(repository.path(), &["switch", "-c", "source"]);
    fs::write(repository.path().join("one.txt"), "one\n").unwrap();
    git(repository.path(), &["add", "one.txt"]);
    git(repository.path(), &["commit", "-m", "source one"]);
    let one = CommitHash(git(repository.path(), &["rev-parse", "HEAD"]));
    fs::write(repository.path().join("two.txt"), "two\n").unwrap();
    git(repository.path(), &["add", "two.txt"]);
    git(repository.path(), &["commit", "-m", "source two"]);
    let two = CommitHash(git(repository.path(), &["rev-parse", "HEAD"]));
    git(repository.path(), &["switch", "main"]);

    let payload = CliGitExecutor
        .execute(
            repository.path(),
            &GitCommand::CherryPick {
                commits: vec![one, two],
            },
        )
        .unwrap();
    assert!(matches!(
        payload,
        ParsedGitPayload::CommandSucceeded { message }
            if message == "Cherry-picked 2 commits"
    ));
    assert_eq!(
        git(
            repository.path(),
            &["log", "-2", "--reverse", "--pretty=%s"]
        ),
        "source one\nsource two"
    );
}

#[test]
fn safe_cherry_pick_aborts_only_the_conflict_started_by_this_request() {
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "-b", "main"]);
    git(repository.path(), &["config", "user.name", "Pitui Test"]);
    git(
        repository.path(),
        &["config", "user.email", "pitui@example.invalid"],
    );
    fs::write(repository.path().join("conflict.txt"), "base\n").unwrap();
    git(repository.path(), &["add", "conflict.txt"]);
    git(repository.path(), &["commit", "-m", "base"]);
    git(repository.path(), &["switch", "-c", "source"]);
    fs::write(repository.path().join("conflict.txt"), "source\n").unwrap();
    git(repository.path(), &["commit", "-am", "source change"]);
    let source = CommitHash(git(repository.path(), &["rev-parse", "HEAD"]));
    git(repository.path(), &["switch", "main"]);
    fs::write(repository.path().join("conflict.txt"), "main\n").unwrap();
    git(repository.path(), &["commit", "-am", "main change"]);
    let head_before = git(repository.path(), &["rev-parse", "HEAD"]);

    let payload = CliGitExecutor
        .execute(
            repository.path(),
            &GitCommand::CherryPick {
                commits: vec![source.clone()],
            },
        )
        .unwrap();
    assert!(matches!(
        payload,
        ParsedGitPayload::ConflictAborted { message, abort_result }
            if message.contains("restored the pre-operation state")
                && abort_result == "git cherry-pick --abort completed"
    ));
    assert_eq!(git(repository.path(), &["rev-parse", "HEAD"]), head_before);
    assert!(git(repository.path(), &["status", "--porcelain=v1"]).is_empty());
    assert!(!repository.path().join(".git/CHERRY_PICK_HEAD").exists());

    let failed = Command::new("git")
        .args(["cherry-pick", &source.0])
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(repository.path().join(".git/CHERRY_PICK_HEAD").exists());
    let failure = CliGitExecutor
        .execute(
            repository.path(),
            &GitCommand::CherryPick {
                commits: vec![source],
            },
        )
        .unwrap_err();
    assert!(failure.stderr.contains("already in progress"));
    assert!(!failure.abort_attempted);
    assert!(repository.path().join(".git/CHERRY_PICK_HEAD").exists());
    git(repository.path(), &["cherry-pick", "--abort"]);
}
