//! Git boundary shared by Pitui runtimes.
//!
//! The crate is intentionally independent from `bevy_ecs` and terminal code.

#![forbid(unsafe_code)]

use std::{
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use pitui_core::{
    BranchName, CommitDetail, CommitHash, FileDiff, GitPath, ReflogEntry, Repository,
    WorkingTreeChange, WorkingTreeDiff, WorkingTreeDiffKind, WorkingTreeDiffSection,
};

pub mod logging;
pub mod parser;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitFailure {
    pub command: String,
    pub stderr: String,
    pub abort_attempted: bool,
    pub abort_result: Option<String>,
}

impl GitFailure {
    pub fn new(command: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            stderr: stderr.into(),
            abort_attempted: false,
            abort_result: None,
        }
    }
}

impl fmt::Display for GitFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.command, self.stderr)
    }
}

impl std::error::Error for GitFailure {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitCommand {
    LoadRepository,
    LoadBranches,
    LoadCommits {
        branch: BranchName,
        limit: usize,
    },
    LoadCommitDetail {
        commit: CommitHash,
    },
    LoadFileDiff {
        commit: CommitHash,
        path: GitPath,
        old_path: Option<GitPath>,
    },
    LoadReflog {
        limit: usize,
    },
    LoadWorkingTree,
    LoadWorkingTreeDiff {
        path: GitPath,
        old_path: Option<GitPath>,
        include_staged: bool,
        include_worktree: bool,
        untracked: bool,
    },
    StagePaths {
        paths: Vec<GitPath>,
    },
    UnstagePaths {
        paths: Vec<GitPath>,
    },
    Commit {
        message: String,
    },
    CherryPick {
        commits: Vec<CommitHash>,
    },
}

impl GitCommand {
    /// Stable, non-sensitive operation name for logs and user-facing notices.
    /// In particular this never includes commit messages, URLs or path argv.
    pub fn operation_name(&self) -> &'static str {
        match self {
            Self::LoadRepository => "load_repository",
            Self::LoadBranches => "load_branches",
            Self::LoadCommits { .. } => "load_commits",
            Self::LoadCommitDetail { .. } => "load_commit_detail",
            Self::LoadFileDiff { .. } => "load_file_diff",
            Self::LoadReflog { .. } => "load_reflog",
            Self::LoadWorkingTree => "load_working_tree",
            Self::LoadWorkingTreeDiff { .. } => "load_working_tree_diff",
            Self::StagePaths { .. } => "stage_paths",
            Self::UnstagePaths { .. } => "unstage_paths",
            Self::Commit { .. } => "commit",
            Self::CherryPick { .. } => "cherry_pick",
        }
    }
}

