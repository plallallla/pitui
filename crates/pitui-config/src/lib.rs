//! Strict configuration boundary for Dataset templates, Operations, Render
//! Proxies and Render Modes.
//!
//! Parsing and effective-config resolution will migrate here after the ECS
//! registries are stable. This crate must never permit shell or Git argv
//! templates to be injected from configuration.

#![forbid(unsafe_code)]

use std::{env, path::PathBuf, time::Duration};

use pitui_data::{
    ActiveDirection, ActiveHandoffRegistry, ActiveHandoffSpec, ActiveHandoffTarget,
    AvailabilityRule, AvailabilityRuleId, CollectionManagerSpec, CommandId, CommandScope,
    CommandSpec, CommandSystemId, DatasetBinding, DatasetIdentity, DatasetKind, DatasetTemplate,
    DatasetTemplateId, DateTimePrecision, FieldFormat, FieldId, FieldSpec, InteractionContextType,
    KeyCode, KeySequence, KeyStroke, LayoutConstraint, OperationId, OperationSpec, RenderBindingId,
    RenderLayout, RenderModeId, RenderModeSpec, RenderProxyId, RenderProxySpec, RendererKind,
    StyleSpec, TargetSource, TreeManagerSpec, TreeSelectionMode, TreeSiblingOrder,
};

/// Version of the Data Driven configuration schema.
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
        (Kind::FileTreeDirectory, "file-tree-directory"),
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
        collection: collection_manager_for(kind),
        operations: operations_for_dataset(kind),
        render_proxies: builtin_proxies(kind),
    })
    .collect()
}

fn collection_manager_for(kind: DatasetKind) -> CollectionManagerSpec {
    use DatasetKind as Kind;

    let tree = |visible_kinds, selectable_kinds, sibling_order| {
        CollectionManagerSpec::Tree(TreeManagerSpec {
            visible_kinds,
            selectable_kinds,
            sibling_order,
            selection: TreeSelectionMode::Cascade,
        })
    };
    match kind {
        Kind::RepositoriesBranches => tree(
            vec![Kind::Repository, Kind::Branch],
            vec![Kind::Repository, Kind::Branch],
            TreeSiblingOrder::Source,
        ),
        Kind::Files => tree(
            vec![Kind::FileTreeDirectory, Kind::File],
            vec![Kind::FileTreeDirectory, Kind::File],
            TreeSiblingOrder::Path,
        ),
        Kind::Changes => tree(
            vec![
                Kind::WorkingTreeFiles,
                Kind::FileTreeDirectory,
                Kind::WorkingTreeFile,
            ],
            vec![Kind::FileTreeDirectory, Kind::WorkingTreeFile],
            TreeSiblingOrder::Path,
        ),
        Kind::WorkingTreeFiles => tree(
            vec![Kind::FileTreeDirectory, Kind::WorkingTreeFile],
            vec![Kind::FileTreeDirectory, Kind::WorkingTreeFile],
            TreeSiblingOrder::Path,
        ),
        Kind::FileTreeDirectory => tree(
            vec![Kind::FileTreeDirectory, Kind::File, Kind::WorkingTreeFile],
            vec![Kind::FileTreeDirectory, Kind::File, Kind::WorkingTreeFile],
            TreeSiblingOrder::Path,
        ),
        _ => CollectionManagerSpec::List,
    }
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
            "files.tree",
            Kind::Files,
            Renderer::PathTree,
            vec![
                plain(Field::FileStatus),
                plain(Field::FilePath),
                plain(Field::FileAdditions),
                plain(Field::FileDeletions),
            ],
        ),
        proxy(
            "file-tree-directory.detail",
            Kind::FileTreeDirectory,
            Renderer::Detail,
            vec![labeled(Field::FilePath, "Directory")],
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
            Renderer::PathTree,
            vec![plain(Field::FileStatus), plain(Field::DatasetLabel)],
        ),
        proxy(
            "working-tree-files.tree",
            Kind::WorkingTreeFiles,
            Renderer::PathTree,
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

/// Built-in reference layouts. Every leaf is declarative:
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
                    activatable: true,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentCommits),
                    proxy: RenderProxyId::from("commits.detailed"),
                    constraint: LayoutConstraint::Fill(1),
                    activatable: true,
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
                    activatable: true,
                },
                RenderLayout::Column(vec![
                    RenderLayout::Dataset {
                        dataset: DatasetBinding::Context(RenderBindingId::CurrentCommit),
                        proxy: RenderProxyId::from("commit.detail"),
                        constraint: LayoutConstraint::Percentage(40),
                        activatable: false,
                    },
                    RenderLayout::Dataset {
                        dataset: DatasetBinding::Context(RenderBindingId::CurrentFiles),
                        proxy: RenderProxyId::from("files.tree"),
                        constraint: LayoutConstraint::Fill(1),
                        activatable: true,
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
                    activatable: true,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentReflogEntry),
                    proxy: RenderProxyId::from("reflog-entry.detail"),
                    constraint: LayoutConstraint::Fill(1),
                    activatable: false,
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
                    activatable: true,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentGitOperationLogEntry),
                    proxy: RenderProxyId::from("git-operation-log-entry.detail"),
                    constraint: LayoutConstraint::Fill(1),
                    activatable: false,
                },
            ]),
        },
    ]
}

