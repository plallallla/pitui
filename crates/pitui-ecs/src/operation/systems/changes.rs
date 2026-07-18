use super::*;

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
    mut pending_relay: ResMut<PendingChangesActiveRelays>,
    mut git: ResMut<GitRequestQueue>,
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
    mut pending_relay: ResMut<PendingChangesActiveRelays>,
    mut git: ResMut<GitRequestQueue>,
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
    pending_relay: &mut PendingChangesActiveRelays,
    git: &mut GitRequestQueue,
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
    let cwd = repository_metadata.0.root.clone();
    let request_id = git.enqueue_with_refresh_tracked(
        GitCommandData {
            repository_dataset: repository_entity,
            cwd,
            command: mutation.clone(),
        },
        GitRefreshPlan::new([GitRefreshTarget::Repository, GitRefreshTarget::WorkingTree]),
    );
    pending_relay.0.push_back(ChangesActiveRelayRequest {
        mutation: mutation.clone(),
        request_id,
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
    let requests = std::mem::take(&mut world.resource_mut::<PendingChangesActiveRelays>().0);
    if requests.is_empty() {
        return;
    }
    let mut waiting = VecDeque::new();
    for request in requests {
        let outcome = world
            .resource::<GitRequestOutcomes>()
            .get(request.request_id)
            .cloned();
        match outcome {
            None => waiting.push_back(request),
            Some(GitRequestOutcome::Failed) => {
                world
                    .resource_mut::<GitRequestOutcomes>()
                    .acknowledge(request.request_id);
            }
            Some(GitRequestOutcome::MutationApplied { command }) if command == request.mutation => {
                if apply_pending_changes_active(world, &request) {
                    world
                        .resource_mut::<GitRequestOutcomes>()
                        .acknowledge(request.request_id);
                } else {
                    waiting.push_back(request);
                }
            }
            Some(GitRequestOutcome::MutationApplied { .. }) => {
                world
                    .resource_mut::<GitRequestOutcomes>()
                    .acknowledge(request.request_id);
            }
        }
    }
    world.resource_mut::<PendingChangesActiveRelays>().0 = waiting;
}

/// Returns `false` only while a successful mutation is still waiting for its
/// repository-scoped Working Tree refresh to advance the Changes revision.
fn apply_pending_changes_active(world: &mut World, request: &ChangesActiveRelayRequest) -> bool {
    let changes = match request.context {
        ChangesActiveContext::Active => {
            world.get_resource::<ActiveUiContext>().and_then(|context| {
                context
                    .render_bindings
                    .get(&pitui_data::RenderBindingId::Changes)
            })
        }
        ChangesActiveContext::Previous => world
            .resource::<ContextStack>()
            .top_overlay_snapshot()
            .and_then(|snapshot| {
                snapshot
                    .render_bindings
                    .get(&pitui_data::RenderBindingId::Changes)
            }),
    };
    let Some(changes) = changes else {
        return true;
    };
    if world
        .get::<DatasetRevision>(changes)
        .is_none_or(|revision| revision.0 <= request.changes_revision)
    {
        return false;
    }
    // A successful mutation replaces the selected boundary snapshot. Clear the
    // old selection before relaying Active; otherwise the persistent empty
    // Staged/Unstaged group would remain selected and create mixed-boundary
    // targets on the next operation.
    if let Some(mut selection) = world.get_mut::<DatasetSelection>(changes) {
        selection.0.clear();
    }
    let Some(collection) = world.get::<DatasetCollection>(changes) else {
        return true;
    };
    let may_be_empty = matches!(&request.target, ChangesActiveTarget::FirstRemainingFile);
    let file = match &request.target {
        ChangesActiveTarget::Exact {
            repository,
            path,
            boundary,
        } => world
            .resource::<DatasetIndex>()
            .get(&DatasetIdentity::WorkingTreeFile {
                repository: repository.clone(),
                boundary: *boundary,
                path: path.clone(),
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
                    if let Some(snapshot) = stack.top_overlay_snapshot_mut() {
                        snapshot
                            .render_bindings
                            .unbind(&pitui_data::RenderBindingId::CurrentChangesFileChanges);
                        snapshot.active_dataset = changes;
                    }
                }
            }
        }
        return true;
    };
    if let Some(mut active) = world.get_mut::<DatasetActiveElement>(changes) {
        active.0 = Some(file);
    }

    let Some(diff) = world
        .get::<pitui_data::DatasetChildren>(file)
        .and_then(|children| children.0.first().copied())
    else {
        return true;
    };
    match request.context {
        ChangesActiveContext::Active => {
            let Some(mut context) = world.get_resource_mut::<ActiveUiContext>() else {
                return true;
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
            let Some(snapshot) = stack.top_overlay_snapshot_mut() else {
                return true;
            };
            snapshot
                .render_bindings
                .bind(pitui_data::RenderBindingId::CurrentChangesFileChanges, diff);
            if request.preserve_diff_active {
                snapshot.active_dataset = diff;
            }
        }
    }
    true
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

#[allow(clippy::too_many_arguments)]
pub fn submit_commit_creation(
    In(invocation): In<OperationInvocation>,
    stack: Res<ContextStack>,
    keys: Query<&DatasetKey>,
    revisions: Query<&DatasetRevision>,
    repositories: Query<&RepositoryMetadata>,
    mut creations: Query<&mut CommitCreationMetadata>,
    mut pending_relay: ResMut<PendingChangesActiveRelays>,
    mut git: ResMut<GitRequestQueue>,
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

    let Some(previous) = stack.top_overlay_snapshot() else {
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
    let mut refresh = vec![
        GitRefreshTarget::Repository,
        GitRefreshTarget::Branches,
        GitRefreshTarget::WorkingTree,
    ];
    if let Some(branch) = current_branch {
        refresh.push(GitRefreshTarget::Commits { branch, limit: 500 });
    }
    let request_id = git.enqueue_with_refresh_tracked(
        GitCommandData {
            repository_dataset: repository_entity,
            cwd,
            command: mutation.clone(),
        },
        GitRefreshPlan::new(refresh),
    );
    pending_relay.0.push_back(ChangesActiveRelayRequest {
        mutation: mutation.clone(),
        request_id,
        changes_revision: changes_revision.0,
        target: ChangesActiveTarget::FirstRemainingFile,
        context: ChangesActiveContext::Previous,
        preserve_diff_active: previous
            .render_bindings
            .get(&pitui_data::RenderBindingId::CurrentChangesFileChanges)
            == Some(previous.active_dataset),
    });
    transitions.write(ContextTransitionRequest::Pop);
    OperationExecution::Completed
}
