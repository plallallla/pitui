use bevy_ecs::prelude::{In, Resource};
use pitui_data::{
    AvailabilityRule, AvailabilityRuleId, CommandId, CommandInvocation, CommandScope, CommandSpec,
    CommandSystemId, DatasetIdentity, DatasetKind, DatasetTemplate, DatasetTemplateId, InputIntent,
    InvocationSource, KeyCode, KeySequence, KeyStroke, LayoutConstraint, OperationId,
    OperationNotice, OperationSpec, RenderContextBindings, RenderModeId, RenderProxyId,
    ResolvedKeyAction, ResolvedOperationSet, ResolvedRenderLayout, TargetSource,
};
use pitui_ecs::{
    CommandExecution, CommandExecutionLog, DatasetRuntime, OperationNotices,
    OperationResolutionDiagnostics, OperationResolutionError,
};

fn register_template(
    runtime: &mut DatasetRuntime,
    id: &str,
    kind: DatasetKind,
    operations: Vec<OperationId>,
) -> DatasetTemplateId {
    let id = DatasetTemplateId::from(id);
    runtime
        .register_template(DatasetTemplate {
            id: id.clone(),
            kind,
            collection: pitui_data::CollectionManagerSpec::default(),
            views: Vec::new(),
            operations,
            render_proxies: vec![RenderProxyId::from("test")],
        })
        .unwrap();
    id
}

fn command(id: &str, name: &str, system: &str, scope: CommandScope) -> CommandSpec {
    CommandSpec {
        id: CommandId::from(id),
        name: name.into(),
        scope,
        system: CommandSystemId::from(system),
    }
}

fn operation(id: &str, command: &str, key: char, target_source: TargetSource) -> OperationSpec {
    OperationSpec {
        id: OperationId::from(id),
        label: id.into(),
        command: CommandId::from(command),
        bindings: vec![KeySequence::single(KeyStroke::character(key))],
        target_source,
        availability: AvailabilityRuleId::from("always"),
    }
}

fn initialize_single_panel(runtime: &mut DatasetRuntime, dataset: bevy_ecs::prelude::Entity) {
    runtime
        .initialize_ui(
            dataset,
            RenderModeId::from("test"),
            RenderContextBindings::default(),
            ResolvedRenderLayout::Dataset {
                dataset,
                proxy: RenderProxyId::from("test"),
                constraint: LayoutConstraint::Fill(1),
                activatable: true,
            },
            ResolvedOperationSet::default(),
        )
        .unwrap();
}

