use super::*;

pub fn request_hard_reset(
    In(invocation): In<OperationInvocation>,
    index: Res<DatasetIndex>,
    keys: Query<&DatasetKey>,
    reflog_entries: Query<&ReflogEntryMetadata>,
    mut contexts: Query<&mut InteractionContextMetadata>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> OperationExecution {
    let Ok((_, target)) = reset_target(&invocation, &keys, &reflog_entries) else {
        return OperationExecution::Rejected(
            "hard reset target must be one Commit or Reflog entry".into(),
        );
    };
    let Some(interaction) = index.get(&DatasetIdentity::GlobalInteractionContext) else {
        return OperationExecution::Rejected("global Interaction Context is unavailable".into());
    };
    let Ok(mut metadata) = contexts.get_mut(interaction) else {
        return OperationExecution::Rejected("Interaction Context has no metadata".into());
    };
    let mut confirmed = invocation;
    confirmed.operation = OperationId::from("reset.hard.confirmed");
    confirmed.command = CommandId::from("reset.hard.confirmed");
    let confirmed = match stabilize_operation_invocation(confirmed, &keys) {
        Ok(invocation) => invocation,
        Err(error) => return OperationExecution::Rejected(error),
    };
    metadata.kind = InteractionContextKind::Confirmation {
        title: "Confirm hard reset".into(),
        prompt: format!(
            "Reset HEAD to {} and permanently discard tracked index/worktree changes?",
            target.short()
        ),
        options: vec!["Cancel".into(), "Reset --hard".into()],
        selected: 0,
        pending: Box::new(confirmed),
    };
    request_interaction_overlay(interaction, &mut transitions);
    OperationExecution::Completed
}

#[allow(clippy::too_many_arguments)]
pub fn reset_soft(
    In(invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    reflog_entries: Query<&ReflogEntryMetadata>,
    repositories: Query<&RepositoryMetadata>,
    mut git: ResMut<GitRequestQueue>,
) -> OperationExecution {
    reset_to_target(
        invocation,
        ResetMode::Soft,
        &context,
        &keys,
        &reflog_entries,
        &repositories,
        &mut git,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn reset_mixed(
    In(invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    reflog_entries: Query<&ReflogEntryMetadata>,
    repositories: Query<&RepositoryMetadata>,
    mut git: ResMut<GitRequestQueue>,
) -> OperationExecution {
    reset_to_target(
        invocation,
        ResetMode::Mixed,
        &context,
        &keys,
        &reflog_entries,
        &repositories,
        &mut git,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn reset_hard_confirmed(
    In(invocation): In<OperationInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    reflog_entries: Query<&ReflogEntryMetadata>,
    repositories: Query<&RepositoryMetadata>,
    mut git: ResMut<GitRequestQueue>,
) -> OperationExecution {
    reset_to_target(
        invocation,
        ResetMode::Hard,
        &context,
        &keys,
        &reflog_entries,
        &repositories,
        &mut git,
    )
}

fn reset_target(
    invocation: &OperationInvocation,
    keys: &Query<&DatasetKey>,
    reflog_entries: &Query<&ReflogEntryMetadata>,
) -> Result<(pitui_data::RepositoryKey, CommitHash), String> {
    let [target] = invocation.targets.as_slice() else {
        return Err("reset requires exactly one target".into());
    };
    match keys.get(*target).map(|key| &key.0) {
        Ok(DatasetIdentity::Commit { repository, hash }) => Ok((repository.clone(), hash.clone())),
        Ok(DatasetIdentity::ReflogEntry { repository, .. }) => {
            let metadata = reflog_entries
                .get(*target)
                .map_err(|_| "Reflog target metadata is unavailable".to_owned())?;
            Ok((repository.clone(), metadata.0.hash.clone()))
        }
        _ => Err("reset target is not a Commit or Reflog entry Dataset".into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn reset_to_target(
    invocation: OperationInvocation,
    mode: ResetMode,
    context: &ActiveUiContext,
    keys: &Query<&DatasetKey>,
    reflog_entries: &Query<&ReflogEntryMetadata>,
    repositories: &Query<&RepositoryMetadata>,
    git: &mut GitRequestQueue,
) -> OperationExecution {
    let (target_repository, target) = match reset_target(&invocation, keys, reflog_entries) {
        Ok(target) => target,
        Err(error) => return OperationExecution::Rejected(error),
    };
    let Some(repository_entity) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return OperationExecution::Rejected("current Repository binding is unavailable".into());
    };
    if !matches!(
        keys.get(repository_entity).map(|key| &key.0),
        Ok(DatasetIdentity::Repository(repository)) if repository == &target_repository
    ) {
        return OperationExecution::Rejected(
            "reset target does not belong to the current Repository".into(),
        );
    }
    let Ok(repository) = repositories.get(repository_entity) else {
        return OperationExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let source_branch = match keys.get(invocation.source_dataset).map(|key| &key.0) {
        Ok(DatasetIdentity::Commits { repository, branch }) if repository == &target_repository => {
            Some(branch.clone())
        }
        _ => None,
    };
    let current_branch = repository.0.current_branch.clone();
    let cwd = repository.0.root.clone();
    let mut refresh = vec![
        GitRefreshTarget::Repository,
        GitRefreshTarget::Branches,
        GitRefreshTarget::WorkingTree,
    ];
    if let Some(current_branch) = &current_branch {
        refresh.push(GitRefreshTarget::Commits {
            branch: current_branch.clone(),
            limit: 500,
        });
    }
    if let Some(source_branch) = source_branch
        && current_branch.as_ref() != Some(&source_branch)
    {
        refresh.push(GitRefreshTarget::Commits {
            branch: source_branch,
            limit: 500,
        });
    }
    if context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentReflog)
        .is_some()
    {
        refresh.push(GitRefreshTarget::Reflog { limit: 500 });
    }
    git.enqueue_with_refresh(
        GitCommandData {
            repository_dataset: repository_entity,
            cwd,
            command: GitCommand::Reset { target, mode },
        },
        GitRefreshPlan::new(refresh),
    );
    OperationExecution::Completed
}
