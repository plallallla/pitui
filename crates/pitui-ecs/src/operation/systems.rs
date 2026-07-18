//! Built-in operation functions. Every entry registered in `OperationManager`
//! is an ordinary Bevy ECS System: invocation context enters through `In`, and
//! World state/effect messages are changed through typed System parameters.

use super::*;

#[derive(Resource, Clone, Debug, Default)]
pub struct PendingChangesActiveRelay(Option<ChangesActiveRelayRequest>);

#[derive(Clone, Debug)]
struct ChangesActiveRelayRequest {
    mutation: GitCommand,
    success_index: usize,
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

pub fn open_command_palette(
    In(_invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    operations: Res<ResolvedOperationSet>,
    index: Res<DatasetIndex>,
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
            Some(PaletteCommandEntry {
                name: name.clone(),
                label: operation.label.clone(),
                invocation: OperationInvocation {
                    operation: operation.id.clone(),
                    command: operation.command.clone(),
                    source_dataset: context.active_dataset,
                    targets,
                    source: InvocationSource::CommandPalette,
                },
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
    context: Res<ActiveUiContext>,
    index: Res<DatasetIndex>,
    repositories: Query<&RepositoryMetadata>,
    mut git: MessageWriter<GitCommandData>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let Some(changes) = index.get(&DatasetIdentity::GlobalChanges) else {
        return OperationExecution::Rejected("global Changes Dataset is unavailable".into());
    };
    if context.active_dataset == changes {
        return OperationExecution::Rejected("Changes is already active".into());
    }
    let Some(repository) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("no current Repository for Changes".into());
    };
    let Ok(metadata) = repositories.get(repository) else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    git.write(GitCommandData {
        repository_dataset: repository,
        cwd: metadata.0.root.clone(),
        command: GitCommand::LoadRepository,
    });
    git.write(GitCommandData {
        repository_dataset: repository,
        cwd: metadata.0.root.clone(),
        command: GitCommand::LoadWorkingTree,
    });
    let mut bindings = context.render_bindings.clone();
    bindings.bind(pitui_data::RenderBindingId::Changes, changes);
    bindings.unbind(&pitui_data::RenderBindingId::CurrentChangesFileChanges);
    transitions.write(ContextTransitionRequest::Push {
        active_dataset: changes,
        render_mode: RenderModeId::from("changes.unified"),
        render_bindings: bindings,
    });
    OperationExecution::Completed
}

/// Opens the repository-scoped Reflog Dataset and queues its synchronous Git
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
        .resource_mut::<Messages<GitCommandData>>()
        .write(GitCommandData {
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
    mut git: MessageWriter<GitCommandData>,
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
    git.write(GitCommandData {
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
            git.write(GitCommandData {
                repository_dataset: repository,
                cwd: cwd.clone(),
                command: GitCommand::LoadWorkingTree,
            });
        }
        Some(pitui_data::DatasetKind::Commits) => {
            if let Ok(DatasetKey(DatasetIdentity::Commits { branch, .. })) =
                keys.get(context.active_dataset)
            {
                git.write(GitCommandData {
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
            git.write(GitCommandData {
                repository_dataset: repository,
                cwd: cwd.clone(),
                command: GitCommand::LoadReflog { limit: 500 },
            });
        }
        _ => {
            git.write(GitCommandData {
                repository_dataset: repository,
                cwd: cwd.clone(),
                command: GitCommand::LoadBranches,
            });
            if let Some(branch) = &metadata.0.current_branch {
                git.write(GitCommandData {
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
        git.write(GitCommandData {
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
    git: &mut MessageWriter<GitCommandData>,
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
        git.write(GitCommandData {
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
        git.write(GitCommandData {
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
    if stack.0.is_empty() {
        return OperationExecution::Rejected("no previous Context to restore".into());
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

pub fn submit_palette_command(
    In(invocation): In<OperationInvocation>,
    contexts: Query<&InteractionContextMetadata>,
    mut deferred: ResMut<DeferredOperationInvocations>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
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
    } else if !stack.0.is_empty() {
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
    mut git: MessageWriter<GitCommandData>,
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
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: GitCommand::CherryPick { commits },
    });
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: GitCommand::LoadRepository,
    });
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: GitCommand::LoadBranches,
    });
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: GitCommand::LoadCommits {
            branch: current_branch.clone(),
            limit: 500,
        },
    });
    if source_branch != &current_branch {
        git.write(GitCommandData {
            repository_dataset: repository_entity,
            cwd: cwd.clone(),
            command: GitCommand::LoadCommits {
                branch: source_branch.clone(),
                limit: 500,
            },
        });
    }
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd,
        command: GitCommand::LoadWorkingTree,
    });
    OperationExecution::Completed
}

pub fn toggle_changes_selection(
    In(invocation): In<OperationInvocation>,
    world: &mut World,
) -> OperationExecution {
    let Some(changes) = world.get_resource::<ActiveUiContext>().and_then(|context| {
        context
            .render_bindings
            .get(&pitui_data::RenderBindingId::Changes)
    }) else {
        return OperationExecution::Rejected("Changes binding is unavailable".into());
    };
    if invocation.targets.iter().any(|target| {
        !matches!(
            world.get::<DatasetKey>(*target).map(|key| &key.0),
            Some(
                DatasetIdentity::WorkingTreeFiles { .. }
                    | DatasetIdentity::WorkingTreeFile { .. }
                    | DatasetIdentity::WorkingTreeDirectory { .. }
            )
        )
    }) {
        return OperationExecution::Rejected(
            "only working-tree groups, files and directories can be selected".into(),
        );
    }
    crate::collection::toggle_selection(world, changes, &invocation.targets)
        .map_or_else(OperationExecution::Rejected, |()| {
            OperationExecution::Completed
        })
}

#[allow(clippy::too_many_arguments)]
pub fn stage_changes(
    In(invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    children: Query<&DatasetChildren>,
    repositories: Query<&RepositoryMetadata>,
    revisions: Query<&DatasetRevision>,
    successes: Res<GitMutationSuccesses>,
    mut pending_relay: ResMut<PendingChangesActiveRelay>,
    mut git: MessageWriter<GitCommandData>,
) -> OperationExecution {
    mutate_working_tree_paths(
        invocation,
        ChangeBoundary::Unstaged,
        |paths| GitCommand::StagePaths { paths },
        &context,
        &keys,
        &children,
        &repositories,
        &revisions,
        &successes,
        &mut pending_relay,
        &mut git,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn unstage_changes(
    In(invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    children: Query<&DatasetChildren>,
    repositories: Query<&RepositoryMetadata>,
    revisions: Query<&DatasetRevision>,
    successes: Res<GitMutationSuccesses>,
    mut pending_relay: ResMut<PendingChangesActiveRelay>,
    mut git: MessageWriter<GitCommandData>,
) -> OperationExecution {
    mutate_working_tree_paths(
        invocation,
        ChangeBoundary::Staged,
        |paths| GitCommand::UnstagePaths { paths },
        &context,
        &keys,
        &children,
        &repositories,
        &revisions,
        &successes,
        &mut pending_relay,
        &mut git,
    )
}

#[allow(clippy::too_many_arguments)]
fn mutate_working_tree_paths(
    invocation: OperationInvocation,
    expected_boundary: ChangeBoundary,
    command: impl FnOnce(Vec<pitui_core::GitPath>) -> GitCommand,
    context: &ActiveUiContext,
    keys: &Query<&DatasetKey>,
    children: &Query<&DatasetChildren>,
    repositories: &Query<&RepositoryMetadata>,
    revisions: &Query<&DatasetRevision>,
    successes: &GitMutationSuccesses,
    pending_relay: &mut PendingChangesActiveRelay,
    git: &mut MessageWriter<GitCommandData>,
) -> OperationExecution {
    let Some(repository_entity) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("current Repository binding is unavailable".into());
    };
    let Ok(repository_metadata) = repositories.get(repository_entity) else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Ok(DatasetKey(DatasetIdentity::Repository(repository_key))) = keys.get(repository_entity)
    else {
        return OperationExecution::Rejected("current Repository identity is unavailable".into());
    };

    let mut seen = HashSet::new();
    let mut paths = Vec::with_capacity(invocation.targets.len());
    for target in &invocation.targets {
        if let Err(error) = collect_working_tree_paths(
            *target,
            repository_key,
            expected_boundary,
            keys,
            children,
            &mut seen,
            &mut paths,
        ) {
            return OperationExecution::Rejected(error);
        }
    }
    if paths.is_empty() {
        return OperationExecution::Rejected("no working-tree files were selected".into());
    }

    let Some(changes) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::Changes)
    else {
        return OperationExecution::Rejected("Changes binding is unavailable".into());
    };
    let Ok(changes_revision) = revisions.get(changes) else {
        return OperationExecution::Rejected("Changes revision is unavailable".into());
    };

    let active_path = paths[0].clone();
    let mutation = command(paths);
    pending_relay.0 = Some(ChangesActiveRelayRequest {
        mutation: mutation.clone(),
        success_index: successes.0.len(),
        changes_revision: changes_revision.0,
        target: ChangesActiveTarget::Exact {
            repository: repository_key.clone(),
            path: active_path,
            boundary: match expected_boundary {
                ChangeBoundary::Staged => ChangeBoundary::Unstaged,
                ChangeBoundary::Unstaged => ChangeBoundary::Staged,
            },
        },
        context: ChangesActiveContext::Active,
        preserve_diff_active: context
            .render_bindings
            .get(&pitui_data::RenderBindingId::CurrentChangesFileChanges)
            == Some(context.active_dataset),
    });

    let cwd = repository_metadata.0.root.clone();
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: mutation,
    });
    // The synchronous executor processes this message batch in order, so the
    // read snapshots observe the mutation while Dataset replacement remains in
    // the common Git result/apply path.
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: GitCommand::LoadRepository,
    });
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd,
        command: GitCommand::LoadWorkingTree,
    });
    OperationExecution::Completed
}

#[allow(clippy::too_many_arguments)]
fn collect_working_tree_paths(
    target: Entity,
    expected_repository: &pitui_data::RepositoryKey,
    expected_boundary: ChangeBoundary,
    keys: &Query<&DatasetKey>,
    children: &Query<&DatasetChildren>,
    seen: &mut HashSet<pitui_core::GitPath>,
    paths: &mut Vec<pitui_core::GitPath>,
) -> Result<(), String> {
    match keys.get(target).map(|key| &key.0) {
        Ok(DatasetIdentity::WorkingTreeFile {
            repository,
            boundary,
            path,
        }) => {
            validate_working_tree_target(
                repository,
                *boundary,
                expected_repository,
                expected_boundary,
            )?;
            if seen.insert(path.clone()) {
                paths.push(path.clone());
            }
            Ok(())
        }
        Ok(
            DatasetIdentity::WorkingTreeFiles {
                repository,
                boundary,
            }
            | DatasetIdentity::WorkingTreeDirectory {
                repository,
                boundary,
                ..
            },
        ) => {
            validate_working_tree_target(
                repository,
                *boundary,
                expected_repository,
                expected_boundary,
            )?;
            let directory_children = children
                .get(target)
                .map_err(|_| "working-tree group contents are unavailable".to_owned())?;
            for child in &directory_children.0 {
                collect_working_tree_paths(
                    *child,
                    expected_repository,
                    expected_boundary,
                    keys,
                    children,
                    seen,
                    paths,
                )?;
            }
            Ok(())
        }
        _ => Err("target is not a working-tree group, file or directory Dataset".into()),
    }
}

fn validate_working_tree_target(
    repository: &pitui_data::RepositoryKey,
    boundary: ChangeBoundary,
    expected_repository: &pitui_data::RepositoryKey,
    expected_boundary: ChangeBoundary,
) -> Result<(), String> {
    if repository != expected_repository {
        return Err("working-tree target belongs to a different Repository".into());
    }
    if boundary != expected_boundary {
        return Err(format!(
            "working-tree target has the wrong change boundary: expected {expected_boundary:?}"
        ));
    }
    Ok(())
}

pub fn reconcile_pending_changes_active(world: &mut World) {
    let request = world.resource_mut::<PendingChangesActiveRelay>().0.take();
    let Some(request) = request else {
        return;
    };
    let mutation_succeeded = world
        .resource::<GitMutationSuccesses>()
        .0
        .get(request.success_index)
        .is_some_and(|success| success.command == request.mutation);
    if !mutation_succeeded {
        return;
    }

    let Some(changes) = world
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalChanges)
    else {
        return;
    };
    if world
        .get::<DatasetRevision>(changes)
        .is_none_or(|revision| revision.0 <= request.changes_revision)
    {
        return;
    }
    // A successful mutation replaces the selected boundary snapshot. Clear the
    // old selection before relaying Active; otherwise the persistent empty
    // Staged/Unstaged group would remain selected and create mixed-boundary
    // targets on the next operation.
    if let Some(mut selection) = world.get_mut::<DatasetSelection>(changes) {
        selection.0.clear();
    }
    let Some(collection) = world.get::<DatasetCollection>(changes) else {
        return;
    };
    let may_be_empty = matches!(&request.target, ChangesActiveTarget::FirstRemainingFile);
    let file = match request.target {
        ChangesActiveTarget::Exact {
            repository,
            path,
            boundary,
        } => world
            .resource::<DatasetIndex>()
            .get(&DatasetIdentity::WorkingTreeFile {
                repository,
                boundary,
                path,
            })
            .filter(|file| collection.contains(*file)),
        ChangesActiveTarget::FirstRemainingFile => collection.entities().find(|row| {
            matches!(
                world.get::<DatasetKey>(*row).map(|key| &key.0),
                Some(DatasetIdentity::WorkingTreeFile { .. })
            )
        }),
    };
    let Some(file) = file else {
        if may_be_empty {
            match request.context {
                ChangesActiveContext::Active => {
                    if let Some(mut context) = world.get_resource_mut::<ActiveUiContext>() {
                        context
                            .render_bindings
                            .unbind(&pitui_data::RenderBindingId::CurrentChangesFileChanges);
                        context.active_dataset = changes;
                        context.generation = context.generation.wrapping_add(1);
                    }
                }
                ChangesActiveContext::Previous => {
                    let mut stack = world.resource_mut::<ContextStack>();
                    if let Some(snapshot) = stack.0.last_mut() {
                        snapshot
                            .render_bindings
                            .unbind(&pitui_data::RenderBindingId::CurrentChangesFileChanges);
                        snapshot.active_dataset = changes;
                    }
                }
            }
        }
        return;
    };
    if let Some(mut active) = world.get_mut::<DatasetActiveElement>(changes) {
        active.0 = Some(file);
    }

    let Some(diff) = world
        .get::<pitui_data::DatasetChildren>(file)
        .and_then(|children| children.0.first().copied())
    else {
        return;
    };
    match request.context {
        ChangesActiveContext::Active => {
            let Some(mut context) = world.get_resource_mut::<ActiveUiContext>() else {
                return;
            };
            context
                .render_bindings
                .bind(pitui_data::RenderBindingId::CurrentChangesFileChanges, diff);
            if request.preserve_diff_active {
                context.active_dataset = diff;
            }
            context.generation = context.generation.wrapping_add(1);
        }
        ChangesActiveContext::Previous => {
            let mut stack = world.resource_mut::<ContextStack>();
            let Some(snapshot) = stack.0.last_mut() else {
                return;
            };
            snapshot
                .render_bindings
                .bind(pitui_data::RenderBindingId::CurrentChangesFileChanges, diff);
            if request.preserve_diff_active {
                snapshot.active_dataset = diff;
            }
        }
    }
}

pub fn open_commit_creation(
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
        return OperationExecution::Rejected("current Repository binding is unavailable".into());
    };
    let Some(DatasetIdentity::Repository(repository)) =
        world.get::<DatasetKey>(repository_entity).map(|key| &key.0)
    else {
        return OperationExecution::Rejected("current Repository identity is unavailable".into());
    };
    let repository = repository.clone();
    let Some(changes) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::Changes)
    else {
        return OperationExecution::Rejected("Changes binding is unavailable".into());
    };
    let Some(revision) = world
        .get::<DatasetRevision>(changes)
        .map(|revision| revision.0)
    else {
        return OperationExecution::Rejected("Changes revision is unavailable".into());
    };
    let staged_paths = world
        .get::<DatasetCollection>(changes)
        .map(|collection| {
            collection
                .entities()
                .filter_map(
                    |entity| match world.get::<DatasetKey>(entity).map(|key| &key.0) {
                        Some(DatasetIdentity::WorkingTreeFile {
                            repository: target_repository,
                            boundary: ChangeBoundary::Staged,
                            path,
                        }) if target_repository == &repository => Some(path.clone()),
                        _ => None,
                    },
                )
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if staged_paths.is_empty() {
        return OperationExecution::Rejected("there are no staged files to commit".into());
    }
    let Some(template) = world
        .resource::<DefaultDatasetTemplates>()
        .get(pitui_data::DatasetKind::CommitCreation)
        .cloned()
    else {
        return OperationExecution::Rejected(
            "default Commit Creation Dataset template is unavailable".into(),
        );
    };
    let creation = match ensure_dataset_in_world(
        world,
        DatasetIdentity::CommitCreation(repository.clone()),
        pitui_data::DatasetKind::CommitCreation,
        template,
    ) {
        Ok(entity) => entity,
        Err(error) => {
            return OperationExecution::Rejected(format!(
                "cannot create Commit Creation Dataset: {error:?}"
            ));
        }
    };
    world.entity_mut(creation).insert(CommitCreationMetadata {
        repository,
        message: String::new(),
        error: None,
        staged_revision: revision,
        staged_paths,
    });
    world
        .resource_mut::<Messages<ContextTransitionRequest>>()
        .write(ContextTransitionRequest::PushOverlay {
            active_dataset: creation,
            render_mode: RenderModeId::from("commit-creation-overlay"),
            proxy: RenderProxyId::from("commit-creation.editor"),
            constraint: LayoutConstraint::Percentage(75),
        });
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
pub fn submit_commit_creation(
    In(invocation): In<OperationInvocation>,
    stack: Res<ContextStack>,
    keys: Query<&DatasetKey>,
    revisions: Query<&DatasetRevision>,
    repositories: Query<&RepositoryMetadata>,
    mut creations: Query<&mut CommitCreationMetadata>,
    successes: Res<GitMutationSuccesses>,
    mut pending_relay: ResMut<PendingChangesActiveRelay>,
    mut git: MessageWriter<GitCommandData>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let (repository_key, message, staged_revision) = {
        let Ok(mut creation) = creations.get_mut(invocation.source_dataset) else {
            return OperationExecution::Rejected("Commit Creation Dataset is unavailable".into());
        };
        let message = creation.message.trim();
        if message.is_empty() {
            creation.error = Some("Commit message cannot be empty".into());
            return OperationExecution::Rejected("commit message cannot be empty".into());
        }
        (
            creation.repository.clone(),
            message.to_owned(),
            creation.staged_revision,
        )
    };

    let Some(previous) = stack.0.last() else {
        return OperationExecution::Rejected("no Changes Context to restore".into());
    };
    let Some(repository_entity) = previous
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("current Repository binding is unavailable".into());
    };
    if !matches!(
        keys.get(repository_entity).map(|key| &key.0),
        Ok(DatasetIdentity::Repository(repository)) if repository == &repository_key
    ) {
        return OperationExecution::Rejected(
            "Commit Creation repository no longer matches the current Repository".into(),
        );
    }
    let Ok(repository) = repositories.get(repository_entity) else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Some(changes) = previous
        .render_bindings
        .get(&pitui_data::RenderBindingId::Changes)
    else {
        return OperationExecution::Rejected("Changes binding is unavailable".into());
    };
    let Ok(changes_revision) = revisions.get(changes) else {
        return OperationExecution::Rejected("Changes revision is unavailable".into());
    };
    if changes_revision.0 != staged_revision {
        if let Ok(mut creation) = creations.get_mut(invocation.source_dataset) {
            creation.error = Some("The staged snapshot changed; reopen commit creation".into());
        }
        return OperationExecution::Rejected("the staged snapshot changed".into());
    }

    let cwd = repository.0.root.clone();
    let current_branch = repository.0.current_branch.clone();
    let mutation = GitCommand::Commit { message };
    pending_relay.0 = Some(ChangesActiveRelayRequest {
        mutation: mutation.clone(),
        success_index: successes.0.len(),
        changes_revision: changes_revision.0,
        target: ChangesActiveTarget::FirstRemainingFile,
        context: ChangesActiveContext::Previous,
        preserve_diff_active: previous
            .render_bindings
            .get(&pitui_data::RenderBindingId::CurrentChangesFileChanges)
            == Some(previous.active_dataset),
    });
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: mutation,
    });
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: GitCommand::LoadRepository,
    });
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd: cwd.clone(),
        command: GitCommand::LoadBranches,
    });
    if let Some(branch) = current_branch {
        git.write(GitCommandData {
            repository_dataset: repository_entity,
            cwd: cwd.clone(),
            command: GitCommand::LoadCommits { branch, limit: 500 },
        });
    }
    git.write(GitCommandData {
        repository_dataset: repository_entity,
        cwd,
        command: GitCommand::LoadWorkingTree,
    });
    transitions.write(ContextTransitionRequest::Pop);
    OperationExecution::Completed
}

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