#[test]
fn global_command_name_wins_while_duplicate_keys_fail_resolution() {
    let mut runtime = DatasetRuntime::new();
    let local_id = OperationId::from("local.same");
    let template = register_template(
        &mut runtime,
        "repository",
        DatasetKind::Repository,
        vec![local_id.clone()],
    );
    runtime
        .register_availability_rule(AvailabilityRuleId::from("always"), AvailabilityRule::Always)
        .unwrap();
    runtime
        .register_command(command(
            "global.command",
            "same",
            "global.system",
            CommandScope::Global,
        ))
        .unwrap();
    runtime
        .register_command(command(
            "local.command",
            "same",
            "local.system",
            CommandScope::Dataset,
        ))
        .unwrap();
    let global = operation("global.same", "global.command", 'g', TargetSource::None);
    let local = operation("local.same", "local.command", 'l', TargetSource::None);
    runtime.register_operation(global.clone()).unwrap();
    runtime.register_operation(local).unwrap();
    runtime.set_global_operations(vec![global.id.clone()]);
    let dataset = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/repo")),
            DatasetKind::Repository,
            template,
        )
        .unwrap();
    runtime.add_root(dataset).unwrap();
    initialize_single_panel(&mut runtime, dataset);
    runtime.run_schedule();

    let resolved = runtime.world().resource::<ResolvedOperationSet>();
    assert_eq!(
        resolved.commands.get("same"),
        Some(&OperationId::from("global.same"))
    );
    assert!(
        resolved
            .key_bindings
            .contains_key(&KeyStroke::character('g'))
    );
    assert!(
        !resolved
            .key_bindings
            .contains_key(&KeyStroke::character('l'))
    );
    assert!(
        runtime
            .world()
            .resource::<OperationResolutionDiagnostics>()
            .last_error
            .is_none()
    );

    let conflicting = operation("local.conflict", "local.command", 'g', TargetSource::None);
    runtime.register_operation(conflicting.clone()).unwrap();
    let template_id = runtime
        .world()
        .get::<pitui_data::DatasetTemplateRef>(dataset)
        .unwrap()
        .0
        .clone();
    runtime
        .world_mut()
        .resource_mut::<pitui_data::DatasetTemplateRegistry>()
        .templates
        .get_mut(&template_id)
        .unwrap()
        .operations = vec![conflicting.id];
    // Give the conflicting local command a distinct name so global name
    // priority no longer removes it before key validation.
    runtime
        .world_mut()
        .resource_mut::<pitui_data::CommandRegistry>()
        .commands
        .get_mut(&CommandId::from("local.command"))
        .unwrap()
        .name = "different".into();
    runtime.run_schedule();
    assert!(matches!(
        runtime
            .world()
            .resource::<OperationResolutionDiagnostics>()
            .last_error
            .as_ref(),
        Some(OperationResolutionError::DuplicateKeySequence { .. })
    ));
    assert_eq!(
        runtime
            .world()
            .resource::<ResolvedOperationSet>()
            .commands
            .get("same"),
        Some(&OperationId::from("global.same")),
        "a failed resolve must retain the last valid effective set"
    );
}

#[derive(Resource, Default)]
struct CapturedInvocations(Vec<CommandInvocation>);

fn capture_invocation(
    In(invocation): In<CommandInvocation>,
    mut captured: bevy_ecs::prelude::ResMut<CapturedInvocations>,
) -> CommandExecution {
    captured.0.push(invocation);
    CommandExecution::Completed
}

#[test]
fn command_palette_and_key_input_emit_the_same_ordered_invocation_data() {
    let mut runtime = DatasetRuntime::new();
    let operation_id = OperationId::from("capture.selection");
    let commits_template = register_template(
        &mut runtime,
        "commits",
        DatasetKind::Commits,
        vec![operation_id.clone()],
    );
    let commit_template =
        register_template(&mut runtime, "commit", DatasetKind::Commit, Vec::new());
    runtime
        .register_availability_rule(AvailabilityRuleId::from("always"), AvailabilityRule::Always)
        .unwrap();
    runtime
        .register_command(command(
            "capture",
            "capture",
            "capture.system",
            CommandScope::Dataset,
        ))
        .unwrap();
    runtime
        .register_operation(operation(
            "capture.selection",
            "capture",
            'x',
            TargetSource::Selection,
        ))
        .unwrap();
    runtime
        .world_mut()
        .insert_resource(CapturedInvocations::default());
    runtime
        .register_command_system(CommandSystemId::from("capture.system"), capture_invocation)
        .unwrap();

    let repository = pitui_data::RepositoryKey::new("/repo");
    let commits = runtime
        .ensure_dataset(
            DatasetIdentity::Commits {
                repository: repository.clone(),
                branch: pitui_core::BranchName("main".into()),
            },
            DatasetKind::Commits,
            commits_template,
        )
        .unwrap();
    let first = runtime
        .ensure_dataset(
            DatasetIdentity::Commit {
                repository: repository.clone(),
                hash: pitui_core::CommitHash("first".into()),
            },
            DatasetKind::Commit,
            commit_template.clone(),
        )
        .unwrap();
    let second = runtime
        .ensure_dataset(
            DatasetIdentity::Commit {
                repository,
                hash: pitui_core::CommitHash("second".into()),
            },
            DatasetKind::Commit,
            commit_template,
        )
        .unwrap();
    runtime
        .replace_children(commits, vec![first, second], true)
        .unwrap();
    runtime.add_root(commits).unwrap();
    // Selection insertion order is intentionally reversed; Reconcile must
    // normalize it to the Dataset's business/display order.
    runtime.set_selection(commits, vec![second, first]).unwrap();
    initialize_single_panel(&mut runtime, commits);
    runtime.run_schedule();

    runtime.enqueue_input_intent(InputIntent::Key(KeyStroke::character('x')));
    runtime.run_schedule();
    runtime.enqueue_input_intent(InputIntent::CommandLine("capture".into()));
    runtime.run_schedule();

    let captured = &runtime.world().resource::<CapturedInvocations>().0;
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].targets, vec![first, second]);
    assert_eq!(captured[1].targets, vec![first, second]);
    assert_eq!(captured[0].source, InvocationSource::KeyBinding);
    assert_eq!(captured[1].source, InvocationSource::CommandPalette);
    assert_eq!(runtime.world().resource::<CommandExecutionLog>().0.len(), 2);

    runtime.enqueue_input_intent(InputIntent::CommandLine("capture extra".into()));
    runtime.enqueue_input_intent(InputIntent::CommandLine("missing".into()));
    runtime.enqueue_input_intent(InputIntent::Key(KeyStroke::plain(KeyCode::Escape)));
    runtime.run_schedule();
    let notices = &runtime.world().resource::<OperationNotices>().0;
    assert!(notices.iter().any(|notice| matches!(
        notice,
        OperationNotice::CommandArgumentsUnsupported(command) if command == "capture"
    )));
    assert!(notices.iter().any(|notice| matches!(
        notice,
        OperationNotice::UnknownCommand(command) if command == "missing"
    )));
}

