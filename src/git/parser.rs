use std::{collections::HashMap, path::PathBuf};

use crate::domain::{
    Branch, BranchKind, BranchName, ChangedFile, Commit, CommitDetail, CommitHash, DiffHunk,
    DiffLine, DiffLineKind, FileChangeKind, FileDiff, GitPath, ReflogEntry, Repository,
    WorkingTreeChange, WorkingTreeStatus,
};

use super::GitFailure;

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn trimmed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

pub fn parse_repository(
    root: &[u8],
    head: &[u8],
    branch: &[u8],
    status: &[u8],
) -> Result<Repository, GitFailure> {
    let root = PathBuf::from(trimmed(root));
    if root.as_os_str().is_empty() {
        return Err(GitFailure {
            command: "git rev-parse --show-toplevel".into(),
            stderr: "Git returned an empty repository root".into(),
        });
    }
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let branch = trimmed(branch);

    Ok(Repository {
        root,
        name,
        current_branch: (!branch.is_empty()).then_some(BranchName(branch)),
        head: CommitHash(trimmed(head)),
        status: parse_worktree_status(status),
    })
}

fn parse_tracking(header: &str, status: &mut WorkingTreeStatus) {
    let Some(open) = header.rfind('[') else {
        return;
    };
    let Some(close) = header[open..].find(']') else {
        return;
    };
    for part in header[open + 1..open + close].split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("ahead ") {
            status.ahead = value.parse().unwrap_or(0);
        } else if let Some(value) = part.strip_prefix("behind ") {
            status.behind = value.parse().unwrap_or(0);
        }
    }
}

fn is_conflict(code: &[u8]) -> bool {
    matches!(code, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU")
}

pub fn parse_worktree_status(input: &[u8]) -> WorkingTreeStatus {
    let records = input.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut status = WorkingTreeStatus::default();
    let mut index = 0;

    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            continue;
        }
        if record.starts_with(b"## ") {
            parse_tracking(&lossy(record), &mut status);
            continue;
        }
        if record.len() < 2 {
            continue;
        }

        let code = &record[..2];
        if code == b"??" {
            status.untracked += 1;
        } else if code == b"!!" {
            // Ignored entries are not normally requested, but do not count them
            // if a caller supplies porcelain output containing `!!`.
        } else if is_conflict(code) {
            status.conflicted += 1;
        } else {
            if code[0] != b' ' {
                status.staged += 1;
            }
            if code[1] != b' ' {
                status.modified += 1;
            }
        }

        // In porcelain v1 -z output a rename/copy is followed by the original
        // path as a separate NUL record. It has no XY prefix and must be skipped.
        if matches!(code[0], b'R' | b'C') || matches!(code[1], b'R' | b'C') {
            index = index.saturating_add(1);
        }
    }

    status
}

pub fn parse_worktree_changes(input: &[u8]) -> Vec<WorkingTreeChange> {
    let records = input.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;

    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.len() < 4 || record.starts_with(b"## ") {
            continue;
        }

        let index_status = record[0] as char;
        let worktree_status = record[1] as char;
        let path = GitPath::from_bytes(record[3..].to_vec());
        let renamed_or_copied =
            matches!(record[0], b'R' | b'C') || matches!(record[1], b'R' | b'C');
        let old_path = if renamed_or_copied && index < records.len() {
            let old = records[index];
            index += 1;
            (!old.is_empty()).then(|| GitPath::from_bytes(old.to_vec()))
        } else {
            None
        };

        changes.push(WorkingTreeChange {
            index_status,
            worktree_status,
            path,
            old_path,
        });
    }

    changes
}

pub fn parse_branches(input: &[u8]) -> Vec<Branch> {
    let text = String::from_utf8_lossy(input);
    let mut branches = text
        .lines()
        .filter_map(|line| {
            let fields = line.split('\0').collect::<Vec<_>>();
            if fields.len() < 7 {
                return None;
            }
            let full_ref = fields[0].to_string();
            let name = fields[1].to_string();
            let kind = if full_ref.starts_with("refs/heads/") {
                BranchKind::Local
            } else if full_ref.starts_with("refs/remotes/") {
                BranchKind::Remote
            } else {
                return None;
            };
            if kind == BranchKind::Remote && name.ends_with("/HEAD") {
                return None;
            }
            Some(Branch {
                name: BranchName(name),
                full_ref,
                kind,
                head: CommitHash(fields[2].to_string()),
                short_head: fields[3].to_string(),
                commit_date: fields[4].to_string(),
                subject: fields[5].to_string(),
                is_current: fields[6].trim() == "*",
            })
        })
        .collect::<Vec<_>>();

    branches.sort_by(|left, right| {
        (!left.is_current)
            .cmp(&(!right.is_current))
            .then_with(|| {
                let left_kind = matches!(left.kind, BranchKind::Remote);
                let right_kind = matches!(right.kind, BranchKind::Remote);
                left_kind.cmp(&right_kind)
            })
            .then_with(|| left.name.cmp(&right.name))
    });
    branches
}

