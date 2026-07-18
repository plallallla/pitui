use bevy_ecs::prelude::{Entity, MessageReader, Messages, ResMut, Resource, World};
use pitui_data::{
    ActiveRenderMode, ActiveUiContext, ContextStack, ContextTransitionRequest, Dataset,
    DatasetBinding, DatasetChildren, DatasetCursor, DatasetIdentity, DatasetIndex, DatasetKey,
    OperationNotice, RenderBindingId, RenderLayout, RenderModeRegistry, RepositoryMetadata,
    ResolvedOperationSet, ResolvedRenderLayout,
};

use crate::{KernelError, resolve_render_layout};

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderReconcileDiagnostics {
    pub last_render_error: Option<KernelError>,
    pub last_transition_error: Option<KernelError>,
}

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct PendingContextTransitions(Vec<ContextTransitionRequest>);

pub(super) fn initialize_binding_reconcile(world: &mut World) {
    world.init_resource::<Messages<ContextTransitionRequest>>();
    world.init_resource::<PendingContextTransitions>();
}

pub(super) fn collect_context_transitions(
    mut transitions: MessageReader<ContextTransitionRequest>,
    mut pending: ResMut<PendingContextTransitions>,
) {
    pending.0.extend(transitions.read().cloned());
}

pub(super) fn apply_context_transitions(world: &mut World) {
    let transitions = std::mem::take(&mut world.resource_mut::<PendingContextTransitions>().0);
    for transition in transitions {
        if let Err(error) = apply_context_transition(world, transition) {
            world
                .resource_mut::<RenderReconcileDiagnostics>()
                .last_transition_error = Some(error.clone());
            world.resource_mut::<Messages<OperationNotice>>().write(
                OperationNotice::ContextTransitionRejected(format!("{error:?}")),
            );
        }
    }
}

