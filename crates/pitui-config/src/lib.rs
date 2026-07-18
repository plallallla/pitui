//! Strict configuration boundary for Dataset templates, Operations, Render
//! Proxies and Render Modes.
//!
//! Parsing and effective-config resolution will migrate here after the ECS
//! registries are stable. This crate must never permit shell or Git argv
//! templates to be injected from configuration.

#![forbid(unsafe_code)]

use std::{env, path::PathBuf, time::Duration};

use pitui_data::{
    AvailabilityRule, AvailabilityRuleId, CommandId, CommandScope, CommandSpec, CommandSystemId,
    DatasetBinding, DatasetIdentity, DatasetKind, DatasetTemplate, DatasetTemplateId,
    DateTimePrecision, FieldFormat, FieldId, FieldSpec, InteractionContextType, KeyCode,
    KeySequence, KeyStroke, LayoutConstraint, NavigationModeRegistry, OperationId, OperationSpec,
    RenderBindingId, RenderLayout, RenderModeId, RenderModeSpec, RenderProxyId, RenderProxySpec,
    RendererKind, StyleSpec, TargetSource,
};

/// Version of the next-generation configuration schema being built.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoggingLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitLoggingConfig {
    pub enabled: bool,
    pub level: LoggingLevel,
    pub path: PathBuf,
    pub max_bytes: u64,
    pub keep_files: usize,
    pub rotate_on_start: bool,
    pub flush_interval: Duration,
    pub buffer_capacity: usize,
    pub max_message_chars: usize,
    pub fail_on_open_error: bool,
}

pub fn default_git_logging_config() -> GitLoggingConfig {
    GitLoggingConfig {
        enabled: true,
        level: LoggingLevel::Info,
        path: default_git_log_path(),
        max_bytes: 5 * 1024 * 1024,
        keep_files: 3,
        rotate_on_start: false,
        flush_interval: Duration::from_secs(1),
        buffer_capacity: 16 * 1024,
        max_message_chars: 4096,
        fail_on_open_error: false,
    }
}

pub fn default_git_log_path() -> PathBuf {
    if let Some(path) = env::var_os("PITUI_GIT_LOG_PATH").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(root) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(root)
            .join("pitui")
            .join("git-operations.jsonl");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join("pitui")
            .join("git-operations.jsonl");
    }
    if let Some(root) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(root)
            .join("pitui")
            .join("git-operations.jsonl");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("pitui")
            .join("git-operations.jsonl");
    }
    env::temp_dir().join("pitui-git-operations.jsonl")
}

/// Built-in templates are ordinary data and go through the same registry as
/// future TOML overrides. Keeping them here avoids hard-coded template choices
/// in Git systems or renderers.
pub fn builtin_dataset_templates() -> Vec<DatasetTemplate> {
    use DatasetKind as Kind;

    [
        (Kind::RepositoriesBranches, "repositories-branches"),
        (Kind::Repository, "repository"),
        (Kind::Branch, "branch"),
        (Kind::Commits, "commits"),
        (Kind::Commit, "commit"),
        (Kind::Files, "files"),
        (Kind::File, "file"),
        (Kind::FileChanges, "file-changes"),
        (Kind::Changes, "changes"),
        (Kind::WorkingTreeFiles, "working-tree-files"),
        (Kind::WorkingTreeFile, "working-tree-file"),
        (Kind::WorkingTreeFileChanges, "working-tree-file-changes"),
        (Kind::CommitCreation, "commit-creation"),
        (Kind::Reflog, "reflog"),
        (Kind::ReflogEntry, "reflog-entry"),
        (Kind::Remotes, "remotes"),
        (Kind::Remote, "remote"),
        (Kind::InteractionContext, "interaction-context"),
        (Kind::GitOperationLog, "git-operation-log"),
        (Kind::GitOperationLogEntry, "git-operation-log-entry"),
    ]
    .into_iter()
    .map(|(kind, name)| DatasetTemplate {
        id: DatasetTemplateId::from(name),
        kind,
        operations: operations_for_dataset(kind),
        render_proxies: builtin_proxies(kind),
    })
    .collect()
}