pub fn parse_commits(input: &[u8]) -> Vec<Commit> {
    input
        .split(|byte| *byte == 0x1e)
        .filter_map(|record| {
            let record = record.strip_suffix(b"\n").unwrap_or(record);
            if record.is_empty() {
                return None;
            }
            let fields = record
                .split(|byte| *byte == 0x1f)
                .map(lossy)
                .collect::<Vec<_>>();
            (fields.len() >= 6).then(|| Commit {
                hash: CommitHash(fields[0].clone()),
                short_hash: fields[1].clone(),
                author: fields[2].clone(),
                authored_at: fields[3].clone(),
                decorations: fields[4].clone(),
                subject: fields[5].trim_end_matches('\n').to_string(),
            })
        })
        .collect()
}

pub fn parse_reflog(input: &[u8]) -> Vec<ReflogEntry> {
    input
        .split(|byte| *byte == 0x1e)
        .filter_map(|record| {
            let record = record.strip_suffix(b"\n").unwrap_or(record);
            if record.is_empty() {
                return None;
            }
            let fields = record
                .split(|byte| *byte == 0x1f)
                .map(lossy)
                .collect::<Vec<_>>();
            if fields.len() < 6 {
                return None;
            }
            let (action, message) = fields[3]
                .split_once(": ")
                .map_or((fields[3].as_str(), ""), |(action, message)| {
                    (action, message)
                });
            Some(ReflogEntry {
                hash: CommitHash(fields[0].clone()),
                short_hash: fields[1].clone(),
                selector: fields[2].clone(),
                action: action.to_string(),
                message: message.trim_end_matches('\n').to_string(),
                author: fields[4].clone(),
                authored_at: fields[5].clone(),
            })
        })
        .collect()
}

#[derive(Clone, Debug)]
struct CommitMetadata {
    commit: Commit,
    author_email: String,
    committer: String,
    committer_email: String,
    committed_at: String,
    message: String,
}

fn parse_commit_metadata(input: &[u8]) -> Result<CommitMetadata, GitFailure> {
    let record = input
        .split(|byte| *byte == 0x1e)
        .find(|record| !record.is_empty())
        .ok_or_else(|| GitFailure {
            command: "git show --no-patch".into(),
            stderr: "Git returned no commit metadata".into(),
        })?;
    let fields = record
        .splitn(10, |byte| *byte == 0x1f)
        .map(lossy)
        .collect::<Vec<_>>();
    if fields.len() < 10 {
        return Err(GitFailure {
            command: "git show --no-patch".into(),
            stderr: format!("Expected 10 metadata fields, got {}", fields.len()),
        });
    }

    Ok(CommitMetadata {
        commit: Commit {
            hash: CommitHash(fields[0].trim().to_string()),
            short_hash: fields[1].clone(),
            author: fields[2].clone(),
            authored_at: fields[4].clone(),
            decorations: String::new(),
            subject: fields[8].clone(),
        },
        author_email: fields[3].clone(),
        committer: fields[5].clone(),
        committer_email: fields[6].clone(),
        committed_at: fields[7].clone(),
        message: fields[9].trim_end().to_string(),
    })
}

fn change_kind(status: &str) -> FileChangeKind {
    let similarity = status.get(1..).and_then(|value| value.parse::<u8>().ok());
    match status.as_bytes().first().copied() {
        Some(b'A') => FileChangeKind::Added,
        Some(b'C') => FileChangeKind::Copied { similarity },
        Some(b'D') => FileChangeKind::Deleted,
        Some(b'M') => FileChangeKind::Modified,
        Some(b'R') => FileChangeKind::Renamed { similarity },
        Some(b'T') => FileChangeKind::TypeChanged,
        Some(b'U') => FileChangeKind::Unmerged,
        _ => FileChangeKind::Unknown(status.to_string()),
    }
}