fn apply_context_transition(
    world: &mut World,
    transition: ContextTransitionRequest,
) -> Result<(), KernelError> {
    let transition = match transition {
        ContextTransitionRequest::PushOverlay {
            active_dataset,
            render_mode,
            proxy,
            constraint,
        } => {
            return push_overlay_context(world, active_dataset, render_mode, proxy, constraint);
        }
        transition => transition,
    };

    let current = world
        .get_resource::<ActiveUiContext>()
        .cloned()
        .ok_or(KernelError::ContextUnavailable)?;
    let (active_dataset, render_mode, render_bindings, stack_action) = match transition {
        ContextTransitionRequest::KeepRenderMode {
            active_dataset,
            binding_patch,
        } => {
            let mut bindings = current.render_bindings.clone();
            binding_patch.apply_to(&mut bindings);
            (
                active_dataset,
                current.render_mode.clone(),
                bindings,
                StackAction::Keep,
            )
        }
        ContextTransitionRequest::Replace {
            active_dataset,
            render_mode,
            render_bindings,
        } => (
            active_dataset,
            render_mode,
            render_bindings,
            StackAction::Keep,
        ),
        ContextTransitionRequest::Push {
            active_dataset,
            render_mode,
            render_bindings,
        } => (
            active_dataset,
            render_mode,
            render_bindings,
            StackAction::Push,
        ),
        ContextTransitionRequest::Drill {
            active_dataset,
            render_mode,
            render_bindings,
        } => (
            active_dataset,
            render_mode,
            render_bindings,
            StackAction::Drill,
        ),
        ContextTransitionRequest::PushOverlay { .. } => {
            unreachable!("overlay transitions return before ordinary resolution")
        }
        ContextTransitionRequest::Pop => {
            let snapshot = world
                .resource::<ContextStack>()
                .0
                .last()
                .cloned()
                .ok_or(KernelError::ContextUnavailable)?;
            (
                snapshot.active_dataset,
                snapshot.render_mode,
                snapshot.render_bindings,
                StackAction::Pop,
            )
        }
    };

    let mode = world
        .resource::<RenderModeRegistry>()
        .get(&render_mode)
        .cloned()
        .ok_or_else(|| KernelError::MissingRenderMode(render_mode.clone()))?;
    let layout = resolve_render_layout(world, &mode.layout, &render_bindings)?;
    validate_context_state(world, active_dataset, &render_bindings, &layout)?;
    if stack_action == StackAction::Drill {
        let mut focus_owners = Vec::new();
        layout.focus_owners(&mut focus_owners);
        let has_deeper = focus_owners
            .iter()
            .position(|entity| *entity == active_dataset)
            .is_some_and(|position| position + 1 < focus_owners.len());
        if !has_deeper {
            return Err(KernelError::NoDeeperFocusableDataset(active_dataset));
        }
    }

    match stack_action {
        StackAction::Keep => {}
        StackAction::Push | StackAction::Drill => {
            world
                .resource_mut::<ContextStack>()
                .0
                .push(pitui_data::UiContextSnapshot {
                    active_dataset: current.active_dataset,
                    render_mode: current.render_mode,
                    render_bindings: current.render_bindings,
                })
        }
        StackAction::Pop => {
            world.resource_mut::<ContextStack>().0.pop();
        }
    }

    let resolved_operations = world
        .get_resource::<ResolvedOperationSet>()
        .map(|operations| operations.id.clone())
        .unwrap_or_else(|| current.resolved_operations.clone());
    world.insert_resource(ActiveUiContext {
        active_dataset,
        render_mode: render_mode.clone(),
        render_bindings,
        resolved_operations,
        generation: current.generation.wrapping_add(1),
    });
    world.insert_resource(ActiveRenderMode {
        id: render_mode,
        layout,
    });
    world
        .resource_mut::<RenderReconcileDiagnostics>()
        .last_transition_error = None;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StackAction {
    Keep,
    Push,
    Drill,
    Pop,
}

pub(super) fn push_overlay_context(
    world: &mut World,
    active_dataset: Entity,
    render_mode: pitui_data::RenderModeId,
    proxy: pitui_data::RenderProxyId,
    constraint: pitui_data::LayoutConstraint,
) -> Result<(), KernelError> {
    let current = world
        .get_resource::<ActiveUiContext>()
        .cloned()
        .ok_or(KernelError::ContextUnavailable)?;
    let current_mode = world
        .get_resource::<ActiveRenderMode>()
        .cloned()
        .ok_or(KernelError::ContextUnavailable)?;
    let mut bindings = current.render_bindings.clone();
    bindings.bind(RenderBindingId::Overlay, active_dataset);
    let overlay = resolve_render_layout(
        world,
        &RenderLayout::Dataset {
            dataset: DatasetBinding::Context(RenderBindingId::Overlay),
            proxy,
            constraint,
            focusable: true,
        },
        &bindings,
    )?;
    let layout = ResolvedRenderLayout::Overlay(vec![current_mode.layout, overlay]);
    validate_context_state(world, active_dataset, &bindings, &layout)?;

    world
        .resource_mut::<ContextStack>()
        .0
        .push(pitui_data::UiContextSnapshot {
            active_dataset: current.active_dataset,
            render_mode: current.render_mode,
            render_bindings: current.render_bindings,
        });
    world.insert_resource(ActiveUiContext {
        active_dataset,
        render_mode: render_mode.clone(),
        render_bindings: bindings,
        resolved_operations: current.resolved_operations,
        generation: current.generation.wrapping_add(1),
    });
    world.insert_resource(ActiveRenderMode {
        id: render_mode,
        layout,
    });
    world
        .resource_mut::<RenderReconcileDiagnostics>()
        .last_transition_error = None;
    Ok(())
}

fn validate_context_state(
    world: &World,
    active_dataset: Entity,
    render_bindings: &pitui_data::RenderContextBindings,
    layout: &pitui_data::ResolvedRenderLayout,
) -> Result<(), KernelError> {
    if world.get::<Dataset>(active_dataset).is_none() {
        return Err(KernelError::MissingDataset(active_dataset));
    }
    if !layout.is_focus_owner(active_dataset) {
        return Err(KernelError::ActiveDatasetNotFocusable(active_dataset));
    }
    for entity in render_bindings.entities() {
        if world.get::<Dataset>(entity).is_none() {
            return Err(KernelError::MissingDataset(entity));
        }
    }
    let mut rendered = Vec::new();
    layout.dataset_entities(&mut rendered);
    for entity in rendered {
        if world.get::<Dataset>(entity).is_none() {
            return Err(KernelError::MissingDataset(entity));
        }
    }
    Ok(())
}

pub(super) fn update_dependent_render_bindings(world: &mut World) {
    let Some(context) = world.get_resource::<ActiveUiContext>().cloned() else {
        return;
    };
    let mut bindings = context.render_bindings.clone();

    reconcile_repository_branch(world, &mut bindings);
    reconcile_commit_files(world, &mut bindings);
    reconcile_file_changes(world, &mut bindings);
    reconcile_changes_file(world, &mut bindings);
    reconcile_reflog(world, &mut bindings);
    reconcile_git_operation_log(world, &mut bindings);

    if bindings != context.render_bindings {
        let mut context = world.resource_mut::<ActiveUiContext>();
        context.render_bindings = bindings;
        context.generation = context.generation.wrapping_add(1);
    }
}

fn reconcile_repository_branch(world: &World, bindings: &mut pitui_data::RenderContextBindings) {
    let root = bindings
        .get(&RenderBindingId::RepositoriesBranches)
        .or_else(|| {
            world
                .resource::<DatasetIndex>()
                .get(&DatasetIdentity::GlobalRepositoriesBranches)
        });
    let Some(row) = root
        .and_then(|root| world.get::<DatasetCursor>(root))
        .and_then(|cursor| cursor.0)
    else {
        return;
    };

    let (repository, branch) = match world.get::<DatasetKey>(row).map(|key| &key.0) {
        Some(DatasetIdentity::Repository(repository)) => {
            let branch = world
                .get::<RepositoryMetadata>(row)
                .and_then(|metadata| metadata.0.current_branch.clone());
            (Some((repository.clone(), row)), branch)
        }
        Some(DatasetIdentity::Branch { repository, name }) => {
            let repository_entity = world
                .resource::<DatasetIndex>()
                .get(&DatasetIdentity::Repository(repository.clone()));
            (
                repository_entity.map(|entity| (repository.clone(), entity)),
                Some(name.clone()),
            )
        }
        _ => return,
    };

    let Some((repository_key, repository_entity)) = repository else {
        clear_from_commits(bindings);
        return;
    };
    bindings.bind(RenderBindingId::CurrentRepository, repository_entity);
    let commits = branch.and_then(|branch| {
        world
            .resource::<DatasetIndex>()
            .get(&DatasetIdentity::Commits {
                repository: repository_key,
                branch,
            })
    });
    set_or_clear(bindings, RenderBindingId::CurrentCommits, commits);
    if commits.is_none() {
        clear_from_commit(bindings);
    }
}

fn reconcile_commit_files(world: &World, bindings: &mut pitui_data::RenderContextBindings) {
    let Some(commits) = bindings.get(&RenderBindingId::CurrentCommits) else {
        return;
    };
    let commit = world
        .get::<DatasetCursor>(commits)
        .and_then(|cursor| cursor.0);
    set_or_clear(bindings, RenderBindingId::CurrentCommit, commit);

    let files = commit.and_then(|commit| {
        world
            .get::<DatasetChildren>(commit)
            .and_then(|children| children.0.first().copied())
    });
    set_or_clear(bindings, RenderBindingId::CurrentFiles, files);
    if files.is_none() {
        bindings.unbind(&RenderBindingId::CurrentFileChanges);
    }
}

fn reconcile_file_changes(world: &World, bindings: &mut pitui_data::RenderContextBindings) {
    let Some(files) = bindings.get(&RenderBindingId::CurrentFiles) else {
        return;
    };
    let file_changes = world
        .get::<DatasetCursor>(files)
        .and_then(|cursor| cursor.0)
        .and_then(|file| {
            world
                .get::<DatasetChildren>(file)
                .and_then(|children| children.0.first().copied())
        });
    set_or_clear(bindings, RenderBindingId::CurrentFileChanges, file_changes);
}

fn reconcile_changes_file(world: &World, bindings: &mut pitui_data::RenderContextBindings) {
    let changes = bindings.get(&RenderBindingId::Changes).or_else(|| {
        world
            .resource::<DatasetIndex>()
            .get(&DatasetIdentity::GlobalChanges)
    });
    let file_changes = changes
        .and_then(|changes| world.get::<DatasetCursor>(changes))
        .and_then(|cursor| cursor.0)
        .filter(|row| {
            matches!(
                world.get::<DatasetKey>(*row).map(|key| &key.0),
                Some(DatasetIdentity::WorkingTreeFile { .. })
            )
        })
        .and_then(|file| {
            world
                .get::<DatasetChildren>(file)
                .and_then(|children| children.0.first().copied())
        });
    set_or_clear(
        bindings,
        RenderBindingId::CurrentChangesFileChanges,
        file_changes,
    );
}

fn reconcile_git_operation_log(world: &World, bindings: &mut pitui_data::RenderContextBindings) {
    let Some(log) = bindings.get(&RenderBindingId::GitOperationLog) else {
        return;
    };
    let entry = world.get::<DatasetCursor>(log).and_then(|cursor| cursor.0);
    set_or_clear(
        bindings,
        RenderBindingId::CurrentGitOperationLogEntry,
        entry,
    );
}

fn reconcile_reflog(world: &World, bindings: &mut pitui_data::RenderContextBindings) {
    let Some(reflog) = bindings.get(&RenderBindingId::CurrentReflog) else {
        return;
    };
    let entry = world
        .get::<DatasetCursor>(reflog)
        .and_then(|cursor| cursor.0);
    set_or_clear(bindings, RenderBindingId::CurrentReflogEntry, entry);
}

fn set_or_clear(
    bindings: &mut pitui_data::RenderContextBindings,
    id: RenderBindingId,
    entity: Option<Entity>,
) {
    if let Some(entity) = entity {
        bindings.bind(id, entity);
    } else {
        bindings.unbind(&id);
    }
}

fn clear_from_commits(bindings: &mut pitui_data::RenderContextBindings) {
    bindings.unbind(&RenderBindingId::CurrentCommits);
    clear_from_commit(bindings);
}

fn clear_from_commit(bindings: &mut pitui_data::RenderContextBindings) {
    bindings.unbind(&RenderBindingId::CurrentCommit);
    bindings.unbind(&RenderBindingId::CurrentFiles);
    bindings.unbind(&RenderBindingId::CurrentFileChanges);
}

pub(super) fn resolve_active_render_mode(world: &mut World) {
    let Some(context) = world.get_resource::<ActiveUiContext>().cloned() else {
        return;
    };
    let mode = world
        .resource::<RenderModeRegistry>()
        .get(&context.render_mode)
        .cloned();
    let Some(mode) = mode else {
        // Manual/bootstrap layouts remain supported for focused kernel tests.
        return;
    };
    let result =
        resolve_render_layout(world, &mode.layout, &context.render_bindings).and_then(|layout| {
            if layout.is_focus_owner(context.active_dataset) {
                Ok(layout)
            } else {
                Err(KernelError::ActiveDatasetNotFocusable(
                    context.active_dataset,
                ))
            }
        });
    match result {
        Ok(layout) => {
            let next = ActiveRenderMode {
                id: mode.id,
                layout,
            };
            if world.get_resource::<ActiveRenderMode>() != Some(&next) {
                world.insert_resource(next);
            }
            world
                .resource_mut::<RenderReconcileDiagnostics>()
                .last_render_error = None;
        }
        Err(error) => {
            world
                .resource_mut::<RenderReconcileDiagnostics>()
                .last_render_error = Some(error);
        }
    }
}
