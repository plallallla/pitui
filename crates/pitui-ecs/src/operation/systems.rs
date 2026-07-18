//! Built-in operation functions. Every entry registered in `OperationManager`
//! is an ordinary Bevy ECS System: invocation context enters through `In`, and
//! World state/effect messages are changed through typed System parameters.

use super::*;

mod changes;
mod copy;
mod reset;
mod viewport;

pub use changes::*;
pub use copy::*;
pub use reset::*;
pub use viewport::*;

#[derive(Resource, Clone, Debug, Default)]
pub struct PendingChangesActiveRelays(VecDeque<ChangesActiveRelayRequest>);

#[derive(Clone, Debug)]
struct ChangesActiveRelayRequest {
    mutation: GitCommand,
    request_id: GitRequestId,
    changes_revision: u64,
    target: ChangesActiveTarget,
    context: ChangesActiveContext,
    preserve_diff_active: bool,
}

#[derive(Clone, Debug)]
enum ChangesActiveTarget {
    Exact {
        repository: pitui_data::RepositoryKey,
        path: pitui_core::GitPath,
        boundary: ChangeBoundary,
    },
    FirstRemainingFile,
}

#[derive(Clone, Copy, Debug)]
enum ChangesActiveContext {
    Active,
    Previous,
}

pub fn activate_previous_element(
    In(invocation): In<OperationInvocation>,
    mut datasets: Query<(&DatasetCollection, &mut DatasetActiveElement)>,
) -> OperationExecution {
    shift_active_element(
        invocation.source_dataset,
        ActiveDirection::Up,
        &mut datasets,
    )
}

pub fn request_quit(
    In(_invocation): In<OperationInvocation>,
    mut requested: ResMut<QuitRequested>,
) -> OperationExecution {
    requested.0 = true;
    OperationExecution::Completed
}

pub fn reject_unimplemented(In(invocation): In<OperationInvocation>) -> OperationExecution {
    OperationExecution::Rejected(format!(
        "{} is not implemented in Pitui Next yet",
        invocation.command.0
    ))
}

pub fn open_help(
    In(_invocation): In<OperationInvocation>,
    operations: Res<ResolvedOperationSet>,
    index: Res<DatasetIndex>,
    mut contexts: Query<&mut InteractionContextMetadata>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let Some(context) = index.get(&DatasetIdentity::GlobalInteractionContext) else {
        return OperationExecution::Rejected("global Interaction Context is unavailable".into());
    };
    let Ok(mut metadata) = contexts.get_mut(context) else {
        return OperationExecution::Rejected("Interaction Context has no metadata".into());
    };
    let mut bindings = operations
        .key_bindings
        .values()
        .cloned()
        .collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.stroke.cmp(&right.stroke));
    metadata.kind = InteractionContextKind::Help {
        entries: bindings
            .into_iter()
            .map(|binding| ShortcutHelpEntry { binding })
            .collect(),
    };
    request_interaction_overlay(context, &mut transitions);
    OperationExecution::Completed
}

