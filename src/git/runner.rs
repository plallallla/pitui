use std::{
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::domain::{
    BranchName, CommitHash, GitPath, WorkingTreeDiff, WorkingTreeDiffKind, WorkingTreeDiffSection,
};

use super::{
    GitRequest, GitResponse, parse_branches, parse_commit_detail, parse_commits, parse_file_diff,
    parse_reflog, parse_repository, parse_worktree_changes,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitFailure {
    pub command: String,
    pub stderr: String,
}

impl fmt::Display for GitFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.command, self.stderr)
    }
}

impl std::error::Error for GitFailure {}

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
    accepted_exit_codes: &[i32],
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
        .map_err(|error| GitFailure {
            command: command.clone(),
            stderr: error.to_string(),
        })?;

    if output
        .status
        .code()
        .is_some_and(|code| accepted_exit_codes.contains(&code))
    {
        Ok(output)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(GitFailure {
            command,
            stderr: if stderr.is_empty() {
                format!("Git exited with {}", output.status)
            } else {
                stderr
            },
        })
    }
}

fn failure_response(failure: GitFailure) -> GitResponse {
    GitResponse::CommandFailed {
        command: failure.command,
        stderr: failure.stderr,
    }
}

fn load_repository(cwd: &Path) -> Result<GitResponse, GitFailure> {
    let root = run_git(cwd, ["rev-parse", "--show-toplevel"])?;
    // An unborn repository is still a valid repository. `HEAD` cannot be
    // resolved until the first commit, so represent it as an empty hash and do
    // not turn normal first-run state into an error popup.
    let head = run_git(cwd, ["rev-parse", "--verify", "--short=8", "HEAD"])
        .map(|output| output.stdout)
        .unwrap_or_default();
    let branch = run_git(cwd, ["branch", "--show-current"])?;
    let status = run_git(cwd, ["status", "--porcelain=v1", "-z", "-b"])?;

    Ok(GitResponse::RepositoryStatusLoaded(parse_repository(
        &root.stdout,
        &head,
        &branch.stdout,
        &status.stdout,
    )?))
}

fn load_branches(cwd: &Path) -> Result<GitResponse, GitFailure> {
    let output = run_git(
        cwd,
        [
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(objectname:short)%00%(committerdate:iso8601-strict)%00%(subject)%00%(HEAD)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(GitResponse::BranchesLoaded(parse_branches(&output.stdout)))
}

fn load_commits(cwd: &Path, branch: BranchName, limit: usize) -> Result<GitResponse, GitFailure> {
    let args = vec![
        OsString::from("log"),
        OsString::from(branch.0.clone()),
        OsString::from(format!("--max-count={limit}")),
        OsString::from("--date=iso-strict"),
        OsString::from("--decorate=short"),
        OsString::from("--format=%x1e%H%x1f%h%x1f%an%x1f%aI%x1f%D%x1f%s"),
        OsString::from("--"),
    ];
    let output = run_git(cwd, args)?;
    Ok(GitResponse::CommitsLoaded {
        branch,
        commits: parse_commits(&output.stdout),
    })
}

fn load_commit_detail(cwd: &Path, commit: CommitHash) -> Result<GitResponse, GitFailure> {
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
            OsString::from(commit.0.clone()),
        ],
    )?;

    let detail = parse_commit_detail(
        &metadata.stdout,
        &name_status.stdout,
        &numstat.stdout,
        &patch.stdout,
    )?;
    Ok(GitResponse::CommitDetailLoaded(detail))
}

fn load_commit_message(cwd: &Path, commit: CommitHash) -> Result<GitResponse, GitFailure> {
    let output = run_git(
        cwd,
        [
            OsString::from("show"),
            OsString::from("--no-patch"),
            OsString::from("--format=%B"),
            OsString::from(commit.0.clone()),
        ],
    )?;
    let mut message = String::from_utf8_lossy(&output.stdout).into_owned();
    while message.ends_with(['\n', '\r']) {
        message.pop();
    }
    Ok(GitResponse::CommitMessageLoaded { commit, message })
}

fn load_file_diff(
    cwd: &Path,
    commit: CommitHash,
    path: GitPath,
    old_path: Option<GitPath>,
) -> Result<GitResponse, GitFailure> {
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
    Ok(GitResponse::FileDiffLoaded(parse_file_diff(
        &patch.stdout,
        commit,
        path,
        old_path,
    )))
}

