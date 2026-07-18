//! Composition root for the next-generation Pitui runtime.

#![forbid(unsafe_code)]

use std::{
    collections::HashSet,
    env,
    error::Error,
    ffi::OsString,
    fmt, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use bevy_ecs::prelude::Entity;
use pitui_data::{
    DatasetIdentity, DatasetIndex, DatasetKind, DatasetTemplateId, InputIntent,
    InteractionContextMetadata, RenderBindingId, RenderContextBindings, RenderModeId,
    RenderProxyId, RepositoryKey, RepositoryMetadata, ResolvedOperationSet, ResolvedOperationSetId,
    UiFrame,
};
use pitui_ecs::{
    DatasetRuntime, GitCommandData, InvariantViolation, KernelError, RegistrationContractError,
};
use pitui_git::GitCommand;

#[derive(Debug)]
pub enum NextError {
    CurrentDirectory(std::io::Error),
    Kernel(KernelError),
    DuplicateBuiltinTemplate(DatasetTemplateId),
    DuplicateBuiltinProxy(RenderProxyId),
    DuplicateBuiltinMode(RenderModeId),
    BuiltinInteraction(String),
    BuiltinContract(Vec<RegistrationContractError>),
    GitLogOpen { path: PathBuf, error: io::Error },
    Invariant(Vec<InvariantViolation>),
}

impl fmt::Display for NextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for NextError {}

impl From<KernelError> for NextError {
    fn from(value: KernelError) -> Self {
        Self::Kernel(value)
    }
}

pub struct NextApp {
    runtime: DatasetRuntime,
    root_dataset: Entity,
    repositories: Vec<Entity>,
}

impl NextApp {
    pub fn open(paths: Vec<PathBuf>) -> Result<Self, NextError> {
        let cwd = env::current_dir().map_err(NextError::CurrentDirectory)?;
        let config = pitui_config::default_git_logging_config();
        let (log_sink, startup_notice): (
            Arc<dyn pitui_git::logging::GitOperationLogSink>,
            Option<pitui_data::InteractionNoticeRequest>,
        ) = if config.enabled {
            let sink_config = pitui_git::logging::JsonlGitLogConfig {
                path: config.path.clone(),
                level: match config.level {
                    pitui_config::LoggingLevel::Info => pitui_git::logging::GitLogLevel::Info,
                    pitui_config::LoggingLevel::Warn => pitui_git::logging::GitLogLevel::Warn,
                    pitui_config::LoggingLevel::Error => pitui_git::logging::GitLogLevel::Error,
                },
                max_bytes: config.max_bytes,
                keep_files: config.keep_files,
                rotate_on_start: config.rotate_on_start,
                flush_interval: config.flush_interval,
                buffer_capacity: config.buffer_capacity,
                max_message_chars: config.max_message_chars,
            };
            match pitui_git::logging::JsonlGitOperationLogSink::open(sink_config) {
                Ok(sink) => (Arc::new(sink), None),
                Err(error) if config.fail_on_open_error => {
                    return Err(NextError::GitLogOpen {
                        path: config.path,
                        error,
                    });
                }
                Err(error) => (
                    Arc::new(pitui_git::logging::NoopGitOperationLogSink),
                    Some(pitui_data::InteractionNoticeRequest {
                        title: "Git operation log disabled".into(),
                        message: format!("{}: {error}", config.path.display()),
                    }),
                ),
            }
        } else {
            (Arc::new(pitui_git::logging::NoopGitOperationLogSink), None)
        };
        Self::open_from_with_log_sink(&cwd, paths, log_sink, startup_notice)
    }

    pub fn open_from(cwd: &Path, paths: Vec<PathBuf>) -> Result<Self, NextError> {
        Self::open_from_with_log_sink(
            cwd,
            paths,
            Arc::new(pitui_git::logging::NoopGitOperationLogSink),
            None,
        )
    }

    fn open_from_with_log_sink(
        cwd: &Path,
        paths: Vec<PathBuf>,
        log_sink: Arc<dyn pitui_git::logging::GitOperationLogSink>,
        startup_notice: Option<pitui_data::InteractionNoticeRequest>,
    ) -> Result<Self, NextError> {
        let mut runtime = DatasetRuntime::with_git_executor_and_log_sink(
            Arc::new(pitui_git::CliGitExecutor),
            log_sink,
        );
        if let Some(notice) = startup_notice {
            runtime.enqueue_interaction_notice(notice);
        }
        for template in pitui_config::builtin_dataset_templates() {
            let id = template.id.clone();
            runtime
                .register_default_template(template)
                .map_err(|_| NextError::DuplicateBuiltinTemplate(id))?;
        }
        for proxy in pitui_config::builtin_render_proxies() {
            let id = proxy.id.clone();
            runtime
                .register_render_proxy(proxy)
                .map_err(|_| NextError::DuplicateBuiltinProxy(id))?;
        }
        for mode in pitui_config::builtin_render_modes() {
            let id = mode.id.clone();
            runtime
                .register_render_mode(mode)
                .map_err(|_| NextError::DuplicateBuiltinMode(id))?;
        }
        for (id, rule) in pitui_config::builtin_availability_rules() {
            runtime
                .register_availability_rule(id, rule)
                .map_err(|error| NextError::BuiltinInteraction(format!("{error:?}")))?;
        }
        for command in pitui_config::builtin_command_specs() {
            runtime
                .register_command(command)
                .map_err(|error| NextError::BuiltinInteraction(format!("{error:?}")))?;
        }
        for operation in pitui_config::builtin_operation_specs() {
            runtime
                .register_operation(operation)
                .map_err(|error| NextError::BuiltinInteraction(format!("{error:?}")))?;
        }
        runtime.set_global_operations(pitui_config::builtin_global_operations());
        runtime.set_navigation_modes(pitui_config::builtin_navigation_modes());
        runtime
            .register_builtin_interaction_systems()
            .map_err(|error| NextError::BuiltinInteraction(format!("{error:?}")))?;
        let contract_errors = runtime.validate_registration_contracts();
        if !contract_errors.is_empty() {
            return Err(NextError::BuiltinContract(contract_errors));
        }

        let root_dataset = runtime.ensure_dataset(
            DatasetIdentity::GlobalRepositoriesBranches,
            DatasetKind::RepositoriesBranches,
            DatasetTemplateId::from("repositories-branches"),
        )?;
        runtime.add_root(root_dataset)?;
        let interaction_context = runtime.ensure_dataset(
            DatasetIdentity::GlobalInteractionContext,
            DatasetKind::InteractionContext,
            DatasetTemplateId::from("interaction-context"),
        )?;
        runtime.add_root(interaction_context)?;
        runtime
            .world_mut()
            .entity_mut(interaction_context)
            .insert(InteractionContextMetadata::default());
        let changes = runtime.ensure_dataset(
            DatasetIdentity::GlobalChanges,
            DatasetKind::Changes,
            DatasetTemplateId::from("changes"),
        )?;
        runtime.add_root(changes)?;
        let git_operation_log = runtime.ensure_dataset(
            DatasetIdentity::GlobalGitOperationLog,
            DatasetKind::GitOperationLog,
            DatasetTemplateId::from("git-operation-log"),
        )?;
        runtime.add_root(git_operation_log)?;

        let paths = if paths.is_empty() {
            vec![cwd.to_path_buf()]
        } else {
            paths
        };
        let mut repositories = Vec::with_capacity(paths.len());
        let mut requested_paths = Vec::with_capacity(paths.len());
        for path in paths {
            let requested = if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            };
            let key_path = requested
                .canonicalize()
                .unwrap_or_else(|_| requested.clone());
            let repository = runtime.ensure_dataset(
                DatasetIdentity::Repository(RepositoryKey::new(key_path)),
                DatasetKind::Repository,
                DatasetTemplateId::from("repository"),
            )?;
            repositories.push(repository);
            requested_paths.push(requested);
        }
        runtime.replace_children(root_dataset, repositories.clone(), true)?;

        for (repository, path) in repositories.iter().zip(&requested_paths) {
            runtime.enqueue_git_command(GitCommandData {
                repository_dataset: *repository,
                cwd: path.clone(),
                command: GitCommand::LoadRepository,
            })?;
        }
        runtime.run_schedule();

        for (repository, path) in repositories.iter().zip(&requested_paths) {
            if runtime
                .world()
                .get::<RepositoryMetadata>(*repository)
                .is_some()
            {
                runtime.enqueue_git_command(GitCommandData {
                    repository_dataset: *repository,
                    cwd: path.clone(),
                    command: GitCommand::LoadBranches,
                })?;
            }
        }
        runtime.run_schedule();

        for repository in &repositories {
            let Some(metadata) = runtime
                .world()
                .get::<RepositoryMetadata>(*repository)
                .cloned()
            else {
                continue;
            };
            let Some(branch) = metadata.0.current_branch else {
                continue;
            };
            if metadata.0.head.0.is_empty() {
                continue;
            }
            runtime.enqueue_git_command(GitCommandData {
                repository_dataset: *repository,
                cwd: metadata.0.root,
                command: GitCommand::LoadCommits { branch, limit: 500 },
            })?;
        }
        runtime.run_schedule();

        initialize_history_context(&mut runtime, root_dataset, &repositories)?;
        runtime.run_schedule();
        let violations = runtime.validate();
        if !violations.is_empty() {
            return Err(NextError::Invariant(violations));
        }

        Ok(Self {
            runtime,
            root_dataset,
            repositories,
        })
    }

    pub fn runtime(&self) -> &DatasetRuntime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut DatasetRuntime {
        &mut self.runtime
    }

    pub fn root_dataset(&self) -> Entity {
        self.root_dataset
    }

    pub fn repositories(&self) -> &[Entity] {
        &self.repositories
    }

    pub fn ui_frame(&self) -> &UiFrame {
        self.runtime.ui_frame()
    }

    pub fn dispatch_input(&mut self, intent: InputIntent) {
        self.runtime.enqueue_input_intent(intent);
        self.runtime.run_schedule();
    }

    pub fn quit_requested(&self) -> bool {
        self.runtime.quit_requested()
    }

    pub fn take_clipboard_requests(&mut self) -> Vec<pitui_data::ClipboardRequest> {
        self.runtime.take_clipboard_requests()
    }
}