/// Redacts URL-like tokens and bounds untrusted Git stderr before it is stored
/// in a session Dataset, persisted log, or displayed in a Notice.
pub fn sanitize_log_text(value: &str, max_chars: usize) -> String {
    let redacted = value
        .split_inclusive(char::is_whitespace)
        .map(|segment| {
            let content_len = segment.trim_end_matches(char::is_whitespace).len();
            let (content, whitespace) = segment.split_at(content_len);
            let lower = content.to_ascii_lowercase();
            let looks_like_url = lower.contains("://")
                || lower.starts_with("git@")
                || (lower.contains('@') && lower.contains(':'));
            if looks_like_url {
                format!("<redacted-url>{whitespace}")
            } else {
                segment.to_owned()
            }
        })
        .collect::<String>();
    if redacted.chars().count() <= max_chars {
        return redacted;
    }
    let mut truncated = redacted
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedGitPayload {
    Repository(Repository),
    Branches(Vec<pitui_core::Branch>),
    Commits {
        branch: BranchName,
        commits: Vec<pitui_core::Commit>,
    },
    CommitDetail(CommitDetail),
    FileDiff(FileDiff),
    Reflog(Vec<ReflogEntry>),
    WorkingTree(Vec<WorkingTreeChange>),
    WorkingTreeDiff(WorkingTreeDiff),
    CommandSucceeded {
        message: String,
    },
    ConflictAborted {
        message: String,
        abort_result: String,
    },
}

/// Synchronous Git execution boundary consumed by ECS systems.
pub trait GitExecutor: Send + Sync + 'static {
    fn execute(&self, cwd: &Path, command: &GitCommand) -> Result<ParsedGitPayload, GitFailure>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CliGitExecutor;

impl GitExecutor for CliGitExecutor {
    fn execute(&self, cwd: &Path, command: &GitCommand) -> Result<ParsedGitPayload, GitFailure> {
        match command {
            GitCommand::LoadRepository => load_repository(cwd),
            GitCommand::LoadBranches => load_branches(cwd),
            GitCommand::LoadCommits { branch, limit } => load_commits(cwd, branch.clone(), *limit),
            GitCommand::LoadCommitDetail { commit } => load_commit_detail(cwd, commit.clone()),
            GitCommand::LoadFileDiff {
                commit,
                path,
                old_path,
            } => load_file_diff(cwd, commit.clone(), path.clone(), old_path.clone()),
            GitCommand::LoadReflog { limit } => load_reflog(cwd, *limit),
            GitCommand::LoadWorkingTree => load_working_tree(cwd),
            GitCommand::LoadWorkingTreeDiff {
                path,
                old_path,
                include_staged,
                include_worktree,
                untracked,
            } => load_working_tree_diff(
                cwd,
                path.clone(),
                old_path.clone(),
                *include_staged,
                *include_worktree,
                *untracked,
            ),
            GitCommand::StagePaths { paths } => stage_paths(cwd, paths.clone()),
            GitCommand::UnstagePaths { paths } => unstage_paths(cwd, paths.clone()),
            GitCommand::Commit { message } => create_commit(cwd, message.clone()),
            GitCommand::CherryPick { commits } => cherry_pick_safely(cwd, commits.clone()),
        }
    }
}

fn display_command(args: &[OsString]) -> String {
    let mut command = String::from("git");
    for argument in args {
        command.push(' ');
        let argument = argument.to_string_lossy();
        if argument
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._/:=@".contains(character))
        {
            command.push_str(&argument);
        } else {
            command.push('\'');
            command.push_str(&argument.replace('\'', "'\\''"));
            command.push('\'');
        }
    }
    command
}

fn run_git<I, S>(cwd: &Path, args: I) -> Result<Output, GitFailure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_git_with_exit_codes(cwd, args, &[0])
}