fn parse_name_status(input: &[u8]) -> Vec<ChangedFile> {
    let tokens = input
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;

    while index < tokens.len() {
        let token = tokens[index];
        index += 1;

        let (status, inline_path) = if let Some(tab) = token.iter().position(|byte| *byte == b'\t')
        {
            (lossy(&token[..tab]), Some(&token[tab + 1..]))
        } else {
            (lossy(token), None)
        };
        if status.is_empty() || !status.as_bytes()[0].is_ascii_alphabetic() {
            continue;
        }
        let kind = change_kind(&status);
        let is_pair = matches!(
            kind,
            FileChangeKind::Renamed { .. } | FileChangeKind::Copied { .. }
        );

        let first_path = match inline_path {
            Some(path) => GitPath::from_bytes(path.to_vec()),
            None if index < tokens.len() => {
                let path = GitPath::from_bytes(tokens[index].to_vec());
                index += 1;
                path
            }
            None => break,
        };
        let (old_path, path) = if is_pair && index < tokens.len() {
            let new_path = GitPath::from_bytes(tokens[index].to_vec());
            index += 1;
            (Some(first_path), new_path)
        } else {
            (None, first_path)
        };

        files.push(ChangedFile {
            kind,
            path,
            old_path,
            additions: None,
            deletions: None,
            hunks: Vec::new(),
            is_binary: false,
        });
    }

    files
}

#[derive(Clone, Debug)]
struct Numstat {
    path: GitPath,
    old_path: Option<GitPath>,
    additions: Option<usize>,
    deletions: Option<usize>,
}

fn count_field(value: &[u8]) -> Option<usize> {
    (value != b"-").then(|| lossy(value).parse().ok()).flatten()
}

fn parse_numstat(input: &[u8]) -> Vec<Numstat> {
    let tokens = input.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut stats = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        index += 1;
        if token.is_empty() {
            continue;
        }
        let mut fields = token.splitn(3, |byte| *byte == b'\t');
        let Some(additions) = fields.next() else {
            continue;
        };
        let Some(deletions) = fields.next() else {
            continue;
        };
        let Some(path) = fields.next() else {
            continue;
        };
        let additions = count_field(additions);
        let deletions = count_field(deletions);

        if path.is_empty() && index + 1 < tokens.len() {
            let old_path = GitPath::from_bytes(tokens[index].to_vec());
            let new_path = GitPath::from_bytes(tokens[index + 1].to_vec());
            index += 2;
            stats.push(Numstat {
                path: new_path,
                old_path: Some(old_path),
                additions,
                deletions,
            });
        } else {
            stats.push(Numstat {
                path: GitPath::from_bytes(path.to_vec()),
                old_path: None,
                additions,
                deletions,
            });
        }
    }
    stats
}

#[derive(Clone, Debug)]
struct ParsedPatch {
    old_path: Option<GitPath>,
    path: Option<GitPath>,
    header: Vec<String>,
    hunks: Vec<DiffHunk>,
    is_binary: bool,
}

impl ParsedPatch {
    fn new() -> Self {
        Self {
            old_path: None,
            path: None,
            header: Vec::new(),
            hunks: Vec::new(),
            is_binary: false,
        }
    }
}