pub fn copy_commit_hashes(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let hashes = invocation
        .targets
        .iter()
        .filter_map(|entity| match keys.get(*entity).ok().map(|key| &key.0) {
            Some(DatasetIdentity::Commit { hash, .. }) => Some(hash.0.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if hashes.len() != invocation.targets.len() || hashes.is_empty() {
        return OperationExecution::Rejected("copy hash target is not a Commit Dataset".into());
    }
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitHashes,
        text: hashes.join("\n"),
        source_entities: invocation.targets,
    });
    OperationExecution::Completed
}

pub fn copy_reflog_hash(
    In(invocation): In<OperationInvocation>,
    entries: Query<&ReflogEntryMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no Reflog entry target".into());
    };
    let Ok(metadata) = entries.get(target) else {
        return OperationExecution::Rejected(
            "copy hash target is not a Reflog entry Dataset".into(),
        );
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::ReflogHash,
        text: metadata.0.hash.0.clone(),
        source_entities: vec![target],
    });
    OperationExecution::Completed
}

pub fn copy_commit_info(
    In(invocation): In<OperationInvocation>,
    commits: Query<&CommitMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no Commit target".into());
    };
    let Ok(metadata) = commits.get(target) else {
        return OperationExecution::Rejected("copy info target has no Commit metadata".into());
    };
    let mut refs = metadata.summary.decorations.clone();
    if refs.is_empty() && !metadata.tags.is_empty() {
        refs = metadata.tags.join(", ");
    }
    let refs = if refs.is_empty() {
        String::new()
    } else {
        format!("\nRefs: {refs}")
    };
    let message = metadata
        .message
        .as_deref()
        .unwrap_or(&metadata.summary.subject);
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitInfo,
        text: format!(
            "commit {}\nAuthor: {}\nDate:   {}{}\n\n{}",
            metadata.summary.hash.0,
            metadata.summary.author,
            metadata.summary.authored_at,
            refs,
            message
        ),
        source_entities: vec![target],
    });
    OperationExecution::Completed
}