const EVENT_POLL_RATE: Duration = Duration::from_millis(250);

/// Terminal composition boundary. Rendering is driven only by UiFrame
/// generation changes or terminal resizes; the timeout is exclusively for
/// event polling and never refreshes Git or redraws the screen.
pub fn run_terminal(mut app: NextApp) -> io::Result<()> {
    let mut terminal = pitui_tui::TerminalSession::enter()?;
    let mut presented_generation = None;
    let mut resize_requested = true;

    while !app.quit_requested() {
        let generation = app.ui_frame().generation;
        if resize_requested || presented_generation != Some(generation) {
            let measurements = terminal.draw(app.ui_frame())?;
            presented_generation = Some(generation);
            resize_requested = false;
            for measurement in measurements {
                app.runtime.enqueue_viewport_measurement(measurement);
            }
            app.runtime.run_schedule();

            // A changed viewport projection needs one immediate follow-up
            // presentation, not a timer-driven refresh.
            if app.ui_frame().generation != generation {
                continue;
            }
        }

        for request in app.take_clipboard_requests() {
            terminal.copy_to_clipboard(&request.text)?;
        }

        match terminal.poll_event(EVENT_POLL_RATE)? {
            Some(pitui_tui::TerminalEvent::Input(intent)) => app.dispatch_input(intent),
            Some(pitui_tui::TerminalEvent::Resize { .. }) => resize_requested = true,
            None => {}
        }
    }

    Ok(())
}

