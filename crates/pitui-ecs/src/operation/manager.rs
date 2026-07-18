//! Operation Manager: direct `OperationId -> ECS System` registration and
//! invocation. Command metadata is deliberately not part of function lookup.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationExecution {
    Completed,
    Rejected(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationSystemRegistrationError {
    DuplicateSystem(OperationId),
}

/// Runtime function table. Dataset Templates bind `OperationId` values; this
/// manager binds those values directly to registered Bevy ECS Systems.
#[derive(Resource, Default)]
pub struct OperationManager(
    HashMap<OperationId, SystemId<In<OperationInvocation>, OperationExecution>>,
);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationExecutionLog(pub Vec<(OperationInvocation, OperationExecution)>);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct OperationNotices(pub Vec<OperationNotice>);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct ClipboardRequests(pub Vec<ClipboardRequest>);

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct PendingInteractionNotices(pub VecDeque<InteractionNoticeRequest>);

#[derive(Resource, Clone, Debug, Default)]
pub struct PendingOperationInvocations(pub(crate) Vec<OperationInvocation>);

pub fn register_operation_system<M, S>(
    world: &mut World,
    id: OperationId,
    system: S,
) -> Result<(), OperationSystemRegistrationError>
where
    S: IntoSystem<In<OperationInvocation>, OperationExecution, M> + 'static,
    M: 'static,
{
    if world.resource::<OperationManager>().0.contains_key(&id) {
        return Err(OperationSystemRegistrationError::DuplicateSystem(id));
    }
    let system_id = world.register_system(system);
    world
        .resource_mut::<OperationManager>()
        .0
        .insert(id, system_id);
    Ok(())
}

pub fn operation_system_registered(world: &World, id: &OperationId) -> bool {
    world.resource::<OperationManager>().0.contains_key(id)
}

/// Registers every built-in operation function in one place. Dataset
/// Templates choose which IDs are effective; this table only supplies the ECS
/// System implementation for each ID.
pub fn register_builtin_operation_systems(
    world: &mut World,
) -> Result<(), OperationSystemRegistrationError> {
    macro_rules! register {
        ($id:literal, $system:path) => {
            register_operation_system(world, OperationId::from($id), $system)?
        };
    }

    register!("global.quit", super::systems::request_quit);
    register!("global.help", super::systems::open_help);
    register!("global.refresh", super::systems::refresh_active_context);
    register!("global.changes", super::systems::open_changes);
    register!(
        "global.command-palette",
        super::systems::open_command_palette
    );
    register!("global.back", super::systems::navigate_back);
    register!("global.reflog", super::systems::open_reflog);
    register!("global.logs", super::systems::open_git_operation_log);
    for id in ["remotes", "fetch", "pull", "push", "sync"] {
        register_operation_system(
            world,
            OperationId::from(format!("global.{id}")),
            super::systems::reject_unimplemented,
        )?;
    }
    register!("interaction.help.close", super::systems::close_interaction);
    register!(
        "interaction.palette.close",
        super::systems::close_interaction
    );
    register!("interaction.palette.up", super::systems::palette_up);
    register!("interaction.palette.down", super::systems::palette_down);
    register!(
        "interaction.palette.submit",
        super::systems::submit_palette_command
    );
    register!("interaction.text.close", super::systems::close_interaction);
    register!("interaction.text.submit", super::systems::submit_text_input);
    register!(
        "interaction.notice.close",
        super::systems::close_interaction
    );
    register!("commit-creation.help", super::systems::open_help);
    register!("commit-creation.cancel", super::systems::navigate_back);
    register!(
        "commit-creation.submit",
        super::systems::submit_commit_creation
    );
    register!("active.up", super::systems::activate_previous_element);
    register!("active.down", super::systems::activate_next_element);
    register!("active.left", super::systems::transfer_active_left);
    register!("active.right", super::systems::transfer_active_right);
    register!("selection.toggle", super::systems::toggle_selection);
    register!(
        "collection.view.next",
        super::systems::cycle_collection_view
    );
    register!("commits.cherry-pick", super::systems::cherry_pick_selected);
    register!(
        "changes.selection.toggle",
        super::systems::toggle_changes_selection
    );
    register!("changes.stage", super::systems::stage_changes);
    register!("changes.unstage", super::systems::unstage_changes);
    register!("changes.commit", super::systems::open_commit_creation);
    register!("copy.commit.hash", super::systems::copy_commit_hashes);
    register!("copy.commit.info", super::systems::copy_commit_info);
    register!("copy.commit.message", super::systems::copy_commit_message);
    register!(
        "copy.commit-field.values",
        super::systems::copy_commit_field_values
    );
    register!("copy.reflog.hash", super::systems::copy_reflog_hash);
    register!("copy.file.name", super::systems::copy_file_name);
    register!(
        "copy.file.absolute",
        super::systems::copy_file_absolute_path
    );
    register!(
        "copy.file.relative",
        super::systems::copy_file_relative_path
    );
    register!("scroll.home", super::systems::scroll_home);
    register!("scroll.end", super::systems::scroll_end);
    register!("scroll.page-up", super::systems::scroll_page_up);
    register!("scroll.page-down", super::systems::scroll_page_down);
    Ok(())
}

pub fn dispatch_pending_operations(world: &mut World) {
    let invocations = std::mem::take(&mut world.resource_mut::<PendingOperationInvocations>().0);
    for invocation in invocations {
        let operation = world
            .resource::<OperationRegistry>()
            .get(&invocation.operation)
            .cloned();
        let Some(operation) = operation else {
            write_notice(
                world,
                OperationNotice::OperationSystemUnavailable(invocation.operation.clone()),
            );
            continue;
        };
        if operation.command != invocation.command {
            write_notice(
                world,
                OperationNotice::OperationSystemUnavailable(invocation.operation.clone()),
            );
            continue;
        }
        let system = world
            .resource::<OperationManager>()
            .0
            .get(&invocation.operation)
            .copied();
        let Some(system) = system else {
            write_notice(
                world,
                OperationNotice::OperationSystemUnavailable(invocation.operation.clone()),
            );
            continue;
        };
        let execution = match world.run_system_with(system, invocation.clone()) {
            Ok(execution) => execution,
            Err(error) => OperationExecution::Rejected(error.to_string()),
        };
        if let OperationExecution::Rejected(message) = &execution {
            write_notice(
                world,
                OperationNotice::OperationRejected {
                    operation: invocation.operation.clone(),
                    message: message.clone(),
                },
            );
        }
        world
            .resource_mut::<OperationExecutionLog>()
            .0
            .push((invocation, execution));
    }
}

pub fn collect_operation_notices(
    mut notices: MessageReader<OperationNotice>,
    mut collected: ResMut<OperationNotices>,
) {
    collected.0.extend(notices.read().cloned());
}

pub fn collect_clipboard_requests(
    mut requests: MessageReader<ClipboardRequest>,
    mut collected: ResMut<ClipboardRequests>,
) {
    collected.0.extend(requests.read().cloned());
}

pub fn collect_interaction_notice_requests(
    mut requests: MessageReader<InteractionNoticeRequest>,
    mut pending: ResMut<PendingInteractionNotices>,
) {
    pending.0.extend(requests.read().cloned());
}

/// Presents at most one queued Notice through the global Interaction Context.
/// It runs after ordinary Context transitions so a failed command submitted by
/// a TextInput first restores its previous view and then overlays the error.
pub fn present_next_interaction_notice(world: &mut World) {
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