fn run_git_with_exit_codes<I, S>(
    cwd: &Path,
    args: I,
    expected_exit_codes: &[i32],
) -> Result<Output, GitFailure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args = args
        .into_iter()
        .map(|argument| argument.as_ref().to_os_string())
        .collect::<Vec<_>>();
    let command = display_command(&args);
    let output = Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| GitFailure::new(command.clone(), error.to_string()))?;
    if output
        .status
        .code()
        .is_some_and(|code| expected_exit_codes.contains(&code))
    {
        Ok(output)
    } else {
        Err(GitFailure::new(
            command,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

fn load_repository(cwd: &Path) -> Result<ParsedGitPayload, GitFailure> {
    let root = run_git(cwd, ["rev-parse", "--show-toplevel"])?;
    let head = run_git(cwd, ["rev-parse", "--verify", "--short=8", "HEAD"])
        .map(|output| output.stdout)
        .unwrap_or_default();
    let branch = run_git(cwd, ["branch", "--show-current"])?;
    let status = run_git(cwd, ["status", "--porcelain=v1", "-z", "-b"])?;
    parser::parse_repository(&root.stdout, &head, &branch.stdout, &status.stdout)
        .map(ParsedGitPayload::Repository)
}

fn load_branches(cwd: &Path) -> Result<ParsedGitPayload, GitFailure> {
    let output = run_git(
        cwd,
        [
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(objectname:short)%00%(committerdate:iso8601-strict)%00%(subject)%00%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(ParsedGitPayload::Branches(parser::parse_branches(
        &output.stdout,
    )))
}

fn load_commits(
    cwd: &Path,
    branch: BranchName,
    limit: usize,
) -> Result<ParsedGitPayload, GitFailure> {
    let output = run_git(
        cwd,
        [
            OsString::from("log"),
            OsString::from(branch.0.clone()),
            OsString::from(format!("--max-count={limit}")),
            OsString::from("--date=iso-strict"),
            OsString::from("--decorate=short"),
            OsString::from("--format=%x1e%H%x1f%h%x1f%an%x1f%aI%x1f%D%x1f%s"),
            OsString::from("--"),
        ],
    )?;
    Ok(ParsedGitPayload::Commits {
        branch,
        commits: parser::parse_commits(&output.stdout),
    })
}

fn load_reflog(cwd: &Path, limit: usize) -> Result<ParsedGitPayload, GitFailure> {
    let output = run_git(
        cwd,
        [
            OsString::from("reflog"),
            OsString::from("show"),
            OsString::from(format!("--max-count={limit}")),
            OsString::from("--format=%x1e%H%x1f%h%x1f%gd%x1f%gs%x1f%gn%x1f%aI"),
        ],
    )?;
    Ok(ParsedGitPayload::Reflog(parser::parse_reflog(
        &output.stdout,
    )))
}

fn load_commit_detail(cwd: &Path, commit: CommitHash) -> Result<ParsedGitPayload, GitFailure> {
    let metadata = run_git(
        cwd,
        [
            OsString::from("show"),
            OsString::from("--no-patch"),
            OsString::from("--date=iso-strict"),
            OsString::from(
                "--format=%x1e%H%x1f%h%x1f%an%x1f%ae%x1f%aI%x1f%cn%x1f%ce%x1f%cI%x1f%s%x1f%B",
            ),
            OsString::from(commit.0.clone()),
        ],
    )?;
    let name_status = run_git(
        cwd,
        [
            OsString::from("diff-tree"),
            OsString::from("--root"),
            OsString::from("-m"),
            OsString::from("--first-parent"),
            OsString::from("--no-commit-id"),
            OsString::from("--name-status"),
            OsString::from("-r"),
            OsString::from("-M"),
            OsString::from("-z"),
            OsString::from(commit.0.clone()),
        ],
    )?;
    let numstat = run_git(
        cwd,
        [
            OsString::from("show"),
            OsString::from("--first-parent"),
            OsString::from("--numstat"),
            OsString::from("-z"),
            OsString::from("--format="),
            OsString::from("--find-renames"),
            OsString::from(commit.0.clone()),
        ],
    )?;
    let patch = run_git(
        cwd,
        [
            OsString::from("show"),
            OsString::from("--first-parent"),
            OsString::from("--format="),
            OsString::from("--patch"),
            OsString::from("--find-renames"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-color"),
            OsString::from(commit.0),
        ],
    )?;
    parser::parse_commit_detail(
        &metadata.stdout,
        &name_status.stdout,
        &numstat.stdout,
        &patch.stdout,
    )
    .map(ParsedGitPayload::CommitDetail)
}

fn load_file_diff(
    cwd: &Path,
    commit: CommitHash,
    path: GitPath,
    old_path: Option<GitPath>,
) -> Result<ParsedGitPayload, GitFailure> {
    let mut args = vec![
        OsString::from("show"),
        OsString::from("--first-parent"),
        OsString::from("--format="),
        OsString::from("--patch"),
        OsString::from("--find-renames"),
        OsString::from("--no-ext-diff"),
        OsString::from("--no-color"),
        OsString::from(commit.0.clone()),
        OsString::from("--"),
        path.to_os_string(),
    ];
    if let Some(old_path) = &old_path
        && old_path != &path
    {
        args.push(old_path.to_os_string());
    }
    let patch = run_git(cwd, args)?;
    Ok(ParsedGitPayload::FileDiff(parser::parse_file_diff(
        &patch.stdout,
        commit,
        path,
        old_path,
    )))
}

fn load_working_tree(cwd: &Path) -> Result<ParsedGitPayload, GitFailure> {
    let output = run_git(
        cwd,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    Ok(ParsedGitPayload::WorkingTree(
        parser::parse_worktree_changes(&output.stdout),
    ))
}

fn push_diff_pathspec(args: &mut Vec<OsString>, path: &GitPath, old_path: Option<&GitPath>) {
    args.push(OsString::from("--"));
    args.push(path.to_os_string());
    if let Some(old_path) = old_path
        && old_path != path
    {
        args.push(old_path.to_os_string());
    }
}

fn diff_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn load_working_tree_diff(
    cwd: &Path,
    path: GitPath,
    old_path: Option<GitPath>,
    include_staged: bool,
    include_worktree: bool,
    untracked: bool,
) -> Result<ParsedGitPayload, GitFailure> {
    let mut sections = Vec::new();
    if include_staged {
        let mut args = vec![
            OsString::from("diff"),
            OsString::from("--cached"),
            OsString::from("--patch"),
            OsString::from("--find-renames"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-color"),
        ];
        push_diff_pathspec(&mut args, &path, old_path.as_ref());
        sections.push(WorkingTreeDiffSection {
            kind: WorkingTreeDiffKind::Staged,
            lines: diff_lines(&run_git(cwd, args)?),
        });
    }
    if include_worktree {
        let mut args = vec![
            OsString::from("diff"),
            OsString::from("--patch"),
            OsString::from("--find-renames"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-color"),
        ];
        push_diff_pathspec(&mut args, &path, old_path.as_ref());
        sections.push(WorkingTreeDiffSection {
            kind: WorkingTreeDiffKind::Worktree,
            lines: diff_lines(&run_git(cwd, args)?),
        });
    }
    if untracked {
        #[cfg(windows)]
        let null_device = OsString::from("NUL");
        #[cfg(not(windows))]
        let null_device = OsString::from("/dev/null");
        let args = vec![
            OsString::from("diff"),
            OsString::from("--no-index"),
            OsString::from("--patch"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-color"),
            OsString::from("--"),
            null_device,
            path.to_os_string(),
        ];
        sections.push(WorkingTreeDiffSection {
            kind: WorkingTreeDiffKind::Untracked,
            lines: diff_lines(&run_git_with_exit_codes(cwd, args, &[0, 1])?),
        });
    }
    Ok(ParsedGitPayload::WorkingTreeDiff(WorkingTreeDiff {
        path,
        sections,
    }))
}

fn stage_paths(cwd: &Path, paths: Vec<GitPath>) -> Result<ParsedGitPayload, GitFailure> {
    if paths.is_empty() {
        return Err(GitFailure::new(
            "git add --all -- <paths>",
            "No files were selected for staging",
        ));
    }
    let count = paths.len();
    let mut args = vec![
        OsString::from("add"),
        OsString::from("--all"),
        OsString::from("--"),
    ];
    args.extend(paths.into_iter().map(|path| path.to_os_string()));
    run_git(cwd, args)?;
    Ok(ParsedGitPayload::CommandSucceeded {
        message: format!("Staged {count} path{}", if count == 1 { "" } else { "s" }),
    })
}

fn unstage_paths(cwd: &Path, paths: Vec<GitPath>) -> Result<ParsedGitPayload, GitFailure> {
    if paths.is_empty() {
        return Err(GitFailure::new(
            "git reset -- <paths>",
            "No files were selected for unstaging",
        ));
    }
    let count = paths.len();
    let mut args = vec![OsString::from("reset"), OsString::from("--")];
    args.extend(paths.into_iter().map(|path| path.to_os_string()));
    run_git(cwd, args)?;
    Ok(ParsedGitPayload::CommandSucceeded {
        message: format!("Unstaged {count} path{}", if count == 1 { "" } else { "s" }),
    })
}

fn create_commit(cwd: &Path, message: String) -> Result<ParsedGitPayload, GitFailure> {
    let message = message.trim();
    if message.is_empty() {
        return Err(GitFailure::new(
            "git commit -m <message>",
            "Commit message cannot be empty",
        ));
    }
    run_git(
        cwd,
        [
            OsString::from("commit"),
            OsString::from("-m"),
            OsString::from(message),
        ],
    )?;
    Ok(ParsedGitPayload::CommandSucceeded {
        message: "Commit created".into(),
    })
}

fn git_internal_path(cwd: &Path, name: &str) -> Result<PathBuf, GitFailure> {
    let output = run_git(cwd, ["rev-parse", "--git-path", name])?;
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn cherry_pick_in_progress(cwd: &Path) -> Result<bool, GitFailure> {
    Ok(git_internal_path(cwd, "CHERRY_PICK_HEAD")?.exists())
}

fn cherry_pick_safely(
    cwd: &Path,
    commits: Vec<CommitHash>,
) -> Result<ParsedGitPayload, GitFailure> {
    let mut args = vec![OsString::from("cherry-pick")];
    args.extend(
        commits
            .iter()
            .map(|commit| OsString::from(commit.0.clone())),
    );
    let command = display_command(&args);
    if commits.is_empty() {
        return Err(GitFailure::new(command, "No commits were selected"));
    }
    if cherry_pick_in_progress(cwd)? {
        return Err(GitFailure::new(
            command,
            "A cherry-pick was already in progress before Pitui's request; Pitui left it untouched. Finish or abort it explicitly before starting another cherry-pick.",
        ));
    }
    let status = run_git(cwd, ["status", "--porcelain=v1", "-z"])?;
    if !status.stdout.is_empty() {
        return Err(GitFailure::new(
            command,
            "Cherry-pick requires a clean working tree and index. Commit, stash, or discard all changes first.",
        ));
    }

    match run_git(cwd, args) {
        Ok(_) => Ok(ParsedGitPayload::CommandSucceeded {
            message: format!(
                "Cherry-picked {} commit{}",
                commits.len(),
                if commits.len() == 1 { "" } else { "s" }
            ),
        }),
        Err(mut failure) => {
            // Preflight proved no user-owned cherry-pick existed. Abort only a
            // conflict state created by this exact invocation.
            let request_started_cherry_pick = cherry_pick_in_progress(cwd).unwrap_or(false);
            let conflicts = run_git(cwd, ["diff", "--name-only", "--diff-filter=U"])
                .map(|output| !output.stdout.is_empty())
                .unwrap_or(false);
            if !request_started_cherry_pick || !conflicts {
                return Err(failure);
            }

            match run_git(cwd, ["cherry-pick", "--abort"]) {
                Ok(_) => Ok(ParsedGitPayload::ConflictAborted {
                    message: format!(
                        "Cherry-pick stopped on a conflict and Pitui restored the pre-operation state: {}",
                        failure.stderr
                    ),
                    abort_result: "git cherry-pick --abort completed".into(),
                }),
                Err(abort_failure) => {
                    failure.stderr = format!(
                        "{}\n\nPitui detected a cherry-pick conflict, but automatic abort failed:\n{}: {}",
                        failure.stderr, abort_failure.command, abort_failure.stderr
                    );
                    failure.abort_attempted = true;
                    failure.abort_result = Some(format!(
                        "{}: {}",
                        abort_failure.command, abort_failure.stderr
                    ));
                    Err(failure)
                }
            }
        }
    }
}