fn decode_quoted_path(value: &str) -> Vec<u8> {
    if !(value.starts_with('"') && value.ends_with('"')) {
        return value.as_bytes().to_vec();
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 1;
    while index + 1 < bytes.len() {
        if bytes[index] != b'\\' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        index += 1;
        if index + 1 >= bytes.len() {
            break;
        }
        match bytes[index] {
            b'n' => decoded.push(b'\n'),
            b'r' => decoded.push(b'\r'),
            b't' => decoded.push(b'\t'),
            b'\\' => decoded.push(b'\\'),
            b'"' => decoded.push(b'"'),
            digit @ b'0'..=b'7' => {
                let mut value = digit - b'0';
                let mut consumed = 1;
                while consumed < 3 && index + consumed < bytes.len() - 1 {
                    let next = bytes[index + consumed];
                    if !(b'0'..=b'7').contains(&next) {
                        break;
                    }
                    value = value.saturating_mul(8).saturating_add(next - b'0');
                    consumed += 1;
                }
                decoded.push(value);
                index += consumed - 1;
            }
            other => decoded.push(other),
        }
        index += 1;
    }
    decoded
}

fn header_path(line: &str, marker: &str, prefix: &str) -> Option<GitPath> {
    let raw = line.strip_prefix(marker)?.trim();
    if raw == "/dev/null" {
        return None;
    }
    let mut decoded = decode_quoted_path(raw);
    if decoded.starts_with(prefix.as_bytes()) {
        decoded.drain(..prefix.len());
    }
    Some(GitPath::from_bytes(decoded))
}

fn strip_diff_prefix(mut path: Vec<u8>, prefix: &[u8]) -> GitPath {
    if path.starts_with(prefix) {
        path.drain(..prefix.len());
    }
    GitPath::from_bytes(path)
}

fn diff_header_paths(line: &str) -> Option<(GitPath, GitPath)> {
    let value = line.strip_prefix("diff --git ")?;
    let split = value.rfind(" \"b/").or_else(|| value.rfind(" b/"))?;
    let old = value[..split].trim();
    let new = value[split + 1..].trim();
    Some((
        strip_diff_prefix(decode_quoted_path(old), b"a/"),
        strip_diff_prefix(decode_quoted_path(new), b"b/"),
    ))
}

fn parse_range(token: &str, marker: char) -> Option<(u32, u32)> {
    let token = token.strip_prefix(marker)?;
    let (start, count) = token
        .split_once(',')
        .map_or((token, "1"), |(start, count)| (start, count));
    Some((start.parse().ok()?, count.parse().ok()?))
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    if !line.starts_with("@@ ") {
        return None;
    }
    let mut fields = line.split_whitespace();
    fields.next()?;
    let (old_start, old_count) = parse_range(fields.next()?, '-')?;
    let (new_start, new_count) = parse_range(fields.next()?, '+')?;
    Some((old_start, old_count, new_start, new_count))
}

fn parse_patch_set(input: &[u8]) -> Vec<ParsedPatch> {
    let text = String::from_utf8_lossy(input);
    let mut patches = Vec::new();
    let mut current: Option<ParsedPatch> = None;
    let mut current_hunk: Option<DiffHunk> = None;
    let mut old_line = 0;
    let mut new_line = 0;

    let finish_hunk = |patch: &mut ParsedPatch, hunk: &mut Option<DiffHunk>| {
        if let Some(hunk) = hunk.take() {
            patch.hunks.push(hunk);
        }
    };
    let finish_patch = |patches: &mut Vec<ParsedPatch>,
                        current: &mut Option<ParsedPatch>,
                        hunk: &mut Option<DiffHunk>| {
        if let Some(mut patch) = current.take() {
            if let Some(hunk) = hunk.take() {
                patch.hunks.push(hunk);
            }
            patches.push(patch);
        }
    };

    for line in text.lines() {
        if line.starts_with("diff --git ") {
            finish_patch(&mut patches, &mut current, &mut current_hunk);
            let mut patch = ParsedPatch::new();
            if let Some((old_path, path)) = diff_header_paths(line) {
                patch.old_path = Some(old_path);
                patch.path = Some(path);
            }
            patch.header.push(line.to_string());
            current = Some(patch);
            continue;
        }

        let Some(patch) = current.as_mut() else {
            continue;
        };

        if let Some((old_start, old_count, new_start, new_count)) = parse_hunk_header(line) {
            finish_hunk(patch, &mut current_hunk);
            old_line = old_start;
            new_line = new_start;
            current_hunk = Some(DiffHunk {
                header: line.to_string(),
                old_start,
                old_count,
                new_start,
                new_count,
                lines: Vec::new(),
            });
            continue;
        }

        if let Some(hunk) = current_hunk.as_mut() {
            let (kind, text, old_no, new_no) = if let Some(text) = line.strip_prefix(' ') {
                let old_no = old_line;
                let new_no = new_line;
                old_line = old_line.saturating_add(1);
                new_line = new_line.saturating_add(1);
                (DiffLineKind::Context, text, Some(old_no), Some(new_no))
            } else if let Some(text) = line.strip_prefix('-') {
                let old_no = old_line;
                old_line = old_line.saturating_add(1);
                (DiffLineKind::Deletion, text, Some(old_no), None)
            } else if let Some(text) = line.strip_prefix('+') {
                let new_no = new_line;
                new_line = new_line.saturating_add(1);
                (DiffLineKind::Addition, text, None, Some(new_no))
            } else {
                (DiffLineKind::Metadata, line, None, None)
            };
            hunk.lines.push(DiffLine {
                old_line_no: old_no,
                new_line_no: new_no,
                kind,
                text: text.to_string(),
            });
            continue;
        }

        if line.starts_with("--- ") {
            patch.old_path = header_path(line, "--- ", "a/");
        } else if line.starts_with("+++ ") {
            patch.path = header_path(line, "+++ ", "b/");
        } else if let Some(path) = line.strip_prefix("rename from ") {
            patch.old_path = Some(GitPath::from_bytes(decode_quoted_path(path)));
        } else if let Some(path) = line.strip_prefix("rename to ") {
            patch.path = Some(GitPath::from_bytes(decode_quoted_path(path)));
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            patch.is_binary = true;
        }
        patch.header.push(line.to_string());
    }

    finish_patch(&mut patches, &mut current, &mut current_hunk);
    patches
}

fn patch_matches(patch: &ParsedPatch, path: &GitPath, old_path: Option<&GitPath>) -> bool {
    patch.path.as_ref() == Some(path)
        || patch.old_path.as_ref() == Some(path)
        || old_path.is_some_and(|old_path| {
            patch.old_path.as_ref() == Some(old_path) || patch.path.as_ref() == Some(old_path)
        })
}

pub fn parse_file_diff(
    input: &[u8],
    commit: CommitHash,
    path: GitPath,
    old_path: Option<GitPath>,
) -> FileDiff {
    let patches = parse_patch_set(input);
    let patch = patches
        .iter()
        .find(|patch| patch_matches(patch, &path, old_path.as_ref()))
        .or_else(|| patches.first());

    match patch {
        Some(patch) => FileDiff {
            commit,
            path: patch.path.clone().unwrap_or(path),
            old_path: patch.old_path.clone().or(old_path),
            header: patch.header.clone(),
            hunks: patch.hunks.clone(),
            is_binary: patch.is_binary,
        },
        None => FileDiff::empty(commit, path, old_path),
    }
}

pub fn parse_commit_detail(
    metadata: &[u8],
    name_status: &[u8],
    numstat: &[u8],
    patch: &[u8],
) -> Result<CommitDetail, GitFailure> {
    let metadata = parse_commit_metadata(metadata)?;
    let mut files = parse_name_status(name_status);
    let stats = parse_numstat(numstat);
    let patches = parse_patch_set(patch);

    let stats_by_path = stats
        .iter()
        .map(|stat| (stat.path.clone(), stat))
        .collect::<HashMap<_, _>>();

    for file in &mut files {
        if let Some(stat) = stats_by_path.get(&file.path) {
            file.additions = stat.additions;
            file.deletions = stat.deletions;
            if file.old_path.is_none() {
                file.old_path = stat.old_path.clone();
            }
        }
        if let Some(parsed_patch) = patches
            .iter()
            .find(|patch| patch_matches(patch, &file.path, file.old_path.as_ref()))
        {
            file.hunks = parsed_patch.hunks.iter().map(DiffHunk::summary).collect();
            file.is_binary = parsed_patch.is_binary;
        }
    }

    // A defensive fallback for unusual Git output (for example some merge
    // forms): never hide a patch merely because name-status omitted it.
    for parsed_patch in &patches {
        let Some(path) = parsed_patch.path.clone().or(parsed_patch.old_path.clone()) else {
            continue;
        };
        if files.iter().any(|file| file.path == path) {
            continue;
        }
        let stat = stats_by_path.get(&path).copied();
        files.push(ChangedFile {
            kind: FileChangeKind::Modified,
            path,
            old_path: parsed_patch.old_path.clone(),
            additions: stat.and_then(|stat| stat.additions),
            deletions: stat.and_then(|stat| stat.deletions),
            hunks: parsed_patch.hunks.iter().map(DiffHunk::summary).collect(),
            is_binary: parsed_patch.is_binary,
        });
    }

    Ok(CommitDetail {
        commit: metadata.commit,
        author_email: metadata.author_email,
        committer: metadata.committer,
        committer_email: metadata.committer_email,
        committed_at: metadata.committed_at,
        message: metadata.message,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_counts_tracking_and_rename() {
        let input = b"## main...origin/main [ahead 2, behind 1]\0M  staged\0 M modified\0?? new\0UU conflict\0R  renamed\0old-name\0";
        let status = parse_worktree_status(input);
        assert_eq!(status.staged, 2);
        assert_eq!(status.modified, 1);
        assert_eq!(status.untracked, 1);
        assert_eq!(status.conflicted, 1);
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
    }

    #[test]
    fn parses_worktree_changes_with_untracked_and_rename_paths() {
        let input = b"M  staged.txt\0 M modified.txt\0?? untracked.txt\0R  new.txt\0old.txt\0UU conflict.txt\0";
        let changes = parse_worktree_changes(input);
        assert_eq!(changes.len(), 5);
        assert_eq!(changes[0].status_code(), "M ");
        assert!(changes[2].is_untracked());
        assert_eq!(changes[3].path.as_str(), "new.txt");
        assert_eq!(changes[3].old_path.as_ref().unwrap().as_str(), "old.txt");
        assert!(changes[4].is_conflicted());
    }

    #[test]
    fn parses_commit_records() {
        let input = b"\x1eabc\x1fabc1234\x1fAda\x1f2026-01-01T00:00:00Z\x1fHEAD -> main\x1fSubject\n\x1edef\x1fdef1234\x1fLin\x1f2026-01-02T00:00:00Z\x1f\x1fNext\n";
        let commits = parse_commits(input);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "Subject");
        assert_eq!(commits[1].decorations, "");
    }

    #[test]
    fn parses_reflog_records_and_splits_action() {
        let input = b"\x1e0123456789\x1f0123456\x1fHEAD@{0}\x1fcommit: add feature\x1fAda\x1f2026-01-01T00:00:00Z\n\x1eabcdef\x1fabcdef0\x1fHEAD@{1}\x1fcheckout: moving from main to feature\x1fAda\x1f2025-12-31T00:00:00Z\n";
        let entries = parse_reflog(input);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, "commit");
        assert_eq!(entries[0].message, "add feature");
        assert_eq!(entries[1].selector, "HEAD@{1}");
        assert_eq!(entries[1].action, "checkout");
    }

    #[test]
    fn parses_unified_diff_and_side_numbers() {
        let patch = b"diff --git a/a.rs b/a.rs\nindex 111..222 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@ fn main\n context\n-old\n+new\n+more\n";
        let diff = parse_file_diff(patch, CommitHash("abc".into()), GitPath::from("a.rs"), None);
        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(diff.hunks[0].lines[0].old_line_no, Some(1));
        assert_eq!(diff.hunks[0].lines[1].kind, DiffLineKind::Deletion);
        assert_eq!(diff.hunks[0].lines[2].new_line_no, Some(2));
        assert_eq!(diff.summaries()[0].additions, 2);
    }

    #[test]
    fn parses_binary_numstat() {
        let stats = parse_numstat(b"-\t-\timage.png\0");
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].additions, None);
        assert_eq!(stats[0].deletions, None);
    }

    #[test]
    fn parses_rename_name_status() {
        let files = parse_name_status(b"R095\0old.rs\0new.rs\0");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_str(), "new.rs");
        assert_eq!(files[0].old_path.as_ref().unwrap().as_str(), "old.rs");
        assert!(matches!(
            files[0].kind,
            FileChangeKind::Renamed {
                similarity: Some(95)
            }
        ));
    }

    #[test]
    fn preserves_raw_non_utf8_path_bytes() {
        let files = parse_name_status(b"M\0non-utf8-\xff.txt\0");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_bytes(), b"non-utf8-\xff.txt");

        let patch = b"diff --git \"a/non-utf8-\\377.txt\" \"b/non-utf8-\\377.txt\"\n--- \"a/non-utf8-\\377.txt\"\n+++ \"b/non-utf8-\\377.txt\"\n@@ -0,0 +1 @@\n+content\n";
        let diff = parse_file_diff(patch, CommitHash("abc".into()), files[0].path.clone(), None);
        assert_eq!(diff.path.as_bytes(), b"non-utf8-\xff.txt");
        assert_eq!(diff.hunks.len(), 1);
    }

    #[test]
    fn associates_binary_patch_without_file_markers() {
        let patch = b"diff --git a/image.bin b/image.bin\nindex 1111111..2222222 100644\nBinary files a/image.bin and b/image.bin differ\n";
        let diff = parse_file_diff(
            patch,
            CommitHash("abc".into()),
            GitPath::from("image.bin"),
            None,
        );
        assert!(diff.is_binary);
        assert_eq!(diff.path.as_str(), "image.bin");

        let spaced = diff_header_paths("diff --git a/a b.txt b/a b.txt").unwrap();
        assert_eq!(spaced.0.as_str(), "a b.txt");
        assert_eq!(spaced.1.as_str(), "a b.txt");
    }
}