/// Built-in render interpretations. Dataset templates only reference these IDs;
/// the projection system resolves the actual typed specification from its
/// registry and rejects kind mismatches instead of guessing from a string.
pub fn builtin_render_proxies() -> Vec<RenderProxySpec> {
    use DatasetKind as Kind;
    use FieldId as Field;
    use RendererKind as Renderer;

    vec![
        proxy(
            "repositories-branches.compact",
            Kind::RepositoriesBranches,
            Renderer::Tree,
            vec![
                plain(Field::BranchCurrentMarker),
                plain(Field::DatasetLabel),
            ],
        ),
        proxy(
            "repository.detail",
            Kind::Repository,
            Renderer::Detail,
            vec![
                labeled(Field::RepositoryName, "Repository"),
                labeled(Field::RepositoryPath, "Path"),
                labeled(Field::RepositoryCurrentBranch, "Branch"),
            ],
        ),
        proxy(
            "branch.detail",
            Kind::Branch,
            Renderer::Detail,
            vec![
                labeled(Field::BranchName, "Branch"),
                labeled(Field::BranchHead, "Head"),
                datetime(Field::BranchAuthoredAt, DateTimePrecision::Minute),
                labeled(Field::BranchSubject, "Subject"),
            ],
        ),
        proxy(
            "commits.compact",
            Kind::Commits,
            Renderer::List,
            vec![hash(Field::CommitHash, 8), plain(Field::CommitSubject)],
        ),
        proxy(
            "commits.detailed",
            Kind::Commits,
            Renderer::List,
            vec![
                hash(Field::CommitHash, 8),
                datetime(Field::CommitAuthoredAt, DateTimePrecision::Minute),
                plain(Field::CommitAuthor),
                joined(Field::CommitTags, ", "),
                plain(Field::CommitSubject),
            ],
        ),
        proxy(
            "commit.detail",
            Kind::Commit,
            Renderer::CommitDetail,
            vec![
                labeled(Field::CommitHash, "Commit"),
                labeled(Field::CommitAuthor, "Author"),
                labeled_datetime(Field::CommitAuthoredAt, "Date", DateTimePrecision::Minute),
                labeled(Field::CommitTags, "Tags"),
                labeled(Field::CommitSubject, "Subject"),
                labeled(Field::CommitMessage, "Message"),
            ],
        ),
        proxy(
            "files.list",
            Kind::Files,
            Renderer::List,
            vec![
                plain(Field::FileStatus),
                plain(Field::FilePath),
                plain(Field::FileAdditions),
                plain(Field::FileDeletions),
            ],
        ),
        proxy(
            "file.detail",
            Kind::File,
            Renderer::Detail,
            vec![
                labeled(Field::FileStatus, "Status"),
                labeled(Field::FilePath, "Path"),
                labeled(Field::FileOldPath, "Old path"),
                labeled(Field::FileAdditions, "Additions"),
                labeled(Field::FileDeletions, "Deletions"),
                labeled(Field::FileBinary, "Binary"),
            ],
        ),
        proxy(
            "file-changes.unified",
            Kind::FileChanges,
            Renderer::UnifiedDiff,
            Vec::new(),
        ),
        proxy(
            "file-changes.side-by-side",
            Kind::FileChanges,
            Renderer::SideBySideDiff,
            Vec::new(),
        ),
        proxy(
            "changes.tree",
            Kind::Changes,
            Renderer::Tree,
            vec![plain(Field::FileStatus), plain(Field::DatasetLabel)],
        ),
        proxy(
            "working-tree-files.list",
            Kind::WorkingTreeFiles,
            Renderer::List,
            vec![plain(Field::FileStatus), plain(Field::FilePath)],
        ),
        proxy(
            "working-tree-file.detail",
            Kind::WorkingTreeFile,
            Renderer::Detail,
            vec![
                labeled(Field::FileStatus, "Status"),
                labeled(Field::FilePath, "Path"),
            ],
        ),
        proxy(
            "working-tree-file-changes.unified",
            Kind::WorkingTreeFileChanges,
            Renderer::UnifiedDiff,
            Vec::new(),
        ),
        proxy(
            "working-tree-file-changes.side-by-side",
            Kind::WorkingTreeFileChanges,
            Renderer::SideBySideDiff,
            Vec::new(),
        ),
        proxy(
            "commit-creation.editor",
            Kind::CommitCreation,
            Renderer::CommitCreation,
            vec![
                labeled(Field::CommitCreationStagedFiles, "Staged files"),
                labeled(Field::CommitCreationMessage, "Message"),
                labeled(Field::CommitCreationError, "Error"),
            ],
        ),
        proxy(
            "reflog.list",
            Kind::Reflog,
            Renderer::List,
            vec![
                plain(Field::ReflogSelector),
                hash(Field::ReflogHash, 8),
                datetime(Field::ReflogAuthoredAt, DateTimePrecision::Minute),
                plain(Field::ReflogAction),
                plain(Field::ReflogMessage),
            ],
        ),
        proxy(
            "reflog-entry.detail",
            Kind::ReflogEntry,
            Renderer::Detail,
            vec![
                labeled(Field::ReflogSelector, "Selector"),
                labeled(Field::ReflogHash, "Commit"),
                labeled(Field::ReflogAuthor, "Author"),
                labeled_datetime(Field::ReflogAuthoredAt, "Date", DateTimePrecision::Minute),
                labeled(Field::ReflogAction, "Action"),
                labeled(Field::ReflogMessage, "Message"),
            ],
        ),
        proxy(
            "remotes.list",
            Kind::Remotes,
            Renderer::List,
            vec![
                plain(Field::RemoteName),
                plain(Field::RemoteUpstream),
                plain(Field::RemotePushTarget),
                plain(Field::RemotePolicy),
            ],
        ),
        proxy(
            "remote.detail",
            Kind::Remote,
            Renderer::Detail,
            vec![
                labeled(Field::RemoteName, "Remote"),
                labeled(Field::RemoteFetchUrls, "Fetch"),
                labeled(Field::RemotePushUrls, "Push"),
                labeled(Field::RemoteUpstream, "Upstream"),
                labeled(Field::RemotePushTarget, "Push target"),
                labeled(Field::RemotePolicy, "URL policy"),
            ],
        ),
        proxy(
            "interaction-context.overlay",
            Kind::InteractionContext,
            Renderer::Confirmation,
            Vec::new(),
        ),
        proxy(
            "git-operation-log.list",
            Kind::GitOperationLog,
            Renderer::LogList,
            vec![
                plain(Field::GitOperationStartedAt),
                plain(Field::GitOperationStatus),
                plain(Field::GitOperationName),
                plain(Field::GitOperationDuration),
            ],
        ),
        proxy(
            "git-operation-log-entry.detail",
            Kind::GitOperationLogEntry,
            Renderer::Detail,
            vec![
                labeled(Field::GitOperationStartedAt, "Started"),
                labeled(Field::GitOperationStatus, "Status"),
                labeled(Field::GitOperationName, "Operation"),
                labeled(Field::GitOperationRepository, "Repository"),
                labeled(Field::GitOperationDuration, "Duration"),
                labeled(Field::GitOperationMessage, "Message"),
                labeled(Field::GitOperationAbort, "Abort"),
            ],
        ),
    ]
}

