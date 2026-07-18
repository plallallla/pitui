// Crate-internal tests live separately so production modules remain navigable.

use std::collections::HashSet;

use super::*;

#[test]
fn builtins_cover_every_dataset_kind_once() {
    let templates = builtin_dataset_templates();
    assert_eq!(templates.len(), 20);
    let kinds = templates
        .iter()
        .map(|template| template.kind)
        .collect::<HashSet<_>>();
    assert_eq!(kinds.len(), templates.len());
    assert_eq!(kinds, DatasetKind::ALL.into_iter().collect::<HashSet<_>>());
    assert!(
        templates
            .iter()
            .all(|template| !template.render_proxies.is_empty())
    );
}

#[test]
fn every_template_proxy_resolves_to_the_same_dataset_kind() {
    let proxies = builtin_render_proxies()
        .into_iter()
        .map(|proxy| (proxy.id.clone(), proxy))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(proxies.len(), 23);

    for template in builtin_dataset_templates() {
        for proxy_id in template.render_proxies {
            let proxy = proxies
                .get(&proxy_id)
                .unwrap_or_else(|| panic!("missing built-in proxy {}", proxy_id.0));
            assert_eq!(proxy.dataset_kind, template.kind);
        }
    }
}

#[test]
fn core_semantic_datasets_have_explicit_proxy_and_operation_contracts() {
    let templates = builtin_dataset_templates()
        .into_iter()
        .map(|template| (template.kind, template))
        .collect::<std::collections::HashMap<_, _>>();

    let repositories_branches = &templates[&DatasetKind::RepositoriesBranches];
    assert_eq!(
        repositories_branches.render_proxies,
        vec![RenderProxyId::from("repositories-branches.compact")]
    );
    assert_eq!(
        repositories_branches.operations,
        [
            "navigation.up",
            "navigation.down",
            "navigation.left",
            "navigation.right",
        ]
        .into_iter()
        .map(OperationId::from)
        .collect::<Vec<_>>()
    );
    assert!(templates[&DatasetKind::Repository].operations.is_empty());
    assert!(templates[&DatasetKind::Branch].operations.is_empty());

    for kind in [
        DatasetKind::Commits,
        DatasetKind::Commit,
        DatasetKind::Files,
        DatasetKind::File,
        DatasetKind::FileChanges,
        DatasetKind::Reflog,
        DatasetKind::CommitCreation,
    ] {
        let template = &templates[&kind];
        assert!(
            !template.render_proxies.is_empty(),
            "{kind:?} has no Render Proxy contract"
        );
        assert!(
            !template.operations.is_empty(),
            "{kind:?} has no Operation Set contract"
        );
    }
    assert_eq!(
        templates[&DatasetKind::CommitCreation].render_proxies,
        vec![RenderProxyId::from("commit-creation.editor")]
    );
    assert_eq!(
        templates[&DatasetKind::Reflog].render_proxies,
        vec![RenderProxyId::from("reflog.list")]
    );
}

#[test]
fn cherry_pick_is_a_selection_only_commits_operation_not_a_global_operation() {
    let operation_id = OperationId::from("commits.cherry-pick");
    assert!(!builtin_global_operations().contains(&operation_id));

    let templates = builtin_dataset_templates();
    let owners = templates
        .iter()
        .filter(|template| template.operations.contains(&operation_id))
        .map(|template| template.kind)
        .collect::<Vec<_>>();
    assert_eq!(owners, vec![DatasetKind::Commits]);

    let operation = builtin_operation_specs()
        .into_iter()
        .find(|operation| operation.id == operation_id)
        .unwrap();
    assert_eq!(operation.command, CommandId::from("commits.cherry-pick"));
    assert_eq!(operation.target_source, TargetSource::Selection);
    assert_eq!(
        operation.availability,
        AvailabilityRuleId::from("has-selection")
    );
    assert!(operation.bindings.is_empty());
}

#[test]
fn built_in_render_modes_have_unique_ids() {
    let modes = builtin_render_modes();
    let ids = modes
        .iter()
        .map(|mode| mode.id.clone())
        .collect::<HashSet<_>>();
    assert_eq!(modes.len(), 8);
    assert_eq!(ids.len(), modes.len());
}