pub fn copy_commit_message(
    In(invocation): In<OperationInvocation>,
    commits: Query<&CommitMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no Commit target".into());
    };
    let Some(message) = commits
        .get(target)
        .ok()
        .and_then(|metadata| metadata.message.clone())
    else {
        return OperationExecution::Rejected("full commit message is not loaded".into());
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitMessage,
        text: message,
        source_entities: vec![target],
    });
    OperationExecution::Completed
}

pub fn copy_commit_field_values(
    In(invocation): In<OperationInvocation>,
    fields: Query<&CommitFieldMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let values = invocation
        .targets
        .iter()
        .map(|target| fields.get(*target).cloned())
        .collect::<Result<Vec<_>, _>>();
    let Ok(values) = values else {
        return OperationExecution::Rejected(
            "copy value target is not a Commit field Dataset".into(),
        );
    };
    if values.is_empty() {
        return OperationExecution::Rejected("no Commit field target".into());
    }
    let text = if values.len() == 1 {
        values[0].value.clone()
    } else {
        values
            .iter()
            .map(|metadata| format!("{}: {}", metadata.field.label(), metadata.value))
            .collect::<Vec<_>>()
            .join("\n")
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitFieldValues,
        text,
        source_entities: invocation.targets,
    });
    OperationExecution::Completed
}

