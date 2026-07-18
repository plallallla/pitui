use std::collections::HashSet;

use bevy_ecs::prelude::{Entity, Resource, World};
use pitui_data::{
    CollectionElement, CollectionManagerSpec, DatasetActiveElement, DatasetChildren,
    DatasetCollection, DatasetKind, DatasetParents, DatasetSelection, DatasetTemplateId,
    DatasetTemplateRef, DatasetTemplateRegistry, DatasetType, DatasetViewState, FileMetadata,
    FileTreeDirectoryMetadata, ListManagerSpec, ListSource, TreeManagerSpec, TreeSelectionMode,
    TreeSiblingOrder, WorkingTreeFileMetadata,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ManagedCollection {
    pub elements: Vec<CollectionElement>,
}

#[derive(Resource, Clone, Debug, Default)]
struct DirtyCollections(HashSet<Entity>);

pub(super) fn initialize_collection_runtime(world: &mut World) {
    world.init_resource::<DirtyCollections>();
}

pub(crate) fn mark_collection_dirty(world: &mut World, dataset: Entity) {
    world.resource_mut::<DirtyCollections>().0.insert(dataset);
}

pub(crate) fn mark_collection_ancestors_dirty(world: &mut World, dataset: Entity) {
    let ancestors = {
        let parents = world.resource::<DatasetParents>();
        let mut pending = vec![dataset];
        let mut visited = HashSet::new();
        while let Some(entity) = pending.pop() {
            if !visited.insert(entity) {
                continue;
            }
            pending.extend(parents.parents(entity).iter().copied());
        }
        visited
    };
    world.resource_mut::<DirtyCollections>().0.extend(ancestors);
}

/// Rebuilds and repairs only collections whose ownership/view source changed.
/// Ordinary cursor movement and unrelated Git results no longer walk every
/// Dataset in the World.
pub(super) fn reconcile_dirty_collections(world: &mut World) {
    let datasets = std::mem::take(&mut world.resource_mut::<DirtyCollections>().0);

    for entity in datasets {
        let Some(children) = world.get::<DatasetChildren>(entity).cloned() else {
            continue;
        };
        let Some(template) = world.get::<DatasetTemplateRef>(entity).cloned() else {
            continue;
        };
        let expected = expected_collection(world, entity, &template.0, &children.0);
        let changed = world
            .get::<DatasetCollection>(entity)
            .is_none_or(|collection| collection.0 != expected.elements);
        if changed {
            world
                .entity_mut(entity)
                .insert(DatasetCollection(expected.elements));
        }
        repair_active_element(world, entity);
    }
}

pub(super) fn expected_collection(
    world: &World,
    dataset: Entity,
    template: &DatasetTemplateId,
    children: &[Entity],
) -> ManagedCollection {
    let manager = collection_manager(world, dataset).unwrap_or_else(|| {
        world
            .resource::<DatasetTemplateRegistry>()
            .get(template)
            .map(|template| template.collection.clone())
            .unwrap_or_default()
    });
    match manager {
        CollectionManagerSpec::List(spec) => list_collection(world, children, &spec),
        CollectionManagerSpec::Tree(spec) => tree_collection(world, children, &spec),
    }
}

/// Resolves the Collection Manager selected by ordinary Dataset data. A
/// switchable View overrides the Template fallback but never changes the
/// Dataset ownership DAG.
pub(super) fn collection_manager(world: &World, dataset: Entity) -> Option<CollectionManagerSpec> {
    let template_id = world.get::<DatasetTemplateRef>(dataset)?;
    let template = world
        .resource::<DatasetTemplateRegistry>()
        .get(&template_id.0)?;
    let selected = world.get::<DatasetViewState>(dataset)?.0.as_ref();
    selected
        .and_then(|selected| template.views.iter().find(|view| &view.id == selected))
        .map(|view| view.collection.clone())
        .or_else(|| Some(template.collection.clone()))
}

fn list_collection(
    world: &World,
    children: &[Entity],
    spec: &ListManagerSpec,
) -> ManagedCollection {
    let visible = spec.visible_kinds.iter().copied().collect::<HashSet<_>>();
    let mut collection = ManagedCollection::default();
    let mut visited = HashSet::new();
    match spec.source {
        ListSource::DirectChildren => flatten_list(
            world,
            children,
            spec,
            &visible,
            false,
            &mut visited,
            &mut collection,
        ),
        ListSource::Descendants => flatten_list(
            world,
            children,
            spec,
            &visible,
            true,
            &mut visited,
            &mut collection,
        ),
    }
    collection
}

fn flatten_list(
    world: &World,
    siblings: &[Entity],
    spec: &ListManagerSpec,
    visible: &HashSet<DatasetKind>,
    recurse: bool,
    visited: &mut HashSet<Entity>,
    collection: &mut ManagedCollection,
) {
    for entity in ordered_siblings(world, siblings, spec.sibling_order) {
        if !visited.insert(entity) {
            continue;
        }
        let kind = world.get::<DatasetType>(entity).map(|kind| kind.0);
        if kind.is_some_and(|kind| visible.is_empty() || visible.contains(&kind)) {
            collection
                .elements
                .push(CollectionElement { entity, depth: 0 });
        }
        if recurse && let Some(children) = world.get::<DatasetChildren>(entity) {
            // Invisible directory/structural nodes are traversal edges in a
            // descendant List, not rows. This is what lets Files reuse its
            // directory DAG while exposing only flat File elements.
            flatten_list(world, &children.0, spec, visible, true, visited, collection);
        }
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

fn repair_active_element(world: &mut World, dataset: Entity) {
    let Some(collection) = world.get::<DatasetCollection>(dataset).cloned() else {
        return;
    };
    let active = world
        .get::<DatasetActiveElement>(dataset)
        .and_then(|active| active.0);
    let selection = world
        .get::<DatasetSelection>(dataset)
        .map(|selection| selection.0.clone())
        .unwrap_or_default();
    let repaired_active = if active.is_none_or(|entity| !collection.contains(entity)) {
        collection.first()
    } else {
        active
    };
    let mut selected = selection.into_iter().collect::<HashSet<_>>();
    selected.retain(|row| collection.contains(*row));
    if let Some(CollectionManagerSpec::Tree(spec)) = collection_manager(world, dataset)
        && spec.selection == TreeSelectionMode::Cascade
    {
        let elements = collection.entities().collect::<Vec<_>>();
        let element_set = elements.iter().copied().collect::<HashSet<_>>();
        let selectable = spec
            .selectable_kinds
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        // A selected Tree parent semantically selects its selectable
        // descendants even when selection data was restored directly or came
        // back from a flat View. Then derive checked parents from the complete
        // descendant set.
        for target in selected.iter().copied().collect::<Vec<_>>() {
            selected.extend(collection_subtree(world, target, &element_set, &selectable));
        }
        normalize_tree_selection(world, &elements, &element_set, &selectable, &mut selected);
    }
    let repaired_selection = collection
        .entities()
        .filter(|row| selected.contains(row))
        .collect::<Vec<_>>();
    if active != repaired_active {
        world
            .entity_mut(dataset)
            .insert(DatasetActiveElement(repaired_active));
    }
    if world
        .get::<DatasetSelection>(dataset)
        .is_none_or(|current| current.0 != repaired_selection)
    {
        world
            .entity_mut(dataset)
            .insert(DatasetSelection(repaired_selection));
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
    let manager = collection_manager(world, dataset)
        .ok_or_else(|| "Dataset Collection Manager is unavailable".to_owned())?;
    let mut selected = world
        .get::<DatasetSelection>(dataset)
        .ok_or_else(|| "Dataset does not own a selection".to_owned())?
        .0
        .iter()
        .copied()
        .collect::<HashSet<_>>();

    match manager {
        CollectionManagerSpec::List(_) => toggle_independent(targets, &mut selected),
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