#[test]
fn every_effective_dataset_operation_set_is_resolvable_and_unambiguous() {
    let commands = builtin_command_specs()
        .into_iter()
        .map(|command| command.id)
        .collect::<HashSet<_>>();
    let rules = builtin_availability_rules()
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    let operations = builtin_operation_specs()
        .into_iter()
        .map(|operation| (operation.id.clone(), operation))
        .collect::<std::collections::HashMap<_, _>>();
    for operation in operations.values() {
        assert!(commands.contains(&operation.command));
        assert!(rules.contains_key(&operation.availability));
    }

    for template in builtin_dataset_templates() {
        let context_types = if template.kind == DatasetKind::InteractionContext {
            vec![
                Some(InteractionContextType::Help),
                Some(InteractionContextType::CommandPalette),
                Some(InteractionContextType::TextInput),
                Some(InteractionContextType::Notice),
            ]
        } else {
            vec![None]
        };
        for context_type in context_types {
            let ids = builtin_global_operations()
                .into_iter()
                .chain(template.operations.clone())
                .collect::<Vec<_>>();
            let sequences = ids
                .iter()
                .map(|id| {
                    operations
                        .get(id)
                        .unwrap_or_else(|| panic!("missing built-in Operation {}", id.0))
                })
                .filter(|operation| {
                    availability_can_match(
                        rules.get(&operation.availability).unwrap(),
                        template.kind,
                        context_type,
                    )
                })
                .flat_map(|operation| operation.bindings.iter())
                .collect::<Vec<_>>();
            assert_eq!(
                sequences.iter().copied().collect::<HashSet<_>>().len(),
                sequences.len(),
                "duplicate effective key sequence for {:?}/{context_type:?}",
                template.kind
            );
            for (index, left) in sequences.iter().enumerate() {
                for right in sequences.iter().skip(index + 1) {
                    let (shorter, longer) = if left.0.len() <= right.0.len() {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    assert!(
                        shorter.0.len() == longer.0.len() || !longer.0.starts_with(&shorter.0),
                        "ambiguous key prefix in {:?}/{context_type:?}: {shorter:?} / {longer:?}",
                        template.kind
                    );
                }
            }
        }
    }
}

fn availability_can_match(
    rule: &AvailabilityRule,
    kind: DatasetKind,
    context_type: Option<InteractionContextType>,
) -> bool {
    match rule {
        AvailabilityRule::Always
        | AvailabilityRule::HasCursor
        | AvailabilityRule::HasSelection
        | AvailabilityRule::HasSelectionOrCursor
        | AvailabilityRule::ContextHasCursor(_)
        | AvailabilityRule::ContextHasSelectionOrCursor(_)
        | AvailabilityRule::ContextCursorKind(_, _)
        | AvailabilityRule::ContextTargetsBoundary(_, _)
        | AvailabilityRule::ChangesHasStagedFiles(_) => true,
        AvailabilityRule::ActiveDatasetKind(expected) => *expected == kind,
        AvailabilityRule::InteractionContextType(expected) => context_type == Some(*expected),
        AvailabilityRule::All(rules) => rules
            .iter()
            .all(|rule| availability_can_match(rule, kind, context_type)),
        AvailabilityRule::Any(rules) => rules
            .iter()
            .any(|rule| availability_can_match(rule, kind, context_type)),
        AvailabilityRule::Not(rule) => !availability_can_match(rule, kind, context_type),
    }
}

#[test]
fn default_profile_has_wasd_arrows_and_keeps_control_space_unbound() {
    let operations = builtin_operation_specs();
    let all_strokes = operations
        .iter()
        .flat_map(|operation| &operation.bindings)
        .flat_map(|sequence| &sequence.0)
        .collect::<HashSet<_>>();
    for stroke in [
        KeyStroke::character('w'),
        KeyStroke::character('a'),
        KeyStroke::character('s'),
        KeyStroke::character('d'),
        KeyStroke::plain(KeyCode::Up),
        KeyStroke::plain(KeyCode::Down),
        KeyStroke::plain(KeyCode::Left),
        KeyStroke::plain(KeyCode::Right),
    ] {
        assert!(all_strokes.contains(&stroke));
    }
    let control_space = KeyStroke {
        code: KeyCode::Space,
        modifiers: pitui_data::KeyModifiers::control(),
    };
    assert!(!all_strokes.contains(&control_space));
}

#[test]
fn default_git_logging_profile_is_bounded_and_persistent() {
    let logging = default_git_logging_config();
    assert!(logging.enabled);
    assert!(logging.max_bytes > 0);
    assert!(logging.keep_files > 0);
    assert!(logging.buffer_capacity > 0);
    assert!(logging.max_message_chars > 0);
    assert!(!logging.path.as_os_str().is_empty());
}
