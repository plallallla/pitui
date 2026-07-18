use std::collections::HashSet;

use bevy_ecs::prelude::{Entity, Query, World};
use pitui_data::{
    CollectionElement, CollectionManagerSpec, DatasetActiveElement, DatasetChildren,
    DatasetCollection, DatasetKind, DatasetSelection, DatasetTemplateId, DatasetTemplateRef,
    DatasetTemplateRegistry, DatasetType, FileMetadata, FileTreeDirectoryMetadata, TreeManagerSpec,
    TreeSelectionMode, TreeSiblingOrder, WorkingTreeFileMetadata,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ManagedCollection {
    pub elements: Vec<CollectionElement>,
}

pub(super) fn rebuild_collections(world: &mut World) {
    let datasets = {
        let mut query = world.query::<(Entity, &DatasetChildren, &DatasetTemplateRef)>();
        query
            .iter(world)
            .map(|(entity, children, template)| (entity, children.0.clone(), template.0.clone()))
            .collect::<Vec<_>>()
    };

    for (entity, children, template) in datasets {
        let expected = expected_collection(world, &template, &children);
        let changed = world
            .get::<DatasetCollection>(entity)
            .is_none_or(|collection| collection.0 != expected.elements);
        if changed {
            world
                .entity_mut(entity)
                .insert(DatasetCollection(expected.elements));
        }
    }
}

pub(super) fn expected_collection(
    world: &World,
    template: &DatasetTemplateId,
    children: &[Entity],
) -> ManagedCollection {
    let manager = world
        .resource::<DatasetTemplateRegistry>()
        .get(template)
        .map(|template| template.collection.clone())
        .unwrap_or_default();
    match manager {
        CollectionManagerSpec::List => list_collection(children),
        CollectionManagerSpec::Tree(spec) => tree_collection(world, children, &spec),
    }
}

fn list_collection(children: &[Entity]) -> ManagedCollection {
    ManagedCollection {
        elements: children
            .iter()
            .copied()
            .map(|entity| CollectionElement { entity, depth: 0 })
            .collect(),
    }
}

fn tree_collection(world: &World, roots: &[Entity], spec: &TreeManagerSpec) -> ManagedCollection {
    let visible = spec.visible_kinds.iter().copied().collect::<HashSet<_>>();
    let mut collection = ManagedCollection::default();
    let mut visited = HashSet::new();
    flatten_tree(
        world,
        roots,
        0,
        spec,
        &visible,
        &mut visited,
        &mut collection,
    );
    collection
}

fn flatten_tree(
    world: &World,
    siblings: &[Entity],
    depth: usize,
    spec: &TreeManagerSpec,
    visible: &HashSet<DatasetKind>,
    visited: &mut HashSet<Entity>,
    collection: &mut ManagedCollection,
) {
    let siblings = ordered_siblings(world, siblings, spec.sibling_order);
    for entity in siblings {
        let kind = world.get::<DatasetType>(entity).map(|kind| kind.0);
        if !kind.is_some_and(|kind| visible.contains(&kind)) || !visited.insert(entity) {
            continue;
        }
        collection
            .elements
            .push(CollectionElement { entity, depth });
        if let Some(children) = world.get::<DatasetChildren>(entity) {
            flatten_tree(
                world,
                &children.0,
                depth.saturating_add(1),
                spec,
                visible,
                visited,
                collection,
            );
        }
    }
}

fn ordered_siblings(world: &World, siblings: &[Entity], order: TreeSiblingOrder) -> Vec<Entity> {
    let mut siblings = siblings.to_vec();
    if order == TreeSiblingOrder::Path {
        siblings.sort_by(|left, right| {
            match (path_sort_key(world, *left), path_sort_key(world, *right)) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
    }
    siblings
}

fn path_sort_key(world: &World, entity: Entity) -> Option<Vec<u8>> {
    let path = if let Some(metadata) = world.get::<FileTreeDirectoryMetadata>(entity) {
        &metadata.0
    } else if let Some(metadata) = world.get::<FileMetadata>(entity) {
        &metadata.0.path
    } else {
        &world.get::<WorkingTreeFileMetadata>(entity)?.0.path
    };
    Some(path.as_bytes().to_vec())
}

pub(super) fn repair_active_elements(
    mut datasets: Query<(
        &DatasetCollection,
        &mut DatasetActiveElement,
        &mut DatasetSelection,
    )>,
) {
    for (collection, mut active, mut selection) in &mut datasets {
        if active.0.is_none_or(|entity| !collection.contains(entity)) {
            active.0 = collection.first();
        }

        let selected = selection.0.iter().copied().collect::<HashSet<_>>();
        selection.0 = collection
            .entities()
            .filter(|row| selected.contains(row))
            .collect();
    }
}

pub(super) fn toggle_selection(
    world: &mut World,
    dataset: Entity,
    targets: &[Entity],
) -> Result<(), String> {
    let elements = world
        .get::<DatasetCollection>(dataset)
        .ok_or_else(|| "Dataset does not own a collection".to_owned())?
        .entities()
        .collect::<Vec<_>>();
    if targets.iter().any(|target| !elements.contains(target)) {
        return Err("selection target is outside the Dataset".into());
    }
    let manager = world
        .get::<DatasetTemplateRef>(dataset)
        .and_then(|template| world.resource::<DatasetTemplateRegistry>().get(&template.0))
        .map(|template| template.collection.clone())
        .ok_or_else(|| "Dataset Collection Manager is unavailable".to_owned())?;
    let mut selected = world
        .get::<DatasetSelection>(dataset)
        .ok_or_else(|| "Dataset does not own a selection".to_owned())?
        .0
        .iter()
        .copied()
        .collect::<HashSet<_>>();

    match manager {
        CollectionManagerSpec::List => toggle_independent(targets, &mut selected),
        CollectionManagerSpec::Tree(spec) => {
            let selectable = spec
                .selectable_kinds
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            if targets.iter().any(|target| {
                world
                    .get::<DatasetType>(*target)
                    .is_none_or(|kind| !selectable.contains(&kind.0))
            }) {
                return Err("selection target is a structural Tree row".into());
            }
            match spec.selection {
                TreeSelectionMode::Independent => toggle_independent(targets, &mut selected),
                TreeSelectionMode::Cascade => {
                    toggle_tree(world, &elements, targets, &selectable, &mut selected)
                }
            }
        }
    }

    let ordered = elements
        .into_iter()
        .filter(|entity| selected.contains(entity))
        .collect();
    world
        .get_mut::<DatasetSelection>(dataset)
        .expect("selection was validated before mutation")
        .0 = ordered;
    Ok(())
}

fn toggle_independent(targets: &[Entity], selected: &mut HashSet<Entity>) {
    for target in targets {
        if !selected.insert(*target) {
            selected.remove(target);
        }
    }
}

fn toggle_tree(
    world: &World,
    elements: &[Entity],
    targets: &[Entity],
    selectable: &HashSet<DatasetKind>,
    selected: &mut HashSet<Entity>,
) {
    let element_set = elements.iter().copied().collect::<HashSet<_>>();
    for target in targets {
        let subtree = collection_subtree(world, *target, &element_set, selectable);
        if selected.contains(target) {
            selected.retain(|entity| !subtree.contains(entity));
        } else {
            selected.extend(subtree);
        }
    }
    normalize_tree_selection(world, elements, &element_set, selectable, selected);
}

fn collection_subtree(
    world: &World,
    root: Entity,
    elements: &HashSet<Entity>,
    selectable: &HashSet<DatasetKind>,
) -> HashSet<Entity> {
    let mut subtree = HashSet::new();
    let mut visited = HashSet::new();
    let mut pending = vec![root];
    while let Some(entity) = pending.pop() {
        if !elements.contains(&entity) || !visited.insert(entity) {
            continue;
        }
        if world
            .get::<DatasetType>(entity)
            .is_some_and(|kind| selectable.contains(&kind.0))
        {
            subtree.insert(entity);
        }
        if let Some(children) = world.get::<DatasetChildren>(entity) {
            pending.extend(children.0.iter().copied());
        }
    }
    subtree
}

fn normalize_tree_selection(
    world: &World,
    elements: &[Entity],
    element_set: &HashSet<Entity>,
    selectable: &HashSet<DatasetKind>,
    selected: &mut HashSet<Entity>,
) {
    // The Tree collection is pre-order, so reverse traversal resolves descendants
    // before parents. A parent is checked only when every visible direct child
    // is checked; partial subtrees therefore never execute as fully selected.
    for parent in elements.iter().rev().copied() {
        if world
            .get::<DatasetType>(parent)
            .is_none_or(|kind| !selectable.contains(&kind.0))
        {
            selected.remove(&parent);
            continue;
        }
        let direct_children = selectable_child_frontier(world, parent, element_set, selectable);
        if direct_children.is_empty() {
            continue;
        }
        if direct_children.iter().all(|child| selected.contains(child)) {
            selected.insert(parent);
        } else {
            selected.remove(&parent);
        }
    }
}

fn selectable_child_frontier(
    world: &World,
    parent: Entity,
    elements: &HashSet<Entity>,
    selectable: &HashSet<DatasetKind>,
) -> Vec<Entity> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut pending = world
        .get::<DatasetChildren>(parent)
        .map(|children| children.0.clone())
        .unwrap_or_default();
    while let Some(entity) = pending.pop() {
        if !elements.contains(&entity) || !visited.insert(entity) {
            continue;
        }
        if world
            .get::<DatasetType>(entity)
            .is_some_and(|kind| selectable.contains(&kind.0))
        {
            result.push(entity);
        } else if let Some(children) = world.get::<DatasetChildren>(entity) {
            pending.extend(children.0.iter().copied());
        }
    }
    result
}
