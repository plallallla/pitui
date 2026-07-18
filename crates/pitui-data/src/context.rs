use std::collections::HashMap;

use bevy_ecs::prelude::{Component, Entity, Message, Resource};

use crate::{
    DatasetKind, RenderModeId, RenderProxyId, ResolvedKeyBinding, ResolvedOperationSetId,
    StableOperationInvocation,
};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RenderBindingId {
    RepositoriesBranches,
    CurrentRepository,
    CurrentCommits,
    CurrentCommit,
    CurrentFiles,
    CurrentFileChanges,
    Changes,
    CurrentChangesFileChanges,
    CurrentReflog,
    CurrentReflogEntry,
    GitOperationLog,
    CurrentGitOperationLogEntry,
    Overlay,
    InteractionContext,
    Custom(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderContextBindings(pub HashMap<RenderBindingId, Entity>);

impl RenderContextBindings {
    pub fn bind(&mut self, id: RenderBindingId, entity: Entity) -> Option<Entity> {
        self.0.insert(id, entity)
    }

    pub fn get(&self, id: &RenderBindingId) -> Option<Entity> {
        self.0.get(id).copied()
    }

    pub fn unbind(&mut self, id: &RenderBindingId) -> Option<Entity> {
        self.0.remove(id)
    }

    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.0.values().copied()
    }
}

#[derive(Resource, Clone, Debug, Eq, PartialEq)]
pub struct ActiveUiContext {
    pub active_dataset: Entity,
    pub render_mode: RenderModeId,
    pub render_bindings: RenderContextBindings,
    pub resolved_operations: ResolvedOperationSetId,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiContextSnapshot {
    pub active_dataset: Entity,
    pub render_mode: RenderModeId,
    pub render_bindings: RenderContextBindings,
}

/// Why a previous UI Context is retained.
///
/// A Context stack frame is not an untyped navigation history entry. The
/// boundary kind determines which operation is allowed to consume it: an
/// Active return may cross a View or [`ActiveHandoff`](Self::ActiveHandoff)
/// boundary, while an interaction/commit overlay mutates only its own
/// [`Overlay`](Self::Overlay) underlay.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UiContextFrameKind {
    View,
    ActiveHandoff { direction: ActiveDirection },
    Overlay,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiContextFrame {
    pub kind: UiContextFrameKind,
    pub snapshot: UiContextSnapshot,
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextStack(pub Vec<UiContextFrame>);

impl ContextStack {
    pub fn top(&self) -> Option<&UiContextFrame> {
        self.0.last()
    }

    pub fn top_mut(&mut self) -> Option<&mut UiContextFrame> {
        self.0.last_mut()
    }

    pub fn top_snapshot(&self) -> Option<&UiContextSnapshot> {
        self.top().map(|frame| &frame.snapshot)
    }

    pub fn top_snapshot_mut(&mut self) -> Option<&mut UiContextSnapshot> {
        self.top_mut().map(|frame| &mut frame.snapshot)
    }

    pub fn top_is(&self, kind: UiContextFrameKind) -> bool {
        self.top().is_some_and(|frame| frame.kind == kind)
    }

    pub fn top_overlay_snapshot(&self) -> Option<&UiContextSnapshot> {
        self.top()
            .filter(|frame| frame.kind == UiContextFrameKind::Overlay)
            .map(|frame| &frame.snapshot)
    }

    pub fn top_overlay_snapshot_mut(&mut self) -> Option<&mut UiContextSnapshot> {
        self.top_mut()
            .filter(|frame| frame.kind == UiContextFrameKind::Overlay)
            .map(|frame| &mut frame.snapshot)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionContextType {
    Inactive,
    Help,
    CommandPalette,
    Notice,
    Confirmation,
    TextInput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutHelpEntry {
    pub binding: ResolvedKeyBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaletteCommandEntry {
    pub name: String,
    pub label: String,
    pub invocation: StableOperationInvocation,
}

impl PaletteCommandEntry {
    pub fn matches(&self, query: &str) -> bool {
        let query = query.to_ascii_lowercase();
        query.is_empty()
            || self.name.to_ascii_lowercase().contains(&query)
            || self.label.to_ascii_lowercase().contains(&query)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextInputPurpose {
    RemoteName,
    RemoteUrl,
    Custom(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionContextKind {
    Inactive,
    Help {
        entries: Vec<ShortcutHelpEntry>,
    },
    CommandPalette {
        query: String,
        entries: Vec<PaletteCommandEntry>,
        selected: usize,
    },
    Notice {
        title: String,
        message: String,
    },
    Confirmation {
        title: String,
        prompt: String,
        options: Vec<String>,
        selected: usize,
        pending: Box<StableOperationInvocation>,
    },
    TextInput {
        title: String,
        prompt: String,
        purpose: TextInputPurpose,
        input: String,
        error: Option<String>,
        pending: Option<Box<StableOperationInvocation>>,
    },
}

impl InteractionContextKind {
    pub fn context_type(&self) -> InteractionContextType {
        match self {
            Self::Inactive => InteractionContextType::Inactive,
            Self::Help { .. } => InteractionContextType::Help,
            Self::CommandPalette { .. } => InteractionContextType::CommandPalette,
            Self::Notice { .. } => InteractionContextType::Notice,
            Self::Confirmation { .. } => InteractionContextType::Confirmation,
            Self::TextInput { .. } => InteractionContextType::TextInput,
        }
    }
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct InteractionContextMetadata {
    pub kind: InteractionContextKind,
}

impl Default for InteractionContextMetadata {
    fn default() -> Self {
        Self {
            kind: InteractionContextKind::Inactive,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextEdit {
    Insert(String),
    Backspace,
}

/// Text editing is Dataset data, not a renderer callback. Any semantic
/// Dataset that exposes an editable text component can consume this intent.
#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub struct TextEditIntent {
    pub dataset: Entity,
    pub edit: TextEdit,
}

/// A request to present a user-visible Notice through the single global
/// Interaction Context. Producers do not call rendering or manipulate the
/// Context stack directly.
#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub struct InteractionNoticeRequest {
    pub title: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderBindingPatch(pub Vec<(RenderBindingId, Option<Entity>)>);

impl RenderBindingPatch {
    pub fn apply_to(&self, bindings: &mut RenderContextBindings) {
        for (id, entity) in &self.0 {
            if let Some(entity) = entity {
                bindings.bind(id.clone(), *entity);
            } else {
                bindings.unbind(id);
            }
        }
    }
}

#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub enum ContextTransitionRequest {
    /// Relays Active ownership between Dataset slots in the same RenderMode.
    ActiveRelay {
        previous_active_dataset: Entity,
        previous_active_kind: DatasetKind,
        direction: ActiveDirection,
        next_active_dataset: Entity,
        binding_patch: RenderBindingPatch,
    },
    Replace {
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
    },
    Push {
        active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
    },
    /// Transfers Active ownership across a RenderMode boundary. The source
    /// type and direction are explicit data so mode changes are driven by the
    /// Active handoff rather than hidden UI controller state.
    ActiveHandoff {
        previous_active_dataset: Entity,
        previous_active_kind: DatasetKind,
        direction: ActiveDirection,
        next_active_dataset: Entity,
        render_mode: RenderModeId,
        render_bindings: RenderContextBindings,
    },
    /// Relays Active ownership back across the previous RenderMode boundary.
    ActiveReturn {
        previous_active_dataset: Entity,
        previous_active_kind: DatasetKind,
        direction: ActiveDirection,
    },
    /// Dynamically wraps the current resolved layout with a Dataset overlay.
    /// This keeps the previous view visible without giving the renderer World
    /// access or introducing a parallel modal controller.
    PushOverlay {
        active_dataset: Entity,
        render_mode: RenderModeId,
        proxy: RenderProxyId,
        constraint: LayoutConstraint,
    },
    Pop,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ActiveDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActiveHandoffTarget {
    /// Change RenderMode while retaining the current Active Dataset and its
    /// Active Element. This is used when a list becomes the left side of the
    /// next detail mode without transferring ownership to the selected row.
    KeepActiveDataset,
    ActiveElement,
    Binding(RenderBindingId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveHandoffSpec {
    pub render_mode: RenderModeId,
    pub target: ActiveHandoffTarget,
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct ActiveHandoffRegistry {
    pub rules: HashMap<(DatasetKind, ActiveDirection), ActiveHandoffSpec>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutConstraint {
    Minimum(u16),
    Percentage(u16),
    Fill(u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedRenderLayout {
    Row(Vec<ResolvedRenderLayout>),
    Column(Vec<ResolvedRenderLayout>),
    Overlay(Vec<ResolvedRenderLayout>),
    Dataset {
        dataset: Entity,
        proxy: RenderProxyId,
        constraint: LayoutConstraint,
        activatable: bool,
    },
}

impl ResolvedRenderLayout {
    pub fn dataset_entities(&self, output: &mut Vec<Entity>) {
        match self {
            Self::Row(children) | Self::Column(children) | Self::Overlay(children) => {
                for child in children {
                    child.dataset_entities(output);
                }
            }
            Self::Dataset { dataset, .. } => output.push(*dataset),
        }
    }

    pub fn can_activate(&self, entity: Entity) -> bool {
        match self {
            Self::Row(children) | Self::Column(children) | Self::Overlay(children) => {
                children.iter().any(|child| child.can_activate(entity))
            }
            Self::Dataset {
                dataset,
                activatable,
                ..
            } => *activatable && *dataset == entity,
        }
    }

    pub fn active_candidates(&self, output: &mut Vec<Entity>) {
        match self {
            Self::Row(children) | Self::Column(children) | Self::Overlay(children) => {
                for child in children {
                    child.active_candidates(output);
                }
            }
            Self::Dataset {
                dataset,
                activatable,
                ..
            } => {
                if *activatable {
                    output.push(*dataset);
                }
            }
        }
    }
}

#[derive(Resource, Clone, Debug, Eq, PartialEq)]
pub struct ActiveRenderMode {
    pub id: RenderModeId,
    pub layout: ResolvedRenderLayout,
}