#[test]
fn a_chord_prefix_replaces_the_single_active_key_map() {
    let mut runtime = DatasetRuntime::new();
    let operations = [
        OperationId::from("copy.hash"),
        OperationId::from("copy.info"),
    ];
    let template = register_template(
        &mut runtime,
        "commits",
        DatasetKind::Commits,
        operations.to_vec(),
    );
    runtime
        .register_availability_rule(AvailabilityRuleId::from("always"), AvailabilityRule::Always)
        .unwrap();
    for (id, name) in [("hash", "hash"), ("info", "info")] {
        runtime
            .register_command(command(id, name, id, CommandScope::Dataset))
            .unwrap();
    }
    for (id, command, suffix) in [("copy.hash", "hash", 'h'), ("copy.info", "info", 'i')] {
        runtime
            .register_operation(OperationSpec {
                id: OperationId::from(id),
                label: id.into(),
                command: CommandId::from(command),
                bindings: vec![KeySequence::chord([
                    KeyStroke::control('c'),
                    KeyStroke::character(suffix),
                ])],
                target_source: TargetSource::None,
                availability: AvailabilityRuleId::from("always"),
            })
            .unwrap();
    }
    let dataset = runtime
        .ensure_dataset(
            DatasetIdentity::Commits {
                repository: pitui_data::RepositoryKey::new("/repo"),
                branch: pitui_core::BranchName("main".into()),
            },
            DatasetKind::Commits,
            template,
        )
        .unwrap();
    runtime.add_root(dataset).unwrap();
    initialize_single_panel(&mut runtime, dataset);
    runtime.run_schedule();

    let base = runtime.world().resource::<ResolvedOperationSet>();
    assert_eq!(base.key_bindings.len(), 1);
    assert!(matches!(
        base.key_bindings
            .get(&KeyStroke::control('c'))
            .map(|binding| &binding.action),
        Some(ResolvedKeyAction::EnterChord(_))
    ));

    runtime.enqueue_input_intent(InputIntent::Key(KeyStroke::control('c')));
    runtime.run_schedule();
    let second = runtime.world().resource::<ResolvedOperationSet>();
    assert_eq!(
        second
            .key_bindings
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from([KeyStroke::character('h'), KeyStroke::character('i')])
    );
    assert!(!second.key_bindings.contains_key(&KeyStroke::control('c')));
    assert!(second.commands.is_empty());
}
