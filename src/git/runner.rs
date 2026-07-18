use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::domain::{
    BranchName, CommitHash, GitPath, RemoteInfo, WorkingTreeDiff, WorkingTreeDiffKind,
    WorkingTreeDiffSection,
};
use pitui_git::{CliGitExecutor, GitCommand as NextGitCommand, GitExecutor, ParsedGitPayload};

use super::{GitFailure, GitRequest, GitResponse, parse_reflog, parse_worktree_changes};

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
    match CliGitExecutor.execute(cwd, &NextGitCommand::LoadRepository)? {
        ParsedGitPayload::Repository(repository) => {
            Ok(GitResponse::RepositoryStatusLoaded(repository))
        }
        _ => unreachable!("LoadRepository returned a different payload kind"),
    }
}

fn load_branches(cwd: &Path) -> Result<GitResponse, GitFailure> {
    match CliGitExecutor.execute(cwd, &NextGitCommand::LoadBranches)? {
        ParsedGitPayload::Branches(branches) => Ok(GitResponse::BranchesLoaded(branches)),
        _ => unreachable!("LoadBranches returned a different payload kind"),
    }
}

fn config_values(cwd: &Path, key: &str) -> Result<Vec<String>, GitFailure> {
    let output = run_git_with_exit_codes(
        cwd,
        ["config", "--local", "--null", "--get-all", key],
        &[0, 1, 5],
    )?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .collect())
}

fn first_config_value(cwd: &Path, key: &str) -> Result<Option<String>, GitFailure> {
    Ok(config_values(cwd, key)?.into_iter().next())
}

fn current_branch_name(cwd: &Path, command: &str) -> Result<String, GitFailure> {
    let output = run_git(cwd, ["symbolic-ref", "--quiet", "--short", "HEAD"]).map_err(|_| {
        GitFailure {
            command: command.to_string(),
            stderr: "This operation requires an attached current branch; detached HEAD is not supported."
                .into(),
        }
    })?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn effective_remote_urls(cwd: &Path, name: &str, push: bool) -> Vec<String> {
    let mut args = vec![OsString::from("remote"), OsString::from("get-url")];
    if push {
        args.push(OsString::from("--push"));
    }
    args.extend([OsString::from("--all"), OsString::from(name)]);
    run_git(cwd, args).map_or_else(
        |_| Vec::new(),
        |output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|url| url.trim_end_matches('\r').to_string())
                .filter(|url| !url.is_empty())
                .collect()
        },
    )
}

struct RemoteConfiguration {
    remotes: Vec<RemoteInfo>,
    upstream_remote: Option<String>,
    push_remote: Option<String>,
}

fn remote_configuration(cwd: &Path) -> Result<RemoteConfiguration, GitFailure> {
    let output = run_git(cwd, ["remote"])?;
    let mut names = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();

    let branch = run_git(cwd, ["branch", "--show-current"])?;
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
    let upstream_remote = if branch.is_empty() {
        None
    } else {
        first_config_value(cwd, &format!("branch.{branch}.remote"))?
    };
    let push_remote = if branch.is_empty() {
        None
    } else {
        first_config_value(cwd, &format!("branch.{branch}.pushRemote"))?
            .or(first_config_value(cwd, "remote.pushDefault")?)
            .or_else(|| upstream_remote.clone())
            .or_else(|| names.iter().find(|name| name.as_str() == "origin").cloned())
            .or_else(|| (names.len() == 1).then(|| names[0].clone()))
    };

    let mut remotes = Vec::with_capacity(names.len());
    for name in names {
        let fetch_urls = effective_remote_urls(cwd, &name, false);
        let push_urls = effective_remote_urls(cwd, &name, true);
        remotes.push(RemoteInfo {
            is_upstream: upstream_remote.as_ref() == Some(&name),
            is_push_target: push_remote.as_ref() == Some(&name),
            name,
            fetch_urls,
            push_urls,
        });
    }

    Ok(RemoteConfiguration {
        remotes,
        upstream_remote,
        push_remote,
    })
}

