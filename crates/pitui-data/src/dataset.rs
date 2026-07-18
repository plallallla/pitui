use std::collections::HashMap;

use bevy_ecs::prelude::{Bundle, Component, Entity, Resource};

use crate::{DatasetIdentity, DatasetKind, DatasetTemplateId, DatasetViewId};

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

/// One renderer-independent element exposed by a Dataset's Collection Manager.
///
/// `DatasetChildren` remains the canonical ownership DAG. A List or Tree
/// manager derives this presentation collection without introducing a second
/// state model. Tree depth is data on the element itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CollectionElement {
    pub entity: Entity,
    pub depth: usize,
}

/// Ordered elements currently exposed by a Dataset.
///
/// The order is used for rendering and for choosing the previous/next active
/// element, but it remains ordinary collection data.
#[derive(Component, Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetCollection(pub Vec<CollectionElement>);

impl DatasetCollection {
    pub fn entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.0.iter().map(|element| element.entity)
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.0.iter().any(|element| element.entity == entity)
    }

    pub fn position(&self, entity: Entity) -> Option<usize> {
        self.0.iter().position(|element| element.entity == entity)
    }

    pub fn first(&self) -> Option<Entity> {
        self.0.first().map(|element| element.entity)
    }

    pub fn depth(&self, entity: Entity) -> usize {
        self.0
            .iter()
            .find(|element| element.entity == entity)
            .map(|element| element.depth)
            .unwrap_or_default()
    }
}

/// The single active element owned by this Dataset.
///
/// Inactive Datasets retain this value so an Active Dataset handoff can return
/// to the previous element. Only the active Dataset's element is highlighted.
#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DatasetActiveElement(pub Option<Entity>);

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

/// Selected presentation of one Dataset. `None` means the Template has no
/// switchable Views; otherwise the ID must resolve in that Template.
#[derive(Component, Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetViewState(pub Option<DatasetViewId>);

#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HasSnapshot(pub bool);

#[derive(Bundle, Clone, Debug)]
pub struct DatasetBundle {
    pub marker: Dataset,
    pub key: DatasetKey,
    pub kind: DatasetType,
    pub revision: DatasetRevision,
    pub children: DatasetChildren,
    pub collection: DatasetCollection,
    pub active_element: DatasetActiveElement,
    pub selection: DatasetSelection,
    pub viewport: DatasetViewport,
    pub template: DatasetTemplateRef,
    pub view: DatasetViewState,
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
            collection: DatasetCollection::default(),
            active_element: DatasetActiveElement::default(),
            selection: DatasetSelection::default(),
            viewport: DatasetViewport::default(),
            template: DatasetTemplateRef(template),
            view: DatasetViewState::default(),
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

/// Derived reverse edge index for the canonical `DatasetChildren` DAG. The
/// ownership source remains the child list; this index lets dirty propagation
/// and diagnostics reach every ancestor without repeatedly scanning the full
/// World after each snapshot relation update.
#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetParents(pub HashMap<Entity, Vec<Entity>>);

impl DatasetParents {
    pub fn parents(&self, child: Entity) -> &[Entity] {
        self.0.get(&child).map_or(&[], Vec::as_slice)
    }
}