/// Reference layouts from the authoritative design. Every leaf is declarative:
/// stable singleton identities resolve through `DatasetIndex`, while current
/// objects resolve through the active context bindings.
pub fn builtin_render_modes() -> Vec<RenderModeSpec> {
    vec![
        RenderModeSpec {
            id: RenderModeId::from("history"),
            layout: RenderLayout::Row(vec![
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Stable(DatasetIdentity::GlobalRepositoriesBranches),
                    proxy: RenderProxyId::from("repositories-branches.compact"),
                    constraint: LayoutConstraint::Percentage(35),
                    focusable: true,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentCommits),
                    proxy: RenderProxyId::from("commits.detailed"),
                    constraint: LayoutConstraint::Fill(1),
                    focusable: true,
                },
            ]),
        },
        RenderModeSpec {
            id: RenderModeId::from("commit"),
            layout: RenderLayout::Row(vec![
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentCommits),
                    proxy: RenderProxyId::from("commits.compact"),
                    constraint: LayoutConstraint::Percentage(35),
                    focusable: true,
                },
                RenderLayout::Column(vec![
                    RenderLayout::Dataset {
                        dataset: DatasetBinding::Context(RenderBindingId::CurrentCommit),
                        proxy: RenderProxyId::from("commit.detail"),
                        constraint: LayoutConstraint::Percentage(40),
                        focusable: false,
                    },
                    RenderLayout::Dataset {
                        dataset: DatasetBinding::Context(RenderBindingId::CurrentFiles),
                        proxy: RenderProxyId::from("files.list"),
                        constraint: LayoutConstraint::Fill(1),
                        focusable: true,
                    },
                ]),
            ]),
        },
        file_diff_mode("file-diff.unified", "file-changes.unified"),
        file_diff_mode("file-diff.side-by-side", "file-changes.side-by-side"),
        changes_mode("changes.unified", "working-tree-file-changes.unified"),
        changes_mode(
            "changes.side-by-side",
            "working-tree-file-changes.side-by-side",
        ),
        RenderModeSpec {
            id: RenderModeId::from("reflog"),
            layout: RenderLayout::Row(vec![
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentReflog),
                    proxy: RenderProxyId::from("reflog.list"),
                    constraint: LayoutConstraint::Percentage(55),
                    focusable: true,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentReflogEntry),
                    proxy: RenderProxyId::from("reflog-entry.detail"),
                    constraint: LayoutConstraint::Fill(1),
                    focusable: false,
                },
            ]),
        },
        RenderModeSpec {
            id: RenderModeId::from("git-operation-log"),
            layout: RenderLayout::Row(vec![
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Stable(DatasetIdentity::GlobalGitOperationLog),
                    proxy: RenderProxyId::from("git-operation-log.list"),
                    constraint: LayoutConstraint::Percentage(45),
                    focusable: true,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentGitOperationLogEntry),
                    proxy: RenderProxyId::from("git-operation-log-entry.detail"),
                    constraint: LayoutConstraint::Fill(1),
                    focusable: false,
                },
            ]),
        },
    ]
}

pub fn builtin_navigation_modes() -> NavigationModeRegistry {
    NavigationModeRegistry {
        drill_down: [
            (DatasetKind::Commits, RenderModeId::from("commit")),
            (DatasetKind::Files, RenderModeId::from("file-diff.unified")),
        ]
        .into_iter()
        .collect(),
    }
}

fn file_diff_mode(id: &str, diff_proxy: &str) -> RenderModeSpec {
    RenderModeSpec {
        id: RenderModeId::from(id),
        layout: RenderLayout::Row(vec![
            RenderLayout::Column(vec![
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentCommit),
                    proxy: RenderProxyId::from("commit.detail"),
                    constraint: LayoutConstraint::Percentage(40),
                    focusable: false,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentFiles),
                    proxy: RenderProxyId::from("files.list"),
                    constraint: LayoutConstraint::Fill(1),
                    focusable: true,
                },
            ]),
            RenderLayout::Dataset {
                dataset: DatasetBinding::Context(RenderBindingId::CurrentFileChanges),
                proxy: RenderProxyId::from(diff_proxy),
                constraint: LayoutConstraint::Fill(2),
                focusable: true,
            },
        ]),
    }
}

fn changes_mode(id: &str, diff_proxy: &str) -> RenderModeSpec {
    RenderModeSpec {
        id: RenderModeId::from(id),
        layout: RenderLayout::Row(vec![
            RenderLayout::Dataset {
                dataset: DatasetBinding::Stable(DatasetIdentity::GlobalChanges),
                proxy: RenderProxyId::from("changes.tree"),
                constraint: LayoutConstraint::Percentage(40),
                focusable: true,
            },
            RenderLayout::Dataset {
                dataset: DatasetBinding::Context(RenderBindingId::CurrentChangesFileChanges),
                proxy: RenderProxyId::from(diff_proxy),
                constraint: LayoutConstraint::Fill(1),
                focusable: true,
            },
        ]),
    }
}

