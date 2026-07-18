//! Dataset ECS kernel for Pitui.
//!
//! The kernel owns lifecycle and invariant enforcement. Operations feed data
//! into this boundary; renderers never receive a mutable [`World`].

#![forbid(unsafe_code)]

use std::{collections::HashSet, error::Error, fmt, sync::Arc};

use bevy_ecs::{
    prelude::{Entity, In, IntoScheduleConfigs, IntoSystem, Resource, Schedule, SystemSet, World},
    schedule::ScheduleLabel,
};
use pitui_data::{
    ActiveHandoffRegistry, ActiveRenderMode, ActiveUiContext, AvailabilityRegistryError,
    AvailabilityRule, AvailabilityRuleId, AvailabilityRuleRegistry, CollectionElement,
    CollectionManagerSpec, CommandId, CommandInvocation, CommandRegistry, CommandRegistryError,
    CommandSpec, CommandSystemId, ContextStack, Dataset, DatasetActiveElement, DatasetBinding,
    DatasetBundle, DatasetChildren, DatasetCollection, DatasetIdentity, DatasetIndex, DatasetKey,
    DatasetKind, DatasetRevision, DatasetRoots, DatasetSelection, DatasetTemplate,
    DatasetTemplateId, DatasetTemplateRef, DatasetTemplateRegistry, DatasetType,
    DefaultDatasetTemplates, GlobalOperationSet, HasSnapshot, InputIntent, OperationId,
    OperationRegistry, OperationRegistryError, OperationSpec, PendingChordState, QuitRequested,
    RenderContextBindings, RenderLayout, RenderModeId, RenderModeRegistry, RenderModeSpec,
    RenderProxyId, RenderProxyRegistry, RenderProxySpec, RenderRegistryError, ResolvedOperationSet,
    ResolvedRenderLayout, UiContextSnapshot, UiFrame, ViewportMeasurement,
};

mod binding_reconcile;
mod collection;
mod git_runtime;
mod operation_runtime;
mod projection;

pub use binding_reconcile::RenderReconcileDiagnostics;
pub use git_runtime::{
    GitCommandData, GitDataError, GitExecutionFailure, GitExecutionFailures, GitExecutorResource,
    GitMutationSuccess, GitMutationSuccesses, GitOperationLogSinkResource, GitResultData,
};
pub use operation_runtime::{
    ClipboardRequests, CommandExecution, CommandExecutionLog, CommandSystemRegistrationError,
    OperationNotices, OperationResolutionDiagnostics, OperationResolutionError,
    PendingInteractionNotices,
};
pub use projection::{ProjectionDiagnostic, ProjectionDiagnostics};

#[derive(ScheduleLabel, Clone, Debug, Eq, Hash, PartialEq)]
pub struct PituiSchedule;