pub fn copy_file_name(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    copy_file_path(
        invocation,
        &keys,
        ClipboardContentKind::FileName,
        |_, path| {
            PathBuf::from(path.to_os_string())
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        },
        &mut clipboard,
    )
}

pub fn copy_file_absolute_path(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    copy_file_path(
        invocation,
        &keys,
        ClipboardContentKind::FileAbsolutePath,
        |repository, path| {
            Some(
                repository
                    .as_path()
                    .join(PathBuf::from(path.to_os_string()))
                    .to_string_lossy()
                    .into_owned(),
            )
        },
        &mut clipboard,
    )
}

pub fn copy_file_relative_path(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    copy_file_path(
        invocation,
        &keys,
        ClipboardContentKind::FileRelativePath,
        |_, path| Some(path.as_str().into()),
        &mut clipboard,
    )
}

fn copy_file_path(
    invocation: OperationInvocation,
    keys: &Query<&DatasetKey>,
    kind: ClipboardContentKind,
    value: impl FnOnce(&pitui_data::RepositoryKey, &pitui_core::GitPath) -> Option<String>,
    clipboard: &mut MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no File target".into());
    };
    let Ok(key) = keys.get(target) else {
        return OperationExecution::Rejected("File target no longer exists".into());
    };
    let (repository, path) = match &key.0 {
        DatasetIdentity::FileDirectory {
            repository, path, ..
        }
        | DatasetIdentity::File {
            repository, path, ..
        }
        | DatasetIdentity::WorkingTreeDirectory {
            repository, path, ..
        }
        | DatasetIdentity::WorkingTreeFile {
            repository, path, ..
        } => (repository, path),
        _ => {
            return OperationExecution::Rejected(
                "copy path target is not a file or directory Dataset".into(),
            );
        }
    };
    let Some(text) = value(repository, path) else {
        return OperationExecution::Rejected("File path has no copyable name".into());
    };
    clipboard.write(ClipboardRequest {
        kind,
        text,
        source_entities: vec![target],
    });
    OperationExecution::Completed
}