fn load_reflog(cwd: &Path, limit: usize) -> Result<GitResponse, GitFailure> {
    let output = run_git(
        cwd,
        [
            OsString::from("reflog"),
            OsString::from("show"),
            OsString::from(format!("--max-count={limit}")),
            OsString::from("--date=iso-strict"),
            OsString::from("--format=%x1e%H%x1f%h%x1f%gd%x1f%gs%x1f%gn%x1f%aI"),
        ],
    )?;
    Ok(GitResponse::ReflogLoaded(parse_reflog(&output.stdout)))
}

fn load_working_tree(cwd: &Path) -> Result<GitResponse, GitFailure> {
    let output = run_git(
        cwd,
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    Ok(GitResponse::WorkingTreeLoaded(parse_worktree_changes(
        &output.stdout,
    )))
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
) -> Result<GitResponse, GitFailure> {
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
        let output = run_git(cwd, args)?;
        sections.push(WorkingTreeDiffSection {
            kind: WorkingTreeDiffKind::Staged,
            lines: diff_lines(&output),
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
        let output = run_git(cwd, args)?;
        sections.push(WorkingTreeDiffSection {
            kind: WorkingTreeDiffKind::Worktree,
            lines: diff_lines(&output),
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
        // `git diff --no-index` returns 1 when differences were found. That is
        // the successful/expected outcome for a non-empty untracked file.
        let output = run_git_with_exit_codes(cwd, args, &[0, 1])?;
        sections.push(WorkingTreeDiffSection {
            kind: WorkingTreeDiffKind::Untracked,
            lines: diff_lines(&output),
        });
    }

    Ok(GitResponse::WorkingTreeDiffLoaded(WorkingTreeDiff {
        path,
        sections,
    }))
}

fn stage_paths(cwd: &Path, paths: Vec<GitPath>) -> Result<GitResponse, GitFailure> {
    if paths.is_empty() {
        return Err(GitFailure {
            command: "git add --all -- <paths>".into(),
            stderr: "No files were selected for staging".into(),
        });
    }
    let count = paths.len();
    let mut args = vec![
        OsString::from("add"),
        OsString::from("--all"),
        OsString::from("--"),
    ];
    args.extend(paths.into_iter().map(|path| path.to_os_string()));
    run_git(cwd, args).map(|_| GitResponse::CommandSucceeded {
        message: format!("Staged {count} path{}", if count == 1 { "" } else { "s" }),
    })
}

fn unstage_paths(cwd: &Path, paths: Vec<GitPath>) -> Result<GitResponse, GitFailure> {
    if paths.is_empty() {
        return Err(GitFailure {
            command: "git reset -- <paths>".into(),
            stderr: "No files were selected for unstaging".into(),
        });
    }
    let count = paths.len();
    // Path-limited reset only changes the index and works for both normal and
    // unborn repositories. The working-tree files are never discarded.
    let mut args = vec![OsString::from("reset"), OsString::from("--")];
    args.extend(paths.into_iter().map(|path| path.to_os_string()));
    run_git(cwd, args).map(|_| GitResponse::CommandSucceeded {
        message: format!("Unstaged {count} path{}", if count == 1 { "" } else { "s" }),
    })
}

fn create_commit(cwd: &Path, message: String) -> Result<GitResponse, GitFailure> {
    let message = message.trim();
    if message.is_empty() {
        return Err(GitFailure {
            command: "git commit -m <message>".into(),
            stderr: "Commit message cannot be empty".into(),
        });
    }
    run_git(
        cwd,
        [
            OsString::from("commit"),
            OsString::from("-m"),
            OsString::from(message),
        ],
    )
    .map(|_| GitResponse::CommandSucceeded {
        message: "Commit created".into(),
    })
}

fn command_succeeded(output: Output, fallback: &str) -> GitResponse {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    GitResponse::CommandSucceeded {
        message: if stdout.is_empty() {
            fallback.to_string()
        } else {
            stdout
        },
    }
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

fn rebase_in_progress(cwd: &Path) -> Result<bool, GitFailure> {
    Ok(git_internal_path(cwd, "rebase-merge")?.exists()
        || git_internal_path(cwd, "rebase-apply")?.exists())
}

fn rebase_command(upstream: &BranchName) -> String {
    display_command(&[OsString::from("rebase"), OsString::from(&upstream.0)])
}

fn rebase_preflight(cwd: &Path, upstream: &BranchName) -> Result<(), GitFailure> {
    let command = rebase_command(upstream);
    if rebase_in_progress(cwd)? {
        return Err(GitFailure {
            command,
            stderr: "A rebase was already in progress before Pitui's request; Pitui left it untouched. Finish or abort it explicitly before starting another rebase."
                .into(),
        });
    }

    let current_branch =
        run_git(cwd, ["symbolic-ref", "--quiet", "--short", "HEAD"]).map_err(|_| GitFailure {
            command: command.clone(),
            stderr:
                "Safe rebase requires an attached current branch; detached HEAD is not supported."
                    .into(),
        })?;
    let current_branch = String::from_utf8_lossy(&current_branch.stdout)
        .trim()
        .to_string();
    if current_branch == upstream.0 {
        return Err(GitFailure {
            command,
            stderr: "The selected upstream is already the current branch.".into(),
        });
    }

    let status = run_git(cwd, ["status", "--porcelain=v1", "-z"])?;
    if !status.stdout.is_empty() {
        return Err(GitFailure {
            command,
            stderr: "Safe rebase requires a clean working tree and index. Commit, stash, or discard all changes first."
                .into(),
        });
    }
    Ok(())
}

fn rebase_safely(cwd: &Path, upstream: BranchName) -> Result<GitResponse, GitFailure> {
    rebase_preflight(cwd, &upstream)?;
    let result = run_git(cwd, [OsString::from("rebase"), OsString::from(upstream.0)]);
    match result {
        Ok(output) => Ok(command_succeeded(output, "Rebase completed")),
        Err(failure) => {
            // Only abort state created by this request. The preflight above
            // rejects an existing rebase, so this cannot destroy a user's
            // independently started operation.
            let request_started_rebase = rebase_in_progress(cwd).unwrap_or(false);
            let conflicts = run_git(cwd, ["diff", "--name-only", "--diff-filter=U"])
                .map(|output| !output.stdout.is_empty())
                .unwrap_or(false);
            if !request_started_rebase || !conflicts {
                return Err(failure);
            }

            match run_git(cwd, ["rebase", "--abort"]) {
                Ok(_) => Ok(GitResponse::RebaseConflictAborted {
                    command: failure.command,
                    stderr: failure.stderr,
                }),
                Err(abort_failure) => Err(GitFailure {
                    command: failure.command,
                    stderr: format!(
                        "{}\n\nPitui detected a rebase conflict, but automatic abort failed:\n{}: {}",
                        failure.stderr, abort_failure.command, abort_failure.stderr
                    ),
                }),
            }
        }
    }
}

/// Executes one request. This is the only production function that invokes
/// Git, and it deliberately uses argv rather than a shell command string.
pub fn execute_request(cwd: &Path, request: GitRequest) -> GitResponse {
    let result = match request {
        GitRequest::LoadRepositoryStatus => load_repository(cwd),
        GitRequest::LoadBranches => load_branches(cwd),
        GitRequest::LoadCommits { branch, limit } => load_commits(cwd, branch, limit),
        GitRequest::LoadCommitDetail { commit } => load_commit_detail(cwd, commit),
        GitRequest::LoadCommitMessage { commit } => load_commit_message(cwd, commit),
        GitRequest::LoadFileDiff {
            commit,
            path,
            old_path,
        } => load_file_diff(cwd, commit, path, old_path),
        GitRequest::LoadReflog { limit } => load_reflog(cwd, limit),
        GitRequest::LoadWorkingTree => load_working_tree(cwd),
        GitRequest::LoadWorkingTreeDiff {
            path,
            old_path,
            include_staged,
            include_worktree,
            untracked,
        } => load_working_tree_diff(
            cwd,
            path,
            old_path,
            include_staged,
            include_worktree,
            untracked,
        ),
        GitRequest::StagePaths { paths } => stage_paths(cwd, paths),
        GitRequest::UnstagePaths { paths } => unstage_paths(cwd, paths),
        GitRequest::Commit { message } => create_commit(cwd, message),
        GitRequest::Fetch => run_git(cwd, ["fetch", "--all", "--prune"])
            .map(|output| command_succeeded(output, "Fetch completed")),
        GitRequest::SwitchBranch { branch } => {
            run_git(cwd, [OsString::from("switch"), OsString::from(branch.0)])
                .map(|output| command_succeeded(output, "Branch switched"))
        }
        GitRequest::CherryPick { commits } => {
            let mut args = vec![OsString::from("cherry-pick")];
            args.extend(commits.into_iter().map(|commit| OsString::from(commit.0)));
            run_git(cwd, args).map(|output| command_succeeded(output, "Cherry-pick completed"))
        }
        GitRequest::Reset { commit, mode } => run_git(
            cwd,
            [
                OsString::from("reset"),
                OsString::from(mode.flag()),
                OsString::from(commit.0),
            ],
        )
        .map(|output| command_succeeded(output, "Reset completed")),
        GitRequest::Rebase { upstream } => rebase_safely(cwd, upstream),
    };

    result.unwrap_or_else(failure_response)
}