pub fn builtin_availability_rules() -> Vec<(AvailabilityRuleId, AvailabilityRule)> {
    vec![
        (AvailabilityRuleId::from("always"), AvailabilityRule::Always),
        (
            AvailabilityRuleId::from("has-cursor"),
            AvailabilityRule::HasCursor,
        ),
        (
            AvailabilityRuleId::from("has-selection"),
            AvailabilityRule::HasSelection,
        ),
        (
            AvailabilityRuleId::from("selection-or-cursor"),
            AvailabilityRule::HasSelectionOrCursor,
        ),
        (
            AvailabilityRuleId::from("current-files-selection-or-cursor"),
            AvailabilityRule::ContextHasSelectionOrCursor(RenderBindingId::CurrentFiles),
        ),
        (
            AvailabilityRuleId::from("normal-context"),
            AvailabilityRule::Not(Box::new(AvailabilityRule::Any(vec![
                AvailabilityRule::ActiveDatasetKind(DatasetKind::InteractionContext),
                AvailabilityRule::ActiveDatasetKind(DatasetKind::CommitCreation),
            ]))),
        ),
        (
            AvailabilityRuleId::from("commit-creation-context"),
            AvailabilityRule::ActiveDatasetKind(DatasetKind::CommitCreation),
        ),
        (
            AvailabilityRuleId::from("help-context"),
            AvailabilityRule::InteractionContextType(InteractionContextType::Help),
        ),
        (
            AvailabilityRuleId::from("command-palette-context"),
            AvailabilityRule::InteractionContextType(InteractionContextType::CommandPalette),
        ),
        (
            AvailabilityRuleId::from("text-input-context"),
            AvailabilityRule::InteractionContextType(InteractionContextType::TextInput),
        ),
        (
            AvailabilityRuleId::from("notice-context"),
            AvailabilityRule::InteractionContextType(InteractionContextType::Notice),
        ),
        (
            AvailabilityRuleId::from("changes-entry-context"),
            AvailabilityRule::Not(Box::new(AvailabilityRule::Any(
                [
                    DatasetKind::Changes,
                    DatasetKind::WorkingTreeFiles,
                    DatasetKind::WorkingTreeFile,
                    DatasetKind::WorkingTreeFileChanges,
                    DatasetKind::CommitCreation,
                    DatasetKind::InteractionContext,
                ]
                .into_iter()
                .map(AvailabilityRule::ActiveDatasetKind)
                .collect(),
            ))),
        ),
        (
            AvailabilityRuleId::from("changes-file-cursor"),
            AvailabilityRule::ContextCursorKind(
                RenderBindingId::Changes,
                DatasetKind::WorkingTreeFile,
            ),
        ),
        (
            AvailabilityRuleId::from("changes-unstaged-targets"),
            AvailabilityRule::ContextTargetsBoundary(
                RenderBindingId::Changes,
                pitui_data::ChangeBoundary::Unstaged,
            ),
        ),
        (
            AvailabilityRuleId::from("changes-staged-targets"),
            AvailabilityRule::ContextTargetsBoundary(
                RenderBindingId::Changes,
                pitui_data::ChangeBoundary::Staged,
            ),
        ),
        (
            AvailabilityRuleId::from("changes-has-staged-files"),
            AvailabilityRule::ChangesHasStagedFiles(RenderBindingId::Changes),
        ),
    ]
}

pub fn builtin_command_specs() -> Vec<CommandSpec> {
    let global = [
        ("quit", "quit"),
        ("help", "help"),
        ("refresh", "refresh"),
        ("changes", "changes"),
        ("reflog", "reflog"),
        ("remotes", "remotes"),
        ("logs", "logs"),
        ("fetch", "fetch"),
        ("pull", "pull"),
        ("push", "push"),
        ("sync", "sync"),
        ("back", "back"),
        ("command-palette", "command-palette"),
    ]
    .into_iter()
    .map(|(id, name)| command(id, name, CommandScope::Global));
    let local = [
        ("navigation.up", "up"),
        ("navigation.down", "down"),
        ("navigation.left", "left"),
        ("navigation.right", "right"),
        ("selection.toggle", "toggle-selection"),
        ("copy.commit.hash", "copy-commit-hash"),
        ("copy.commit.info", "copy-commit-info"),
        ("copy.commit.message", "copy-commit-message"),
        ("copy.reflog.hash", "copy-reflog-hash"),
        ("commits.cherry-pick", "cherry-pick"),
        ("copy.file.name", "copy-file-name"),
        ("copy.file.absolute", "copy-file-absolute-path"),
        ("copy.file.relative", "copy-file-relative-path"),
        ("scroll.home", "home"),
        ("scroll.end", "end"),
        ("scroll.page-up", "page-up"),
        ("scroll.page-down", "page-down"),
        ("interaction.close", "close"),
        ("palette.up", "palette-up"),
        ("palette.down", "palette-down"),
        ("palette.submit", "palette-submit"),
        ("changes.selection.toggle", "toggle-change-selection"),
        ("changes.stage", "stage"),
        ("changes.unstage", "unstage"),
        ("changes.commit", "commit"),
        ("commit-creation.cancel", "cancel"),
        ("commit-creation.submit", "commit"),
        ("text.submit", "submit-text"),
    ]
    .into_iter()
    .map(|(id, name)| command(id, name, CommandScope::Dataset));
    global.chain(local).collect()
}

fn command(id: &str, name: &str, scope: CommandScope) -> CommandSpec {
    CommandSpec {
        id: CommandId::from(id),
        name: name.into(),
        scope,
        system: CommandSystemId::from(id),
    }
}

