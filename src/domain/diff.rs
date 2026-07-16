use super::{CommitHash, GitPath, HunkSummary};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
    Metadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffLine {
    pub old_line_no: Option<u32>,
    pub new_line_no: Option<u32>,
    pub kind: DiffLineKind,
    /// Text without the leading unified-diff marker.
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

impl DiffHunk {
    pub fn summary(&self) -> HunkSummary {
        HunkSummary {
            header: self.header.clone(),
            additions: self
                .lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Addition)
                .count(),
            deletions: self
                .lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Deletion)
                .count(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileDiff {
    pub commit: CommitHash,
    pub path: GitPath,
    pub old_path: Option<GitPath>,
    pub header: Vec<String>,
    pub hunks: Vec<DiffHunk>,
    pub is_binary: bool,
}

impl FileDiff {
    pub fn empty(commit: CommitHash, path: GitPath, old_path: Option<GitPath>) -> Self {
        Self {
            commit,
            path,
            old_path,
            header: Vec::new(),
            hunks: Vec::new(),
            is_binary: false,
        }
    }

    pub fn summaries(&self) -> Vec<HunkSummary> {
        self.hunks.iter().map(DiffHunk::summary).collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffCellKind {
    Empty,
    Context,
    Added,
    Deleted,
    Modified,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SideBySideRow {
    pub left_line_no: Option<u32>,
    pub left_text: Option<String>,
    pub left_kind: DiffCellKind,
    pub right_line_no: Option<u32>,
    pub right_text: Option<String>,
    pub right_kind: DiffCellKind,
}

impl SideBySideRow {
    fn context(line: &DiffLine) -> Self {
        Self {
            left_line_no: line.old_line_no,
            left_text: Some(line.text.clone()),
            left_kind: DiffCellKind::Context,
            right_line_no: line.new_line_no,
            right_text: Some(line.text.clone()),
            right_kind: DiffCellKind::Context,
        }
    }

    fn pair(deleted: Option<&DiffLine>, added: Option<&DiffLine>) -> Self {
        let modified = deleted.is_some() && added.is_some();
        Self {
            left_line_no: deleted.and_then(|line| line.old_line_no),
            left_text: deleted.map(|line| line.text.clone()),
            left_kind: match (deleted.is_some(), modified) {
                (false, _) => DiffCellKind::Empty,
                (true, true) => DiffCellKind::Modified,
                (true, false) => DiffCellKind::Deleted,
            },
            right_line_no: added.and_then(|line| line.new_line_no),
            right_text: added.map(|line| line.text.clone()),
            right_kind: match (added.is_some(), modified) {
                (false, _) => DiffCellKind::Empty,
                (true, true) => DiffCellKind::Modified,
                (true, false) => DiffCellKind::Added,
            },
        }
    }
}

/// Converts a parsed hunk to aligned rows. Consecutive deletion/addition runs
/// are paired by position, which is considerably more useful than pairing only
/// one adjacent `-`/`+` line.
pub fn side_by_side_rows(hunk: &DiffHunk) -> Vec<SideBySideRow> {
    let mut rows = Vec::new();
    let mut index = 0;

    while index < hunk.lines.len() {
        match hunk.lines[index].kind {
            DiffLineKind::Context => {
                rows.push(SideBySideRow::context(&hunk.lines[index]));
                index += 1;
            }
            DiffLineKind::Metadata => {
                let line = &hunk.lines[index];
                rows.push(SideBySideRow {
                    left_line_no: None,
                    left_text: Some(line.text.clone()),
                    left_kind: DiffCellKind::Context,
                    right_line_no: None,
                    right_text: Some(line.text.clone()),
                    right_kind: DiffCellKind::Context,
                });
                index += 1;
            }
            DiffLineKind::Deletion => {
                let delete_start = index;
                while index < hunk.lines.len() && hunk.lines[index].kind == DiffLineKind::Deletion {
                    index += 1;
                }
                let add_start = index;
                while index < hunk.lines.len() && hunk.lines[index].kind == DiffLineKind::Addition {
                    index += 1;
                }

                let deletes = &hunk.lines[delete_start..add_start];
                let adds = &hunk.lines[add_start..index];
                for pair_index in 0..deletes.len().max(adds.len()) {
                    rows.push(SideBySideRow::pair(
                        deletes.get(pair_index),
                        adds.get(pair_index),
                    ));
                }
            }
            DiffLineKind::Addition => {
                rows.push(SideBySideRow::pair(None, Some(&hunk.lines[index])));
                index += 1;
            }
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_delete_and_add_runs() {
        let hunk = DiffHunk {
            header: "@@ -1,2 +1,3 @@".into(),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine {
                    old_line_no: Some(1),
                    new_line_no: None,
                    kind: DiffLineKind::Deletion,
                    text: "old one".into(),
                },
                DiffLine {
                    old_line_no: Some(2),
                    new_line_no: None,
                    kind: DiffLineKind::Deletion,
                    text: "old two".into(),
                },
                DiffLine {
                    old_line_no: None,
                    new_line_no: Some(1),
                    kind: DiffLineKind::Addition,
                    text: "new one".into(),
                },
                DiffLine {
                    old_line_no: None,
                    new_line_no: Some(2),
                    kind: DiffLineKind::Addition,
                    text: "new two".into(),
                },
                DiffLine {
                    old_line_no: None,
                    new_line_no: Some(3),
                    kind: DiffLineKind::Addition,
                    text: "new three".into(),
                },
            ],
        };

        let rows = side_by_side_rows(&hunk);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].left_kind, DiffCellKind::Modified);
        assert_eq!(rows[1].right_kind, DiffCellKind::Modified);
        assert_eq!(rows[2].left_kind, DiffCellKind::Empty);
        assert_eq!(rows[2].right_kind, DiffCellKind::Added);
    }
}
