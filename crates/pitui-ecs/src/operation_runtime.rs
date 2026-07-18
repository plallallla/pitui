use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
};

use bevy_ecs::{
    prelude::{
        Entity, In, IntoSystem, MessageReader, MessageWriter, Messages, Query, Res, ResMut,
        Resource, With, World,
    },
    system::SystemId,
};
use pitui_data::{
    ActiveRenderMode, ActiveUiContext, AvailabilityRule, AvailabilityRuleId,
    AvailabilityRuleRegistry, ChangeBoundary, ClipboardContentKind, ClipboardRequest, CommandId,
    CommandInvocation, CommandRegistry, CommandSystemId, CommitCreationMetadata, CommitMetadata,
    ContextStack, ContextTransitionRequest, DatasetCursor, DatasetIdentity, DatasetIndex,
    DatasetKey, DatasetNavigationOrder, DatasetRevision, DatasetSelection, DatasetTemplateRef,
    DatasetTemplateRegistry, DatasetType, DatasetViewport, DefaultDatasetTemplates,
    GlobalOperationSet, InputIntent, InteractionContextKind, InteractionContextMetadata,
    InteractionNoticeRequest, InvocationSource, KeyCode, KeySequence, LayoutConstraint,
    NavigationModeRegistry, OperationId, OperationNotice, OperationRegistry, OperationSpec,
    PaletteCommandEntry, PendingChordState, QuitRequested, ReflogEntryMetadata, RenderBindingPatch,
    RenderModeId, RenderProxyId, RepositoryMetadata, ResolvedKeyAction, ResolvedKeyBinding,
    ResolvedOperation, ResolvedOperationSet, ResolvedOperationSetId, ShortcutHelpEntry,
    TargetSource, TextEdit, TextEditIntent, WorkingTreeFileMetadata,
};
use pitui_git::GitCommand;