pub fn builtin_operation_specs() -> Vec<OperationSpec> {
    let always = AvailabilityRuleId::from("always");
    let normal = AvailabilityRuleId::from("normal-context");
    let help_context = AvailabilityRuleId::from("help-context");
    let palette_context = AvailabilityRuleId::from("command-palette-context");
    let text_context = AvailabilityRuleId::from("text-input-context");
    let commit_creation_context = AvailabilityRuleId::from("commit-creation-context");
    let notice_context = AvailabilityRuleId::from("notice-context");
    let changes_entry = AvailabilityRuleId::from("changes-entry-context");
    let changes_file_cursor = AvailabilityRuleId::from("changes-file-cursor");
    let changes_unstaged = AvailabilityRuleId::from("changes-unstaged-targets");
    let changes_staged = AvailabilityRuleId::from("changes-staged-targets");
    let changes_has_staged = AvailabilityRuleId::from("changes-has-staged-files");
    let cursor = AvailabilityRuleId::from("has-cursor");
    let selected_or_cursor = AvailabilityRuleId::from("selection-or-cursor");
    let current_file = AvailabilityRuleId::from("current-files-selection-or-cursor");
    let mut operations = vec![
        operation(
            "global.quit",
            "Quit",
            "quit",
            vec![single(KeyStroke::character('q'))],
            TargetSource::None,
            normal.clone(),
        ),
        operation(
            "global.help",
            "Help",
            "help",
            vec![single(KeyStroke::character('h'))],
            TargetSource::None,
            normal.clone(),
        ),
        operation(
            "global.refresh",
            "Refresh",
            "refresh",
            vec![single(KeyStroke::control('r'))],
            TargetSource::ActiveDataset,
            normal.clone(),
        ),
        operation(
            "global.changes",
            "Changes",
            "changes",
            vec![single(KeyStroke::control('g'))],
            TargetSource::None,
            changes_entry,
        ),
        operation(
            "global.command-palette",
            "Command",
            "command-palette",
            vec![single(KeyStroke::control('p'))],
            TargetSource::None,
            normal.clone(),
        ),
        operation(
            "global.back",
            "Back",
            "back",
            vec![single(KeyStroke::plain(KeyCode::Escape))],
            TargetSource::None,
            normal.clone(),
        ),
    ];
    for command in ["reflog", "remotes", "logs", "fetch", "pull", "push", "sync"] {
        operations.push(operation(
            &format!("global.{command}"),
            command,
            command,
            Vec::new(),
            TargetSource::None,
            normal.clone(),
        ));
    }

    operations.extend([
        operation(
            "interaction.help.close",
            "Close",
            "interaction.close",
            vec![
                single(KeyStroke::plain(KeyCode::Escape)),
                single(KeyStroke::character('q')),
            ],
            TargetSource::None,
            help_context,
        ),
        operation(
            "interaction.palette.close",
            "Close",
            "interaction.close",
            vec![single(KeyStroke::plain(KeyCode::Escape))],
            TargetSource::None,
            palette_context.clone(),
        ),
        operation(
            "interaction.palette.up",
            "Up",
            "palette.up",
            vec![single(KeyStroke::plain(KeyCode::Up))],
            TargetSource::ActiveDataset,
            palette_context.clone(),
        ),
        operation(
            "interaction.palette.down",
            "Down",
            "palette.down",
            vec![single(KeyStroke::plain(KeyCode::Down))],
            TargetSource::ActiveDataset,
            palette_context.clone(),
        ),
        operation(
            "interaction.palette.submit",
            "Run",
            "palette.submit",
            vec![single(KeyStroke::plain(KeyCode::Enter))],
            TargetSource::ActiveDataset,
            palette_context,
        ),
        operation(
            "interaction.text.close",
            "Cancel",
            "interaction.close",
            vec![single(KeyStroke::plain(KeyCode::Escape))],
            TargetSource::None,
            text_context.clone(),
        ),
        operation(
            "interaction.text.submit",
            "Submit",
            "text.submit",
            vec![single(KeyStroke::plain(KeyCode::Enter))],
            TargetSource::ActiveDataset,
            text_context,
        ),
        operation(
            "interaction.notice.close",
            "Close",
            "interaction.close",
            vec![
                single(KeyStroke::plain(KeyCode::Escape)),
                single(KeyStroke::plain(KeyCode::Enter)),
                single(KeyStroke::character('q')),
            ],
            TargetSource::None,
            notice_context,
        ),
        operation(
            "changes.selection.toggle",
            "Select",
            "changes.selection.toggle",
            vec![single(KeyStroke::plain(KeyCode::Space))],
            TargetSource::ContextCursor(RenderBindingId::Changes),
            changes_file_cursor,
        ),
        operation(
            "changes.stage",
            "Stage",
            "changes.stage",
            vec![single(shifted('s'))],
            TargetSource::ContextSelectionOrCursor(RenderBindingId::Changes),
            changes_unstaged,
        ),
        operation(
            "changes.unstage",
            "Unstage",
            "changes.unstage",
            vec![single(shifted('u'))],
            TargetSource::ContextSelectionOrCursor(RenderBindingId::Changes),
            changes_staged,
        ),
        operation(
            "changes.commit",
            "Commit",
            "changes.commit",
            vec![single(shifted('c'))],
            TargetSource::None,
            changes_has_staged,
        ),
        operation(
            "commit-creation.help",
            "Help",
            "help",
            vec![single(KeyStroke::character('h'))],
            TargetSource::None,
            commit_creation_context.clone(),
        ),
        operation(
            "commit-creation.cancel",
            "Cancel",
            "commit-creation.cancel",
            vec![single(KeyStroke::plain(KeyCode::Escape))],
            TargetSource::ActiveDataset,
            commit_creation_context.clone(),
        ),
        operation(
            "commit-creation.submit",
            "Commit",
            "commit-creation.submit",
            vec![single(KeyStroke::plain(KeyCode::Enter))],
            TargetSource::ActiveDataset,
            commit_creation_context,
        ),
    ]);

    operations.extend([
        operation(
            "navigation.up",
            "Up",
            "navigation.up",
            vec![
                single(KeyStroke::character('w')),
                single(KeyStroke::plain(KeyCode::Up)),
            ],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "navigation.down",
            "Down",
            "navigation.down",
            vec![
                single(KeyStroke::character('s')),
                single(KeyStroke::plain(KeyCode::Down)),
            ],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "navigation.left",
            "Left",
            "navigation.left",
            vec![
                single(KeyStroke::character('a')),
                single(KeyStroke::plain(KeyCode::Left)),
            ],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "navigation.right",
            "Right",
            "navigation.right",
            vec![
                single(KeyStroke::character('d')),
                single(KeyStroke::plain(KeyCode::Right)),
            ],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "selection.toggle",
            "Select",
            "selection.toggle",
            vec![single(KeyStroke::plain(KeyCode::Space))],
            TargetSource::Cursor,
            cursor.clone(),
        ),
        operation(
            "copy.commit.hash",
            "Copy hash",
            "copy.commit.hash",
            vec![copy_chord('h')],
            TargetSource::SelectionOrCursor,
            selected_or_cursor.clone(),
        ),
        operation(
            "copy.commit.info",
            "Copy info",
            "copy.commit.info",
            vec![copy_chord('i')],
            TargetSource::Cursor,
            cursor.clone(),
        ),
        operation(
            "copy.commit.message",
            "Copy message",
            "copy.commit.message",
            vec![copy_chord('m')],
            TargetSource::Cursor,
            cursor.clone(),
        ),
        operation(
            "copy.reflog.hash",
            "Copy hash",
            "copy.reflog.hash",
            vec![copy_chord('h')],
            TargetSource::Cursor,
            cursor.clone(),
        ),
        operation(
            "commits.cherry-pick",
            "Cherry-pick selected",
            "commits.cherry-pick",
            Vec::new(),
            TargetSource::Selection,
            AvailabilityRuleId::from("has-selection"),
        ),
        operation(
            "copy.file.name",
            "Copy name",
            "copy.file.name",
            vec![copy_chord('n')],
            TargetSource::ContextSelectionOrCursor(RenderBindingId::CurrentFiles),
            current_file.clone(),
        ),
        operation(
            "copy.file.absolute",
            "Copy absolute path",
            "copy.file.absolute",
            vec![copy_chord('a')],
            TargetSource::ContextSelectionOrCursor(RenderBindingId::CurrentFiles),
            current_file.clone(),
        ),
        operation(
            "copy.file.relative",
            "Copy relative path",
            "copy.file.relative",
            vec![copy_chord('r')],
            TargetSource::ContextSelectionOrCursor(RenderBindingId::CurrentFiles),
            current_file,
        ),
        operation(
            "scroll.home",
            "Home",
            "scroll.home",
            vec![single(KeyStroke::plain(KeyCode::Home))],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "scroll.end",
            "End",
            "scroll.end",
            vec![single(KeyStroke::plain(KeyCode::End))],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "scroll.page-up",
            "Page up",
            "scroll.page-up",
            vec![single(KeyStroke::plain(KeyCode::PageUp))],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "scroll.page-down",
            "Page down",
            "scroll.page-down",
            vec![single(KeyStroke::plain(KeyCode::PageDown))],
            TargetSource::ActiveDataset,
            always,
        ),
    ]);
    operations
}