pub fn builtin_active_handoffs() -> ActiveHandoffRegistry {
    ActiveHandoffRegistry {
        rules: [
            (
                (DatasetKind::Commits, ActiveDirection::Right),
                ActiveHandoffSpec {
                    render_mode: RenderModeId::from("commit"),
                    target: ActiveHandoffTarget::KeepActiveDataset,
                },
            ),
            (
                (DatasetKind::Files, ActiveDirection::Right),
                ActiveHandoffSpec {
                    render_mode: RenderModeId::from("file-diff.unified"),
                    target: ActiveHandoffTarget::KeepActiveDataset,
                },
            ),
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
                    activatable: false,
                },
                RenderLayout::Dataset {
                    dataset: DatasetBinding::Context(RenderBindingId::CurrentFiles),
                    proxy: RenderProxyId::from("files.tree"),
                    constraint: LayoutConstraint::Fill(1),
                    activatable: true,
                },
            ]),
            RenderLayout::Dataset {
                dataset: DatasetBinding::Context(RenderBindingId::CurrentFileChanges),
                proxy: RenderProxyId::from(diff_proxy),
                constraint: LayoutConstraint::Fill(2),
                activatable: true,
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
                activatable: true,
            },
            RenderLayout::Dataset {
                dataset: DatasetBinding::Context(RenderBindingId::CurrentChangesFileChanges),
                proxy: RenderProxyId::from(diff_proxy),
                constraint: LayoutConstraint::Fill(1),
                activatable: true,
            },
        ]),
    }
}

