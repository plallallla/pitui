use std::collections::HashMap;

use bevy_ecs::prelude::Resource;

use crate::DatasetKind;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

string_id!(DatasetTemplateId);
string_id!(DatasetViewId);
string_id!(OperationId);
string_id!(CommandId);
string_id!(AvailabilityRuleId);
string_id!(RenderProxyId);
string_id!(RenderModeId);
string_id!(ResolvedOperationSetId);

/// Data-selected manager for one Dataset's row collection.
///
/// `List` exposes configured direct children or flattens selected descendant
/// kinds with independent selection. `Tree` walks the Dataset DAG through
/// configured visible kinds and owns hierarchical depth and selection
/// semantics. A future `Table` manager can be added here without changing
/// Dataset identity, command targeting or renderer ownership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CollectionManagerSpec {
    List(ListManagerSpec),
    Tree(TreeManagerSpec),
}

impl Default for CollectionManagerSpec {
    fn default() -> Self {
        Self::List(ListManagerSpec::default())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListManagerSpec {
    /// Direct lists expose only immediate children. Descendant lists flatten
    /// the same ownership DAG without changing it, which lets a file tree and
    /// flat file list share identical source entities.
    pub source: ListSource,
    /// Empty means every encountered kind is visible.
    pub visible_kinds: Vec<DatasetKind>,
    pub sibling_order: TreeSiblingOrder,
}

impl Default for ListManagerSpec {
    fn default() -> Self {
        Self {
            source: ListSource::DirectChildren,
            visible_kinds: Vec::new(),
            sibling_order: TreeSiblingOrder::Source,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListSource {
    DirectChildren,
    Descendants,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeManagerSpec {
    pub visible_kinds: Vec<DatasetKind>,
    /// Kinds that may enter the owner's `DatasetSelection`. Structural tree
    /// rows can remain visible without accidentally becoming operation targets.
    pub selectable_kinds: Vec<DatasetKind>,
    pub sibling_order: TreeSiblingOrder,
    pub selection: TreeSelectionMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeSiblingOrder {
    Source,
    Path,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeSelectionMode {
    Independent,
    Cascade,
}

/// One selectable presentation of the same Dataset data. A View chooses both
/// the Collection Manager and Render Proxy; switching it never rewrites the
/// ownership DAG or semantic metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetViewSpec {
    pub id: DatasetViewId,
    pub collection: CollectionManagerSpec,
    pub render_proxy: RenderProxyId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetTemplate {
    pub id: DatasetTemplateId,
    pub kind: DatasetKind,
    pub collection: CollectionManagerSpec,
    /// The first View is the default. Empty keeps the template's legacy
    /// collection plus the RenderMode-selected Proxy.
    pub views: Vec<DatasetViewSpec>,
    pub operations: Vec<OperationId>,
    pub render_proxies: Vec<RenderProxyId>,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct DatasetTemplateRegistry {
    pub templates: HashMap<DatasetTemplateId, DatasetTemplate>,
}

impl DatasetTemplateRegistry {
    pub fn register(&mut self, template: DatasetTemplate) -> Result<(), Box<DatasetTemplate>> {
        if self.templates.contains_key(&template.id) {
            return Err(Box::new(template));
        }
        self.templates.insert(template.id.clone(), template);
        Ok(())
    }

    pub fn get(&self, id: &DatasetTemplateId) -> Option<&DatasetTemplate> {
        self.templates.get(id)
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct DefaultDatasetTemplates {
    pub by_kind: HashMap<DatasetKind, DatasetTemplateId>,
}

impl DefaultDatasetTemplates {
    pub fn bind(&mut self, kind: DatasetKind, template: DatasetTemplateId) {
        self.by_kind.insert(kind, template);
    }

    pub fn get(&self, kind: DatasetKind) -> Option<&DatasetTemplateId> {
        self.by_kind.get(&kind)
    }
}