fn operation(
    id: &str,
    label: &str,
    command: &str,
    bindings: Vec<KeySequence>,
    target_source: TargetSource,
    availability: AvailabilityRuleId,
) -> OperationSpec {
    OperationSpec {
        id: OperationId::from(id),
        label: label.into(),
        command: CommandId::from(command),
        bindings,
        target_source,
        availability,
    }
}

fn single(stroke: KeyStroke) -> KeySequence {
    KeySequence::single(stroke)
}

fn copy_chord(suffix: char) -> KeySequence {
    KeySequence::chord([KeyStroke::control('c'), KeyStroke::character(suffix)])
}

fn shifted(character: char) -> KeyStroke {
    let mut stroke = KeyStroke::character(character);
    stroke.modifiers.shift = true;
    stroke
}

pub fn builtin_global_operations() -> Vec<OperationId> {
    [
        "quit",
        "help",
        "refresh",
        "changes",
        "command-palette",
        "reflog",
        "remotes",
        "logs",
        "fetch",
        "pull",
        "push",
        "sync",
        "back",
    ]
    .into_iter()
    .map(|name| OperationId::from(format!("global.{name}")))
    .collect()
}

fn proxy(
    id: &str,
    dataset_kind: DatasetKind,
    renderer: RendererKind,
    fields: Vec<FieldSpec>,
) -> RenderProxySpec {
    RenderProxySpec {
        id: RenderProxyId::from(id),
        dataset_kind,
        renderer,
        fields,
        style: StyleSpec::default(),
    }
}

fn plain(field: FieldId) -> FieldSpec {
    FieldSpec::plain(field)
}

fn labeled(field: FieldId, label: &str) -> FieldSpec {
    FieldSpec::labeled(field, label)
}

fn hash(field: FieldId, length: usize) -> FieldSpec {
    FieldSpec::formatted(field, FieldFormat::Hash { length })
}

fn datetime(field: FieldId, precision: DateTimePrecision) -> FieldSpec {
    FieldSpec::formatted(field, FieldFormat::DateTime { precision })
}

fn labeled_datetime(field: FieldId, label: &str, precision: DateTimePrecision) -> FieldSpec {
    FieldSpec {
        field,
        label: Some(label.into()),
        format: FieldFormat::DateTime { precision },
    }
}

fn joined(field: FieldId, separator: &str) -> FieldSpec {
    FieldSpec::formatted(
        field,
        FieldFormat::Joined {
            separator: separator.into(),
        },
    )
}

fn operations_for_dataset(kind: DatasetKind) -> Vec<OperationId> {
    use DatasetKind as Kind;
    let names: &[&str] = match kind {
        Kind::RepositoriesBranches => &[
            "navigation.up",
            "navigation.down",
            "navigation.left",
            "navigation.right",
        ],
        Kind::Commits => &[
            "navigation.up",
            "navigation.down",
            "navigation.left",
            "navigation.right",
            "selection.toggle",
            "copy.commit.hash",
            "copy.commit.info",
            "copy.commit.message",
            "commits.cherry-pick",
        ],
        Kind::Commit => &[
            "navigation.left",
            "navigation.right",
            "copy.commit.hash",
            "copy.commit.info",
            "copy.commit.message",
            "scroll.home",
            "scroll.end",
            "scroll.page-up",
            "scroll.page-down",
        ],
        Kind::Files => &[
            "navigation.up",
            "navigation.down",
            "navigation.left",
            "navigation.right",
            "selection.toggle",
            "copy.file.name",
            "copy.file.absolute",
            "copy.file.relative",
        ],
        Kind::Changes => &[
            "navigation.up",
            "navigation.down",
            "navigation.left",
            "navigation.right",
            "changes.selection.toggle",
            "changes.stage",
            "changes.unstage",
            "changes.commit",
        ],
        Kind::CommitCreation => &[
            "commit-creation.help",
            "commit-creation.cancel",
            "commit-creation.submit",
        ],
        Kind::WorkingTreeFiles | Kind::Remotes => &[
            "navigation.up",
            "navigation.down",
            "navigation.left",
            "navigation.right",
            "selection.toggle",
        ],
        Kind::Reflog => &[
            "navigation.up",
            "navigation.down",
            "navigation.left",
            "navigation.right",
            "copy.reflog.hash",
        ],
        Kind::File | Kind::WorkingTreeFile => &[
            "navigation.left",
            "navigation.right",
            "copy.file.name",
            "copy.file.absolute",
            "copy.file.relative",
            "scroll.home",
            "scroll.end",
            "scroll.page-up",
            "scroll.page-down",
        ],
        Kind::FileChanges | Kind::WorkingTreeFileChanges => &[
            "navigation.left",
            "copy.file.name",
            "copy.file.absolute",
            "copy.file.relative",
            "scroll.home",
            "scroll.end",
            "scroll.page-up",
            "scroll.page-down",
        ],
        Kind::GitOperationLog => &[
            "navigation.up",
            "navigation.down",
            "scroll.home",
            "scroll.end",
            "scroll.page-up",
            "scroll.page-down",
        ],
        Kind::InteractionContext => &[
            "interaction.help.close",
            "interaction.palette.close",
            "interaction.palette.up",
            "interaction.palette.down",
            "interaction.palette.submit",
            "interaction.text.close",
            "interaction.text.submit",
            "interaction.notice.close",
        ],
        Kind::Repository
        | Kind::Branch
        | Kind::ReflogEntry
        | Kind::Remote
        | Kind::GitOperationLogEntry => &[],
    };
    let mut names = names.to_vec();
    if kind == Kind::WorkingTreeFileChanges {
        names.extend([
            "changes.selection.toggle",
            "changes.stage",
            "changes.unstage",
            "changes.commit",
        ]);
    }
    names.into_iter().map(OperationId::from).collect()
}

