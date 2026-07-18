//! Dataset-bound Operation execution layer.
//!
//! ```text
//! InputIntent (already parsed by pitui-tui)
//!   -> executor queries the cached Operation Set for the Active Dataset
//!   -> OperationInvocation
//!   -> OperationManager selects the ECS System by OperationId
//!   -> System mutates ECS data or emits typed effect data
//! ```

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
use pitui_core::{CommitHash, ResetMode};
use pitui_data::{
    ActiveDirection, ActiveHandoffRegistry, ActiveHandoffTarget, ActiveRenderMode, ActiveUiContext,
    AvailabilityRule, AvailabilityRuleId, AvailabilityRuleRegistry, ChangeBoundary,
    ClipboardContentKind, ClipboardRequest, CommandId, CommandRegistry, CommitCreationMetadata,
    CommitFieldMetadata, CommitMetadata, ContextStack, ContextTransitionRequest,
    DatasetActiveElement, DatasetChildren, DatasetCollection, DatasetIdentity, DatasetIndex,
    DatasetKey, DatasetRevision, DatasetSelection, DatasetTemplateRef, DatasetTemplateRegistry,
    DatasetType, DatasetViewState, DatasetViewport, DefaultDatasetTemplates, GlobalOperationSet,
    InputIntent, InteractionContextKind, InteractionContextMetadata, InteractionNoticeRequest,
    InvocationSource, KeyCode, KeySequence, LayoutConstraint, OperationId, OperationInvocation,
    OperationNotice, OperationRegistry, OperationSpec, PaletteCommandEntry, PendingChordState,
    QuitRequested, ReflogEntryMetadata, RenderBindingPatch, RenderModeId, RenderProxyId,
    RepositoryMetadata, ResolvedKeyAction, ResolvedKeyBinding, ResolvedOperation,
    ResolvedOperationSet, ResolvedOperationSetId, ShortcutHelpEntry, StableOperationInvocation,
    TargetSource, TextEdit, TextEditIntent, UiContextFrameKind, WorkingTreeFileMetadata,
};
use pitui_git::GitCommand;

use crate::{
    ensure_dataset_in_world,
    git_runtime::{
        GitCommandData, GitRefreshPlan, GitRefreshTarget, GitRequestId, GitRequestOutcome,
        GitRequestOutcomes, GitRequestQueue,
    },
};

mod executor;
mod manager;
mod resolver;
mod systems;

pub use executor::*;
pub use manager::*;
pub use resolver::*;
pub use systems::*;

pub fn initialize_operation_layer(world: &mut World) {
    world.init_resource::<CommandRegistry>();
    world.init_resource::<OperationRegistry>();
    world.init_resource::<AvailabilityRuleRegistry>();
    world.init_resource::<GlobalOperationSet>();
    world.init_resource::<OperationResolutionDiagnostics>();
    world.init_resource::<OperationManager>();
    world.init_resource::<OperationExecutionLog>();
    world.init_resource::<OperationNotices>();
    world.init_resource::<OperationRuntimeRetention>();
    world.init_resource::<ClipboardRequests>();
    world.init_resource::<PendingInteractionNotices>();
    world.init_resource::<PendingOperationInvocations>();
    world.init_resource::<DeferredStableOperationInvocations>();
    world.init_resource::<PendingChangesActiveRelays>();
    world.init_resource::<QuitRequested>();
    world.init_resource::<Messages<InputIntent>>();
    world.init_resource::<Messages<OperationInvocation>>();
    world.init_resource::<Messages<OperationNotice>>();
    world.init_resource::<Messages<ContextTransitionRequest>>();
    world.init_resource::<Messages<ClipboardRequest>>();
    world.init_resource::<Messages<TextEditIntent>>();
    world.init_resource::<Messages<InteractionNoticeRequest>>();
}

pub fn update_operation_messages(world: &mut World) {
    world.resource_mut::<Messages<InputIntent>>().update();
    world
        .resource_mut::<Messages<OperationInvocation>>()
        .update();
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