pub fn builtin_availability_rules() -> Vec<(AvailabilityRuleId, AvailabilityRule)> {
    vec![
        (AvailabilityRuleId::from("always"), AvailabilityRule::Always),
        (
            AvailabilityRuleId::from("has-active-element"),
            AvailabilityRule::HasActiveElement,
        ),
        (
            AvailabilityRuleId::from("has-selection"),
            AvailabilityRule::HasSelection,
        ),
        (
            AvailabilityRuleId::from("selection-or-active-element"),
            AvailabilityRule::HasSelectionOrActiveElement,
        ),
        (
            AvailabilityRuleId::from("current-files-selection-or-active-element"),
            AvailabilityRule::ContextHasSelectionOrActiveElement(RenderBindingId::CurrentFiles),
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
            AvailabilityRuleId::from("changes-file-active-element"),
            AvailabilityRule::Any(vec![
                AvailabilityRule::ContextActiveElementKind(
                    RenderBindingId::Changes,
                    DatasetKind::WorkingTreeFile,
                ),
                AvailabilityRule::ContextActiveElementKind(
                    RenderBindingId::Changes,
                    DatasetKind::FileTreeDirectory,
                ),
            ]),
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
        ("active.up", "up"),
        ("active.down", "down"),
        ("active.left", "left"),
        ("active.right", "right"),
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
    let changes_file_active = AvailabilityRuleId::from("changes-file-active-element");
    let changes_unstaged = AvailabilityRuleId::from("changes-unstaged-targets");
    let changes_staged = AvailabilityRuleId::from("changes-staged-targets");
    let changes_has_staged = AvailabilityRuleId::from("changes-has-staged-files");
    let active_element = AvailabilityRuleId::from("has-active-element");
    let selected_or_active = AvailabilityRuleId::from("selection-or-active-element");
    let current_file = AvailabilityRuleId::from("current-files-selection-or-active-element");
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
            TargetSource::ContextActiveElement(RenderBindingId::Changes),
            changes_file_active,
        ),
        operation(
            "changes.stage",
            "Stage",
            "changes.stage",
            vec![single(shifted('s'))],
            TargetSource::ContextSelectionOrActiveElement(RenderBindingId::Changes),
            changes_unstaged,
        ),
        operation(
            "changes.unstage",
            "Unstage",
            "changes.unstage",
            vec![single(shifted('u'))],
            TargetSource::ContextSelectionOrActiveElement(RenderBindingId::Changes),
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
            "active.up",
            "Up",
            "active.up",
            vec![
                single(KeyStroke::character('w')),
                single(KeyStroke::plain(KeyCode::Up)),
            ],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "active.down",
            "Down",
            "active.down",
            vec![
                single(KeyStroke::character('s')),
                single(KeyStroke::plain(KeyCode::Down)),
            ],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "active.left",
            "Left",
            "active.left",
            vec![
                single(KeyStroke::character('a')),
                single(KeyStroke::plain(KeyCode::Left)),
            ],
            TargetSource::ActiveDataset,
            always.clone(),
        ),
        operation(
            "active.right",
            "Right",
            "active.right",
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
            TargetSource::ActiveElement,
            active_element.clone(),
        ),
        operation(
            "copy.commit.hash",
            "Copy hash",
            "copy.commit.hash",
            vec![copy_chord('h')],
            TargetSource::SelectionOrActiveElement,
            selected_or_active.clone(),
        ),
        operation(
            "copy.commit.info",
            "Copy info",
            "copy.commit.info",
            vec![copy_chord('i')],
            TargetSource::ActiveElement,
            active_element.clone(),
        ),
        operation(
            "copy.commit.message",
            "Copy message",
            "copy.commit.message",
            vec![copy_chord('m')],
            TargetSource::ActiveElement,
            active_element.clone(),
        ),
        operation(
            "copy.reflog.hash",
            "Copy hash",
            "copy.reflog.hash",
            vec![copy_chord('h')],
            TargetSource::ActiveElement,
            active_element.clone(),
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
            TargetSource::ContextSelectionOrActiveElement(RenderBindingId::CurrentFiles),
            current_file.clone(),
        ),
        operation(
            "copy.file.absolute",
            "Copy absolute path",
            "copy.file.absolute",
            vec![copy_chord('a')],
            TargetSource::ContextSelectionOrActiveElement(RenderBindingId::CurrentFiles),
            current_file.clone(),
        ),
        operation(
            "copy.file.relative",
            "Copy relative path",
            "copy.file.relative",
            vec![copy_chord('r')],
            TargetSource::ContextSelectionOrActiveElement(RenderBindingId::CurrentFiles),
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
            "active.up",
            "active.down",
            "active.left",
            "active.right",
            "selection.toggle",
        ],
        Kind::Commits => &[
            "active.up",
            "active.down",
            "active.left",
            "active.right",
            "selection.toggle",
            "copy.commit.hash",
            "copy.commit.info",
            "copy.commit.message",
            "commits.cherry-pick",
        ],
        Kind::Commit => &[
            "active.left",
            "active.right",
            "copy.commit.hash",
            "copy.commit.info",
            "copy.commit.message",
            "scroll.home",
            "scroll.end",
            "scroll.page-up",
            "scroll.page-down",
        ],
        Kind::Files => &[
            "active.up",
            "active.down",
            "active.left",
            "active.right",
            "selection.toggle",
            "copy.file.name",
            "copy.file.absolute",
            "copy.file.relative",
        ],
        Kind::Changes => &[
            "active.up",
            "active.down",
            "active.left",
            "active.right",
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
            "active.up",
            "active.down",
            "active.left",
            "active.right",
            "selection.toggle",
        ],
        Kind::Reflog => &[
            "active.up",
            "active.down",
            "active.left",
            "active.right",
            "copy.reflog.hash",
        ],
        Kind::File | Kind::WorkingTreeFile => &[
            "active.left",
            "active.right",
            "copy.file.name",
            "copy.file.absolute",
            "copy.file.relative",
            "scroll.home",
            "scroll.end",
            "scroll.page-up",
            "scroll.page-down",
        ],
        Kind::FileChanges | Kind::WorkingTreeFileChanges => &[
            "active.left",
            "copy.file.name",
            "copy.file.absolute",
            "copy.file.relative",
            "scroll.home",
            "scroll.end",
            "scroll.page-up",
            "scroll.page-down",
        ],
        Kind::GitOperationLog => &[
            "active.up",
            "active.down",
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
        | Kind::FileTreeDirectory
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
        Kind::Files => &["files.tree"],
        Kind::FileTreeDirectory => &["file-tree-directory.detail"],
        Kind::File => &["file.detail"],
        Kind::FileChanges => &["file-changes.unified", "file-changes.side-by-side"],
        Kind::Changes => &["changes.tree"],
        Kind::WorkingTreeFiles => &["working-tree-files.tree"],
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
mod tests;
