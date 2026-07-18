//! Resolves global plus Dataset-bound Operation declarations into the single
//! cache owned by the current Active Dataset.

use super::*;

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

pub fn resolve_active_operation_set(world: &mut World) {
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
        active_dataset: Some(active_dataset),
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
        AvailabilityRule::HasActiveElement => world
            .get::<DatasetActiveElement>(active)
            .is_some_and(|element| element.0.is_some()),
        AvailabilityRule::HasSelection => world
            .get::<DatasetSelection>(active)
            .is_some_and(|selection| !selection.0.is_empty()),
        AvailabilityRule::HasSelectionOrActiveElement => {
            availability_matches(world, active, &AvailabilityRule::HasSelection)
                || availability_matches(world, active, &AvailabilityRule::HasActiveElement)
        }
        AvailabilityRule::ContextHasActiveElement(binding) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| world.get::<DatasetActiveElement>(dataset))
            .is_some_and(|element| element.0.is_some()),
        AvailabilityRule::ContextHasSelectionOrActiveElement(binding) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .is_some_and(|dataset| {
                world
                    .get::<DatasetSelection>(dataset)
                    .is_some_and(|selection| !selection.0.is_empty())
                    || world
                        .get::<DatasetActiveElement>(dataset)
                        .is_some_and(|element| element.0.is_some())
            }),
        AvailabilityRule::ContextActiveElementKind(binding, expected) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| world.get::<DatasetActiveElement>(dataset))
            .and_then(|active| active.0)
            .and_then(|element| world.get::<DatasetType>(element))
            .is_some_and(|kind| kind.0 == *expected),
        AvailabilityRule::ContextTargetsBoundary(binding, expected) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| {
                let active_element = world.get::<DatasetActiveElement>(dataset)?.0;
                let selection = world.get::<DatasetSelection>(dataset)?;
                let targets = if selection.0.is_empty() {
                    active_element.into_iter().collect::<Vec<_>>()
                } else {
                    selection.0.clone()
                };
                (!targets.is_empty()).then_some(targets)
            })
            .is_some_and(|targets| {
                targets.iter().all(|target| {
                    matches!(
                        world.get::<DatasetKey>(*target).map(|key| &key.0),
                        Some(
                            DatasetIdentity::WorkingTreeFiles { boundary, .. }
                                | DatasetIdentity::WorkingTreeFile { boundary, .. }
                                | DatasetIdentity::WorkingTreeDirectory { boundary, .. }
                        )
                            if boundary == expected
                    )
                })
            }),
        AvailabilityRule::ChangesHasStagedFiles(binding) => world
            .get_resource::<ActiveUiContext>()
            .and_then(|context| context.render_bindings.get(binding))
            .and_then(|dataset| world.get::<DatasetCollection>(dataset))
            .is_some_and(|collection| {
                collection.entities().any(|row| {
                    matches!(
                        world.get::<DatasetKey>(row).map(|key| &key.0),
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
