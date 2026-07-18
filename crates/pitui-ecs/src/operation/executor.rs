//! Input-to-Operation executor. It owns shortcut/chord lookup and invocation
//! context construction, but contains no operation implementation functions.

use super::*;

#[derive(Resource, Clone, Debug, Default)]
pub struct DeferredStableOperationInvocations(pub(crate) Vec<StableOperationInvocation>);

pub fn stabilize_operation_invocation(
    invocation: OperationInvocation,
    keys: &Query<&DatasetKey>,
) -> Result<StableOperationInvocation, String> {
    let source_dataset = keys
        .get(invocation.source_dataset)
        .map_err(|_| "operation source has no stable Dataset identity")?
        .0
        .clone();
    let targets = invocation
        .targets
        .iter()
        .map(|target| {
            keys.get(*target)
                .map(|key| key.0.clone())
                .map_err(|_| "operation target has no stable Dataset identity")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StableOperationInvocation {
        operation: invocation.operation,
        command: invocation.command,
        source_dataset,
        targets,
        source: invocation.source,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn execute_operation_inputs(
    mut intents: MessageReader<InputIntent>,
    operations: Option<Res<ResolvedOperationSet>>,
    context: Option<Res<ActiveUiContext>>,
    dataset_states: Query<(&DatasetActiveElement, &DatasetSelection)>,
    interactions: Query<&InteractionContextMetadata>,
    commit_creations: Query<(), With<CommitCreationMetadata>>,
    mut chord: ResMut<PendingChordState>,
    mut invocations: MessageWriter<OperationInvocation>,
    mut notices: MessageWriter<OperationNotice>,
    mut edits: MessageWriter<TextEditIntent>,
) {
    let Some(operations) = operations else {
        return;
    };
    let Some(context) = context else {
        return;
    };
    // `ResolvedOperationSet` is the query cache built for exactly one Active
    // Dataset. A stale cache never falls through to another Dataset's System.
    if operations.active_dataset != Some(context.active_dataset) {
        return;
    }

    for intent in intents.read() {
        match intent {
            InputIntent::CancelChord => chord.prefix.clear(),
            InputIntent::Key(stroke) => {
                let Some(binding) = operations.key_bindings.get(stroke) else {
                    chord.prefix.clear();
                    capture_text_edit_key(
                        context.active_dataset,
                        stroke,
                        &interactions,
                        &commit_creations,
                        &mut edits,
                    );
                    continue;
                };
                match &binding.action {
                    ResolvedKeyAction::EnterChord(prefix) => chord.prefix.clone_from(prefix),
                    ResolvedKeyAction::Invoke(operation_id) => {
                        if let Some(operation) = operations
                            .operations
                            .iter()
                            .find(|operation| &operation.id == operation_id)
                        {
                            emit_invocation(
                                operation,
                                InvocationSource::KeyBinding,
                                &context,
                                &dataset_states,
                                &mut invocations,
                                &mut notices,
                            );
                        }
                        chord.prefix.clear();
                    }
                }
            }
            InputIntent::Paste(text) => {
                chord.prefix.clear();
                if dataset_accepts_text(context.active_dataset, &interactions, &commit_creations) {
                    edits.write(TextEditIntent {
                        dataset: context.active_dataset,
                        edit: TextEdit::Insert(text.clone()),
                    });
                }
            }
            InputIntent::CommandLine(line) => {
                chord.prefix.clear();
                let tokens = line.split_whitespace().collect::<Vec<_>>();
                let Some(name) = tokens.first() else {
                    continue;
                };
                if tokens.len() > 1 {
                    notices.write(OperationNotice::CommandArgumentsUnsupported((*name).into()));
                    continue;
                }
                let normalized = name.to_ascii_lowercase();
                let Some(operation_id) = operations.commands.get(&normalized) else {
                    notices.write(OperationNotice::UnknownCommand((*name).into()));
                    continue;
                };
                if let Some(operation) = operations
                    .operations
                    .iter()
                    .find(|operation| &operation.id == operation_id)
                {
                    emit_invocation(
                        operation,
                        InvocationSource::CommandPalette,
                        &context,
                        &dataset_states,
                        &mut invocations,
                        &mut notices,
                    );
                }
            }
        }
    }
}

fn capture_text_edit_key(
    dataset: Entity,
    stroke: &pitui_data::KeyStroke,
    interactions: &Query<&InteractionContextMetadata>,
    commit_creations: &Query<(), With<CommitCreationMetadata>>,
    edits: &mut MessageWriter<TextEditIntent>,
) {
    if !dataset_accepts_text(dataset, interactions, commit_creations)
        || stroke.modifiers.control
        || stroke.modifiers.alt
        || stroke.modifiers.super_key
    {
        return;
    }
    let edit = match stroke.code {
        KeyCode::Character(mut character) => {
            if stroke.modifiers.shift && character.is_ascii_alphabetic() {
                character = character.to_ascii_uppercase();
            }
            TextEdit::Insert(character.to_string())
        }
        KeyCode::Space => TextEdit::Insert(" ".into()),
        KeyCode::Backspace => TextEdit::Backspace,
        _ => return,
    };
    edits.write(TextEditIntent { dataset, edit });
}

fn dataset_accepts_text(
    dataset: Entity,
    interactions: &Query<&InteractionContextMetadata>,
    commit_creations: &Query<(), With<CommitCreationMetadata>>,
) -> bool {
    interactions
        .get(dataset)
        .is_ok_and(|metadata| interaction_accepts_text(&metadata.kind))
        || commit_creations.get(dataset).is_ok()
}

fn interaction_accepts_text(kind: &InteractionContextKind) -> bool {
    matches!(
        kind,
        InteractionContextKind::CommandPalette { .. } | InteractionContextKind::TextInput { .. }
    )
}

fn emit_invocation(
    operation: &ResolvedOperation,
    source: InvocationSource,
    context: &ActiveUiContext,
    dataset_states: &Query<(&DatasetActiveElement, &DatasetSelection)>,
    invocations: &mut MessageWriter<OperationInvocation>,
    notices: &mut MessageWriter<OperationNotice>,
) {
    let Some(targets) = resolve_operation_targets(operation, context, dataset_states) else {
        notices.write(OperationNotice::TargetUnavailable(operation.id.clone()));
        return;
    };
    invocations.write(OperationInvocation {
        operation: operation.id.clone(),
        command: operation.command.clone(),
        source_dataset: context.active_dataset,
        targets,
        source,
    });
}

pub fn resolve_operation_targets(
    operation: &ResolvedOperation,
    context: &ActiveUiContext,
    dataset_states: &Query<(&DatasetActiveElement, &DatasetSelection)>,
) -> Option<Vec<Entity>> {
    let target_dataset = match &operation.target_source {
        TargetSource::ContextActiveElement(binding)
        | TargetSource::ContextSelectionOrActiveElement(binding) => {
            context.render_bindings.get(binding)
        }
        _ => Some(context.active_dataset),
    };
    let targets = match (&operation.target_source, target_dataset) {
        (TargetSource::None, _) => Vec::new(),
        (TargetSource::ActiveDataset, _) => vec![context.active_dataset],
        (TargetSource::ActiveElement | TargetSource::ContextActiveElement(_), Some(dataset)) => {
            dataset_states
                .get(dataset)
                .ok()
                .and_then(|(active, _)| active.0)
                .into_iter()
                .collect()
        }
        (TargetSource::Selection, Some(dataset)) => dataset_states
            .get(dataset)
            .map(|(_, selection)| selection.0.clone())
            .unwrap_or_default(),
        (
            TargetSource::SelectionOrActiveElement
            | TargetSource::ContextSelectionOrActiveElement(_),
            Some(dataset),
        ) => dataset_states
            .get(dataset)
            .ok()
            .filter(|(_, selection)| !selection.0.is_empty())
            .map(|(_, selection)| selection.0.clone())
            .unwrap_or_else(|| {
                dataset_states
                    .get(dataset)
                    .ok()
                    .and_then(|(active, _)| active.0)
                    .into_iter()
                    .collect()
            }),
        (_, None) => Vec::new(),
    };

    if !matches!(operation.target_source, TargetSource::None) && targets.is_empty() {
        None
    } else {
        Some(targets)
    }
}

pub fn collect_operation_invocations(
    mut invocations: MessageReader<OperationInvocation>,
    mut pending: ResMut<PendingOperationInvocations>,
) {
    pending.0.extend(invocations.read().cloned());
}

pub fn release_deferred_invocations(
    context: Option<Res<ActiveUiContext>>,
    kinds: Query<&DatasetType>,
    index: Res<DatasetIndex>,
    mut deferred: ResMut<DeferredStableOperationInvocations>,
    mut invocations: MessageWriter<OperationInvocation>,
    mut notices: MessageWriter<OperationNotice>,
) {
    let Some(context) = context else {
        return;
    };
    if kinds
        .get(context.active_dataset)
        .is_ok_and(|kind| kind.0 == pitui_data::DatasetKind::InteractionContext)
    {
        return;
    }
    for stable in std::mem::take(&mut deferred.0) {
        let Some(source_dataset) = index.get(&stable.source_dataset) else {
            notices.write(OperationNotice::TargetUnavailable(stable.operation));
            continue;
        };
        let targets = stable
            .targets
            .iter()
            .map(|target| index.get(target))
            .collect::<Option<Vec<_>>>();
        let Some(targets) = targets else {
            notices.write(OperationNotice::TargetUnavailable(stable.operation));
            continue;
        };
        invocations.write(OperationInvocation {
            operation: stable.operation,
            command: stable.command,
            source_dataset,
            targets,
            source: stable.source,
        });
    }
}

pub fn apply_text_edits(
    mut edits: MessageReader<TextEditIntent>,
    mut contexts: Query<&mut InteractionContextMetadata>,
    mut commit_creations: Query<&mut CommitCreationMetadata>,
) {
    for intent in edits.read() {
        if let Ok(mut metadata) = contexts.get_mut(intent.dataset) {
            match &mut metadata.kind {
                InteractionContextKind::CommandPalette {
                    query, selected, ..
                } => {
                    apply_text_edit(query, &intent.edit, 256);
                    *selected = 0;
                }
                InteractionContextKind::TextInput { input, error, .. } => {
                    apply_text_edit(input, &intent.edit, 4096);
                    *error = None;
                }
                _ => {}
            }
        } else if let Ok(mut metadata) = commit_creations.get_mut(intent.dataset) {
            apply_text_edit(&mut metadata.message, &intent.edit, 4096);
            metadata.error = None;
        }
    }
}

fn apply_text_edit(value: &mut String, edit: &TextEdit, max_chars: usize) {
    match edit {
        TextEdit::Insert(inserted) => {
            let remaining = max_chars.saturating_sub(value.chars().count());
            value.extend(
                inserted
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(remaining),
            );
        }
        TextEdit::Backspace => {
            value.pop();
        }
    }
}