#[allow(clippy::too_many_arguments)]
pub fn open_command_palette(
    In(_invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    operations: Res<ResolvedOperationSet>,
    index: Res<DatasetIndex>,
    keys: Query<&DatasetKey>,
    dataset_states: Query<(&DatasetActiveElement, &DatasetSelection)>,
    mut contexts: Query<&mut InteractionContextMetadata>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let Some(interaction) = index.get(&DatasetIdentity::GlobalInteractionContext) else {
        return OperationExecution::Rejected("global Interaction Context is unavailable".into());
    };
    let mut entries = operations
        .commands
        .iter()
        .filter_map(|(name, operation_id)| {
            let operation = operations
                .operations
                .iter()
                .find(|operation| &operation.id == operation_id)?;
            let targets = resolve_operation_targets(operation, &context, &dataset_states)?;
            let invocation = stabilize_operation_invocation(
                OperationInvocation {
                    operation: operation.id.clone(),
                    command: operation.command.clone(),
                    source_dataset: context.active_dataset,
                    targets,
                    source: InvocationSource::CommandPalette,
                },
                &keys,
            )
            .ok()?;
            Some(PaletteCommandEntry {
                name: name.clone(),
                label: operation.label.clone(),
                invocation,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let Ok(mut metadata) = contexts.get_mut(interaction) else {
        return OperationExecution::Rejected("Interaction Context has no metadata".into());
    };
    metadata.kind = InteractionContextKind::CommandPalette {
        query: String::new(),
        entries,
        selected: 0,
    };
    request_interaction_overlay(interaction, &mut transitions);
    OperationExecution::Completed
}

pub fn open_changes(
    In(_invocation): In<OperationInvocation>,
    world: &mut World,
) -> OperationExecution {
    let Some(context) = world.get_resource::<ActiveUiContext>().cloned() else {
        return OperationExecution::Rejected("active UI Context is unavailable".into());
    };
    let Some(repository) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("no current Repository for Changes".into());
    };
    let Some(metadata) = world.get::<RepositoryMetadata>(repository).cloned() else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Some(DatasetIdentity::Repository(repository_key)) =
        world.get::<DatasetKey>(repository).map(|key| &key.0)
    else {
        return OperationExecution::Rejected("current Repository identity is unavailable".into());
    };
    let identity = DatasetIdentity::Changes(repository_key.clone());
    if world
        .get::<DatasetKey>(context.active_dataset)
        .is_some_and(|key| key.0 == identity)
    {
        return OperationExecution::Rejected("Changes is already active".into());
    }
    let Some(template) = world
        .resource::<DefaultDatasetTemplates>()
        .get(pitui_data::DatasetKind::Changes)
        .cloned()
    else {
        return OperationExecution::Rejected(
            "default Changes Dataset template is unavailable".into(),
        );
    };
    let changes = match ensure_dataset_in_world(
        world,
        identity,
        pitui_data::DatasetKind::Changes,
        template,
    ) {
        Ok(entity) => entity,
        Err(error) => {
            return OperationExecution::Rejected(format!(
                "cannot create Changes Dataset: {error:?}"
            ));
        }
    };
    if !world
        .resource::<pitui_data::DatasetRoots>()
        .0
        .contains(&changes)
    {
        world
            .resource_mut::<pitui_data::DatasetRoots>()
            .0
            .push(changes);
    }
    world
        .resource_mut::<GitRequestQueue>()
        .enqueue(GitCommandData {
            repository_dataset: repository,
            cwd: metadata.0.root.clone(),
            command: GitCommand::LoadRepository,
        });
    world
        .resource_mut::<GitRequestQueue>()
        .enqueue(GitCommandData {
            repository_dataset: repository,
            cwd: metadata.0.root.clone(),
            command: GitCommand::LoadWorkingTree,
        });
    let mut bindings = context.render_bindings.clone();
    bindings.bind(pitui_data::RenderBindingId::Changes, changes);
    bindings.unbind(&pitui_data::RenderBindingId::CurrentChangesFileChanges);
    world
        .resource_mut::<Messages<ContextTransitionRequest>>()
        .write(ContextTransitionRequest::Push {
            active_dataset: changes,
            render_mode: RenderModeId::from("changes.unified"),
            render_bindings: bindings,
        });
    OperationExecution::Completed
}

/// Opens the repository-scoped Reflog Dataset and queues its Git
/// snapshot refresh. Dataset creation, Git execution and the context change are
/// all represented as World data; no renderer is called from this operation.
pub fn open_reflog(
    In(_invocation): In<OperationInvocation>,
    world: &mut World,
) -> OperationExecution {
    let Some(context) = world.get_resource::<ActiveUiContext>().cloned() else {
        return OperationExecution::Rejected("active UI Context is unavailable".into());
    };
    let Some(repository_entity) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("no current Repository for Reflog".into());
    };
    let Some(repository) = world.get::<RepositoryMetadata>(repository_entity).cloned() else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Some(DatasetIdentity::Repository(repository_key)) =
        world.get::<DatasetKey>(repository_entity).map(|key| &key.0)
    else {
        return OperationExecution::Rejected("current Repository identity is unavailable".into());
    };
    let identity = DatasetIdentity::Reflog(repository_key.clone());
    if world
        .get::<DatasetKey>(context.active_dataset)
        .is_some_and(|key| key.0 == identity)
    {
        return OperationExecution::Rejected("Reflog is already active".into());
    }
    let Some(template) = world
        .resource::<DefaultDatasetTemplates>()
        .get(pitui_data::DatasetKind::Reflog)
        .cloned()
    else {
        return OperationExecution::Rejected(
            "default Reflog Dataset template is unavailable".into(),
        );
    };
    let reflog =
        match ensure_dataset_in_world(world, identity, pitui_data::DatasetKind::Reflog, template) {
            Ok(entity) => entity,
            Err(error) => {
                return OperationExecution::Rejected(format!(
                    "cannot create Reflog Dataset: {error:?}"
                ));
            }
        };

    let mut bindings = context.render_bindings;
    bindings.bind(pitui_data::RenderBindingId::CurrentReflog, reflog);
    if let Some(entry) = world
        .get::<DatasetActiveElement>(reflog)
        .and_then(|active| active.0)
    {
        bindings.bind(pitui_data::RenderBindingId::CurrentReflogEntry, entry);
    } else {
        bindings.unbind(&pitui_data::RenderBindingId::CurrentReflogEntry);
    }
    world
        .resource_mut::<GitRequestQueue>()
        .enqueue(GitCommandData {
            repository_dataset: repository_entity,
            cwd: repository.0.root,
            command: GitCommand::LoadReflog { limit: 500 },
        });
    world
        .resource_mut::<Messages<ContextTransitionRequest>>()
        .write(ContextTransitionRequest::Push {
            active_dataset: reflog,
            render_mode: RenderModeId::from("reflog"),
            render_bindings: bindings,
        });
    OperationExecution::Completed
}

pub fn open_git_operation_log(
    In(_invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    index: Res<DatasetIndex>,
    active_elements: Query<&DatasetActiveElement>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let Some(log) = index.get(&DatasetIdentity::GlobalGitOperationLog) else {
        return OperationExecution::Rejected("global Git Operation Log is unavailable".into());
    };
    if context.active_dataset == log {
        return OperationExecution::Rejected("Git Operation Log is already active".into());
    }
    let mut bindings = context.render_bindings.clone();
    bindings.bind(pitui_data::RenderBindingId::GitOperationLog, log);
    if let Some(entry) = active_elements.get(log).ok().and_then(|active| active.0) {
        bindings.bind(
            pitui_data::RenderBindingId::CurrentGitOperationLogEntry,
            entry,
        );
    } else {
        bindings.unbind(&pitui_data::RenderBindingId::CurrentGitOperationLogEntry);
    }
    transitions.write(ContextTransitionRequest::Push {
        active_dataset: log,
        render_mode: RenderModeId::from("git-operation-log"),
        render_bindings: bindings,
    });
    OperationExecution::Completed
}

#[allow(clippy::too_many_arguments)]
pub fn refresh_active_context(
    In(_invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    index: Res<DatasetIndex>,
    kinds: Query<&DatasetType>,
    keys: Query<&DatasetKey>,
    repositories: Query<&RepositoryMetadata>,
    files: Query<&pitui_data::FileMetadata>,
    working_files: Query<&WorkingTreeFileMetadata>,
    mut git: ResMut<GitRequestQueue>,
) -> OperationExecution {
    let Some(repository) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("no current Repository to refresh".into());
    };
    let Ok(metadata) = repositories.get(repository) else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let cwd = metadata.0.root.clone();
    git.enqueue(GitCommandData {
        repository_dataset: repository,
        cwd: cwd.clone(),
        command: GitCommand::LoadRepository,
    });

    let active_kind = kinds.get(context.active_dataset).ok().map(|kind| kind.0);
    match active_kind {
        Some(
            pitui_data::DatasetKind::Changes
            | pitui_data::DatasetKind::WorkingTreeFiles
            | pitui_data::DatasetKind::WorkingTreeFile
            | pitui_data::DatasetKind::WorkingTreeFileChanges,
        ) => {
            git.enqueue(GitCommandData {
                repository_dataset: repository,
                cwd: cwd.clone(),
                command: GitCommand::LoadWorkingTree,
            });
        }
        Some(pitui_data::DatasetKind::Commits) => {
            if let Ok(DatasetKey(DatasetIdentity::Commits { branch, .. })) =
                keys.get(context.active_dataset)
            {
                git.enqueue(GitCommandData {
                    repository_dataset: repository,
                    cwd: cwd.clone(),
                    command: GitCommand::LoadCommits {
                        branch: branch.clone(),
                        limit: 500,
                    },
                });
            }
        }
        Some(pitui_data::DatasetKind::Reflog) => {
            git.enqueue(GitCommandData {
                repository_dataset: repository,
                cwd: cwd.clone(),
                command: GitCommand::LoadReflog { limit: 500 },
            });
        }
        _ => {
            git.enqueue(GitCommandData {
                repository_dataset: repository,
                cwd: cwd.clone(),
                command: GitCommand::LoadBranches,
            });
            if let Some(branch) = &metadata.0.current_branch {
                git.enqueue(GitCommandData {
                    repository_dataset: repository,
                    cwd: cwd.clone(),
                    command: GitCommand::LoadCommits {
                        branch: branch.clone(),
                        limit: 500,
                    },
                });
            }
        }
    }

    if let Some(commit) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentCommit)
        && let Ok(DatasetKey(DatasetIdentity::Commit { hash, .. })) = keys.get(commit)
    {
        git.enqueue(GitCommandData {
            repository_dataset: repository,
            cwd: cwd.clone(),
            command: GitCommand::LoadCommitDetail {
                commit: hash.clone(),
            },
        });
    }
    enqueue_current_file_diff(
        &context,
        repository,
        &cwd,
        &index,
        &keys,
        &files,
        &working_files,
        &mut git,
    );
    OperationExecution::Completed
}

#[allow(clippy::too_many_arguments)]
fn enqueue_current_file_diff(
    context: &ActiveUiContext,
    repository_entity: Entity,
    cwd: &std::path::Path,
    index: &DatasetIndex,
    keys: &Query<&DatasetKey>,
    files: &Query<&pitui_data::FileMetadata>,
    working_files: &Query<&WorkingTreeFileMetadata>,
    git: &mut GitRequestQueue,
) {
    if let Some(diff) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentFileChanges)
        && let Ok(DatasetKey(DatasetIdentity::FileChanges {
            repository,
            commit,
            path,
        })) = keys.get(diff)
    {
        let file = index.get(&DatasetIdentity::File {
            repository: repository.clone(),
            commit: commit.clone(),
            path: path.clone(),
        });
        git.enqueue(GitCommandData {
            repository_dataset: repository_entity,
            cwd: cwd.into(),
            command: GitCommand::LoadFileDiff {
                commit: commit.clone(),
                path: path.clone(),
                old_path: file
                    .and_then(|file| files.get(file).ok())
                    .and_then(|metadata| metadata.0.old_path.clone()),
            },
        });
    }
    if let Some(diff) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentChangesFileChanges)
        && let Ok(DatasetKey(DatasetIdentity::WorkingTreeFileChanges {
            repository,
            boundary,
            path,
        })) = keys.get(diff)
    {
        let file = index.get(&DatasetIdentity::WorkingTreeFile {
            repository: repository.clone(),
            boundary: *boundary,
            path: path.clone(),
        });
        let metadata = file.and_then(|file| working_files.get(file).ok());
        git.enqueue(GitCommandData {
            repository_dataset: repository_entity,
            cwd: cwd.into(),
            command: GitCommand::LoadWorkingTreeDiff {
                path: path.clone(),
                old_path: metadata.and_then(|metadata| metadata.0.old_path.clone()),
                include_staged: *boundary == pitui_data::ChangeBoundary::Staged,
                include_worktree: *boundary == pitui_data::ChangeBoundary::Unstaged
                    && metadata.is_some_and(|metadata| !metadata.0.is_untracked()),
                untracked: *boundary == pitui_data::ChangeBoundary::Unstaged
                    && metadata.is_some_and(|metadata| metadata.0.is_untracked()),
            },
        });
    }
}

pub fn close_interaction(
    In(_invocation): In<OperationInvocation>,
    stack: Res<ContextStack>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    if !stack.top_is(UiContextFrameKind::Overlay) {
        return OperationExecution::Rejected("no interaction Overlay to close".into());
    }
    transitions.write(ContextTransitionRequest::Pop);
    OperationExecution::Completed
}

pub fn palette_up(
    In(invocation): In<OperationInvocation>,
    mut contexts: Query<&mut InteractionContextMetadata>,
) -> OperationExecution {
    move_palette_selection(invocation.source_dataset, -1, &mut contexts)
}

pub fn palette_down(
    In(invocation): In<OperationInvocation>,
    mut contexts: Query<&mut InteractionContextMetadata>,
) -> OperationExecution {
    move_palette_selection(invocation.source_dataset, 1, &mut contexts)
}

pub fn confirmation_up(
    In(invocation): In<OperationInvocation>,
    mut contexts: Query<&mut InteractionContextMetadata>,
) -> OperationExecution {
    move_confirmation_selection(invocation.source_dataset, -1, &mut contexts)
}

pub fn confirmation_down(
    In(invocation): In<OperationInvocation>,
    mut contexts: Query<&mut InteractionContextMetadata>,
) -> OperationExecution {
    move_confirmation_selection(invocation.source_dataset, 1, &mut contexts)
}

pub fn submit_confirmation(
    In(invocation): In<OperationInvocation>,
    stack: Res<ContextStack>,
    contexts: Query<&InteractionContextMetadata>,
    mut deferred: ResMut<DeferredStableOperationInvocations>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    if !stack.top_is(UiContextFrameKind::Overlay) {
        return OperationExecution::Rejected("Confirmation has no owning Overlay".into());
    }
    let Ok(metadata) = contexts.get(invocation.source_dataset) else {
        return OperationExecution::Rejected("Confirmation Context is unavailable".into());
    };
    let InteractionContextKind::Confirmation {
        options,
        selected,
        pending,
        ..
    } = &metadata.kind
    else {
        return OperationExecution::Rejected("active Context is not a Confirmation".into());
    };
    if *selected >= options.len() {
        return OperationExecution::Rejected("Confirmation selection is out of range".into());
    }
    // Index zero is always the safe/cancel choice. The destructive invocation
    // is released only after the overlay has been popped.
    if *selected != 0 {
        deferred.0.push((**pending).clone());
    }
    transitions.write(ContextTransitionRequest::Pop);
    OperationExecution::Completed
}

pub fn submit_palette_command(
    In(invocation): In<OperationInvocation>,
    stack: Res<ContextStack>,
    contexts: Query<&InteractionContextMetadata>,
    mut deferred: ResMut<DeferredStableOperationInvocations>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    if !stack.top_is(UiContextFrameKind::Overlay) {
        return OperationExecution::Rejected("Command Palette has no owning Overlay".into());
    }
    let Ok(metadata) = contexts.get(invocation.source_dataset) else {
        return OperationExecution::Rejected("Command Context is unavailable".into());
    };
    let InteractionContextKind::CommandPalette {
        query,
        entries,
        selected,
    } = &metadata.kind
    else {
        return OperationExecution::Rejected("active Context is not the command palette".into());
    };
    let Some(entry) = entries
        .iter()
        .filter(|entry| entry.matches(query))
        .nth(*selected)
    else {
        return OperationExecution::Rejected("no command matches the current query".into());
    };
    deferred.0.push(entry.invocation.clone());
    transitions.write(ContextTransitionRequest::Pop);
    OperationExecution::Completed
}

fn request_interaction_overlay(
    context: Entity,
    transitions: &mut MessageWriter<ContextTransitionRequest>,
) {
    transitions.write(ContextTransitionRequest::PushOverlay {
        active_dataset: context,
        render_mode: RenderModeId::from("interaction-overlay"),
        proxy: RenderProxyId::from("interaction-context.overlay"),
        constraint: LayoutConstraint::Percentage(75),
    });
}

fn move_palette_selection(
    context: Entity,
    delta: isize,
    contexts: &mut Query<&mut InteractionContextMetadata>,
) -> OperationExecution {
    let Ok(mut metadata) = contexts.get_mut(context) else {
        return OperationExecution::Rejected("Command Context is unavailable".into());
    };
    let InteractionContextKind::CommandPalette {
        query,
        entries,
        selected,
    } = &mut metadata.kind
    else {
        return OperationExecution::Rejected("active Context is not the command palette".into());
    };
    let count = entries.iter().filter(|entry| entry.matches(query)).count();
    if count == 0 {
        *selected = 0;
        return OperationExecution::Completed;
    }
    *selected = selected
        .saturating_add_signed(delta)
        .min(count.saturating_sub(1));
    OperationExecution::Completed
}

fn move_confirmation_selection(
    context: Entity,
    delta: isize,
    contexts: &mut Query<&mut InteractionContextMetadata>,
) -> OperationExecution {
    let Ok(mut metadata) = contexts.get_mut(context) else {
        return OperationExecution::Rejected("Confirmation Context is unavailable".into());
    };
    let InteractionContextKind::Confirmation {
        options, selected, ..
    } = &mut metadata.kind
    else {
        return OperationExecution::Rejected("active Context is not a Confirmation".into());
    };
    if options.is_empty() {
        *selected = 0;
    } else {
        *selected = selected
            .saturating_add_signed(delta)
            .min(options.len().saturating_sub(1));
    }
    OperationExecution::Completed
}

pub fn activate_next_element(
    In(invocation): In<OperationInvocation>,
    mut datasets: Query<(&DatasetCollection, &mut DatasetActiveElement)>,
) -> OperationExecution {
    shift_active_element(
        invocation.source_dataset,
        ActiveDirection::Down,
        &mut datasets,
    )
}

pub fn transfer_active_left(
    In(_invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    mode: Res<ActiveRenderMode>,
    stack: Res<ContextStack>,
    dataset_types: Query<&DatasetType>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let mut active_candidates = Vec::new();
    mode.layout.active_candidates(&mut active_candidates);
    let Some(position) = active_candidates
        .iter()
        .position(|dataset| *dataset == context.active_dataset)
    else {
        return OperationExecution::Rejected("Active Dataset is not an Active candidate".into());
    };
    let Ok(kind) = dataset_types.get(context.active_dataset) else {
        return OperationExecution::Rejected("Active Dataset no longer exists".into());
    };
    if position > 0 {
        transitions.write(ContextTransitionRequest::ActiveRelay {
            previous_active_dataset: context.active_dataset,
            previous_active_kind: kind.0,
            direction: ActiveDirection::Left,
            next_active_dataset: active_candidates[position - 1],
            binding_patch: RenderBindingPatch::default(),
        });
        OperationExecution::Completed
    } else if matches!(
        stack.top().map(|frame| frame.kind),
        Some(UiContextFrameKind::View | UiContextFrameKind::ActiveHandoff { .. })
    ) {
        transitions.write(ContextTransitionRequest::ActiveReturn {
            previous_active_dataset: context.active_dataset,
            previous_active_kind: kind.0,
            direction: ActiveDirection::Left,
        });
        OperationExecution::Completed
    } else {
        OperationExecution::Rejected("already at the outermost Dataset level".into())
    }
}

pub fn transfer_active_right(
    In(_invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    mode: Res<ActiveRenderMode>,
    dataset_types: Query<&DatasetType>,
    active_elements: Query<&DatasetActiveElement>,
    handoffs: Res<ActiveHandoffRegistry>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let mut active_candidates = Vec::new();
    mode.layout.active_candidates(&mut active_candidates);
    let Some(position) = active_candidates
        .iter()
        .position(|dataset| *dataset == context.active_dataset)
    else {
        return OperationExecution::Rejected("Active Dataset is not an Active candidate".into());
    };
    if let Some(next) = active_candidates.get(position + 1) {
        let Ok(kind) = dataset_types.get(context.active_dataset) else {
            return OperationExecution::Rejected("Active Dataset no longer exists".into());
        };
        transitions.write(ContextTransitionRequest::ActiveRelay {
            previous_active_dataset: context.active_dataset,
            previous_active_kind: kind.0,
            direction: ActiveDirection::Right,
            next_active_dataset: *next,
            binding_patch: RenderBindingPatch::default(),
        });
        return OperationExecution::Completed;
    }

    let Ok(kind) = dataset_types.get(context.active_dataset) else {
        return OperationExecution::Rejected("Active Dataset no longer exists".into());
    };
    let Some(handoff) = handoffs.rules.get(&(kind.0, ActiveDirection::Right)) else {
        return OperationExecution::Rejected("already at the deepest Dataset level".into());
    };
    let next_active_dataset = match &handoff.target {
        ActiveHandoffTarget::KeepActiveDataset => Some(context.active_dataset),
        ActiveHandoffTarget::ActiveElement => active_elements
            .get(context.active_dataset)
            .ok()
            .and_then(|active| active.0),
        ActiveHandoffTarget::Binding(binding) => context.render_bindings.get(binding),
    };
    let Some(next_active_dataset) = next_active_dataset else {
        return OperationExecution::Rejected("Active handoff target is unavailable".into());
    };
    transitions.write(ContextTransitionRequest::ActiveHandoff {
        previous_active_dataset: context.active_dataset,
        previous_active_kind: kind.0,
        direction: ActiveDirection::Right,
        next_active_dataset,
        render_mode: handoff.render_mode.clone(),
        render_bindings: context.render_bindings.clone(),
    });
    OperationExecution::Completed
}

pub fn toggle_selection(
    In(invocation): In<OperationInvocation>,
    world: &mut World,
) -> OperationExecution {
    crate::collection::toggle_selection(world, invocation.source_dataset, &invocation.targets)
        .map_or_else(OperationExecution::Rejected, |()| {
            OperationExecution::Completed
        })
}

pub fn cycle_collection_view(
    In(invocation): In<OperationInvocation>,
    world: &mut World,
) -> OperationExecution {
    let Some(template_ref) = world
        .get::<DatasetTemplateRef>(invocation.source_dataset)
        .cloned()
    else {
        return OperationExecution::Rejected("Dataset Template is unavailable".into());
    };
    let Some(template) = world
        .resource::<DatasetTemplateRegistry>()
        .get(&template_ref.0)
        .cloned()
    else {
        return OperationExecution::Rejected("Dataset Template is not registered".into());
    };
    if template.views.len() < 2 {
        return OperationExecution::Rejected("Dataset has no alternate collection View".into());
    }
    let current = world
        .get::<DatasetViewState>(invocation.source_dataset)
        .and_then(|state| state.0.as_ref());
    let next = current
        .and_then(|current| template.views.iter().position(|view| &view.id == current))
        .map_or(0, |index| (index + 1) % template.views.len());
    world
        .entity_mut(invocation.source_dataset)
        .insert(DatasetViewState(Some(template.views[next].id.clone())));
    crate::collection::mark_collection_dirty(world, invocation.source_dataset);
    OperationExecution::Completed
}

/// Cherry-pick is owned by the Commits Dataset Operation Set. Its targets are
/// the Dataset's ordered Selection (never a queue and never an implicit
/// active element), normalized to oldest-to-newest replay order before Git argv data is
/// emitted.
pub fn cherry_pick_selected(
    In(invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    collections: Query<&DatasetCollection>,
    repositories: Query<&RepositoryMetadata>,
    mut git: ResMut<GitRequestQueue>,
) -> OperationExecution {
    let Ok(DatasetKey(DatasetIdentity::Commits {
        repository: source_repository,
        branch: source_branch,
    })) = keys.get(invocation.source_dataset)
    else {
        return OperationExecution::Rejected(
            "cherry-pick is only available from a Commits Dataset".into(),
        );
    };
    if invocation.targets.is_empty() {
        return OperationExecution::Rejected("select at least one commit to cherry-pick".into());
    }
    let Ok(collection) = collections.get(invocation.source_dataset) else {
        return OperationExecution::Rejected("Commits collection is unavailable".into());
    };
    let selected = invocation.targets.iter().copied().collect::<HashSet<_>>();
    if selected.len() != invocation.targets.len() {
        return OperationExecution::Rejected("Commit selection contains duplicate targets".into());
    }
    let ordered = collection
        .entities()
        .filter(|entity| selected.contains(entity))
        .collect::<Vec<_>>();
    if ordered.len() != selected.len() {
        return OperationExecution::Rejected(
            "Commit selection contains targets outside the active Commits Dataset".into(),
        );
    }
    let mut commits = Vec::with_capacity(ordered.len());
    for target in ordered {
        let Ok(DatasetKey(DatasetIdentity::Commit { repository, hash })) = keys.get(target) else {
            return OperationExecution::Rejected(
                "cherry-pick selection contains a non-Commit Dataset".into(),
            );
        };
        if repository != source_repository {
            return OperationExecution::Rejected(
                "cherry-pick cannot mix commits from different repositories".into(),
            );
        }
        commits.push(hash.clone());
    }
    commits.reverse();

    let Some(repository_entity) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("current Repository binding is unavailable".into());
    };
    if !matches!(
        keys.get(repository_entity).map(|key| &key.0),
        Ok(DatasetIdentity::Repository(repository)) if repository == source_repository
    ) {
        return OperationExecution::Rejected(
            "active Commits Dataset does not belong to the current Repository".into(),
        );
    }
    let Ok(repository) = repositories.get(repository_entity) else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Some(current_branch) = repository.0.current_branch.clone() else {
        return OperationExecution::Rejected(
            "cherry-pick requires an attached current branch".into(),
        );
    };
    let cwd = repository.0.root.clone();
    let mut refresh = vec![
        GitRefreshTarget::Repository,
        GitRefreshTarget::Branches,
        GitRefreshTarget::Commits {
            branch: current_branch.clone(),
            limit: 500,
        },
        GitRefreshTarget::WorkingTree,
    ];
    if source_branch != &current_branch {
        refresh.push(GitRefreshTarget::Commits {
            branch: source_branch.clone(),
            limit: 500,
        });
    }
    git.enqueue_with_refresh(
        GitCommandData {
            repository_dataset: repository_entity,
            cwd,
            command: GitCommand::CherryPick { commits },
        },
        GitRefreshPlan::new(refresh),
    );
    OperationExecution::Completed
}

pub fn submit_text_input(
    In(invocation): In<OperationInvocation>,
    contexts: Query<&InteractionContextMetadata>,
) -> OperationExecution {
    let Ok(metadata) = contexts.get(invocation.source_dataset) else {
        return OperationExecution::Rejected("Text Input Context is unavailable".into());
    };
    let InteractionContextKind::TextInput { purpose, .. } = &metadata.kind else {
        return OperationExecution::Rejected("active Context is not a Text Input".into());
    };
    OperationExecution::Rejected(format!(
        "Text Input purpose {purpose:?} is not implemented yet"
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn navigate_back(
    In(_invocation): In<OperationInvocation>,
    stack: Res<ContextStack>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    if stack.0.is_empty() {
        OperationExecution::Rejected("already at the outermost Dataset level".into())
    } else {
        transitions.write(ContextTransitionRequest::Pop);
        OperationExecution::Completed
    }
}

fn shift_active_element(
    dataset: Entity,
    direction: ActiveDirection,
    datasets: &mut Query<(&DatasetCollection, &mut DatasetActiveElement)>,
) -> OperationExecution {
    let delta = match direction {
        ActiveDirection::Up => -1,
        ActiveDirection::Down => 1,
        ActiveDirection::Left | ActiveDirection::Right => {
            return OperationExecution::Rejected(
                "horizontal direction cannot change a Dataset element".into(),
            );
        }
    };
    let Ok((collection, mut active)) = datasets.get_mut(dataset) else {
        return OperationExecution::Rejected(
            "active Dataset has no Collection Manager state".into(),
        );
    };
    if collection.0.is_empty() {
        active.0 = None;
        return OperationExecution::Completed;
    }
    let current = active
        .0
        .and_then(|current| collection.position(current))
        .unwrap_or_default();
    let next = current
        .saturating_add_signed(delta)
        .min(collection.0.len() - 1);
    active.0 = Some(collection.0[next].entity);
    OperationExecution::Completed
}