fn load_remotes(cwd: &Path) -> Result<GitResponse, GitFailure> {
    Ok(GitResponse::RemotesLoaded(
        remote_configuration(cwd)?.remotes,
    ))
}

fn validate_remote_policy(cwd: &Path, command: &str) -> Result<(), GitFailure> {
    let RemoteConfiguration {
        remotes,
        upstream_remote,
        push_remote,
    } = remote_configuration(cwd)?;
    if let Some(remote) = remotes.iter().find(|remote| !remote.urls_match()) {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: format!(
                "Pitui requires identical fetch and push URLs for every remote. Remote `{}` violates this policy. Open Remote Management with `o`, select it, and press `e` to set one shared URL.",
                remote.name
            ),
        });
    }

    if let Some(upstream) = upstream_remote
        .as_ref()
        .filter(|remote| remote.as_str() != ".")
        && !remotes.iter().any(|remote| &remote.name == upstream)
    {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: format!(
                "The current branch references missing upstream remote `{upstream}`. Open Remote Management with `o` and choose an existing remote with `u`."
            ),
        });
    }

    if let Some(push) = push_remote.as_ref()
        && !remotes.iter().any(|remote| &remote.name == push)
    {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: format!(
                "The current branch references missing push remote `{push}`. Open Remote Management with `o` and choose an existing remote with `u`."
            ),
        });
    }

    if let (Some(upstream), Some(push)) = (&upstream_remote, &push_remote)
        && upstream != "."
        && upstream != push
    {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: format!(
                "The current branch fetches from `{upstream}` but pushes to `{push}`. Pitui requires one upstream remote for both directions. Open Remote Management with `o` and press `u` on the desired remote."
            ),
        });
    }

    Ok(())
}

fn load_commits(cwd: &Path, branch: BranchName, limit: usize) -> Result<GitResponse, GitFailure> {
    match CliGitExecutor.execute(cwd, &NextGitCommand::LoadCommits { branch, limit })? {
        ParsedGitPayload::Commits { branch, commits } => {
            Ok(GitResponse::CommitsLoaded { branch, commits })
        }
        _ => unreachable!("LoadCommits returned a different payload kind"),
    }
}

fn load_commit_detail(cwd: &Path, commit: CommitHash) -> Result<GitResponse, GitFailure> {
    match CliGitExecutor.execute(cwd, &NextGitCommand::LoadCommitDetail { commit })? {
        ParsedGitPayload::CommitDetail(detail) => Ok(GitResponse::CommitDetailLoaded(detail)),
        _ => unreachable!("LoadCommitDetail returned a different payload kind"),
    }
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
    match CliGitExecutor.execute(
        cwd,
        &NextGitCommand::LoadFileDiff {
            commit,
            path,
            old_path,
        },
    )? {
        ParsedGitPayload::FileDiff(diff) => Ok(GitResponse::FileDiffLoaded(diff)),
        _ => unreachable!("LoadFileDiff returned a different payload kind"),
    }
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

fn validate_remote_name(name: &str, command: &str) -> Result<(), GitFailure> {
    if name.is_empty()
        || name.trim() != name
        || name.starts_with('-')
        || name
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: "Remote name must be non-empty, cannot start with `-`, and cannot contain whitespace or control characters."
                .into(),
        });
    }
    Ok(())
}

fn validate_remote_url(url: &str, command: &str) -> Result<(), GitFailure> {
    if url.is_empty() || url.trim() != url || url.chars().any(|character| character.is_control()) {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: "Remote URL must be non-empty and cannot contain leading/trailing whitespace or control characters."
                .into(),
        });
    }
    Ok(())
}

fn add_remote(cwd: &Path, name: String, url: String) -> Result<GitResponse, GitFailure> {
    let command = format!("git remote add -- {name} <url>");
    validate_remote_name(&name, &command)?;
    validate_remote_url(&url, &command)?;
    run_git(
        cwd,
        [
            OsString::from("remote"),
            OsString::from("add"),
            OsString::from("--"),
            OsString::from(&name),
            OsString::from(url),
        ],
    )
    .map_err(|failure| GitFailure {
        command,
        stderr: failure.stderr,
    })?;
    Ok(GitResponse::CommandSucceeded {
        message: format!("Remote `{name}` added with one shared fetch/push URL"),
    })
}

