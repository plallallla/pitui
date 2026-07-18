// Crate-internal tests live separately so production modules remain navigable.

use std::collections::HashSet;

use super::*;

#[test]
fn builtins_cover_every_dataset_kind_once() {
    let templates = builtin_dataset_templates();
    assert_eq!(templates.len(), 22);
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
    assert_eq!(proxies.len(), 26);

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
fn file_collection_proxies_support_tree_and_flat_list() {
    let proxies = builtin_render_proxies()
        .into_iter()
        .map(|proxy| (proxy.id.clone(), proxy))
        .collect::<std::collections::HashMap<_, _>>();

    for id in ["files.tree", "changes.tree", "working-tree-files.tree"] {
        assert_eq!(
            proxies[&RenderProxyId::from(id)].renderer,
            RendererKind::PathTree,
            "{id} must preserve directory structure instead of flattening Git paths"
        );
    }
    assert_eq!(
        proxies[&RenderProxyId::from("files.list")].renderer,
        RendererKind::List
    );
    assert!(!proxies.contains_key(&RenderProxyId::from("working-tree-files.list")));
}

#[test]
fn collection_managers_are_declared_by_dataset_templates() {
    let templates = builtin_dataset_templates()
        .into_iter()
        .map(|template| (template.kind, template))
        .collect::<std::collections::HashMap<_, _>>();

    for kind in [
        DatasetKind::RepositoriesBranches,
        DatasetKind::Files,
        DatasetKind::Changes,
        DatasetKind::WorkingTreeFiles,
        DatasetKind::FileTreeDirectory,
    ] {
        let CollectionManagerSpec::Tree(tree) = &templates[&kind].collection else {
            panic!("{kind:?} must use the shared Tree Manager");
        };
        assert_eq!(tree.selection, TreeSelectionMode::Cascade);
        assert!(!tree.visible_kinds.is_empty());
        assert!(
            tree.selectable_kinds
                .iter()
                .all(|kind| tree.visible_kinds.contains(kind))
        );
    }
    let CollectionManagerSpec::Tree(changes) = &templates[&DatasetKind::Changes].collection else {
        unreachable!("Changes was already verified as a Tree Manager")
    };
    assert!(
        changes
            .selectable_kinds
            .contains(&DatasetKind::WorkingTreeFiles),
        "the Staged/Unstaged group rows must select their complete subtrees"
    );
    for kind in [
        DatasetKind::Commits,
        DatasetKind::Reflog,
        DatasetKind::Remotes,
        DatasetKind::GitOperationLog,
    ] {
        assert_eq!(
            templates[&kind].collection,
            CollectionManagerSpec::default(),
            "{kind:?} must keep independent List Manager semantics"
        );
    }

    let files = &templates[&DatasetKind::Files];
    assert_eq!(
        files
            .views
            .iter()
            .map(|view| view.id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["tree", "list"]
    );
    assert!(matches!(
        &files.views[0].collection,
        CollectionManagerSpec::Tree(_)
    ));
    assert!(matches!(
        &files.views[1].collection,
        CollectionManagerSpec::List(ListManagerSpec {
            source: ListSource::Descendants,
            ..
        })
    ));
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
            "active.up",
            "active.down",
            "active.left",
            "active.right",
            "selection.toggle",
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
    let commits = builtin_dataset_templates()
        .into_iter()
        .find(|template| template.kind == DatasetKind::Commits)
        .unwrap();
    assert!(commits.hotkeys.bindings_for(&operation_id).is_empty());
}

#[test]
fn reset_is_compiled_behavior_with_dataset_owned_hotkey_tables() {
    let templates = builtin_dataset_templates()
        .into_iter()
        .map(|template| (template.kind, template))
        .collect::<std::collections::HashMap<_, _>>();
    let global = builtin_global_operation_set();

    for (operation, suffix) in [
        (OperationId::from("reset.soft"), 's'),
        (OperationId::from("reset.mixed"), 'm'),
        (OperationId::from("reset.hard"), 'h'),
    ] {
        assert!(!global.operations.contains(&operation));
        let expected = [KeySequence::chord([
            KeyStroke::control('x'),
            KeyStroke::character(suffix),
        ])];
        for kind in [DatasetKind::Commits, DatasetKind::Reflog] {
            let template = &templates[&kind];
            assert!(template.operations.contains(&operation));
            assert_eq!(template.hotkeys.bindings_for(&operation), expected);
        }
    }

    let hard = builtin_operation_specs()
        .into_iter()
        .find(|operation| operation.id == OperationId::from("reset.hard"))
        .unwrap();
    assert_eq!(hard.command, CommandId::from("reset.hard"));
    assert_eq!(hard.target_source, TargetSource::ActiveElement);
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
                Some(InteractionContextType::Confirmation),
                Some(InteractionContextType::TextInput),
                Some(InteractionContextType::Notice),
            ]
        } else {
            vec![None]
        };
        for context_type in context_types {
            let global = builtin_global_operation_set();
            let candidates = global
                .operations
                .iter()
                .map(|id| (id, global.hotkeys.bindings_for(id)))
                .chain(
                    template
                        .operations
                        .iter()
                        .map(|id| (id, template.hotkeys.bindings_for(id))),
                )
                .collect::<Vec<_>>();
            let sequences = candidates
                .iter()
                .filter(|(id, _)| {
                    let operation = operations
                        .get(*id)
                        .unwrap_or_else(|| panic!("missing built-in Operation {}", id.0));
                    availability_can_match(
                        rules.get(&operation.availability).unwrap(),
                        template.kind,
                        context_type,
                    )
                })
                .flat_map(|(_, bindings)| bindings.iter())
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
        | AvailabilityRule::HasActiveElement
        | AvailabilityRule::HasSelection
        | AvailabilityRule::HasSelectionOrActiveElement
        | AvailabilityRule::ContextHasActiveElement(_)
        | AvailabilityRule::ContextHasSelectionOrActiveElement(_)
        | AvailabilityRule::ContextActiveElementKind(_, _)
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
    let global = builtin_global_operation_set();
    let templates = builtin_dataset_templates();
    let all_strokes = std::iter::once(&global.hotkeys)
        .chain(templates.iter().map(|template| &template.hotkeys))
        .flat_map(|table| &table.0)
        .flat_map(|entry| &entry.bindings)
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
fn detail_mode_handoffs_keep_the_source_list_active() {
    let handoffs = builtin_active_handoffs();
    for kind in [DatasetKind::Commits, DatasetKind::Files] {
        let spec = handoffs
            .rules
            .get(&(kind, ActiveDirection::Right))
            .unwrap_or_else(|| panic!("missing Right handoff for {kind:?}"));
        assert_eq!(spec.target, ActiveHandoffTarget::KeepActiveDataset);
    }
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