pub fn scroll_home(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::Home,
        &mut viewports,
    )
}

pub fn scroll_end(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(invocation.source_dataset, ScrollAction::End, &mut viewports)
}

pub fn scroll_page_up(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::PageUp,
        &mut viewports,
    )
}

pub fn scroll_page_down(
    In(invocation): In<OperationInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> OperationExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::PageDown,
        &mut viewports,
    )
}

#[derive(Clone, Copy)]
enum ScrollAction {
    Home,
    End,
    PageUp,
    PageDown,
}

fn update_scroll(
    dataset: Entity,
    action: ScrollAction,
    viewports: &mut Query<&mut DatasetViewport>,
) -> OperationExecution {
    let Ok(mut viewport) = viewports.get_mut(dataset) else {
        return OperationExecution::Rejected("active Dataset has no text viewport".into());
    };
    let page_size = viewport.page_size.max(1);
    let max_offset = viewport.content_length.saturating_sub(page_size);
    viewport.offset = match action {
        ScrollAction::Home => 0,
        ScrollAction::End => max_offset,
        ScrollAction::PageUp => viewport.offset.saturating_sub(page_size),
        ScrollAction::PageDown => viewport.offset.saturating_add(page_size).min(max_offset),
    };
    OperationExecution::Completed
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