fn write_config_values(cwd: &Path, key: &str, values: &[String]) -> Result<(), GitFailure> {
    run_git_with_exit_codes(cwd, ["config", "--local", "--unset-all", key], &[0, 1, 5])?;
    for value in values {
        run_git(
            cwd,
            [
                OsString::from("config"),
                OsString::from("--local"),
                OsString::from("--add"),
                OsString::from("--"),
                OsString::from(key),
                OsString::from(value),
            ],
        )?;
    }
    Ok(())
}

fn update_config_transaction(
    cwd: &Path,
    command: &str,
    updates: Vec<(String, Vec<String>)>,
) -> Result<(), GitFailure> {
    let snapshots = updates
        .iter()
        .map(|(key, _)| Ok((key.clone(), config_values(cwd, key)?)))
        .collect::<Result<Vec<_>, GitFailure>>()?;

    for (key, values) in &updates {
        if let Err(failure) = write_config_values(cwd, key, values) {
            let rollback_failures = snapshots
                .iter()
                .filter_map(|(snapshot_key, snapshot_values)| {
                    write_config_values(cwd, snapshot_key, snapshot_values)
                        .err()
                        .map(|rollback| rollback.stderr)
                })
                .collect::<Vec<_>>();
            let rollback = if rollback_failures.is_empty() {
                "Pitui restored the previous Git configuration.".to_string()
            } else {
                format!(
                    "Git configuration rollback also failed: {}",
                    rollback_failures.join("; ")
                )
            };
            return Err(GitFailure {
                command: command.to_string(),
                stderr: format!("{}\n\n{rollback}", failure.stderr),
            });
        }
    }
    Ok(())
}

fn set_remote_url(cwd: &Path, name: String, url: String) -> Result<GitResponse, GitFailure> {
    let command = format!("git remote set-url {name} <shared-url>");
    validate_remote_name(&name, &command)?;
    validate_remote_url(&url, &command)?;
    let remotes = remote_configuration(cwd)?.remotes;
    if !remotes.iter().any(|remote| remote.name == name) {
        return Err(GitFailure {
            command,
            stderr: format!("Remote `{name}` does not exist."),
        });
    }

    update_config_transaction(
        cwd,
        &command,
        vec![
            (format!("remote.{name}.url"), vec![url]),
            (format!("remote.{name}.pushurl"), Vec::new()),
        ],
    )?;
    Ok(GitResponse::CommandSucceeded {
        message: format!("Remote `{name}` now uses one shared fetch/push URL"),
    })
}

fn set_upstream_remote(cwd: &Path, name: String) -> Result<GitResponse, GitFailure> {
    let command = format!("git config branch.<current>.remote {name}");
    validate_remote_name(&name, &command)?;
    let remotes = remote_configuration(cwd)?.remotes;
    let Some(remote) = remotes.iter().find(|remote| remote.name == name) else {
        return Err(GitFailure {
            command,
            stderr: format!("Remote `{name}` does not exist."),
        });
    };
    if !remote.urls_match() {
        return Err(GitFailure {
            command,
            stderr: format!(
                "Remote `{name}` has different fetch and push URLs. Set one shared URL before selecting it as upstream."
            ),
        });
    }

    let branch = current_branch_name(cwd, &command)?;
    update_config_transaction(
        cwd,
        &command,
        vec![
            (format!("branch.{branch}.remote"), vec![name.clone()]),
            (
                format!("branch.{branch}.merge"),
                vec![format!("refs/heads/{branch}")],
            ),
            (format!("branch.{branch}.pushRemote"), vec![name.clone()]),
        ],
    )?;
    Ok(GitResponse::CommandSucceeded {
        message: format!(
            "Remote `{name}` is now upstream for `{branch}` in both fetch and push directions"
        ),
    })
}