use crate::{
    ensure_dataset_in_world,
    git_runtime::{GitCommandData, GitMutationSuccesses},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationResolutionError {
    MissingDatasetTemplate(Entity),
    MissingOperation(OperationId),
    MissingCommand(CommandId),
    MissingAvailabilityRule(AvailabilityRuleId),
    DuplicateCommandName(String),
    DuplicateKeySequence {
        sequence: KeySequence,
        first: OperationId,
        second: OperationId,
    },
    AmbiguousKeyPrefix {
        shorter: KeySequence,
        longer: KeySequence,
    },
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationResolutionDiagnostics {
    pub last_error: Option<OperationResolutionError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandExecution {
    Completed,
    Rejected(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandSystemRegistrationError {
    DuplicateSystem(CommandSystemId),
}

#[derive(Resource, Default)]
pub struct CommandSystemRegistry(
    HashMap<CommandSystemId, SystemId<In<CommandInvocation>, CommandExecution>>,
);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandExecutionLog(pub Vec<(CommandInvocation, CommandExecution)>);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationNotices(pub Vec<OperationNotice>);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct ClipboardRequests(pub Vec<ClipboardRequest>);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct PendingInteractionNotices(pub VecDeque<InteractionNoticeRequest>);

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct PendingCommandInvocations(Vec<CommandInvocation>);

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct DeferredCommandInvocations(Vec<CommandInvocation>);

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct PendingChangesFocus(Option<ChangesFocusRequest>);

#[derive(Clone, Debug)]
struct ChangesFocusRequest {
    mutation: GitCommand,
    success_index: usize,
    changes_revision: u64,
    target: ChangesFocusTarget,
    context: ChangesFocusContext,
    preserve_diff_focus: bool,
}

#[derive(Clone, Debug)]
enum ChangesFocusTarget {
    Exact {
        repository: pitui_data::RepositoryKey,
        path: pitui_core::GitPath,
        boundary: ChangeBoundary,
    },
    FirstRemainingFile,
}

#[derive(Clone, Copy, Debug)]
enum ChangesFocusContext {
    Active,
    Previous,
}

pub(super) fn initialize_operation_runtime(world: &mut World) {
    world.init_resource::<CommandRegistry>();
    world.init_resource::<OperationRegistry>();
    world.init_resource::<AvailabilityRuleRegistry>();
    world.init_resource::<GlobalOperationSet>();
    world.init_resource::<OperationResolutionDiagnostics>();
    world.init_resource::<CommandSystemRegistry>();
    world.init_resource::<CommandExecutionLog>();
    world.init_resource::<OperationNotices>();
    world.init_resource::<ClipboardRequests>();
    world.init_resource::<PendingInteractionNotices>();
    world.init_resource::<PendingCommandInvocations>();
    world.init_resource::<DeferredCommandInvocations>();
    world.init_resource::<PendingChangesFocus>();
    world.init_resource::<QuitRequested>();
    world.init_resource::<Messages<InputIntent>>();
    world.init_resource::<Messages<CommandInvocation>>();
    world.init_resource::<Messages<OperationNotice>>();
    world.init_resource::<Messages<ContextTransitionRequest>>();
    world.init_resource::<Messages<ClipboardRequest>>();
    world.init_resource::<Messages<TextEditIntent>>();
    world.init_resource::<Messages<InteractionNoticeRequest>>();
}

pub(super) fn update_operation_messages(world: &mut World) {
    world.resource_mut::<Messages<InputIntent>>().update();
    world.resource_mut::<Messages<CommandInvocation>>().update();
    world.resource_mut::<Messages<OperationNotice>>().update();
    world
        .resource_mut::<Messages<ContextTransitionRequest>>()
        .update();
    world.resource_mut::<Messages<ClipboardRequest>>().update();
    world.resource_mut::<Messages<TextEditIntent>>().update();
    world
        .resource_mut::<Messages<InteractionNoticeRequest>>()
        .update();
}

pub(super) fn register_command_system<M, S>(
    world: &mut World,
    id: CommandSystemId,
    system: S,
) -> Result<(), CommandSystemRegistrationError>
where
    S: IntoSystem<In<CommandInvocation>, CommandExecution, M> + 'static,
    M: 'static,
{
    if world
        .resource::<CommandSystemRegistry>()
        .0
        .contains_key(&id)
    {
        return Err(CommandSystemRegistrationError::DuplicateSystem(id));
    }
    let system_id = world.register_system(system);
    world
        .resource_mut::<CommandSystemRegistry>()
        .0
        .insert(id, system_id);
    Ok(())
}

pub(super) fn command_system_registered(world: &World, id: &CommandSystemId) -> bool {
    world.resource::<CommandSystemRegistry>().0.contains_key(id)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_input_intents(
    mut intents: MessageReader<InputIntent>,
    operations: Option<Res<ResolvedOperationSet>>,
    context: Option<Res<ActiveUiContext>>,
    navigation: Query<(&DatasetCursor, &DatasetSelection)>,
    interactions: Query<&InteractionContextMetadata>,
    commit_creations: Query<(), With<CommitCreationMetadata>>,
    mut chord: ResMut<PendingChordState>,
    mut invocations: MessageWriter<CommandInvocation>,
    mut notices: MessageWriter<OperationNotice>,
    mut edits: MessageWriter<TextEditIntent>,
) {
    let Some(operations) = operations else {
        return;
    };
    let Some(context) = context else {
        return;
    };

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
                                &navigation,
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
                        &navigation,
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
    navigation: &Query<(&DatasetCursor, &DatasetSelection)>,
    invocations: &mut MessageWriter<CommandInvocation>,
    notices: &mut MessageWriter<OperationNotice>,
) {
    let Some(targets) = resolve_operation_targets(operation, context, navigation) else {
        notices.write(OperationNotice::TargetUnavailable(operation.id.clone()));
        return;
    };
    invocations.write(CommandInvocation {
        command: operation.command.clone(),
        source_dataset: context.active_dataset,
        targets,
        source,
    });
}

fn resolve_operation_targets(
    operation: &ResolvedOperation,
    context: &ActiveUiContext,
    navigation: &Query<(&DatasetCursor, &DatasetSelection)>,
) -> Option<Vec<Entity>> {
    let target_dataset = match &operation.target_source {
        TargetSource::ContextCursor(binding) | TargetSource::ContextSelectionOrCursor(binding) => {
            context.render_bindings.get(binding)
        }
        _ => Some(context.active_dataset),
    };
    let targets = match (&operation.target_source, target_dataset) {
        (TargetSource::None, _) => Vec::new(),
        (TargetSource::ActiveDataset, _) => vec![context.active_dataset],
        (TargetSource::Cursor | TargetSource::ContextCursor(_), Some(dataset)) => navigation
            .get(dataset)
            .ok()
            .and_then(|(cursor, _)| cursor.0)
            .into_iter()
            .collect(),
        (TargetSource::Selection, Some(dataset)) => navigation
            .get(dataset)
            .map(|(_, selection)| selection.0.clone())
            .unwrap_or_default(),
        (
            TargetSource::SelectionOrCursor | TargetSource::ContextSelectionOrCursor(_),
            Some(dataset),
        ) => navigation
            .get(dataset)
            .ok()
            .filter(|(_, selection)| !selection.0.is_empty())
            .map(|(_, selection)| selection.0.clone())
            .unwrap_or_else(|| {
                navigation
                    .get(dataset)
                    .ok()
                    .and_then(|(cursor, _)| cursor.0)
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

pub(super) fn collect_command_invocations(
    mut invocations: MessageReader<CommandInvocation>,
    mut pending: ResMut<PendingCommandInvocations>,
) {
    pending.0.extend(invocations.read().cloned());
}

pub(super) fn release_deferred_invocations(
    context: Option<Res<ActiveUiContext>>,
    kinds: Query<&DatasetType>,
    mut deferred: ResMut<DeferredCommandInvocations>,
    mut invocations: MessageWriter<CommandInvocation>,
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
    for invocation in std::mem::take(&mut deferred.0) {
        invocations.write(invocation);
    }
}

pub(super) fn apply_text_edits(
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

pub(super) fn dispatch_pending_commands(world: &mut World) {
    let invocations = std::mem::take(&mut world.resource_mut::<PendingCommandInvocations>().0);
    for invocation in invocations {
        let command = world
            .resource::<CommandRegistry>()
            .get(&invocation.command)
            .cloned();
        let Some(command) = command else {
            write_notice(
                world,
                OperationNotice::CommandSystemUnavailable(invocation.command.clone()),
            );
            continue;
        };
        let system = world
            .resource::<CommandSystemRegistry>()
            .0
            .get(&command.system)
            .copied();
        let Some(system) = system else {
            write_notice(
                world,
                OperationNotice::CommandSystemUnavailable(invocation.command.clone()),
            );
            continue;
        };
        let execution = match world.run_system_with(system, invocation.clone()) {
            Ok(execution) => execution,
            Err(error) => CommandExecution::Rejected(error.to_string()),
        };
        if let CommandExecution::Rejected(message) = &execution {
            write_notice(
                world,
                OperationNotice::CommandRejected {
                    command: invocation.command.clone(),
                    message: message.clone(),
                },
            );
        }
        world
            .resource_mut::<CommandExecutionLog>()
            .0
            .push((invocation, execution));
    }
}

pub(super) fn collect_operation_notices(
    mut notices: MessageReader<OperationNotice>,
    mut collected: ResMut<OperationNotices>,
) {
    collected.0.extend(notices.read().cloned());
}

pub(super) fn collect_clipboard_requests(
    mut requests: MessageReader<ClipboardRequest>,
    mut collected: ResMut<ClipboardRequests>,
) {
    collected.0.extend(requests.read().cloned());
}

pub(super) fn collect_interaction_notice_requests(
    mut requests: MessageReader<InteractionNoticeRequest>,
    mut pending: ResMut<PendingInteractionNotices>,
) {
    pending.0.extend(requests.read().cloned());
}

/// Presents at most one queued Notice through the global Interaction Context.
/// It runs after ordinary Context transitions so a failed command submitted by
/// a TextInput first restores its previous view and then overlays the error.
pub(super) fn present_next_interaction_notice(world: &mut World) {
    let Some(context) = world.get_resource::<ActiveUiContext>() else {
        return;
    };
    if world
        .get::<DatasetType>(context.active_dataset)
        .is_some_and(|kind| kind.0 == pitui_data::DatasetKind::InteractionContext)
    {
        return;
    }
    let Some(request) = world
        .resource::<PendingInteractionNotices>()
        .0
        .front()
        .cloned()
    else {
        return;
    };
    let Some(interaction) = world
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalInteractionContext)
    else {
        return;
    };
    let Some(previous_kind) = world
        .get::<InteractionContextMetadata>(interaction)
        .map(|metadata| metadata.kind.clone())
    else {
        return;
    };
    world
        .entity_mut(interaction)
        .insert(InteractionContextMetadata {
            kind: InteractionContextKind::Notice {
                title: request.title,
                message: request.message,
            },
        });
    let result = crate::binding_reconcile::push_overlay_context(
        world,
        interaction,
        RenderModeId::from("notice-overlay"),
        RenderProxyId::from("interaction-context.overlay"),
        LayoutConstraint::Percentage(65),
    );
    match result {
        Ok(()) => {
            world
                .resource_mut::<PendingInteractionNotices>()
                .0
                .pop_front();
        }
        Err(error) => {
            world
                .entity_mut(interaction)
                .insert(InteractionContextMetadata {
                    kind: previous_kind,
                });
            world
                .resource_mut::<crate::RenderReconcileDiagnostics>()
                .last_transition_error = Some(error);
        }
    }
}

fn write_notice(world: &mut World, notice: OperationNotice) {
    world
        .resource_mut::<Messages<OperationNotice>>()
        .write(notice);
}

pub(super) fn resolve_active_operation_set(world: &mut World) {
    let Some(context) = world.get_resource::<ActiveUiContext>().cloned() else {
        return;
    };
    let result = build_resolved_operation_set(world, context.active_dataset);
    match result {
        Ok(mut resolved) => {
            let previous = world.get_resource::<ResolvedOperationSet>().cloned();
            if let Some(previous) = &previous {
                resolved.generation = previous.generation;
            }
            if previous.as_ref() != Some(&resolved) {
                resolved.generation = previous
                    .as_ref()
                    .map_or(0, |previous| previous.generation.wrapping_add(1));
                let id = resolved.id.clone();
                world.insert_resource(resolved);
                if let Some(mut context) = world.get_resource_mut::<ActiveUiContext>() {
                    context.resolved_operations = id;
                }
            }
            world
                .resource_mut::<OperationResolutionDiagnostics>()
                .last_error = None;
        }
        Err(error) => {
            world
                .resource_mut::<OperationResolutionDiagnostics>()
                .last_error = Some(error);
        }
    }
}

fn build_resolved_operation_set(
    world: &World,
    active_dataset: Entity,
) -> Result<ResolvedOperationSet, OperationResolutionError> {
    let template_id = world
        .get::<DatasetTemplateRef>(active_dataset)
        .ok_or(OperationResolutionError::MissingDatasetTemplate(
            active_dataset,
        ))?
        .0
        .clone();
    let local = world
        .resource::<DatasetTemplateRegistry>()
        .get(&template_id)
        .ok_or(OperationResolutionError::MissingDatasetTemplate(
            active_dataset,
        ))?
        .operations
        .clone();
    let global = world.resource::<GlobalOperationSet>().0.clone();
    let global_ids = global.iter().cloned().collect::<HashSet<_>>();
    let mut candidates = global;
    candidates.extend(local);

    let operation_registry = world.resource::<OperationRegistry>();
    let command_registry = world.resource::<CommandRegistry>();
    let availability_registry = world.resource::<AvailabilityRuleRegistry>();
    let mut available = Vec::<(OperationSpec, String, bool)>::new();
    let mut names = HashMap::<String, (OperationId, bool)>::new();
    let mut seen_operations = HashSet::new();
    for operation_id in candidates {
        if !seen_operations.insert(operation_id.clone()) {
            continue;
        }
        let operation = operation_registry
            .get(&operation_id)
            .cloned()
            .ok_or_else(|| OperationResolutionError::MissingOperation(operation_id.clone()))?;
        let command = command_registry
            .get(&operation.command)
            .ok_or_else(|| OperationResolutionError::MissingCommand(operation.command.clone()))?;
        let rule = availability_registry
            .get(&operation.availability)
            .ok_or_else(|| {
                OperationResolutionError::MissingAvailabilityRule(operation.availability.clone())
            })?;
        if !availability_matches(world, active_dataset, rule) {
            continue;
        }
        let is_global = global_ids.contains(&operation_id);
        let name = command.name.to_ascii_lowercase();
        if let Some((_, existing_is_global)) = names.get(&name) {
            if *existing_is_global && !is_global {
                continue;
            }
            return Err(OperationResolutionError::DuplicateCommandName(name));
        }
        names.insert(name.clone(), (operation_id, is_global));
        available.push((operation, name, is_global));
    }

    validate_key_sequences(&available)?;
    let prefix = &world.resource::<PendingChordState>().prefix;
    let mut key_bindings = HashMap::<pitui_data::KeyStroke, ResolvedKeyBinding>::new();
    for (operation, _, _) in &available {
        for sequence in &operation.bindings {
            if !sequence.0.starts_with(prefix) || sequence.0.len() <= prefix.len() {
                continue;
            }
            let stroke = sequence.0[prefix.len()].clone();
            let action = if sequence.0.len() == prefix.len() + 1 {
                ResolvedKeyAction::Invoke(operation.id.clone())
            } else {
                let mut next_prefix = prefix.clone();
                next_prefix.push(stroke.clone());
                ResolvedKeyAction::EnterChord(next_prefix)
            };
            let binding = ResolvedKeyBinding {
                stroke: stroke.clone(),
                label: if matches!(action, ResolvedKeyAction::EnterChord(_)) {
                    "More…".into()
                } else {
                    operation.label.clone()
                },
                action,
            };
            match key_bindings.get(&stroke) {
                Some(existing) if existing.action == binding.action => {}
                Some(_) => unreachable!("prefix ambiguity is validated before resolution"),
                None => {
                    key_bindings.insert(stroke, binding);
                }
            }
        }
    }

    let commands = if prefix.is_empty() {
        available
            .iter()
            .map(|(operation, name, _)| (name.clone(), operation.id.clone()))
            .collect()
    } else {
        HashMap::new()
    };
    let operations = available
        .into_iter()
        .map(|(operation, _, _)| ResolvedOperation {
            id: operation.id,
            label: operation.label,
            command: operation.command,
            target_source: operation.target_source,
        })
        .collect();
    Ok(ResolvedOperationSet {
        id: ResolvedOperationSetId::from(format!(
            "active:{active_dataset:?}:chord:{}",
            prefix.len()
        )),
        operations,
        key_bindings,
        commands,
        generation: 0,
    })
}

fn validate_key_sequences(
    operations: &[(OperationSpec, String, bool)],
) -> Result<(), OperationResolutionError> {
    let mut sequences = Vec::<(KeySequence, OperationId)>::new();
    for (operation, _, _) in operations {
        for sequence in &operation.bindings {
            if let Some((_, first)) = sequences.iter().find(|(existing, _)| existing == sequence) {
                return Err(OperationResolutionError::DuplicateKeySequence {
                    sequence: sequence.clone(),
                    first: first.clone(),
                    second: operation.id.clone(),
                });
            }
            sequences.push((sequence.clone(), operation.id.clone()));
        }
    }
    for (index, (left, _)) in sequences.iter().enumerate() {
        for (right, _) in sequences.iter().skip(index + 1) {
            let (shorter, longer) = if left.0.len() <= right.0.len() {
                (left, right)
            } else {
                (right, left)
            };
            if shorter.0.len() < longer.0.len() && longer.0.starts_with(&shorter.0) {
                return Err(OperationResolutionError::AmbiguousKeyPrefix {
                    shorter: shorter.clone(),
                    longer: longer.clone(),
                });
            }
        }
    }
    Ok(())
}

fn availability_matches(world: &World, active: Entity, rule: &AvailabilityRule) -> bool {
    match rule {
        AvailabilityRule::Always => true,
        AvailabilityRule::ActiveDatasetKind(expected) => world
            .get::<DatasetType>(active)
            .is_some_and(|kind| kind.0 == *expected),
        AvailabilityRule::HasCursor => world
            .get::<DatasetCursor>(active)
            .is_some_and(|cursor| cursor.0.is_some()),
        AvailabilityRule::HasSelection => world
            .get::<DatasetSelection>(active)
            .is_some_and(|selection| !selection.0.is_empty()),
        AvailabilityRule::HasSelectionOrCursor => {
            availability_matches(world, active, &AvailabilityRule::HasSelection)
                || availability_matches(world, active, &AvailabilityRule::HasCursor)
        }
        AvailabilityRule::ContextHasCursor(binding) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| world.get::<DatasetCursor>(dataset))
            .is_some_and(|cursor| cursor.0.is_some()),
        AvailabilityRule::ContextHasSelectionOrCursor(binding) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .is_some_and(|dataset| {
                world
                    .get::<DatasetSelection>(dataset)
                    .is_some_and(|selection| !selection.0.is_empty())
                    || world
                        .get::<DatasetCursor>(dataset)
                        .is_some_and(|cursor| cursor.0.is_some())
            }),
        AvailabilityRule::ContextCursorKind(binding, expected) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| world.get::<DatasetCursor>(dataset))
            .and_then(|cursor| cursor.0)
            .and_then(|cursor| world.get::<DatasetType>(cursor))
            .is_some_and(|kind| kind.0 == *expected),
        AvailabilityRule::ContextTargetsBoundary(binding, expected) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| {
                let cursor = world.get::<DatasetCursor>(dataset)?.0;
                let selection = world.get::<DatasetSelection>(dataset)?;
                let targets = if selection.0.is_empty() {
                    cursor.into_iter().collect::<Vec<_>>()
                } else {
                    selection.0.clone()
                };
                (!targets.is_empty()).then_some(targets)
            })
            .is_some_and(|targets| {
                targets.iter().all(|target| {
                    matches!(
                        world.get::<DatasetKey>(*target).map(|key| &key.0),
                        Some(DatasetIdentity::WorkingTreeFile { boundary, .. })
                            if boundary == expected
                    )
                })
            }),
        AvailabilityRule::ChangesHasStagedFiles(binding) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| world.get::<DatasetNavigationOrder>(dataset))
            .is_some_and(|navigation| {
                navigation.0.iter().any(|row| {
                    matches!(
                        world.get::<DatasetKey>(*row).map(|key| &key.0),
                        Some(DatasetIdentity::WorkingTreeFile {
                            boundary: pitui_data::ChangeBoundary::Staged,
                            ..
                        })
                    )
                })
            }),
        AvailabilityRule::InteractionContextType(expected) => world
            .get::<InteractionContextMetadata>(active)
            .is_some_and(|metadata| metadata.kind.context_type() == *expected),
        AvailabilityRule::All(rules) => rules
            .iter()
            .all(|rule| availability_matches(world, active, rule)),
        AvailabilityRule::Any(rules) => rules
            .iter()
            .any(|rule| availability_matches(world, active, rule)),
        AvailabilityRule::Not(rule) => !availability_matches(world, active, rule),
    }
}

pub fn navigate_up(
    In(invocation): In<CommandInvocation>,
    mut datasets: Query<(&DatasetNavigationOrder, &mut DatasetCursor)>,
) -> CommandExecution {
    navigate(invocation.source_dataset, -1, &mut datasets)
}

pub fn request_quit(
    In(_invocation): In<CommandInvocation>,
    mut requested: ResMut<QuitRequested>,
) -> CommandExecution {
    requested.0 = true;
    CommandExecution::Completed
}

pub fn reject_unimplemented(In(invocation): In<CommandInvocation>) -> CommandExecution {
    CommandExecution::Rejected(format!(
        "{} is not implemented in Pitui Next yet",
        invocation.command.0
    ))
}

pub fn open_help(
    In(_invocation): In<CommandInvocation>,
    operations: Res<ResolvedOperationSet>,
    index: Res<DatasetIndex>,
    mut contexts: Query<&mut InteractionContextMetadata>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let Some(context) = index.get(&DatasetIdentity::GlobalInteractionContext) else {
        return CommandExecution::Rejected("global Interaction Context is unavailable".into());
    };
    let Ok(mut metadata) = contexts.get_mut(context) else {
        return CommandExecution::Rejected("Interaction Context has no metadata".into());
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
    CommandExecution::Completed
}

pub fn open_command_palette(
    In(_invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    operations: Res<ResolvedOperationSet>,
    index: Res<DatasetIndex>,
    navigation: Query<(&DatasetCursor, &DatasetSelection)>,
    mut contexts: Query<&mut InteractionContextMetadata>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let Some(interaction) = index.get(&DatasetIdentity::GlobalInteractionContext) else {
        return CommandExecution::Rejected("global Interaction Context is unavailable".into());
    };
    let mut entries = operations
        .commands
        .iter()
        .filter_map(|(name, operation_id)| {
            let operation = operations
                .operations
                .iter()
                .find(|operation| &operation.id == operation_id)?;
            let targets = resolve_operation_targets(operation, &context, &navigation)?;
            Some(PaletteCommandEntry {
                name: name.clone(),
                label: operation.label.clone(),
                invocation: CommandInvocation {
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
        return CommandExecution::Rejected("Interaction Context has no metadata".into());
    };
    metadata.kind = InteractionContextKind::CommandPalette {
        query: String::new(),
        entries,
        selected: 0,
    };
    request_interaction_overlay(interaction, &mut transitions);
    CommandExecution::Completed
}

pub fn open_changes(
    In(_invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    index: Res<DatasetIndex>,
    repositories: Query<&RepositoryMetadata>,
    mut git: MessageWriter<GitCommandData>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let Some(changes) = index.get(&DatasetIdentity::GlobalChanges) else {
        return CommandExecution::Rejected("global Changes Dataset is unavailable".into());
    };
    if context.active_dataset == changes {
        return CommandExecution::Rejected("Changes is already active".into());
    }
    let Some(repository) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return CommandExecution::Rejected("no current Repository for Changes".into());
    };
    let Ok(metadata) = repositories.get(repository) else {
        return CommandExecution::Rejected("current Repository metadata is unavailable".into());
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
    CommandExecution::Completed
}

/// Opens the repository-scoped Reflog Dataset and queues its synchronous Git
/// snapshot refresh. Dataset creation, Git execution and the context change are
/// all represented as World data; no renderer is called from this operation.
pub fn open_reflog(In(_invocation): In<CommandInvocation>, world: &mut World) -> CommandExecution {
    let Some(context) = world.get_resource::<ActiveUiContext>().cloned() else {
        return CommandExecution::Rejected("active UI Context is unavailable".into());
    };
    let Some(repository_entity) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return CommandExecution::Rejected("no current Repository for Reflog".into());
    };
    let Some(repository) = world.get::<RepositoryMetadata>(repository_entity).cloned() else {
        return CommandExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Some(DatasetIdentity::Repository(repository_key)) =
        world.get::<DatasetKey>(repository_entity).map(|key| &key.0)
    else {
        return CommandExecution::Rejected("current Repository identity is unavailable".into());
    };
    let identity = DatasetIdentity::Reflog(repository_key.clone());
    if world
        .get::<DatasetKey>(context.active_dataset)
        .is_some_and(|key| key.0 == identity)
    {
        return CommandExecution::Rejected("Reflog is already active".into());
    }
    let Some(template) = world
        .resource::<DefaultDatasetTemplates>()
        .get(pitui_data::DatasetKind::Reflog)
        .cloned()
    else {
        return CommandExecution::Rejected("default Reflog Dataset template is unavailable".into());
    };
    let reflog =
        match ensure_dataset_in_world(world, identity, pitui_data::DatasetKind::Reflog, template) {
            Ok(entity) => entity,
            Err(error) => {
                return CommandExecution::Rejected(format!(
                    "cannot create Reflog Dataset: {error:?}"
                ));
            }
        };

    let mut bindings = context.render_bindings;
    bindings.bind(pitui_data::RenderBindingId::CurrentReflog, reflog);
    if let Some(entry) = world
        .get::<DatasetCursor>(reflog)
        .and_then(|cursor| cursor.0)
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
    CommandExecution::Completed
}

pub fn open_git_operation_log(
    In(_invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    index: Res<DatasetIndex>,
    cursors: Query<&DatasetCursor>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let Some(log) = index.get(&DatasetIdentity::GlobalGitOperationLog) else {
        return CommandExecution::Rejected("global Git Operation Log is unavailable".into());
    };
    if context.active_dataset == log {
        return CommandExecution::Rejected("Git Operation Log is already active".into());
    }
    let mut bindings = context.render_bindings.clone();
    bindings.bind(pitui_data::RenderBindingId::GitOperationLog, log);
    if let Some(entry) = cursors.get(log).ok().and_then(|cursor| cursor.0) {
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
    CommandExecution::Completed
}

#[allow(clippy::too_many_arguments)]
pub fn refresh_active_context(
    In(_invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    index: Res<DatasetIndex>,
    kinds: Query<&DatasetType>,
    keys: Query<&DatasetKey>,
    repositories: Query<&RepositoryMetadata>,
    files: Query<&pitui_data::FileMetadata>,
    working_files: Query<&WorkingTreeFileMetadata>,
    mut git: MessageWriter<GitCommandData>,
) -> CommandExecution {
    let Some(repository) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return CommandExecution::Rejected("no current Repository to refresh".into());
    };
    let Ok(metadata) = repositories.get(repository) else {
        return CommandExecution::Rejected("current Repository metadata is unavailable".into());
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
    CommandExecution::Completed
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
    In(_invocation): In<CommandInvocation>,
    stack: Res<ContextStack>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    if stack.0.is_empty() {
        return CommandExecution::Rejected("no previous Context to restore".into());
    }
    transitions.write(ContextTransitionRequest::Pop);
    CommandExecution::Completed
}

pub fn palette_up(
    In(invocation): In<CommandInvocation>,
    mut contexts: Query<&mut InteractionContextMetadata>,
) -> CommandExecution {
    move_palette_selection(invocation.source_dataset, -1, &mut contexts)
}

pub fn palette_down(
    In(invocation): In<CommandInvocation>,
    mut contexts: Query<&mut InteractionContextMetadata>,
) -> CommandExecution {
    move_palette_selection(invocation.source_dataset, 1, &mut contexts)
}

pub fn submit_palette_command(
    In(invocation): In<CommandInvocation>,
    contexts: Query<&InteractionContextMetadata>,
    mut deferred: ResMut<DeferredCommandInvocations>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let Ok(metadata) = contexts.get(invocation.source_dataset) else {
        return CommandExecution::Rejected("Command Context is unavailable".into());
    };
    let InteractionContextKind::CommandPalette {
        query,
        entries,
        selected,
    } = &metadata.kind
    else {
        return CommandExecution::Rejected("active Context is not the command palette".into());
    };
    let Some(entry) = entries
        .iter()
        .filter(|entry| entry.matches(query))
        .nth(*selected)
    else {
        return CommandExecution::Rejected("no command matches the current query".into());
    };
    deferred.0.push(entry.invocation.clone());
    transitions.write(ContextTransitionRequest::Pop);
    CommandExecution::Completed
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
) -> CommandExecution {
    let Ok(mut metadata) = contexts.get_mut(context) else {
        return CommandExecution::Rejected("Command Context is unavailable".into());
    };
    let InteractionContextKind::CommandPalette {
        query,
        entries,
        selected,
    } = &mut metadata.kind
    else {
        return CommandExecution::Rejected("active Context is not the command palette".into());
    };
    let count = entries.iter().filter(|entry| entry.matches(query)).count();
    if count == 0 {
        *selected = 0;
        return CommandExecution::Completed;
    }
    *selected = selected
        .saturating_add_signed(delta)
        .min(count.saturating_sub(1));
    CommandExecution::Completed
}

pub fn navigate_down(
    In(invocation): In<CommandInvocation>,
    mut datasets: Query<(&DatasetNavigationOrder, &mut DatasetCursor)>,
) -> CommandExecution {
    navigate(invocation.source_dataset, 1, &mut datasets)
}

pub fn navigate_left(
    In(_invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    mode: Res<ActiveRenderMode>,
    stack: Res<ContextStack>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let mut focus_owners = Vec::new();
    mode.layout.focus_owners(&mut focus_owners);
    let Some(position) = focus_owners
        .iter()
        .position(|dataset| *dataset == context.active_dataset)
    else {
        return CommandExecution::Rejected("Active Dataset is not rendered as focusable".into());
    };
    if position > 0 {
        transitions.write(ContextTransitionRequest::KeepRenderMode {
            active_dataset: focus_owners[position - 1],
            binding_patch: RenderBindingPatch::default(),
        });
        CommandExecution::Completed
    } else if !stack.0.is_empty() {
        transitions.write(ContextTransitionRequest::Pop);
        CommandExecution::Completed
    } else {
        CommandExecution::Rejected("already at the outermost Dataset level".into())
    }
}

pub fn navigate_right(
    In(_invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    mode: Res<ActiveRenderMode>,
    dataset_types: Query<&DatasetType>,
    navigation_modes: Res<NavigationModeRegistry>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let mut focus_owners = Vec::new();
    mode.layout.focus_owners(&mut focus_owners);
    let Some(position) = focus_owners
        .iter()
        .position(|dataset| *dataset == context.active_dataset)
    else {
        return CommandExecution::Rejected("Active Dataset is not rendered as focusable".into());
    };
    if let Some(next) = focus_owners.get(position + 1) {
        transitions.write(ContextTransitionRequest::KeepRenderMode {
            active_dataset: *next,
            binding_patch: RenderBindingPatch::default(),
        });
        return CommandExecution::Completed;
    }

    let Ok(kind) = dataset_types.get(context.active_dataset) else {
        return CommandExecution::Rejected("Active Dataset no longer exists".into());
    };
    let Some(render_mode) = navigation_modes.drill_down.get(&kind.0) else {
        return CommandExecution::Rejected("already at the deepest Dataset level".into());
    };
    transitions.write(ContextTransitionRequest::Drill {
        active_dataset: context.active_dataset,
        render_mode: render_mode.clone(),
        render_bindings: context.render_bindings.clone(),
    });
    CommandExecution::Completed
}

pub fn toggle_selection(
    In(invocation): In<CommandInvocation>,
    mut datasets: Query<(&DatasetNavigationOrder, &mut DatasetSelection)>,
) -> CommandExecution {
    let Ok((navigation, mut selection)) = datasets.get_mut(invocation.source_dataset) else {
        return CommandExecution::Rejected("active Dataset does not own a selection".into());
    };
    toggle_targets_in_navigation(navigation, &mut selection, &invocation.targets)
}

/// Cherry-pick is owned by the Commits Dataset Operation Set. Its targets are
/// the Dataset's ordered Selection (never a queue and never an implicit
/// Cursor), normalized to oldest-to-newest replay order before Git argv data is
/// emitted.
pub fn cherry_pick_selected(
    In(invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    navigation: Query<&DatasetNavigationOrder>,
    repositories: Query<&RepositoryMetadata>,
    mut git: MessageWriter<GitCommandData>,
) -> CommandExecution {
    let Ok(DatasetKey(DatasetIdentity::Commits {
        repository: source_repository,
        branch: source_branch,
    })) = keys.get(invocation.source_dataset)
    else {
        return CommandExecution::Rejected(
            "cherry-pick is only available from a Commits Dataset".into(),
        );
    };
    if invocation.targets.is_empty() {
        return CommandExecution::Rejected("select at least one commit to cherry-pick".into());
    }
    let Ok(order) = navigation.get(invocation.source_dataset) else {
        return CommandExecution::Rejected("Commits navigation order is unavailable".into());
    };
    let selected = invocation.targets.iter().copied().collect::<HashSet<_>>();
    if selected.len() != invocation.targets.len() {
        return CommandExecution::Rejected("Commit selection contains duplicate targets".into());
    }
    let ordered = order
        .0
        .iter()
        .copied()
        .filter(|entity| selected.contains(entity))
        .collect::<Vec<_>>();
    if ordered.len() != selected.len() {
        return CommandExecution::Rejected(
            "Commit selection contains targets outside the active Commits Dataset".into(),
        );
    }
    let mut commits = Vec::with_capacity(ordered.len());
    for target in ordered {
        let Ok(DatasetKey(DatasetIdentity::Commit { repository, hash })) = keys.get(target) else {
            return CommandExecution::Rejected(
                "cherry-pick selection contains a non-Commit Dataset".into(),
            );
        };
        if repository != source_repository {
            return CommandExecution::Rejected(
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
        return CommandExecution::Rejected("current Repository binding is unavailable".into());
    };
    if !matches!(
        keys.get(repository_entity).map(|key| &key.0),
        Ok(DatasetIdentity::Repository(repository)) if repository == source_repository
    ) {
        return CommandExecution::Rejected(
            "active Commits Dataset does not belong to the current Repository".into(),
        );
    }
    let Ok(repository) = repositories.get(repository_entity) else {
        return CommandExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Some(current_branch) = repository.0.current_branch.clone() else {
        return CommandExecution::Rejected(
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
    CommandExecution::Completed
}

pub fn toggle_changes_selection(
    In(invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    kinds: Query<&DatasetType>,
    mut datasets: Query<(&DatasetNavigationOrder, &mut DatasetSelection)>,
) -> CommandExecution {
    let Some(changes) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::Changes)
    else {
        return CommandExecution::Rejected("Changes binding is unavailable".into());
    };
    if invocation.targets.iter().any(|target| {
        !kinds
            .get(*target)
            .is_ok_and(|kind| kind.0 == pitui_data::DatasetKind::WorkingTreeFile)
    }) {
        return CommandExecution::Rejected("only working-tree files can be selected".into());
    }
    let Ok((navigation, mut selection)) = datasets.get_mut(changes) else {
        return CommandExecution::Rejected("Changes Dataset does not own a selection".into());
    };
    toggle_targets_in_navigation(navigation, &mut selection, &invocation.targets)
}

fn toggle_targets_in_navigation(
    navigation: &DatasetNavigationOrder,
    selection: &mut DatasetSelection,
    targets: &[Entity],
) -> CommandExecution {
    if targets.iter().any(|target| !navigation.0.contains(target)) {
        return CommandExecution::Rejected("selection target is outside the Dataset".into());
    }
    let mut selected = selection.0.iter().copied().collect::<HashSet<_>>();
    for target in targets {
        if !selected.insert(*target) {
            selected.remove(target);
        }
    }
    selection.0 = navigation
        .0
        .iter()
        .copied()
        .filter(|entity| selected.contains(entity))
        .collect();
    CommandExecution::Completed
}

#[allow(clippy::too_many_arguments)]
pub(super) fn stage_changes(
    In(invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    repositories: Query<&RepositoryMetadata>,
    revisions: Query<&DatasetRevision>,
    successes: Res<GitMutationSuccesses>,
    mut pending_focus: ResMut<PendingChangesFocus>,
    mut git: MessageWriter<GitCommandData>,
) -> CommandExecution {
    mutate_working_tree_paths(
        invocation,
        ChangeBoundary::Unstaged,
        |paths| GitCommand::StagePaths { paths },
        &context,
        &keys,
        &repositories,
        &revisions,
        &successes,
        &mut pending_focus,
        &mut git,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn unstage_changes(
    In(invocation): In<CommandInvocation>,
    context: Res<ActiveUiContext>,
    keys: Query<&DatasetKey>,
    repositories: Query<&RepositoryMetadata>,
    revisions: Query<&DatasetRevision>,
    successes: Res<GitMutationSuccesses>,
    mut pending_focus: ResMut<PendingChangesFocus>,
    mut git: MessageWriter<GitCommandData>,
) -> CommandExecution {
    mutate_working_tree_paths(
        invocation,
        ChangeBoundary::Staged,
        |paths| GitCommand::UnstagePaths { paths },
        &context,
        &keys,
        &repositories,
        &revisions,
        &successes,
        &mut pending_focus,
        &mut git,
    )
}

#[allow(clippy::too_many_arguments)]
fn mutate_working_tree_paths(
    invocation: CommandInvocation,
    expected_boundary: ChangeBoundary,
    command: impl FnOnce(Vec<pitui_core::GitPath>) -> GitCommand,
    context: &ActiveUiContext,
    keys: &Query<&DatasetKey>,
    repositories: &Query<&RepositoryMetadata>,
    revisions: &Query<&DatasetRevision>,
    successes: &GitMutationSuccesses,
    pending_focus: &mut PendingChangesFocus,
    git: &mut MessageWriter<GitCommandData>,
) -> CommandExecution {
    let Some(repository_entity) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return CommandExecution::Rejected("current Repository binding is unavailable".into());
    };
    let Ok(repository_metadata) = repositories.get(repository_entity) else {
        return CommandExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Ok(DatasetKey(DatasetIdentity::Repository(repository_key))) = keys.get(repository_entity)
    else {
        return CommandExecution::Rejected("current Repository identity is unavailable".into());
    };

    let mut seen = HashSet::new();
    let mut paths = Vec::with_capacity(invocation.targets.len());
    for target in &invocation.targets {
        let Ok(DatasetKey(DatasetIdentity::WorkingTreeFile {
            repository,
            boundary,
            path,
        })) = keys.get(*target)
        else {
            return CommandExecution::Rejected(
                "stage target is not a working-tree file Dataset".into(),
            );
        };
        if repository != repository_key {
            return CommandExecution::Rejected(
                "stage target belongs to a different Repository".into(),
            );
        }
        if *boundary != expected_boundary {
            return CommandExecution::Rejected(format!(
                "stage target has the wrong change boundary: expected {expected_boundary:?}"
            ));
        }
        if seen.insert(path.clone()) {
            paths.push(path.clone());
        }
    }
    if paths.is_empty() {
        return CommandExecution::Rejected("no working-tree files were selected".into());
    }

    let Some(changes) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::Changes)
    else {
        return CommandExecution::Rejected("Changes binding is unavailable".into());
    };
    let Ok(changes_revision) = revisions.get(changes) else {
        return CommandExecution::Rejected("Changes revision is unavailable".into());
    };

    let focus_path = paths[0].clone();
    let mutation = command(paths);
    pending_focus.0 = Some(ChangesFocusRequest {
        mutation: mutation.clone(),
        success_index: successes.0.len(),
        changes_revision: changes_revision.0,
        target: ChangesFocusTarget::Exact {
            repository: repository_key.clone(),
            path: focus_path,
            boundary: match expected_boundary {
                ChangeBoundary::Staged => ChangeBoundary::Unstaged,
                ChangeBoundary::Unstaged => ChangeBoundary::Staged,
            },
        },
        context: ChangesFocusContext::Active,
        preserve_diff_focus: context
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
    CommandExecution::Completed
}

pub(super) fn reconcile_pending_changes_focus(world: &mut World) {
    let request = world.resource_mut::<PendingChangesFocus>().0.take();
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
    let Some(navigation) = world.get::<DatasetNavigationOrder>(changes) else {
        return;
    };
    let may_be_empty = matches!(&request.target, ChangesFocusTarget::FirstRemainingFile);
    let file = match request.target {
        ChangesFocusTarget::Exact {
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
            .filter(|file| navigation.0.contains(file)),
        ChangesFocusTarget::FirstRemainingFile => navigation.0.iter().copied().find(|row| {
            matches!(
                world.get::<DatasetKey>(*row).map(|key| &key.0),
                Some(DatasetIdentity::WorkingTreeFile { .. })
            )
        }),
    };
    let Some(file) = file else {
        if may_be_empty {
            match request.context {
                ChangesFocusContext::Active => {
                    if let Some(mut context) = world.get_resource_mut::<ActiveUiContext>() {
                        context
                            .render_bindings
                            .unbind(&pitui_data::RenderBindingId::CurrentChangesFileChanges);
                        context.active_dataset = changes;
                        context.generation = context.generation.wrapping_add(1);
                    }
                }
                ChangesFocusContext::Previous => {
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
    if let Some(mut cursor) = world.get_mut::<DatasetCursor>(changes) {
        cursor.0 = Some(file);
    }

    let Some(diff) = world
        .get::<pitui_data::DatasetChildren>(file)
        .and_then(|children| children.0.first().copied())
    else {
        return;
    };
    match request.context {
        ChangesFocusContext::Active => {
            let Some(mut context) = world.get_resource_mut::<ActiveUiContext>() else {
                return;
            };
            context
                .render_bindings
                .bind(pitui_data::RenderBindingId::CurrentChangesFileChanges, diff);
            if request.preserve_diff_focus {
                context.active_dataset = diff;
            }
            context.generation = context.generation.wrapping_add(1);
        }
        ChangesFocusContext::Previous => {
            let mut stack = world.resource_mut::<ContextStack>();
            let Some(snapshot) = stack.0.last_mut() else {
                return;
            };
            snapshot
                .render_bindings
                .bind(pitui_data::RenderBindingId::CurrentChangesFileChanges, diff);
            if request.preserve_diff_focus {
                snapshot.active_dataset = diff;
            }
        }
    }
}

pub fn open_commit_creation(
    In(_invocation): In<CommandInvocation>,
    world: &mut World,
) -> CommandExecution {
    let Some(context) = world.get_resource::<ActiveUiContext>().cloned() else {
        return CommandExecution::Rejected("active UI Context is unavailable".into());
    };
    let Some(repository_entity) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return CommandExecution::Rejected("current Repository binding is unavailable".into());
    };
    let Some(DatasetIdentity::Repository(repository)) =
        world.get::<DatasetKey>(repository_entity).map(|key| &key.0)
    else {
        return CommandExecution::Rejected("current Repository identity is unavailable".into());
    };
    let repository = repository.clone();
    let Some(changes) = context
        .render_bindings
        .get(&pitui_data::RenderBindingId::Changes)
    else {
        return CommandExecution::Rejected("Changes binding is unavailable".into());
    };
    let Some(revision) = world
        .get::<DatasetRevision>(changes)
        .map(|revision| revision.0)
    else {
        return CommandExecution::Rejected("Changes revision is unavailable".into());
    };
    let staged_paths = world
        .get::<DatasetNavigationOrder>(changes)
        .map(|navigation| {
            navigation
                .0
                .iter()
                .filter_map(
                    |entity| match world.get::<DatasetKey>(*entity).map(|key| &key.0) {
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
        return CommandExecution::Rejected("there are no staged files to commit".into());
    }
    let Some(template) = world
        .resource::<DefaultDatasetTemplates>()
        .get(pitui_data::DatasetKind::CommitCreation)
        .cloned()
    else {
        return CommandExecution::Rejected(
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
            return CommandExecution::Rejected(format!(
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
    CommandExecution::Completed
}

pub(super) fn submit_text_input(
    In(invocation): In<CommandInvocation>,
    contexts: Query<&InteractionContextMetadata>,
) -> CommandExecution {
    let Ok(metadata) = contexts.get(invocation.source_dataset) else {
        return CommandExecution::Rejected("Text Input Context is unavailable".into());
    };
    let InteractionContextKind::TextInput { purpose, .. } = &metadata.kind else {
        return CommandExecution::Rejected("active Context is not a Text Input".into());
    };
    CommandExecution::Rejected(format!(
        "Text Input purpose {purpose:?} is not implemented yet"
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn submit_commit_creation(
    In(invocation): In<CommandInvocation>,
    stack: Res<ContextStack>,
    keys: Query<&DatasetKey>,
    revisions: Query<&DatasetRevision>,
    repositories: Query<&RepositoryMetadata>,
    mut creations: Query<&mut CommitCreationMetadata>,
    successes: Res<GitMutationSuccesses>,
    mut pending_focus: ResMut<PendingChangesFocus>,
    mut git: MessageWriter<GitCommandData>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    let (repository_key, message, staged_revision) = {
        let Ok(mut creation) = creations.get_mut(invocation.source_dataset) else {
            return CommandExecution::Rejected("Commit Creation Dataset is unavailable".into());
        };
        let message = creation.message.trim();
        if message.is_empty() {
            creation.error = Some("Commit message cannot be empty".into());
            return CommandExecution::Rejected("commit message cannot be empty".into());
        }
        (
            creation.repository.clone(),
            message.to_owned(),
            creation.staged_revision,
        )
    };

    let Some(previous) = stack.0.last() else {
        return CommandExecution::Rejected("no Changes Context to restore".into());
    };
    let Some(repository_entity) = previous
        .render_bindings
        .get(&pitui_data::RenderBindingId::CurrentRepository)
    else {
        return CommandExecution::Rejected("current Repository binding is unavailable".into());
    };
    if !matches!(
        keys.get(repository_entity).map(|key| &key.0),
        Ok(DatasetIdentity::Repository(repository)) if repository == &repository_key
    ) {
        return CommandExecution::Rejected(
            "Commit Creation repository no longer matches the current Repository".into(),
        );
    }
    let Ok(repository) = repositories.get(repository_entity) else {
        return CommandExecution::Rejected("current Repository metadata is unavailable".into());
    };
    let Some(changes) = previous
        .render_bindings
        .get(&pitui_data::RenderBindingId::Changes)
    else {
        return CommandExecution::Rejected("Changes binding is unavailable".into());
    };
    let Ok(changes_revision) = revisions.get(changes) else {
        return CommandExecution::Rejected("Changes revision is unavailable".into());
    };
    if changes_revision.0 != staged_revision {
        if let Ok(mut creation) = creations.get_mut(invocation.source_dataset) {
            creation.error = Some("The staged snapshot changed; reopen commit creation".into());
        }
        return CommandExecution::Rejected("the staged snapshot changed".into());
    }

    let cwd = repository.0.root.clone();
    let current_branch = repository.0.current_branch.clone();
    let mutation = GitCommand::Commit { message };
    pending_focus.0 = Some(ChangesFocusRequest {
        mutation: mutation.clone(),
        success_index: successes.0.len(),
        changes_revision: changes_revision.0,
        target: ChangesFocusTarget::FirstRemainingFile,
        context: ChangesFocusContext::Previous,
        preserve_diff_focus: previous
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
    CommandExecution::Completed
}

pub fn navigate_back(
    In(_invocation): In<CommandInvocation>,
    stack: Res<ContextStack>,
    mut transitions: MessageWriter<ContextTransitionRequest>,
) -> CommandExecution {
    if stack.0.is_empty() {
        CommandExecution::Rejected("already at the outermost Dataset level".into())
    } else {
        transitions.write(ContextTransitionRequest::Pop);
        CommandExecution::Completed
    }
}

pub fn copy_commit_hashes(
    In(invocation): In<CommandInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> CommandExecution {
    let hashes = invocation
        .targets
        .iter()
        .filter_map(|entity| match keys.get(*entity).ok().map(|key| &key.0) {
            Some(DatasetIdentity::Commit { hash, .. }) => Some(hash.0.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if hashes.len() != invocation.targets.len() || hashes.is_empty() {
        return CommandExecution::Rejected("copy hash target is not a Commit Dataset".into());
    }
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitHashes,
        text: hashes.join("\n"),
        source_entities: invocation.targets,
    });
    CommandExecution::Completed
}

pub fn copy_reflog_hash(
    In(invocation): In<CommandInvocation>,
    entries: Query<&ReflogEntryMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> CommandExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return CommandExecution::Rejected("no Reflog entry target".into());
    };
    let Ok(metadata) = entries.get(target) else {
        return CommandExecution::Rejected("copy hash target is not a Reflog entry Dataset".into());
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::ReflogHash,
        text: metadata.0.hash.0.clone(),
        source_entities: vec![target],
    });
    CommandExecution::Completed
}

pub fn copy_commit_info(
    In(invocation): In<CommandInvocation>,
    commits: Query<&CommitMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> CommandExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return CommandExecution::Rejected("no Commit target".into());
    };
    let Ok(metadata) = commits.get(target) else {
        return CommandExecution::Rejected("copy info target has no Commit metadata".into());
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
    CommandExecution::Completed
}

pub fn copy_commit_message(
    In(invocation): In<CommandInvocation>,
    commits: Query<&CommitMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> CommandExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return CommandExecution::Rejected("no Commit target".into());
    };
    let Some(message) = commits
        .get(target)
        .ok()
        .and_then(|metadata| metadata.message.clone())
    else {
        return CommandExecution::Rejected("full commit message is not loaded".into());
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitMessage,
        text: message,
        source_entities: vec![target],
    });
    CommandExecution::Completed
}

pub fn copy_file_name(
    In(invocation): In<CommandInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> CommandExecution {
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
    In(invocation): In<CommandInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> CommandExecution {
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
    In(invocation): In<CommandInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> CommandExecution {
    copy_file_path(
        invocation,
        &keys,
        ClipboardContentKind::FileRelativePath,
        |_, path| Some(path.as_str().into()),
        &mut clipboard,
    )
}

fn copy_file_path(
    invocation: CommandInvocation,
    keys: &Query<&DatasetKey>,
    kind: ClipboardContentKind,
    value: impl FnOnce(&pitui_data::RepositoryKey, &pitui_core::GitPath) -> Option<String>,
    clipboard: &mut MessageWriter<ClipboardRequest>,
) -> CommandExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return CommandExecution::Rejected("no File target".into());
    };
    let Ok(key) = keys.get(target) else {
        return CommandExecution::Rejected("File target no longer exists".into());
    };
    let (repository, path) = match &key.0 {
        DatasetIdentity::File {
            repository, path, ..
        }
        | DatasetIdentity::WorkingTreeFile {
            repository, path, ..
        } => (repository, path),
        _ => return CommandExecution::Rejected("copy path target is not a File Dataset".into()),
    };
    let Some(text) = value(repository, path) else {
        return CommandExecution::Rejected("File path has no copyable name".into());
    };
    clipboard.write(ClipboardRequest {
        kind,
        text,
        source_entities: vec![target],
    });
    CommandExecution::Completed
}

pub fn scroll_home(
    In(invocation): In<CommandInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> CommandExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::Home,
        &mut viewports,
    )
}

pub fn scroll_end(
    In(invocation): In<CommandInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> CommandExecution {
    update_scroll(invocation.source_dataset, ScrollAction::End, &mut viewports)
}

pub fn scroll_page_up(
    In(invocation): In<CommandInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> CommandExecution {
    update_scroll(
        invocation.source_dataset,
        ScrollAction::PageUp,
        &mut viewports,
    )
}

pub fn scroll_page_down(
    In(invocation): In<CommandInvocation>,
    mut viewports: Query<&mut DatasetViewport>,
) -> CommandExecution {
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
) -> CommandExecution {
    let Ok(mut viewport) = viewports.get_mut(dataset) else {
        return CommandExecution::Rejected("active Dataset has no text viewport".into());
    };
    let page_size = viewport.page_size.max(1);
    let max_offset = viewport.content_length.saturating_sub(page_size);
    viewport.offset = match action {
        ScrollAction::Home => 0,
        ScrollAction::End => max_offset,
        ScrollAction::PageUp => viewport.offset.saturating_sub(page_size),
        ScrollAction::PageDown => viewport.offset.saturating_add(page_size).min(max_offset),
    };
    CommandExecution::Completed
}

fn navigate(
    dataset: Entity,
    delta: isize,
    datasets: &mut Query<(&DatasetNavigationOrder, &mut DatasetCursor)>,
) -> CommandExecution {
    let Ok((navigation, mut cursor)) = datasets.get_mut(dataset) else {
        return CommandExecution::Rejected("active Dataset is not navigable".into());
    };
    if navigation.0.is_empty() {
        cursor.0 = None;
        return CommandExecution::Completed;
    }
    let current = cursor
        .0
        .and_then(|current| navigation.0.iter().position(|row| *row == current))
        .unwrap_or_default();
    let next = current
        .saturating_add_signed(delta)
        .min(navigation.0.len() - 1);
    cursor.0 = Some(navigation.0[next]);
    CommandExecution::Completed
}
