use std::collections::HashMap;

use bevy_ecs::prelude::{Bundle, Component, Entity, Resource};

use crate::{DatasetIdentity, DatasetKind, DatasetTemplateId};

#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Dataset;

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct DatasetKey(pub DatasetIdentity);

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatasetType(pub DatasetKind);

#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatasetRevision(pub u64);

#[derive(Component, Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetChildren(pub Vec<Entity>);

/// Ordered logical rows owned by a navigable Dataset.
///
/// For ordinary list Datasets this is identical to [`DatasetChildren`]. Tree
/// Datasets may expose selected descendants without changing the ownership DAG:
/// `RepositoriesBranches` exposes repository and branch rows, while `Changes`
/// exposes boundary groups and working-tree file rows. Cursor and selection
/// validation use this component rather than assuming every row is a direct
/// child.
#[derive(Component, Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetNavigationOrder(pub Vec<Entity>);

#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatasetCursor(pub Option<Entity>);

#[derive(Component, Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetSelection(pub Vec<Entity>);

#[derive(Component, Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatasetViewport {
    pub offset: usize,
    pub page_size: usize,
    pub content_length: usize,
}

impl Default for DatasetViewport {
    fn default() -> Self {
        Self {
            offset: 0,
            page_size: 20,
            content_length: 0,
        }
    }
}

#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub struct DatasetTemplateRef(pub DatasetTemplateId);

#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HasSnapshot(pub bool);

#[derive(Bundle, Clone, Debug)]
pub struct DatasetBundle {
    pub marker: Dataset,
    pub key: DatasetKey,
    pub kind: DatasetType,
    pub revision: DatasetRevision,
    pub children: DatasetChildren,
    pub navigation: DatasetNavigationOrder,
    pub cursor: DatasetCursor,
    pub selection: DatasetSelection,
    pub viewport: DatasetViewport,
    pub template: DatasetTemplateRef,
    pub has_snapshot: HasSnapshot,
}

impl DatasetBundle {
    pub fn new(identity: DatasetIdentity, kind: DatasetKind, template: DatasetTemplateId) -> Self {
        Self {
            marker: Dataset,
            key: DatasetKey(identity),
            kind: DatasetType(kind),
            revision: DatasetRevision::default(),
            children: DatasetChildren::default(),
            navigation: DatasetNavigationOrder::default(),
            cursor: DatasetCursor::default(),
            selection: DatasetSelection::default(),
            viewport: DatasetViewport::default(),
            template: DatasetTemplateRef(template),
            has_snapshot: HasSnapshot::default(),
        }
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct DatasetIndex {
    pub by_key: HashMap<DatasetIdentity, Entity>,
}

impl DatasetIndex {
    pub fn get(&self, identity: &DatasetIdentity) -> Option<Entity> {
        self.by_key.get(identity).copied()
    }
}

/// Explicit roots of the Dataset DAG. Active/render/context entities are
/// additional runtime roots and are intentionally stored in their own data.
#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetRoots(pub Vec<Entity>);
