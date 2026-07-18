use std::collections::HashMap;

use bevy_ecs::prelude::{Entity, Message, Resource};
use pitui_core::{DiffLine, SideBySideRow};

use crate::{
    DatasetIdentity, DatasetKind, KeyStroke, LayoutConstraint, RenderBindingId, RenderModeId,
    RenderProxyId,
};

/// A registered, renderer-independent way to interpret one Dataset kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderProxySpec {
    pub id: RenderProxyId,
    pub dataset_kind: DatasetKind,
    pub renderer: RendererKind,
    pub fields: Vec<FieldSpec>,
    pub style: StyleSpec,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RendererKind {
    Tree,
    List,
    Detail,
    CommitDetail,
    UnifiedDiff,
    SideBySideDiff,
    Confirmation,
    CommitCreation,
    LogList,
}

/// Strictly registered fields that may be selected by configuration.
///
/// Configuration parsing can map stable names onto these variants, but it
/// cannot ask Projection to execute an arbitrary accessor.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldId {
    DatasetLabel,
    RepositoryName,
    RepositoryPath,
    RepositoryCurrentBranch,
    BranchCurrentMarker,
    BranchName,
    BranchHead,
    BranchAuthoredAt,
    BranchSubject,
    CommitHash,
    CommitAuthor,
    CommitAuthoredAt,
    CommitTags,
    CommitSubject,
    CommitMessage,
    CommitCreationStagedFiles,
    CommitCreationMessage,
    CommitCreationError,
    FileStatus,
    FilePath,
    FileOldPath,
    FileAdditions,
    FileDeletions,
    FileBinary,
    ReflogSelector,
    ReflogHash,
    ReflogAction,
    ReflogMessage,
    ReflogAuthor,
    ReflogAuthoredAt,
    RemoteName,
    RemoteFetchUrls,
    RemotePushUrls,
    RemoteUpstream,
    RemotePushTarget,
    RemotePolicy,
    GitOperationStartedAt,
    GitOperationName,
    GitOperationRepository,
    GitOperationDuration,
    GitOperationStatus,
    GitOperationMessage,
    GitOperationAbort,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldSpec {
    pub field: FieldId,
    pub label: Option<String>,
    pub format: FieldFormat,
}

impl FieldSpec {
    pub fn plain(field: FieldId) -> Self {
        Self {
            field,
            label: None,
            format: FieldFormat::Plain,
        }
    }

    pub fn labeled(field: FieldId, label: impl Into<String>) -> Self {
        Self {
            field,
            label: Some(label.into()),
            format: FieldFormat::Plain,
        }
    }