fn builtin_proxies(kind: DatasetKind) -> Vec<RenderProxyId> {
    use DatasetKind as Kind;
    let names: &[&str] = match kind {
        Kind::RepositoriesBranches => &["repositories-branches.compact"],
        Kind::Repository => &["repository.detail"],
        Kind::Branch => &["branch.detail"],
        Kind::Commits => &["commits.compact", "commits.detailed"],
        Kind::Commit => &["commit.detail"],
        Kind::Files => &["files.list"],
        Kind::File => &["file.detail"],
        Kind::FileChanges => &["file-changes.unified", "file-changes.side-by-side"],
        Kind::Changes => &["changes.tree"],
        Kind::WorkingTreeFiles => &["working-tree-files.list"],
        Kind::WorkingTreeFile => &["working-tree-file.detail"],
        Kind::WorkingTreeFileChanges => &[
            "working-tree-file-changes.unified",
            "working-tree-file-changes.side-by-side",
        ],
        Kind::CommitCreation => &["commit-creation.editor"],
        Kind::Reflog => &["reflog.list"],
        Kind::ReflogEntry => &["reflog-entry.detail"],
        Kind::Remotes => &["remotes.list"],
        Kind::Remote => &["remote.detail"],
        Kind::InteractionContext => &["interaction-context.overlay"],
        Kind::GitOperationLog => &["git-operation-log.list"],
        Kind::GitOperationLogEntry => &["git-operation-log-entry.detail"],
    };
    names.iter().copied().map(RenderProxyId::from).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn builtins_cover_every_dataset_kind_once() {
        let templates = builtin_dataset_templates();
        assert_eq!(templates.len(), 20);
        let kinds = templates
            .iter()
            .map(|template| template.kind)
            .collect::<HashSet<_>>();
        assert_eq!(kinds.len(), templates.len());
        assert_eq!(kinds, DatasetKind::ALL.into_iter().collect::<HashSet<_>>());
        assert!(
            templates
                .iter()
                .all(|template| !template.render_proxies.is_empty())
        );
    }

    #[test]
    fn every_template_proxy_resolves_to_the_same_dataset_kind() {
        let proxies = builtin_render_proxies()
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(proxies.len(), 23);

        for template in builtin_dataset_templates() {
            for proxy_id in template.render_proxies {
                let proxy = proxies
                    .get(&proxy_id)
                    .unwrap_or_else(|| panic!("missing built-in proxy {}", proxy_id.0));
                assert_eq!(proxy.dataset_kind, template.kind);
            }
        }
    }

    #[test]
    fn core_semantic_datasets_have_explicit_proxy_and_operation_contracts() {
        let templates = builtin_dataset_templates()
            .into_iter()
            .map(|template| (template.kind, template))
            .collect::<std::collections::HashMap<_, _>>();

        let repositories_branches = &templates[&DatasetKind::RepositoriesBranches];
        assert_eq!(
            repositories_branches.render_proxies,
            vec![RenderProxyId::from("repositories-branches.compact")]
        );
        assert_eq!(
            repositories_branches.operations,
            [
                "navigation.up",
                "navigation.down",
                "navigation.left",
                "navigation.right",
            ]
            .into_iter()
            .map(OperationId::from)
            .collect::<Vec<_>>()
        );
        assert!(templates[&DatasetKind::Repository].operations.is_empty());
        assert!(templates[&DatasetKind::Branch].operations.is_empty());

        for kind in [
            DatasetKind::Commits,
            DatasetKind::Commit,
            DatasetKind::Files,
            DatasetKind::File,
            DatasetKind::FileChanges,
            DatasetKind::Reflog,
            DatasetKind::CommitCreation,
        ] {
            let template = &templates[&kind];
            assert!(
                !template.render_proxies.is_empty(),
                "{kind:?} has no Render Proxy contract"
            );
            assert!(
                !template.operations.is_empty(),
                "{kind:?} has no Operation Set contract"
            );
        }
        assert_eq!(
            templates[&DatasetKind::CommitCreation].render_proxies,
            vec![RenderProxyId::from("commit-creation.editor")]
        );
        assert_eq!(
            templates[&DatasetKind::Reflog].render_proxies,
            vec![RenderProxyId::from("reflog.list")]
        );
    }

    #[test]
    fn cherry_pick_is_a_selection_only_commits_operation_not_a_global_operation() {
        let operation_id = OperationId::from("commits.cherry-pick");
        assert!(!builtin_global_operations().contains(&operation_id));

        let templates = builtin_dataset_templates();
        let owners = templates
            .iter()
            .filter(|template| template.operations.contains(&operation_id))
            .map(|template| template.kind)
            .collect::<Vec<_>>();
        assert_eq!(owners, vec![DatasetKind::Commits]);

        let operation = builtin_operation_specs()
            .into_iter()
            .find(|operation| operation.id == operation_id)
            .unwrap();
        assert_eq!(operation.command, CommandId::from("commits.cherry-pick"));
        assert_eq!(operation.target_source, TargetSource::Selection);
        assert_eq!(
            operation.availability,
            AvailabilityRuleId::from("has-selection")
        );
        assert!(operation.bindings.is_empty());
    }

    #[test]
    fn built_in_render_modes_have_unique_ids() {
        let modes = builtin_render_modes();
        let ids = modes
            .iter()
            .map(|mode| mode.id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(modes.len(), 8);
        assert_eq!(ids.len(), modes.len());
    }

    #[test]
    fn every_effective_dataset_operation_set_is_resolvable_and_unambiguous() {
        let commands = builtin_command_specs()
            .into_iter()
            .map(|command| command.id)
            .collect::<HashSet<_>>();
        let rules = builtin_availability_rules()
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let operations = builtin_operation_specs()
            .into_iter()
            .map(|operation| (operation.id.clone(), operation))
            .collect::<std::collections::HashMap<_, _>>();
        for operation in operations.values() {
            assert!(commands.contains(&operation.command));
            assert!(rules.contains_key(&operation.availability));
        }

        for template in builtin_dataset_templates() {
            let context_types = if template.kind == DatasetKind::InteractionContext {
                vec![
                    Some(InteractionContextType::Help),
                    Some(InteractionContextType::CommandPalette),
                    Some(InteractionContextType::TextInput),
                    Some(InteractionContextType::Notice),
                ]
            } else {
                vec![None]
            };
            for context_type in context_types {
                let ids = builtin_global_operations()
                    .into_iter()
                    .chain(template.operations.clone())
                    .collect::<Vec<_>>();
                let sequences = ids
                    .iter()
                    .map(|id| {
                        operations
                            .get(id)
                            .unwrap_or_else(|| panic!("missing built-in Operation {}", id.0))
                    })
                    .filter(|operation| {
                        availability_can_match(
                            rules.get(&operation.availability).unwrap(),
                            template.kind,
                            context_type,
                        )
                    })
                    .flat_map(|operation| operation.bindings.iter())
                    .collect::<Vec<_>>();
                assert_eq!(
                    sequences.iter().copied().collect::<HashSet<_>>().len(),
                    sequences.len(),
                    "duplicate effective key sequence for {:?}/{context_type:?}",
                    template.kind
                );
                for (index, left) in sequences.iter().enumerate() {
                    for right in sequences.iter().skip(index + 1) {
                        let (shorter, longer) = if left.0.len() <= right.0.len() {
                            (left, right)
                        } else {
                            (right, left)
                        };
                        assert!(
                            shorter.0.len() == longer.0.len() || !longer.0.starts_with(&shorter.0),
                            "ambiguous key prefix in {:?}/{context_type:?}: {shorter:?} / {longer:?}",
                            template.kind
                        );
                    }
                }
            }
        }
    }

    fn availability_can_match(
        rule: &AvailabilityRule,
        kind: DatasetKind,
        context_type: Option<InteractionContextType>,
    ) -> bool {
        match rule {
            AvailabilityRule::Always
            | AvailabilityRule::HasCursor
            | AvailabilityRule::HasSelection
            | AvailabilityRule::HasSelectionOrCursor
            | AvailabilityRule::ContextHasCursor(_)
            | AvailabilityRule::ContextHasSelectionOrCursor(_)
            | AvailabilityRule::ContextCursorKind(_, _)
            | AvailabilityRule::ContextTargetsBoundary(_, _)
            | AvailabilityRule::ChangesHasStagedFiles(_) => true,
            AvailabilityRule::ActiveDatasetKind(expected) => *expected == kind,
            AvailabilityRule::InteractionContextType(expected) => context_type == Some(*expected),
            AvailabilityRule::All(rules) => rules
                .iter()
                .all(|rule| availability_can_match(rule, kind, context_type)),
            AvailabilityRule::Any(rules) => rules
                .iter()
                .any(|rule| availability_can_match(rule, kind, context_type)),
            AvailabilityRule::Not(rule) => !availability_can_match(rule, kind, context_type),
        }
    }

    #[test]
    fn default_profile_has_wasd_arrows_and_keeps_control_space_unbound() {
        let operations = builtin_operation_specs();
        let all_strokes = operations
            .iter()
            .flat_map(|operation| &operation.bindings)
            .flat_map(|sequence| &sequence.0)
            .collect::<HashSet<_>>();
        for stroke in [
            KeyStroke::character('w'),
            KeyStroke::character('a'),
            KeyStroke::character('s'),
            KeyStroke::character('d'),
            KeyStroke::plain(KeyCode::Up),
            KeyStroke::plain(KeyCode::Down),
            KeyStroke::plain(KeyCode::Left),
            KeyStroke::plain(KeyCode::Right),
        ] {
            assert!(all_strokes.contains(&stroke));
        }
        let control_space = KeyStroke {
            code: KeyCode::Space,
            modifiers: pitui_data::KeyModifiers::control(),
        };
        assert!(!all_strokes.contains(&control_space));
    }

    #[test]
    fn default_git_logging_profile_is_bounded_and_persistent() {
        let logging = default_git_logging_config();
        assert!(logging.enabled);
        assert!(logging.max_bytes > 0);
        assert!(logging.keep_files > 0);
        assert!(logging.buffer_capacity > 0);
        assert!(logging.max_message_chars > 0);
        assert!(!logging.path.as_os_str().is_empty());
    }
}