fn fetch_safely(cwd: &Path) -> Result<GitResponse, GitFailure> {
    let command = display_command(&[
        OsString::from("fetch"),
        OsString::from("--all"),
        OsString::from("--prune"),
    ]);
    validate_remote_policy(cwd, &command)?;
    run_git(cwd, ["fetch", "--all", "--prune"])
        .map(|output| command_succeeded(output, "Fetch completed"))
}

fn push_safely(cwd: &Path) -> Result<GitResponse, GitFailure> {
    let command = display_command(&[OsString::from("push")]);
    validate_remote_policy(cwd, &command)?;
    run_git(cwd, ["push"]).map(|output| command_succeeded(output, "Push completed"))
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

fn rebase_preflight(
    cwd: &Path,
    command: &str,
    operation_label: &str,
) -> Result<String, GitFailure> {
    if rebase_in_progress(cwd)? {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: "A rebase was already in progress before Pitui's request; Pitui left it untouched. Finish or abort it explicitly before starting another rebase."
                .into(),
        });
    }

    let current_branch =
        run_git(cwd, ["symbolic-ref", "--quiet", "--short", "HEAD"]).map_err(|_| GitFailure {
            command: command.to_string(),
            stderr: format!(
                "{operation_label} requires an attached current branch; detached HEAD is not supported."
            ),
        })?;
    let current_branch = String::from_utf8_lossy(&current_branch.stdout)
        .trim()
        .to_string();

    let status = run_git(cwd, ["status", "--porcelain=v1", "-z"])?;
    if !status.stdout.is_empty() {
        return Err(GitFailure {
            command: command.to_string(),
            stderr: format!(
                "{operation_label} requires a clean working tree and index. Commit, stash, or discard all changes first."
            ),
        });
    }
    Ok(current_branch)
}

fn finish_rebase_capable_operation(
    cwd: &Path,
    result: Result<Output, GitFailure>,
    success_message: &str,
) -> Result<GitResponse, GitFailure> {
    match result {
        Ok(output) => Ok(command_succeeded(output, success_message)),
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

fn rebase_safely(cwd: &Path, upstream: BranchName) -> Result<GitResponse, GitFailure> {
    let command = rebase_command(&upstream);
    let current_branch = rebase_preflight(cwd, &command, "Safe rebase")?;
    if current_branch == upstream.0 {
        return Err(GitFailure {
            command,
            stderr: "The selected upstream is already the current branch.".into(),
        });
    }
    let result = run_git(cwd, [OsString::from("rebase"), OsString::from(upstream.0)]);
    finish_rebase_capable_operation(cwd, result, "Rebase completed")
}

fn pull_rebase_safely(cwd: &Path) -> Result<GitResponse, GitFailure> {
    let command = display_command(&[OsString::from("pull"), OsString::from("--rebase")]);
    rebase_preflight(cwd, &command, "Pull --rebase")?;
    validate_remote_policy(cwd, &command)?;
    let result = run_git(cwd, ["pull", "--rebase"]);
    finish_rebase_capable_operation(cwd, result, "Pull --rebase completed")
}

/// Executes one request. This is the only production function that invokes
/// Git, and it deliberately uses argv rather than a shell command string.
pub fn execute_request(cwd: &Path, request: GitRequest) -> GitResponse {
    let result = match request {
        GitRequest::LoadRepositoryStatus => load_repository(cwd),
        GitRequest::LoadBranches => load_branches(cwd),
        GitRequest::LoadRemotes => load_remotes(cwd),
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
        GitRequest::Fetch => fetch_safely(cwd),
        GitRequest::PullRebase => pull_rebase_safely(cwd),
        GitRequest::Push => push_safely(cwd),
        GitRequest::AddRemote { name, url } => add_remote(cwd, name, url),
        GitRequest::SetRemoteUrl { name, url } => set_remote_url(cwd, name, url),
        GitRequest::SetUpstreamRemote { name } => set_upstream_remote(cwd, name),
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