#[derive(SystemSet, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RuntimeSet {
    Ingress,
    Resolve,
    Execute,
    Reconcile,
    Projection,
    Present,
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeDiagnostics {
    pub invariant_violations: Vec<InvariantViolation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KernelError {
    MissingTemplate(DatasetTemplateId),
    TemplateKindMismatch {
        template: DatasetTemplateId,
        expected: DatasetKind,
        actual: DatasetKind,
    },
    IdentityKindMismatch {
        identity: Box<DatasetIdentity>,
        expected: DatasetKind,
        actual: DatasetKind,
    },
    IdentityTemplateMismatch {
        identity: Box<DatasetIdentity>,
        expected: DatasetTemplateId,
        actual: DatasetTemplateId,
    },
    MissingDataset(Entity),
    DuplicateChild(Entity),
    Cycle {
        parent: Entity,
        child: Entity,
    },
    ActiveElementOutsideDataset {
        dataset: Entity,
        element: Entity,
    },
    SelectionOutsideDataset {
        dataset: Entity,
        selected: Entity,
    },
    ActiveRelaySourceChanged {
        expected: Entity,
        actual: Entity,
    },
    ActiveRelayKindMismatch {
        dataset: Entity,
        expected: DatasetKind,
        actual: DatasetKind,
    },
    ActiveDatasetNotActivatable(Entity),
    ContextAlreadyInitialized,
    ContextUnavailable,
    MissingRenderMode(RenderModeId),
    MissingStableRenderDataset(Box<DatasetIdentity>),
    MissingRenderProxy(pitui_data::RenderProxyId),
    RenderProxyKindMismatch {
        proxy: pitui_data::RenderProxyId,
        expected: DatasetKind,
        actual: DatasetKind,
    },
}

impl fmt::Display for KernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl Error for KernelError {}

/// Cross-registry contract failures detected after configuration is loaded and
/// command Systems are registered, but before any Dataset or terminal state is
/// created. This is the extension boundary for every future semantic Dataset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistrationContractError {
    MissingDefaultTemplate(DatasetKind),
    MissingDefaultTemplateDefinition {
        kind: DatasetKind,
        template: DatasetTemplateId,
    },
    DefaultTemplateKindMismatch {
        kind: DatasetKind,
        template: DatasetTemplateId,
        actual: DatasetKind,
    },
    DatasetTemplateHasNoRenderProxy(DatasetTemplateId),
    DuplicateTemplateOperation {
        template: DatasetTemplateId,
        operation: OperationId,
    },
    DuplicateTemplateProxy {
        template: DatasetTemplateId,
        proxy: RenderProxyId,
    },
    DuplicateTreeVisibleKind {
        template: DatasetTemplateId,
        kind: DatasetKind,
    },
    DuplicateTreeSelectableKind {
        template: DatasetTemplateId,
        kind: DatasetKind,
    },
    TreeSelectableKindNotVisible {
        template: DatasetTemplateId,
        kind: DatasetKind,
    },
    MissingTemplateOperation {
        template: DatasetTemplateId,
        operation: OperationId,
    },
    MissingTemplateProxy {
        template: DatasetTemplateId,
        proxy: RenderProxyId,
    },
    TemplateProxyKindMismatch {
        template: DatasetTemplateId,
        proxy: RenderProxyId,
        expected: DatasetKind,
        actual: DatasetKind,
    },
    MissingGlobalOperation(OperationId),
    DuplicateGlobalOperation(OperationId),
    OperationMissingCommand {
        operation: OperationId,
        command: CommandId,
    },
    OperationMissingAvailability {
        operation: OperationId,
        availability: AvailabilityRuleId,
    },
    CommandSystemMissing {
        command: CommandId,
        system: CommandSystemId,
    },
    RenderModeMissingProxy {
        mode: RenderModeId,
        proxy: RenderProxyId,
    },
    StableRenderProxyKindMismatch {
        mode: RenderModeId,
        identity: Box<DatasetIdentity>,
        proxy: RenderProxyId,
        expected: DatasetKind,
        actual: DatasetKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvariantViolation {
    IndexPointsToMissingEntity(DatasetIdentity),
    IndexKeyMismatch(DatasetIdentity),
    DatasetMissingFromIndex(DatasetIdentity),
    IdentityKindMismatch {
        identity: DatasetIdentity,
        actual: DatasetKind,
    },
    DanglingChild {
        parent: Entity,
        child: Entity,
    },
    DanglingCollectionElement {
        dataset: Entity,
        target: Entity,
    },
    ActiveElementOutsideCollection {
        dataset: Entity,
        element: Entity,
    },
    SelectionOutsideCollection {
        dataset: Entity,
        selected: Entity,
    },
    InvalidCollection(Entity),
    DatasetCycle(Entity),
    DanglingRoot(Entity),
    DanglingActiveDataset(Entity),
    DanglingRenderBinding(Entity),
    DanglingContextEntity(Entity),
    ActiveDatasetNotActivatable(Entity),
}

/// Runtime owner for the Data Driven ECS world.
pub struct DatasetRuntime {
    world: World,
    schedule: Schedule,
}

impl Default for DatasetRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl DatasetRuntime {
    pub fn new() -> Self {
        Self::with_git_executor(Arc::new(pitui_git::CliGitExecutor))
    }

    pub fn with_git_executor(executor: Arc<dyn pitui_git::GitExecutor>) -> Self {
        Self::with_git_executor_and_log_sink(
            executor,
            Arc::new(pitui_git::logging::NoopGitOperationLogSink),
        )
    }

    pub fn with_git_executor_and_log_sink(
        executor: Arc<dyn pitui_git::GitExecutor>,
        log_sink: Arc<dyn pitui_git::logging::GitOperationLogSink>,
    ) -> Self {
        let mut world = World::new();
        world.init_resource::<DatasetIndex>();
        world.init_resource::<DatasetRoots>();
        world.init_resource::<DatasetTemplateRegistry>();
        world.init_resource::<DefaultDatasetTemplates>();
        world.init_resource::<RenderProxyRegistry>();
        world.init_resource::<RenderModeRegistry>();
        world.init_resource::<ActiveHandoffRegistry>();
        world.init_resource::<UiFrame>();
        world.init_resource::<ProjectionDiagnostics>();
        world.init_resource::<bevy_ecs::prelude::Messages<ViewportMeasurement>>();
        world.init_resource::<ContextStack>();
        world.init_resource::<PendingChordState>();
        world.init_resource::<RuntimeDiagnostics>();
        world.init_resource::<RenderReconcileDiagnostics>();
        binding_reconcile::initialize_binding_reconcile(&mut world);
        git_runtime::initialize_git_runtime(&mut world, executor, log_sink);
        operation_runtime::initialize_operation_runtime(&mut world);

        let mut schedule = Schedule::new(PituiSchedule);
        schedule.configure_sets(
            (
                RuntimeSet::Ingress,
                RuntimeSet::Resolve,
                RuntimeSet::Execute,
                RuntimeSet::Reconcile,
                RuntimeSet::Projection,
                RuntimeSet::Present,
            )
                .chain(),
        );
        schedule.add_systems(
            (
                operation_runtime::apply_text_edits,
                operation_runtime::collect_command_invocations,
                operation_runtime::dispatch_pending_commands,
                binding_reconcile::update_dependent_render_bindings,
                git_runtime::enqueue_dependent_reads,
                git_runtime::execute_git_commands,
                git_runtime::collect_git_results,
                git_runtime::apply_pending_git_results,
                operation_runtime::collect_clipboard_requests,
                operation_runtime::collect_operation_notices,
                operation_runtime::collect_interaction_notice_requests,
            )
                .chain()
                .in_set(RuntimeSet::Execute),
        );
        schedule.add_systems(
            (
                operation_runtime::release_deferred_invocations,
                operation_runtime::resolve_input_intents,
            )
                .chain()
                .in_set(RuntimeSet::Resolve),
        );
        schedule.add_systems(projection::apply_viewport_measurements.in_set(RuntimeSet::Ingress));
        schedule.add_systems(projection::build_ui_frame.in_set(RuntimeSet::Projection));
        schedule.add_systems(
            (
                collection::rebuild_collections,
                collection::repair_active_elements,
                operation_runtime::reconcile_pending_changes_active,
                binding_reconcile::collect_context_transitions,
                binding_reconcile::apply_context_transitions,
                binding_reconcile::update_dependent_render_bindings,
                binding_reconcile::resolve_active_render_mode,
                operation_runtime::present_next_interaction_notice,
                operation_runtime::resolve_active_operation_set,
                projection::update_dataset_viewports,
                collect_unreachable_datasets,
            )
                .chain()
                .in_set(RuntimeSet::Reconcile),
        );

        Self { world, schedule }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Mutable World access is kept at the composition/testing boundary. UI
    /// renderers must consume a projection instead of calling this method.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn register_template(
        &mut self,
        template: DatasetTemplate,
    ) -> Result<(), Box<DatasetTemplate>> {
        self.world
            .resource_mut::<DatasetTemplateRegistry>()
            .register(template)
    }

    pub fn register_default_template(
        &mut self,
        template: DatasetTemplate,
    ) -> Result<(), Box<DatasetTemplate>> {
        let id = template.id.clone();
        let kind = template.kind;
        if self
            .world
            .resource::<DefaultDatasetTemplates>()
            .get(kind)
            .is_some()
        {
            return Err(Box::new(template));
        }
        self.register_template(template)?;
        self.world
            .resource_mut::<DefaultDatasetTemplates>()
            .bind(kind, id);
        Ok(())
    }

    pub fn register_render_proxy(
        &mut self,
        spec: RenderProxySpec,
    ) -> Result<(), RenderRegistryError> {
        self.world
            .resource_mut::<RenderProxyRegistry>()
            .register(spec)
    }

    pub fn register_render_mode(
        &mut self,
        spec: RenderModeSpec,
    ) -> Result<(), RenderRegistryError> {
        self.world
            .resource_mut::<RenderModeRegistry>()
            .register(spec)
    }

    pub fn register_command(&mut self, spec: CommandSpec) -> Result<(), CommandRegistryError> {
        self.world.resource_mut::<CommandRegistry>().register(spec)
    }

    pub fn register_operation(
        &mut self,
        spec: OperationSpec,
    ) -> Result<(), OperationRegistryError> {
        self.world
            .resource_mut::<OperationRegistry>()
            .register(spec)
    }

    pub fn register_availability_rule(
        &mut self,
        id: AvailabilityRuleId,
        rule: AvailabilityRule,
    ) -> Result<(), AvailabilityRegistryError> {
        self.world
            .resource_mut::<AvailabilityRuleRegistry>()
            .register(id, rule)
    }

    pub fn set_global_operations(&mut self, operations: Vec<OperationId>) {
        self.world.resource_mut::<GlobalOperationSet>().0 = operations;
    }

    pub fn set_active_handoffs(&mut self, modes: ActiveHandoffRegistry) {
        self.world.insert_resource(modes);
    }

    /// Validates the complete Dataset/Proxy/Operation/Command graph. Call this
    /// once after composing effective configuration and registering Systems;
    /// failures are deterministic startup errors rather than latent input-time
    /// surprises.
    pub fn validate_registration_contracts(&self) -> Vec<RegistrationContractError> {
        let templates = self.world.resource::<DatasetTemplateRegistry>();
        let defaults = self.world.resource::<DefaultDatasetTemplates>();
        let proxies = self.world.resource::<RenderProxyRegistry>();
        let modes = self.world.resource::<RenderModeRegistry>();
        let commands = self.world.resource::<CommandRegistry>();
        let operations = self.world.resource::<OperationRegistry>();
        let availability = self.world.resource::<AvailabilityRuleRegistry>();
        let global = self.world.resource::<GlobalOperationSet>();
        let mut errors = Vec::new();

        for kind in DatasetKind::ALL {
            let Some(template_id) = defaults.get(kind) else {
                errors.push(RegistrationContractError::MissingDefaultTemplate(kind));
                continue;
            };
            let Some(template) = templates.get(template_id) else {
                errors.push(
                    RegistrationContractError::MissingDefaultTemplateDefinition {
                        kind,
                        template: template_id.clone(),
                    },
                );
                continue;
            };
            if template.kind != kind {
                errors.push(RegistrationContractError::DefaultTemplateKindMismatch {
                    kind,
                    template: template_id.clone(),
                    actual: template.kind,
                });
            }
        }

        for template in templates.templates.values() {
            if template.render_proxies.is_empty() {
                errors.push(RegistrationContractError::DatasetTemplateHasNoRenderProxy(
                    template.id.clone(),
                ));
            }
            if let CollectionManagerSpec::Tree(tree) = &template.collection {
                let mut visible = HashSet::new();
                for kind in &tree.visible_kinds {
                    if !visible.insert(*kind) {
                        errors.push(RegistrationContractError::DuplicateTreeVisibleKind {
                            template: template.id.clone(),
                            kind: *kind,
                        });
                    }
                }
                let mut selectable = HashSet::new();
                for kind in &tree.selectable_kinds {
                    if !selectable.insert(*kind) {
                        errors.push(RegistrationContractError::DuplicateTreeSelectableKind {
                            template: template.id.clone(),
                            kind: *kind,
                        });
                    }
                    if !visible.contains(kind) {
                        errors.push(RegistrationContractError::TreeSelectableKindNotVisible {
                            template: template.id.clone(),
                            kind: *kind,
                        });
                    }
                }
            }
            let mut seen_operations = HashSet::new();
            for operation_id in &template.operations {
                if !seen_operations.insert(operation_id) {
                    errors.push(RegistrationContractError::DuplicateTemplateOperation {
                        template: template.id.clone(),
                        operation: operation_id.clone(),
                    });
                }
                if operations.get(operation_id).is_none() {
                    errors.push(RegistrationContractError::MissingTemplateOperation {
                        template: template.id.clone(),
                        operation: operation_id.clone(),
                    });
                }
            }
            let mut seen_proxies = HashSet::new();
            for proxy_id in &template.render_proxies {
                if !seen_proxies.insert(proxy_id) {
                    errors.push(RegistrationContractError::DuplicateTemplateProxy {
                        template: template.id.clone(),
                        proxy: proxy_id.clone(),
                    });
                }
                let Some(proxy) = proxies.get(proxy_id) else {
                    errors.push(RegistrationContractError::MissingTemplateProxy {
                        template: template.id.clone(),
                        proxy: proxy_id.clone(),
                    });
                    continue;
                };
                if proxy.dataset_kind != template.kind {
                    errors.push(RegistrationContractError::TemplateProxyKindMismatch {
                        template: template.id.clone(),
                        proxy: proxy_id.clone(),
                        expected: template.kind,
                        actual: proxy.dataset_kind,
                    });
                }
            }
        }

        let mut seen_global = HashSet::new();
        for operation_id in &global.0 {
            if !seen_global.insert(operation_id) {
                errors.push(RegistrationContractError::DuplicateGlobalOperation(
                    operation_id.clone(),
                ));
            }
            if operations.get(operation_id).is_none() {
                errors.push(RegistrationContractError::MissingGlobalOperation(
                    operation_id.clone(),
                ));
            }
        }
        for operation in operations.operations.values() {
            if commands.get(&operation.command).is_none() {
                errors.push(RegistrationContractError::OperationMissingCommand {
                    operation: operation.id.clone(),
                    command: operation.command.clone(),
                });
            }
            if availability.get(&operation.availability).is_none() {
                errors.push(RegistrationContractError::OperationMissingAvailability {
                    operation: operation.id.clone(),
                    availability: operation.availability.clone(),
                });
            }
        }
        for command in commands.commands.values() {
            if !operation_runtime::command_system_registered(&self.world, &command.system) {
                errors.push(RegistrationContractError::CommandSystemMissing {
                    command: command.id.clone(),
                    system: command.system.clone(),
                });
            }
        }
        for mode in modes.modes.values() {
            validate_render_layout_contract(&mode.id, &mode.layout, proxies, &mut errors);
        }

        errors
    }

    pub fn register_command_system<M, S>(
        &mut self,
        id: CommandSystemId,
        system: S,
    ) -> Result<(), CommandSystemRegistrationError>
    where
        S: IntoSystem<In<CommandInvocation>, CommandExecution, M> + 'static,
        M: 'static,
    {
        operation_runtime::register_command_system(&mut self.world, id, system)
    }

    pub fn register_builtin_interaction_systems(
        &mut self,
    ) -> Result<(), CommandSystemRegistrationError> {
        self.register_command_system(
            CommandSystemId::from("quit"),
            operation_runtime::request_quit,
        )?;
        self.register_command_system(CommandSystemId::from("help"), operation_runtime::open_help)?;
        self.register_command_system(
            CommandSystemId::from("command-palette"),
            operation_runtime::open_command_palette,
        )?;
        self.register_command_system(
            CommandSystemId::from("changes"),
            operation_runtime::open_changes,
        )?;
        self.register_command_system(
            CommandSystemId::from("reflog"),
            operation_runtime::open_reflog,
        )?;
        self.register_command_system(
            CommandSystemId::from("logs"),
            operation_runtime::open_git_operation_log,
        )?;
        for id in ["remotes", "fetch", "pull", "push", "sync"] {
            self.register_command_system(
                CommandSystemId::from(id),
                operation_runtime::reject_unimplemented,
            )?;
        }
        self.register_command_system(
            CommandSystemId::from("refresh"),
            operation_runtime::refresh_active_context,
        )?;
        self.register_command_system(
            CommandSystemId::from("interaction.close"),
            operation_runtime::close_interaction,
        )?;
        self.register_command_system(
            CommandSystemId::from("palette.up"),
            operation_runtime::palette_up,
        )?;
        self.register_command_system(
            CommandSystemId::from("palette.down"),
            operation_runtime::palette_down,
        )?;
        self.register_command_system(
            CommandSystemId::from("palette.submit"),
            operation_runtime::submit_palette_command,
        )?;
        self.register_command_system(
            CommandSystemId::from("active.up"),
            operation_runtime::activate_previous_element,
        )?;
        self.register_command_system(
            CommandSystemId::from("active.down"),
            operation_runtime::activate_next_element,
        )?;
        self.register_command_system(
            CommandSystemId::from("active.left"),
            operation_runtime::transfer_active_left,
        )?;
        self.register_command_system(
            CommandSystemId::from("active.right"),
            operation_runtime::transfer_active_right,
        )?;
        self.register_command_system(
            CommandSystemId::from("selection.toggle"),
            operation_runtime::toggle_selection,
        )?;
        self.register_command_system(
            CommandSystemId::from("commits.cherry-pick"),
            operation_runtime::cherry_pick_selected,
        )?;
        self.register_command_system(
            CommandSystemId::from("changes.selection.toggle"),
            operation_runtime::toggle_changes_selection,
        )?;
        self.register_command_system(
            CommandSystemId::from("changes.stage"),
            operation_runtime::stage_changes,
        )?;
        self.register_command_system(
            CommandSystemId::from("changes.unstage"),
            operation_runtime::unstage_changes,
        )?;
        self.register_command_system(
            CommandSystemId::from("changes.commit"),
            operation_runtime::open_commit_creation,
        )?;
        self.register_command_system(
            CommandSystemId::from("commit-creation.cancel"),
            operation_runtime::navigate_back,
        )?;
        self.register_command_system(
            CommandSystemId::from("commit-creation.submit"),
            operation_runtime::submit_commit_creation,
        )?;
        self.register_command_system(
            CommandSystemId::from("text.submit"),
            operation_runtime::submit_text_input,
        )?;
        self.register_command_system(
            CommandSystemId::from("back"),
            operation_runtime::navigate_back,
        )?;
        self.register_command_system(
            CommandSystemId::from("copy.commit.hash"),
            operation_runtime::copy_commit_hashes,
        )?;
        self.register_command_system(
            CommandSystemId::from("copy.commit.info"),
            operation_runtime::copy_commit_info,
        )?;
        self.register_command_system(
            CommandSystemId::from("copy.commit.message"),
            operation_runtime::copy_commit_message,
        )?;
        self.register_command_system(
            CommandSystemId::from("copy.reflog.hash"),
            operation_runtime::copy_reflog_hash,
        )?;
        self.register_command_system(
            CommandSystemId::from("copy.file.name"),
            operation_runtime::copy_file_name,
        )?;
        self.register_command_system(
            CommandSystemId::from("copy.file.absolute"),
            operation_runtime::copy_file_absolute_path,
        )?;
        self.register_command_system(
            CommandSystemId::from("copy.file.relative"),
            operation_runtime::copy_file_relative_path,
        )?;
        self.register_command_system(
            CommandSystemId::from("scroll.home"),
            operation_runtime::scroll_home,
        )?;
        self.register_command_system(
            CommandSystemId::from("scroll.end"),
            operation_runtime::scroll_end,
        )?;
        self.register_command_system(
            CommandSystemId::from("scroll.page-up"),
            operation_runtime::scroll_page_up,
        )?;
        self.register_command_system(
            CommandSystemId::from("scroll.page-down"),
            operation_runtime::scroll_page_down,
        )
    }

    pub fn enqueue_input_intent(&mut self, intent: InputIntent) {
        self.world
            .resource_mut::<bevy_ecs::prelude::Messages<InputIntent>>()
            .write(intent);
    }

    pub fn enqueue_interaction_notice(&mut self, request: pitui_data::InteractionNoticeRequest) {
        self.world
            .resource_mut::<bevy_ecs::prelude::Messages<pitui_data::InteractionNoticeRequest>>()
            .write(request);
    }

    pub fn enqueue_viewport_measurement(&mut self, measurement: ViewportMeasurement) {
        self.world
            .resource_mut::<bevy_ecs::prelude::Messages<ViewportMeasurement>>()
            .write(measurement);
    }

    pub fn ui_frame(&self) -> &UiFrame {
        self.world.resource::<UiFrame>()
    }

    pub fn quit_requested(&self) -> bool {
        self.world.resource::<QuitRequested>().0
    }

    pub fn take_clipboard_requests(&mut self) -> Vec<pitui_data::ClipboardRequest> {
        std::mem::take(&mut self.world.resource_mut::<ClipboardRequests>().0)
    }

    pub fn resolve_render_mode(
        &self,
        mode: &RenderModeId,
        bindings: &RenderContextBindings,
    ) -> Result<ResolvedRenderLayout, KernelError> {
        let layout = self
            .world
            .resource::<RenderModeRegistry>()
            .get(mode)
            .ok_or_else(|| KernelError::MissingRenderMode(mode.clone()))?
            .layout
            .clone();
        resolve_render_layout(&self.world, &layout, bindings)
    }

    pub fn enqueue_git_command(&mut self, data: GitCommandData) -> Result<(), KernelError> {
        self.require_dataset(data.repository_dataset)?;
        self.world
            .resource_mut::<bevy_ecs::prelude::Messages<GitCommandData>>()
            .write(data);
        Ok(())
    }

    /// Returns the canonical Entity for an identity, creating it when needed.
    pub fn ensure_dataset(
        &mut self,
        identity: DatasetIdentity,
        kind: DatasetKind,
        template: DatasetTemplateId,
    ) -> Result<Entity, KernelError> {
        ensure_dataset_in_world(&mut self.world, identity, kind, template)
    }

    pub fn add_root(&mut self, entity: Entity) -> Result<(), KernelError> {
        self.require_dataset(entity)?;
        let mut roots = self.world.resource_mut::<DatasetRoots>();
        if !roots.0.contains(&entity) {
            roots.0.push(entity);
        }
        Ok(())
    }

    pub fn remove_root(&mut self, entity: Entity) {
        self.world
            .resource_mut::<DatasetRoots>()
            .0
            .retain(|root| *root != entity);
    }

    /// Transactionally replaces a Dataset's ordered child references. All
    /// validation happens before any component is changed.
    pub fn replace_children(
        &mut self,
        parent: Entity,
        children: Vec<Entity>,
        has_snapshot: bool,
    ) -> Result<(), KernelError> {
        replace_children_in_world(&mut self.world, parent, children, has_snapshot)
    }

    pub fn set_active_element(
        &mut self,
        dataset: Entity,
        element: Option<Entity>,
    ) -> Result<(), KernelError> {
        self.require_dataset(dataset)?;
        if let Some(element) = element
            && !self
                .world
                .get::<DatasetCollection>(dataset)
                .is_some_and(|collection| collection.contains(element))
        {
            return Err(KernelError::ActiveElementOutsideDataset { dataset, element });
        }
        self.world
            .entity_mut(dataset)
            .insert(DatasetActiveElement(element));
        Ok(())
    }

    pub fn set_selection(
        &mut self,
        dataset: Entity,
        selection: Vec<Entity>,
    ) -> Result<(), KernelError> {
        self.require_dataset(dataset)?;
        let collection = self
            .world
            .get::<DatasetCollection>(dataset)
            .expect("validated Dataset must have a collection");
        if let Some(selected) = selection
            .iter()
            .find(|selected| !collection.contains(**selected))
        {
            return Err(KernelError::SelectionOutsideDataset {
                dataset,
                selected: *selected,
            });
        }
        self.world
            .entity_mut(dataset)
            .insert(DatasetSelection(selection));
        Ok(())
    }

    pub fn initialize_ui_from_mode(
        &mut self,
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        let layout = self.resolve_render_mode(&render_mode, &render_bindings)?;
        self.initialize_ui(
            active_dataset,
            render_mode,
            render_bindings,
            layout,
            operations,
        )
    }

    pub fn replace_context_from_mode(
        &mut self,
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        let layout = self.resolve_render_mode(&render_mode, &render_bindings)?;
        self.replace_context(
            active_dataset,
            render_mode,
            render_bindings,
            layout,
            operations,
        )
    }

    pub fn push_context_from_mode(
        &mut self,
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        let layout = self.resolve_render_mode(&render_mode, &render_bindings)?;
        self.push_context(
            active_dataset,
            render_mode,
            render_bindings,
            layout,
            operations,
        )
    }

    pub fn pop_context_from_mode(
        &mut self,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        let snapshot = self
            .world
            .resource::<ContextStack>()
            .0
            .last()
            .cloned()
            .ok_or(KernelError::ContextUnavailable)?;
        let layout = self.resolve_render_mode(&snapshot.render_mode, &snapshot.render_bindings)?;
        self.pop_context(layout, operations)
    }

    pub fn initialize_ui(
        &mut self,
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
        layout: ResolvedRenderLayout,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        if self.world.contains_resource::<ActiveUiContext>() {
            return Err(KernelError::ContextAlreadyInitialized);
        }
        self.validate_ui_state(active_dataset, &render_bindings, &layout)?;
        self.apply_ui_state(
            active_dataset,
            render_mode,
            render_bindings,
            layout,
            operations,
            0,
        );
        Ok(())
    }

    pub fn replace_context(
        &mut self,
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
        layout: ResolvedRenderLayout,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        self.validate_ui_state(active_dataset, &render_bindings, &layout)?;
        let generation = self
            .world
            .get_resource::<ActiveUiContext>()
            .ok_or(KernelError::ContextUnavailable)?
            .generation
            + 1;
        self.apply_ui_state(
            active_dataset,
            render_mode,
            render_bindings,
            layout,
            operations,
            generation,
        );
        Ok(())
    }

    pub fn push_context(
        &mut self,
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
        layout: ResolvedRenderLayout,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        self.validate_ui_state(active_dataset, &render_bindings, &layout)?;
        let current = self
            .world
            .get_resource::<ActiveUiContext>()
            .cloned()
            .ok_or(KernelError::ContextUnavailable)?;
        self.world
            .resource_mut::<ContextStack>()
            .0
            .push(UiContextSnapshot {
                active_dataset: current.active_dataset,
                render_mode: current.render_mode,
                render_bindings: current.render_bindings,
            });
        self.apply_ui_state(
            active_dataset,
            render_mode,
            render_bindings,
            layout,
            operations,
            current.generation + 1,
        );
        Ok(())
    }

    /// Pops the active context. The caller supplies projections resolved
    /// from the restored Mode/Active Dataset; only validated data is committed.
    pub fn pop_context(
        &mut self,
        layout: ResolvedRenderLayout,
        operations: ResolvedOperationSet,
    ) -> Result<(), KernelError> {
        let snapshot = self
            .world
            .resource::<ContextStack>()
            .0
            .last()
            .cloned()
            .ok_or(KernelError::ContextUnavailable)?;
        self.validate_ui_state(snapshot.active_dataset, &snapshot.render_bindings, &layout)?;
        let generation = self
            .world
            .get_resource::<ActiveUiContext>()
            .ok_or(KernelError::ContextUnavailable)?
            .generation
            + 1;
        self.world.resource_mut::<ContextStack>().0.pop();
        self.apply_ui_state(
            snapshot.active_dataset,
            snapshot.render_mode,
            snapshot.render_bindings,
            layout,
            operations,
            generation,
        );
        Ok(())
    }

    pub fn run_schedule(&mut self) {
        self.schedule.run(&mut self.world);
        git_runtime::update_git_messages(&mut self.world);
        operation_runtime::update_operation_messages(&mut self.world);
        self.world
            .resource_mut::<bevy_ecs::prelude::Messages<ViewportMeasurement>>()
            .update();
        let violations = validate_invariants(&mut self.world);
        self.world
            .resource_mut::<RuntimeDiagnostics>()
            .invariant_violations = violations;
    }

    pub fn validate(&mut self) -> Vec<InvariantViolation> {
        validate_invariants(&mut self.world)
    }

    fn require_dataset(&self, entity: Entity) -> Result<(), KernelError> {
        self.world
            .get::<Dataset>(entity)
            .map(|_| ())
            .ok_or(KernelError::MissingDataset(entity))
    }

    fn validate_ui_state(
        &self,
        active_dataset: Entity,
        render_bindings: &RenderContextBindings,
        layout: &ResolvedRenderLayout,
    ) -> Result<(), KernelError> {
        self.require_dataset(active_dataset)?;
        if !layout.can_activate(active_dataset) {
            return Err(KernelError::ActiveDatasetNotActivatable(active_dataset));
        }
        for entity in render_bindings.entities() {
            self.require_dataset(entity)?;
        }
        let mut rendered = Vec::new();
        layout.dataset_entities(&mut rendered);
        for entity in rendered {
            self.require_dataset(entity)?;
        }
        Ok(())
    }

    fn apply_ui_state(
        &mut self,
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
        layout: ResolvedRenderLayout,
        operations: ResolvedOperationSet,
        generation: u64,
    ) {
        self.world.insert_resource(ActiveUiContext {
            active_dataset,
            render_mode: render_mode.clone(),
            render_bindings,
            resolved_operations: operations.id.clone(),
            generation,
        });
        self.world.insert_resource(ActiveRenderMode {
            id: render_mode,
            layout,
        });
        self.world.insert_resource(operations);
    }
}

fn validate_render_layout_contract(
    mode: &RenderModeId,
    layout: &RenderLayout,
    proxies: &RenderProxyRegistry,
    errors: &mut Vec<RegistrationContractError>,
) {
    match layout {
        RenderLayout::Row(children)
        | RenderLayout::Column(children)
        | RenderLayout::Overlay(children) => {
            for child in children {
                validate_render_layout_contract(mode, child, proxies, errors);
            }
        }
        RenderLayout::Dataset { dataset, proxy, .. } => {
            let Some(spec) = proxies.get(proxy) else {
                errors.push(RegistrationContractError::RenderModeMissingProxy {
                    mode: mode.clone(),
                    proxy: proxy.clone(),
                });
                return;
            };
            if let DatasetBinding::Stable(identity) = dataset {
                let expected = identity.kind();
                if spec.dataset_kind != expected {
                    errors.push(RegistrationContractError::StableRenderProxyKindMismatch {
                        mode: mode.clone(),
                        identity: Box::new(identity.clone()),
                        proxy: proxy.clone(),
                        expected,
                        actual: spec.dataset_kind,
                    });
                }
            }
        }
    }
}

pub(crate) fn ensure_dataset_in_world(
    world: &mut World,
    identity: DatasetIdentity,
    kind: DatasetKind,
    template: DatasetTemplateId,
) -> Result<Entity, KernelError> {
    let identity_kind = identity.kind();
    if identity_kind != kind {
        return Err(KernelError::IdentityKindMismatch {
            identity: Box::new(identity),
            expected: identity_kind,
            actual: kind,
        });
    }
    let template_kind = world
        .resource::<DatasetTemplateRegistry>()
        .get(&template)
        .map(|definition| definition.kind)
        .ok_or_else(|| KernelError::MissingTemplate(template.clone()))?;
    if template_kind != kind {
        return Err(KernelError::TemplateKindMismatch {
            template,
            expected: kind,
            actual: template_kind,
        });
    }

    if let Some(entity) = world.resource::<DatasetIndex>().get(&identity) {
        let actual_kind = world
            .get::<DatasetType>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0;
        if actual_kind != kind {
            return Err(KernelError::IdentityKindMismatch {
                identity: Box::new(identity),
                expected: kind,
                actual: actual_kind,
            });
        }
        let actual_template = world
            .get::<DatasetTemplateRef>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0
            .clone();
        if actual_template != template {
            return Err(KernelError::IdentityTemplateMismatch {
                identity: Box::new(identity),
                expected: template,
                actual: actual_template,
            });
        }
        return Ok(entity);
    }

    let entity = world
        .spawn(DatasetBundle::new(identity.clone(), kind, template))
        .id();
    world
        .resource_mut::<DatasetIndex>()
        .by_key
        .insert(identity, entity);
    Ok(entity)
}

fn resolve_render_layout(
    world: &World,
    layout: &RenderLayout,
    bindings: &RenderContextBindings,
) -> Result<ResolvedRenderLayout, KernelError> {
    fn resolve(
        world: &World,
        layout: &RenderLayout,
        bindings: &RenderContextBindings,
    ) -> Result<Option<ResolvedRenderLayout>, KernelError> {
        let resolve_children = |children: &[RenderLayout]| {
            children
                .iter()
                .map(|child| resolve(world, child, bindings))
                .filter_map(|result| match result {
                    Ok(Some(layout)) => Some(Ok(layout)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()
        };

        match layout {
            RenderLayout::Row(children) => {
                Ok(Some(ResolvedRenderLayout::Row(resolve_children(children)?)))
            }
            RenderLayout::Column(children) => Ok(Some(ResolvedRenderLayout::Column(
                resolve_children(children)?,
            ))),
            RenderLayout::Overlay(children) => Ok(Some(ResolvedRenderLayout::Overlay(
                resolve_children(children)?,
            ))),
            RenderLayout::Dataset {
                dataset,
                proxy,
                constraint,
                activatable,
            } => {
                let entity = match dataset {
                    DatasetBinding::Stable(identity) => world
                        .resource::<DatasetIndex>()
                        .get(identity)
                        .ok_or_else(|| {
                            KernelError::MissingStableRenderDataset(Box::new(identity.clone()))
                        })?,
                    DatasetBinding::Context(binding) => {
                        let Some(entity) = bindings.get(binding) else {
                            // A context leaf is optional until its semantic object
                            // exists (for example an unborn repository has no
                            // CurrentCommits). The rest of the configured layout
                            // remains intact and no renderer guesses a fallback.
                            return Ok(None);
                        };
                        entity
                    }
                };
                require_dataset(world, entity)?;
                let actual = world
                    .get::<DatasetType>(entity)
                    .ok_or(KernelError::MissingDataset(entity))?
                    .0;
                let spec = world
                    .resource::<RenderProxyRegistry>()
                    .get(proxy)
                    .ok_or_else(|| KernelError::MissingRenderProxy(proxy.clone()))?;
                if spec.dataset_kind != actual {
                    return Err(KernelError::RenderProxyKindMismatch {
                        proxy: proxy.clone(),
                        expected: spec.dataset_kind,
                        actual,
                    });
                }
                Ok(Some(ResolvedRenderLayout::Dataset {
                    dataset: entity,
                    proxy: proxy.clone(),
                    constraint: *constraint,
                    activatable: *activatable,
                }))
            }
        }
    }

    resolve(world, layout, bindings)
        .map(|layout| layout.unwrap_or_else(|| ResolvedRenderLayout::Row(Vec::new())))
}

fn replace_children_in_world(
    world: &mut World,
    parent: Entity,
    children: Vec<Entity>,
    has_snapshot: bool,
) -> Result<(), KernelError> {
    require_dataset(world, parent)?;
    let mut seen = HashSet::new();
    for child in &children {
        require_dataset(world, *child)?;
        if !seen.insert(*child) {
            return Err(KernelError::DuplicateChild(*child));
        }
        if *child == parent || is_reachable(world, *child, parent) {
            return Err(KernelError::Cycle {
                parent,
                child: *child,
            });
        }
    }

    world.entity_mut(parent).insert((
        DatasetChildren(children.clone()),
        DatasetCollection(
            children
                .into_iter()
                .map(|entity| CollectionElement { entity, depth: 0 })
                .collect(),
        ),
        HasSnapshot(has_snapshot),
    ));
    world
        .get_mut::<DatasetRevision>(parent)
        .expect("validated Dataset must have a revision")
        .0 += 1;
    Ok(())
}

fn require_dataset(world: &World, entity: Entity) -> Result<(), KernelError> {
    world
        .get::<Dataset>(entity)
        .map(|_| ())
        .ok_or(KernelError::MissingDataset(entity))
}

fn collect_unreachable_datasets(world: &mut World) {
    let mut reachable = HashSet::new();
    let mut pending = world.resource::<DatasetRoots>().0.clone();

    if let Some(context) = world.get_resource::<ActiveUiContext>() {
        pending.push(context.active_dataset);
        pending.extend(context.render_bindings.entities());
    }
    if let Some(mode) = world.get_resource::<ActiveRenderMode>() {
        mode.layout.dataset_entities(&mut pending);
    }
    for snapshot in &world.resource::<ContextStack>().0 {
        pending.push(snapshot.active_dataset);
        pending.extend(snapshot.render_bindings.entities());
    }

    while let Some(entity) = pending.pop() {
        if !reachable.insert(entity) {
            continue;
        }
        if let Some(children) = world.get::<DatasetChildren>(entity) {
            pending.extend(children.0.iter().copied());
        }
    }

    let all_datasets = {
        let mut query = world.query_filtered::<Entity, bevy_ecs::query::With<Dataset>>();
        query.iter(world).collect::<Vec<_>>()
    };
    for entity in all_datasets {
        if !reachable.contains(&entity) {
            let _ = world.despawn(entity);
        }
    }
    world
        .resource_mut::<DatasetIndex>()
        .by_key
        .retain(|_, entity| reachable.contains(entity));
}

fn is_reachable(world: &World, start: Entity, target: Entity) -> bool {
    let mut seen = HashSet::new();
    let mut pending = vec![start];
    while let Some(entity) = pending.pop() {
        if entity == target {
            return true;
        }
        if !seen.insert(entity) {
            continue;
        }
        if let Some(children) = world.get::<DatasetChildren>(entity) {
            pending.extend(children.0.iter().copied());
        }
    }
    false
}

fn validate_invariants(world: &mut World) -> Vec<InvariantViolation> {
    let mut violations = Vec::new();
    let index = world.resource::<DatasetIndex>().clone();

    for (identity, entity) in &index.by_key {
        let Some(key) = world.get::<DatasetKey>(*entity) else {
            violations.push(InvariantViolation::IndexPointsToMissingEntity(
                identity.clone(),
            ));
            continue;
        };
        if &key.0 != identity {
            violations.push(InvariantViolation::IndexKeyMismatch(identity.clone()));
        }
    }

    let datasets = {
        let mut query = world.query::<(
            Entity,
            &DatasetKey,
            &DatasetType,
            &DatasetChildren,
            &DatasetCollection,
            &DatasetActiveElement,
            &DatasetSelection,
            &DatasetTemplateRef,
        )>();
        query
            .iter(world)
            .map(
                |(entity, key, kind, children, collection, active, selection, template)| {
                    (
                        entity,
                        key.0.clone(),
                        kind.0,
                        children.0.clone(),
                        collection.0.clone(),
                        active.0,
                        selection.0.clone(),
                        template.0.clone(),
                    )
                },
            )
            .collect::<Vec<_>>()
    };
    for (entity, identity, kind, children, elements, active, selection, template) in &datasets {
        if index.get(identity) != Some(*entity) {
            violations.push(InvariantViolation::DatasetMissingFromIndex(
                identity.clone(),
            ));
        }
        if identity.kind() != *kind {
            violations.push(InvariantViolation::IdentityKindMismatch {
                identity: identity.clone(),
                actual: *kind,
            });
        }
        for child in children {
            if world.get::<Dataset>(*child).is_none() {
                violations.push(InvariantViolation::DanglingChild {
                    parent: *entity,
                    child: *child,
                });
            }
        }
        for element in elements {
            if world.get::<Dataset>(element.entity).is_none() {
                violations.push(InvariantViolation::DanglingCollectionElement {
                    dataset: *entity,
                    target: element.entity,
                });
            }
        }
        if let Some(active) = active
            && !elements.iter().any(|element| element.entity == *active)
        {
            violations.push(InvariantViolation::ActiveElementOutsideCollection {
                dataset: *entity,
                element: *active,
            });
        }
        for selected in selection {
            if !elements.iter().any(|element| element.entity == *selected) {
                violations.push(InvariantViolation::SelectionOutsideCollection {
                    dataset: *entity,
                    selected: *selected,
                });
            }
        }
        let expected = collection::expected_collection(world, template, children);
        if elements != &expected.elements {
            violations.push(InvariantViolation::InvalidCollection(*entity));
        }
        if is_reachable_without_origin(world, *entity, *entity) {
            violations.push(InvariantViolation::DatasetCycle(*entity));
        }
    }

    for root in &world.resource::<DatasetRoots>().0 {
        if world.get::<Dataset>(*root).is_none() {
            violations.push(InvariantViolation::DanglingRoot(*root));
        }
    }
    if let Some(context) = world.get_resource::<ActiveUiContext>() {
        if world.get::<Dataset>(context.active_dataset).is_none() {
            violations.push(InvariantViolation::DanglingActiveDataset(
                context.active_dataset,
            ));
        }
        for entity in context.render_bindings.entities() {
            if world.get::<Dataset>(entity).is_none() {
                violations.push(InvariantViolation::DanglingRenderBinding(entity));
            }
        }
        if let Some(mode) = world.get_resource::<ActiveRenderMode>()
            && !mode.layout.can_activate(context.active_dataset)
        {
            violations.push(InvariantViolation::ActiveDatasetNotActivatable(
                context.active_dataset,
            ));
        }
    }
    for snapshot in &world.resource::<ContextStack>().0 {
        if world.get::<Dataset>(snapshot.active_dataset).is_none() {
            violations.push(InvariantViolation::DanglingContextEntity(
                snapshot.active_dataset,
            ));
        }
        for entity in snapshot.render_bindings.entities() {
            if world.get::<Dataset>(entity).is_none() {
                violations.push(InvariantViolation::DanglingContextEntity(entity));
            }
        }
    }

    violations
}

fn is_reachable_without_origin(world: &World, origin: Entity, target: Entity) -> bool {
    let Some(children) = world.get::<DatasetChildren>(origin) else {
        return false;
    };
    children
        .0
        .iter()
        .copied()
        .any(|child| is_reachable(world, child, target))
}

#[cfg(test)]
mod tests;