    pub fn formatted(field: FieldId, format: FieldFormat) -> Self {
        Self {
            field,
            label: None,
            format,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldFormat {
    Plain,
    Hash { length: usize },
    DateTime { precision: DateTimePrecision },
    Joined { separator: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DateTimePrecision {
    Date,
    Minute,
    Second,
    Raw,
}

/// Semantic style names stay outside ratatui so configured Proxies remain pure
/// data. The terminal crate resolves these names to concrete colors/modifiers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleSpec {
    pub normal: String,
    pub cursor: String,
    pub selected: String,
    pub active_border: String,
}

impl Default for StyleSpec {
    fn default() -> Self {
        Self {
            normal: "default".into(),
            cursor: "cursor".into(),
            selected: "selected".into(),
            active_border: "active-border".into(),
        }
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct RenderProxyRegistry {
    pub specs: HashMap<RenderProxyId, RenderProxySpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderRegistryError {
    DuplicateProxy(RenderProxyId),
    DuplicateMode(RenderModeId),
}

impl RenderProxyRegistry {
    pub fn register(&mut self, spec: RenderProxySpec) -> Result<(), RenderRegistryError> {
        if self.specs.contains_key(&spec.id) {
            return Err(RenderRegistryError::DuplicateProxy(spec.id));
        }
        self.specs.insert(spec.id.clone(), spec);
        Ok(())
    }

    pub fn get(&self, id: &RenderProxyId) -> Option<&RenderProxySpec> {
        self.specs.get(id)
    }
}

/// Configurable layout template. Reconcile resolves every binding before a
/// frame reaches Projection or the terminal renderer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderLayout {
    Row(Vec<RenderLayout>),
    Column(Vec<RenderLayout>),
    Overlay(Vec<RenderLayout>),
    Dataset {
        dataset: DatasetBinding,
        proxy: RenderProxyId,
        constraint: LayoutConstraint,
        focusable: bool,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum DatasetBinding {
    Stable(DatasetIdentity),
    Context(RenderBindingId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderModeSpec {
    pub id: RenderModeId,
    pub layout: RenderLayout,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct RenderModeRegistry {
    pub modes: HashMap<RenderModeId, RenderModeSpec>,
}

impl RenderModeRegistry {
    pub fn register(&mut self, spec: RenderModeSpec) -> Result<(), RenderRegistryError> {
        if self.modes.contains_key(&spec.id) {
            return Err(RenderRegistryError::DuplicateMode(spec.id));
        }
        self.modes.insert(spec.id.clone(), spec);
        Ok(())
    }

    pub fn get(&self, id: &RenderModeId) -> Option<&RenderModeSpec> {
        self.modes.get(id)
    }
}

/// Immutable, renderer-facing frame. It contains no World access and no
/// callback capable of mutating application state.
#[derive(Resource, Clone, Debug, Eq, PartialEq)]
pub struct UiFrame {
    pub generation: u64,
    pub layout: UiLayoutProjection,
    pub footer: FooterProjection,
    pub status: StatusProjection,
}

impl Default for UiFrame {
    fn default() -> Self {
        Self {
            generation: 0,
            layout: UiLayoutProjection::Empty,
            footer: FooterProjection::default(),
            status: StatusProjection::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiLayoutProjection {
    Empty,
    Row(Vec<UiLayoutProjection>),
    Column(Vec<UiLayoutProjection>),
    Overlay(Vec<UiLayoutProjection>),
    Dataset {
        constraint: LayoutConstraint,
        focusable: bool,
        panel: Box<RenderProxyProjection>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderProxyProjection {
    pub dataset: Entity,
    pub proxy: RenderProxyId,
    pub renderer: RendererKind,
    pub active: bool,
    pub title: String,
    pub style: StyleSpec,
    pub content: RenderContentProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderContentProjection {
    Empty,
    Rows(RowsProjection),
    Detail(DetailProjection),
    UnifiedDiff(UnifiedDiffProjection),
    SideBySideDiff(SideBySideDiffProjection),
    Interaction(InteractionProjection),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionProjection {
    pub title: String,
    pub prompt: Option<String>,
    pub input: Option<String>,
    pub lines: Vec<InteractionLineProjection>,
    pub error: Option<String>,
    pub viewport: ViewportProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionLineProjection {
    pub key: Option<KeyStroke>,
    pub text: String,
    pub selected: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RowsProjection {
    pub rows: Vec<RowProjection>,
    pub viewport: ViewportProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowProjection {
    pub entity: Entity,
    pub depth: usize,
    pub cells: Vec<CellProjection>,
    pub cursor: bool,
    pub selected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CellProjection {
    pub field: FieldId,
    pub label: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DetailProjection {
    pub fields: Vec<CellProjection>,
    pub viewport: ViewportProjection,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportProjection {
    pub offset: usize,
    pub page_size: usize,
    pub content_length: usize,
}

#[derive(Message, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewportMeasurement {
    pub dataset: Entity,
    pub page_size: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnifiedDiffProjection {
    pub path: String,
    pub header: Vec<String>,
    pub binary: bool,
    pub hunks: Vec<UnifiedDiffHunkProjection>,
    pub viewport: ViewportProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnifiedDiffHunkProjection {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SideBySideDiffProjection {
    pub path: String,
    pub header: Vec<String>,
    pub binary: bool,
    pub hunks: Vec<SideBySideHunkProjection>,
    pub viewport: ViewportProjection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SideBySideHunkProjection {
    pub header: String,
    pub rows: Vec<SideBySideRow>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FooterProjection {
    /// Exactly the keys that perform an action in the current context. Chord
    /// descendants appear only after their prefix replaces the active set.
    pub bindings: Vec<crate::ResolvedKeyBinding>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatusProjection {
    /// Product-facing text only; internal names such as view/focus are never
    /// synthesized here.
    pub items: Vec<String>,
}