fn initialize_history_context(
    runtime: &mut DatasetRuntime,
    root: Entity,
    repositories: &[Entity],
) -> Result<(), KernelError> {
    let mut bindings = RenderContextBindings::default();
    bindings.bind(RenderBindingId::RepositoriesBranches, root);

    if let Some(repository) = repositories.first().copied() {
        bindings.bind(RenderBindingId::CurrentRepository, repository);
        if let Some(metadata) = runtime.world().get::<RepositoryMetadata>(repository)
            && let Some(branch) = &metadata.0.current_branch
            && let Some(DatasetIdentity::Repository(repository_key)) = runtime
                .world()
                .get::<pitui_data::DatasetKey>(repository)
                .map(|key| &key.0)
        {
            let commits_identity = DatasetIdentity::Commits {
                repository: repository_key.clone(),
                branch: branch.clone(),
            };
            if let Some(commits) = runtime
                .world()
                .resource::<DatasetIndex>()
                .get(&commits_identity)
            {
                bindings.bind(RenderBindingId::CurrentCommits, commits);
            }
        }
    }

    runtime.initialize_ui_from_mode(
        root,
        RenderModeId::from("history"),
        bindings,
        ResolvedOperationSet {
            id: ResolvedOperationSetId::from("history.repositories-branches"),
            ..ResolvedOperationSet::default()
        },
    )?;
    Ok(())
}

pub fn repository_paths_from_args(
    cwd: &Path,
    args: impl IntoIterator<Item = OsString>,
) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let paths = args
        .into_iter()
        .filter(|argument| argument != "--")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .filter(|path| seen.insert(path.clone()))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        vec![cwd.to_path_buf()]
    } else {
        paths
    }
}

#[cfg(test)]
mod tests;
