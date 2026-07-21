use std::collections::HashSet;

use bevy_ecs::{
    change_detection::DetectChanges,
    prelude::{Changed, Entity, MessageReader, Query, Res, ResMut, Resource, World},
};
use pitui_core::{DiffHunk, DiffLineKind, WorkingTreeDiff, WorkingTreeDiffKind, side_by_side_rows};
use pitui_data::{
    ActiveRenderMode, ActiveUiContext, BranchMetadata, CellProjection, ChangeBoundary,
    CommitCreationMetadata, CommitFieldMetadata, CommitMetadata, DatasetActiveElement,
    DatasetCollection, DatasetIdentity, DatasetKey, DatasetKind, DatasetSelection, DatasetType,
    DatasetViewState, DatasetViewport, DateTimePrecision, DetailProjection, DiffCellKindProjection,
    DiffLineKindProjection, DiffLineProjection, FieldFormat, FieldId, FieldSpec,
    FileChangesMetadata, FileMetadata, FileTreeDirectoryMetadata, FooterProjection,
    GitOperationLogEntryMetadata, InteractionContextKind, InteractionContextMetadata,
    InteractionLineProjection, InteractionProjection, ReflogEntryMetadata, RemoteMetadata,
    RenderBindingId, RenderContentProjection, RenderProxyId, RenderProxyProjection,
    RenderProxyRegistry, RenderProxySpec, RendererKind, RepositoryMetadata, ResolvedOperationSet,
    ResolvedRenderLayout, RowProjection, RowProjectionKind, RowsProjection,
    SideBySideDiffProjection, SideBySideHunkProjection, SideBySideRowProjection, StatusProjection,
    UiFrame, UiLayoutProjection, UnifiedDiffHunkProjection, UnifiedDiffProjection,
    ViewportMeasurement, ViewportProjection, WorkingTreeFileChangesMetadata,
    WorkingTreeFileMetadata,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionDiagnostic {
    MissingDataset(Entity),
    MissingRenderProxy(RenderProxyId),
    DatasetKindMismatch {
        dataset: Entity,
        proxy: RenderProxyId,
        expected: DatasetKind,
        actual: DatasetKind,
    },
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectionDiagnostics(pub Vec<ProjectionDiagnostic>);

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct ProjectionDependencies(HashSet<Entity>);

/// Conservative invalidation state for the immutable `UiFrame` projection.
/// Component change detection marks this resource; an idle schedule therefore
/// does not rebuild large diff/list projections just to compare them with the
/// previous frame.
#[derive(Resource, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectionRuntimeState {
    pub requested: bool,
    pub build_count: u64,
    pub skipped_count: u64,
    /// Bitmask of the precise resource/component families that caused the
    /// latest rebuild. Kept for diagnostics and performance tests.
    pub last_invalidation_reasons: u64,
    pending_invalidation_reasons: u64,
}

impl Default for ProjectionRuntimeState {
    fn default() -> Self {
        Self {
            requested: true,
            build_count: 0,
            skipped_count: 0,
            last_invalidation_reasons: 0,
            pending_invalidation_reasons: 0,
        }
    }
}

pub(super) fn mark_projection_from_resources(
    active: Option<Res<ActiveUiContext>>,
    mode: Option<Res<ActiveRenderMode>>,
    operations: Option<Res<ResolvedOperationSet>>,
    proxies: Res<RenderProxyRegistry>,
    mut state: ResMut<ProjectionRuntimeState>,
) {
    let changed = active.is_some_and(|value| value.is_changed())
        || mode.is_some_and(|value| value.is_changed())
        || operations.is_some_and(|value| value.is_changed())
        || proxies.is_changed();
    if changed {
        state.requested = true;
        state.pending_invalidation_reasons |= 1;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mark_projection_from_dataset_state(
    dependencies: Res<ProjectionDependencies>,
    keys: Query<Entity, Changed<DatasetKey>>,
    kinds: Query<Entity, Changed<DatasetType>>,
    collections: Query<Entity, Changed<DatasetCollection>>,
    active: Query<Entity, Changed<DatasetActiveElement>>,
    selections: Query<Entity, Changed<DatasetSelection>>,
    views: Query<Entity, Changed<DatasetViewState>>,
    viewports: Query<Entity, Changed<DatasetViewport>>,
    mut state: ResMut<ProjectionRuntimeState>,
) {
    let visible = |entity| dependencies.0.contains(&entity);
    let reasons = u64::from(keys.iter().any(visible)) << 1
        | u64::from(kinds.iter().any(visible)) << 2
        | u64::from(collections.iter().any(visible)) << 3
        | u64::from(active.iter().any(visible)) << 4
        | u64::from(selections.iter().any(visible)) << 5
        | u64::from(views.iter().any(visible)) << 6
        | u64::from(viewports.iter().any(visible)) << 7;
    if reasons != 0 {
        state.requested = true;
        state.pending_invalidation_reasons |= reasons;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mark_projection_from_metadata_a(
    dependencies: Res<ProjectionDependencies>,
    repositories: Query<Entity, Changed<RepositoryMetadata>>,
    branches: Query<Entity, Changed<BranchMetadata>>,
    commits: Query<Entity, Changed<CommitMetadata>>,
    commit_fields: Query<Entity, Changed<CommitFieldMetadata>>,
    commit_creation: Query<Entity, Changed<CommitCreationMetadata>>,
    files: Query<Entity, Changed<FileMetadata>>,
    directories: Query<Entity, Changed<FileTreeDirectoryMetadata>>,
    diffs: Query<Entity, Changed<FileChangesMetadata>>,
    mut state: ResMut<ProjectionRuntimeState>,
) {
    let visible = |entity| dependencies.0.contains(&entity);
    let reasons = u64::from(repositories.iter().any(visible)) << 8
        | u64::from(branches.iter().any(visible)) << 9
        | u64::from(commits.iter().any(visible)) << 10
        | u64::from(commit_fields.iter().any(visible)) << 11
        | u64::from(commit_creation.iter().any(visible)) << 12
        | u64::from(files.iter().any(visible)) << 13
        | u64::from(directories.iter().any(visible)) << 14
        | u64::from(diffs.iter().any(visible)) << 15;
    if reasons != 0 {
        state.requested = true;
        state.pending_invalidation_reasons |= reasons;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mark_projection_from_metadata_b(
    dependencies: Res<ProjectionDependencies>,
    working_files: Query<Entity, Changed<WorkingTreeFileMetadata>>,
    working_diffs: Query<Entity, Changed<WorkingTreeFileChangesMetadata>>,
    reflog: Query<Entity, Changed<ReflogEntryMetadata>>,
    remotes: Query<Entity, Changed<RemoteMetadata>>,
    log_entries: Query<Entity, Changed<GitOperationLogEntryMetadata>>,
    interactions: Query<Entity, Changed<InteractionContextMetadata>>,
    mut state: ResMut<ProjectionRuntimeState>,
) {
    let visible = |entity| dependencies.0.contains(&entity);
    let reasons = u64::from(working_files.iter().any(visible)) << 16
        | u64::from(working_diffs.iter().any(visible)) << 17
        | u64::from(reflog.iter().any(visible)) << 18
        | u64::from(remotes.iter().any(visible)) << 19
        | u64::from(log_entries.iter().any(visible)) << 20
        | u64::from(interactions.iter().any(visible)) << 21;
    if reasons != 0 {
        state.requested = true;
        state.pending_invalidation_reasons |= reasons;
    }
}

pub(super) fn apply_viewport_measurements(
    mut measurements: MessageReader<ViewportMeasurement>,
    mut viewports: Query<&mut DatasetViewport>,
) {
    for measurement in measurements.read() {
        if let Ok(mut viewport) = viewports.get_mut(measurement.dataset) {
            let page_size = measurement.page_size.max(1);
            let max_offset = viewport.content_length.saturating_sub(page_size);
            let offset = viewport.offset.min(max_offset);
            if viewport.page_size != page_size || viewport.offset != offset {
                viewport.page_size = page_size;
                viewport.offset = offset;
            }
        }
    }
}

pub(super) fn update_dataset_viewports(world: &mut World) {
    let mut entities = Vec::new();
    if let Some(mode) = world.get_resource::<ActiveRenderMode>() {
        mode.layout.dataset_entities(&mut entities);
    }
    entities.sort_unstable();
    entities.dedup();
    for entity in entities {
        let content_length = dataset_content_length(world, entity);
        let active_element = world
            .get::<DatasetActiveElement>(entity)
            .and_then(|active| active.0);
        let active_position = active_element.and_then(|active| {
            world
                .get::<DatasetCollection>(entity)
                .and_then(|collection| collection.position(active))
        });
        if let Some(mut viewport) = world.get_mut::<DatasetViewport>(entity) {
            let page_size = viewport.page_size.max(1);
            let max_offset = content_length.saturating_sub(page_size);
            let mut offset = viewport.offset.min(max_offset);
            if let Some(active_position) = active_position {
                if active_position < offset {
                    offset = active_position;
                } else if active_position >= offset.saturating_add(page_size) {
                    offset = active_position
                        .saturating_add(1)
                        .saturating_sub(page_size)
                        .min(max_offset);
                }
            }
            if viewport.content_length != content_length
                || viewport.page_size != page_size
                || viewport.offset != offset
            {
                viewport.content_length = content_length;
                viewport.page_size = page_size;
                viewport.offset = offset;
            }
        }
    }
}

fn dataset_content_length(world: &World, entity: Entity) -> usize {
    if let Some(interaction) = world.get::<InteractionContextMetadata>(entity) {
        return match &interaction.kind {
            InteractionContextKind::Inactive => 0,
            InteractionContextKind::Help { entries } => entries.len(),
            InteractionContextKind::CommandPalette { query, entries, .. } => {
                entries.iter().filter(|entry| entry.matches(query)).count() + 2
            }
            InteractionContextKind::Notice { message, .. } => message.lines().count(),
            InteractionContextKind::Confirmation { options, .. } => options.len() + 1,
            InteractionContextKind::TextInput { .. } => 3,
        };
    }
    if let Some(diff) = world.get::<FileChangesMetadata>(entity) {
        return diff.0.header.len()
            + diff
                .0
                .hunks
                .iter()
                .map(|hunk| 1 + hunk.lines.len())
                .sum::<usize>();
    }
    if let Some(commit) = world.get::<CommitCreationMetadata>(entity) {
        return commit.staged_paths.len() + 3;
    }
    if let Some(diff) = world.get::<WorkingTreeFileChangesMetadata>(entity) {
        return diff
            .0
            .sections
            .iter()
            .map(|section| 1 + section.lines.len())
            .sum();
    }
    if world
        .get::<DatasetType>(entity)
        .is_some_and(|kind| kind.0 == DatasetKind::Commit)
    {
        return world
            .get::<DatasetCollection>(entity)
            .map_or(0, |collection| collection.0.len());
    }
    if world.get::<FileMetadata>(entity).is_some()
        || world.get::<WorkingTreeFileMetadata>(entity).is_some()
    {
        return 6;
    }
    if world.get::<FileTreeDirectoryMetadata>(entity).is_some() {
        return 1;
    }
    if world.get::<GitOperationLogEntryMetadata>(entity).is_some() {
        return 7;
    }
    world
        .get::<DatasetCollection>(entity)
        .map_or(0, |collection| collection.0.len())
}

pub(super) fn build_ui_frame(world: &mut World) {
    if !world.resource::<ProjectionRuntimeState>().requested {
        world.resource_mut::<ProjectionRuntimeState>().skipped_count += 1;
        return;
    }
    {
        let mut state = world.resource_mut::<ProjectionRuntimeState>();
        state.requested = false;
        state.build_count = state.build_count.wrapping_add(1);
        state.last_invalidation_reasons = state.pending_invalidation_reasons;
        state.pending_invalidation_reasons = 0;
    }
    if !world.contains_resource::<ProjectionDiagnostics>() {
        world.init_resource::<ProjectionDiagnostics>();
    }

    let mut diagnostics = Vec::new();
    let active = world
        .get_resource::<ActiveUiContext>()
        .map(|context| context.active_dataset);
    let layout = world
        .get_resource::<ActiveRenderMode>()
        .map(|mode| mode.layout.clone());
    let projected_layout = layout
        .as_ref()
        .map(|layout| project_layout(world, layout, active, &mut diagnostics))
        .unwrap_or(UiLayoutProjection::Empty);

    let mut dependencies = HashSet::new();
    if let Some(layout) = &layout {
        let mut panels = Vec::new();
        layout.dataset_entities(&mut panels);
        for panel in panels {
            dependencies.insert(panel);
            if let Some(collection) = world.get::<DatasetCollection>(panel) {
                dependencies.extend(collection.entities());
            }
        }
    }
    if let Some(repository) = world.get_resource::<ActiveUiContext>().and_then(|context| {
        context
            .render_bindings
            .get(&RenderBindingId::CurrentRepository)
    }) {
        dependencies.insert(repository);
    }
    world.resource_mut::<ProjectionDependencies>().0 = dependencies;

    let mut footer_bindings = world
        .get_resource::<ResolvedOperationSet>()
        .map(|operations| {
            operations
                .key_bindings
                .values()
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    footer_bindings.sort_by(|left, right| left.stroke.cmp(&right.stroke));
    let footer = FooterProjection {
        bindings: footer_bindings,
    };
    let status = project_status(world);

    let previous = world.resource::<UiFrame>().clone();
    let mut next = UiFrame {
        generation: previous.generation,
        layout: projected_layout,
        footer,
        status,
    };
    if next != previous {
        next.generation = previous.generation.wrapping_add(1);
        *world.resource_mut::<UiFrame>() = next;
    }

    if world.resource::<ProjectionDiagnostics>().0 != diagnostics {
        world.resource_mut::<ProjectionDiagnostics>().0 = diagnostics;
    }
}

fn project_layout(
    world: &World,
    layout: &ResolvedRenderLayout,
    active: Option<Entity>,
    diagnostics: &mut Vec<ProjectionDiagnostic>,
) -> UiLayoutProjection {
    match layout {
        ResolvedRenderLayout::Row(children) => UiLayoutProjection::Row(
            children
                .iter()
                .map(|child| project_layout(world, child, active, diagnostics))
                .collect(),
        ),
        ResolvedRenderLayout::Column(children) => UiLayoutProjection::Column(
            children
                .iter()
                .map(|child| project_layout(world, child, active, diagnostics))
                .collect(),
        ),
        ResolvedRenderLayout::Overlay(children) => UiLayoutProjection::Overlay(
            children
                .iter()
                .map(|child| project_layout(world, child, active, diagnostics))
                .collect(),
        ),
        ResolvedRenderLayout::Dataset {
            dataset,
            proxy,
            constraint,
            activatable,
        } => {
            let panel = project_dataset(world, *dataset, proxy, active, diagnostics);
            UiLayoutProjection::Dataset {
                constraint: *constraint,
                activatable: *activatable,
                panel: Box::new(panel),
            }
        }
    }
}

fn project_dataset(
    world: &World,
    dataset: Entity,
    proxy_id: &RenderProxyId,
    active: Option<Entity>,
    diagnostics: &mut Vec<ProjectionDiagnostic>,
) -> RenderProxyProjection {
    let Some(kind) = world.get::<DatasetType>(dataset).map(|kind| kind.0) else {
        diagnostics.push(ProjectionDiagnostic::MissingDataset(dataset));
        return empty_panel(dataset, proxy_id.clone(), active == Some(dataset));
    };
    let Some(spec) = world
        .resource::<RenderProxyRegistry>()
        .get(proxy_id)
        .cloned()
    else {
        diagnostics.push(ProjectionDiagnostic::MissingRenderProxy(proxy_id.clone()));
        return empty_panel(dataset, proxy_id.clone(), active == Some(dataset));
    };
    if spec.dataset_kind != kind {
        diagnostics.push(ProjectionDiagnostic::DatasetKindMismatch {
            dataset,
            proxy: proxy_id.clone(),
            expected: spec.dataset_kind,
            actual: kind,
        });
        return empty_panel(dataset, proxy_id.clone(), active == Some(dataset));
    }

    let content = match spec.renderer {
        RendererKind::Tree | RendererKind::List | RendererKind::LogList => {
            RenderContentProjection::Rows(project_rows(
                world,
                dataset,
                &spec,
                active == Some(dataset),
            ))
        }
        RendererKind::PathTree => RenderContentProjection::Rows(project_path_tree(
            world,
            dataset,
            &spec,
            active == Some(dataset),
        )),
        RendererKind::Detail => {
            RenderContentProjection::Detail(project_detail(world, dataset, &spec))
        }
        RendererKind::UnifiedDiff => project_unified_diff(world, dataset),
        RendererKind::SideBySideDiff => project_side_by_side_diff(world, dataset),
        RendererKind::Confirmation => project_interaction(world, dataset),
        RendererKind::CommitCreation => project_commit_creation(world, dataset, &spec),
    };

    RenderProxyProjection {
        dataset,
        proxy: spec.id,
        active: active == Some(dataset),
        title: interaction_title(world, dataset)
            .unwrap_or_else(|| dataset_title(world, dataset, kind)),
        style: spec.style,
        content,
    }
}

fn interaction_title(world: &World, dataset: Entity) -> Option<String> {
    let metadata = world.get::<InteractionContextMetadata>(dataset)?;
    Some(match &metadata.kind {
        InteractionContextKind::Inactive => String::new(),
        InteractionContextKind::Help { .. } => "Help".into(),
        InteractionContextKind::CommandPalette { .. } => "Command".into(),
        InteractionContextKind::Notice { title, .. }
        | InteractionContextKind::Confirmation { title, .. }
        | InteractionContextKind::TextInput { title, .. } => title.clone(),
    })
}

fn project_interaction(world: &World, dataset: Entity) -> RenderContentProjection {
    let Some(metadata) = world.get::<InteractionContextMetadata>(dataset) else {
        return RenderContentProjection::Empty;
    };
    let viewport = viewport_projection(world, dataset);
    let projection = match &metadata.kind {
        InteractionContextKind::Inactive => InteractionProjection {
            title: String::new(),
            prompt: None,
            input: None,
            lines: Vec::new(),
            error: None,
            viewport,
        },
        InteractionContextKind::Help { entries } => InteractionProjection {
            title: "Help".into(),
            prompt: Some("Shortcuts for the current view".into()),
            input: None,
            lines: entries
                .iter()
                .map(|entry| InteractionLineProjection {
                    key: Some(entry.binding.stroke.clone()),
                    text: entry.binding.label.clone(),
                    selected: false,
                })
                .collect(),
            error: None,
            viewport,
        },
        InteractionContextKind::CommandPalette {
            query,
            entries,
            selected,
        } => InteractionProjection {
            title: "Command".into(),
            prompt: Some("Type a command name".into()),
            input: Some(query.clone()),
            lines: entries
                .iter()
                .filter(|entry| entry.matches(query))
                .enumerate()
                .map(|(index, entry)| InteractionLineProjection {
                    key: None,
                    text: format!("{}  {}", entry.name, entry.label),
                    selected: index == *selected,
                })
                .collect(),
            error: None,
            viewport,
        },
        InteractionContextKind::Notice { message, .. } => InteractionProjection {
            title: interaction_title(world, dataset).unwrap_or_default(),
            prompt: None,
            input: None,
            lines: message
                .lines()
                .map(|line| InteractionLineProjection {
                    key: None,
                    text: line.into(),
                    selected: false,
                })
                .collect(),
            error: None,
            viewport,
        },
        InteractionContextKind::Confirmation {
            prompt,
            options,
            selected,
            ..
        } => InteractionProjection {
            title: interaction_title(world, dataset).unwrap_or_default(),
            prompt: Some(prompt.clone()),
            input: None,
            lines: options
                .iter()
                .enumerate()
                .map(|(index, option)| InteractionLineProjection {
                    key: None,
                    text: option.clone(),
                    selected: index == *selected,
                })
                .collect(),
            error: None,
            viewport,
        },
        InteractionContextKind::TextInput {
            prompt,
            input,
            error,
            ..
        } => InteractionProjection {
            title: interaction_title(world, dataset).unwrap_or_default(),
            prompt: Some(prompt.clone()),
            input: Some(input.clone()),
            lines: Vec::new(),
            error: error.clone(),
            viewport,
        },
    };
    RenderContentProjection::Interaction(projection)
}

fn project_commit_creation(
    world: &World,
    dataset: Entity,
    spec: &RenderProxySpec,
) -> RenderContentProjection {
    let Some(metadata) = world.get::<CommitCreationMetadata>(dataset) else {
        return RenderContentProjection::Empty;
    };
    let has_field = |field| spec.fields.iter().any(|spec| spec.field == field);
    let lines = if has_field(FieldId::CommitCreationStagedFiles) {
        std::iter::once(InteractionLineProjection {
            key: None,
            text: format!("{} staged file(s)", metadata.staged_paths.len()),
            selected: false,
        })
        .chain(
            metadata
                .staged_paths
                .iter()
                .map(|path| InteractionLineProjection {
                    key: None,
                    text: format!("  {}", path.as_str()),
                    selected: false,
                }),
        )
        .collect()
    } else {
        Vec::new()
    };
    RenderContentProjection::Interaction(InteractionProjection {
        title: "Create Commit".into(),
        prompt: has_field(FieldId::CommitCreationMessage).then(|| "Commit message".into()),
        input: has_field(FieldId::CommitCreationMessage).then(|| metadata.message.clone()),
        lines,
        error: has_field(FieldId::CommitCreationError)
            .then(|| metadata.error.clone())
            .flatten(),
        viewport: viewport_projection(world, dataset),
    })
}

fn empty_panel(dataset: Entity, proxy: RenderProxyId, active: bool) -> RenderProxyProjection {
    RenderProxyProjection {
        dataset,
        proxy,
        active,
        title: String::new(),
        style: pitui_data::StyleSpec::default(),
        content: RenderContentProjection::Empty,
    }
}

fn project_rows(
    world: &World,
    dataset: Entity,
    spec: &RenderProxySpec,
    dataset_is_active: bool,
) -> RowsProjection {
    let elements = world
        .get::<DatasetCollection>(dataset)
        .map(|collection| collection.0.as_slice())
        .unwrap_or_default();
    let active_element = world
        .get::<DatasetActiveElement>(dataset)
        .and_then(|active| active.0);
    let selected = world
        .get::<DatasetSelection>(dataset)
        .map(|selection| selection.0.iter().copied().collect::<HashSet<_>>())
        .unwrap_or_default();
    RowsProjection {
        rows: elements
            .iter()
            .copied()
            .map(|element| RowProjection {
                entity: element.entity,
                kind: RowProjectionKind::Item,
                depth: element.depth,
                cells: project_fields(world, element.entity, &spec.fields),
                active: dataset_is_active && active_element == Some(element.entity),
                selected: selected.contains(&element.entity),
            })
            .collect(),
        viewport: viewport_projection(world, dataset),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathTreeEntry {
    entity: Entity,
    depth: usize,
    directory: bool,
    file_leaf: bool,
}

fn project_path_tree(
    world: &World,
    dataset: Entity,
    spec: &RenderProxySpec,
    dataset_is_active: bool,
) -> RowsProjection {
    let active_element = world
        .get::<DatasetActiveElement>(dataset)
        .and_then(|active| active.0);
    let selected = world
        .get::<DatasetSelection>(dataset)
        .map(|selection| selection.0.iter().copied().collect::<HashSet<_>>())
        .unwrap_or_default();
    let entries = path_tree_entries(world, dataset);
    let path_field_indices: Vec<usize> = spec
        .fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            matches!(field.field, FieldId::FilePath | FieldId::DatasetLabel).then_some(index)
        })
        .collect();

    RowsProjection {
        rows: entries
            .into_iter()
            .map(|entry| {
                let mut cells = project_fields(world, entry.entity, &spec.fields);
                if (entry.file_leaf || entry.directory)
                    && let Some(mut name) = file_path(world, entry.entity).map(file_name)
                {
                    if entry.directory {
                        name.push('/');
                    }
                    let mut replaced = false;
                    for &index in &path_field_indices {
                        if let Some(cell) = cells.get_mut(index) {
                            cell.text.clone_from(&name);
                            replaced = true;
                        }
                    }
                    if !replaced {
                        cells.push(CellProjection {
                            label: None,
                            text: name,
                        });
                    }
                }
                RowProjection {
                    entity: entry.entity,
                    kind: if entry.directory {
                        RowProjectionKind::Directory
                    } else {
                        RowProjectionKind::Item
                    },
                    depth: entry.depth,
                    cells,
                    active: dataset_is_active && active_element == Some(entry.entity),
                    selected: selected.contains(&entry.entity),
                }
            })
            .collect(),
        viewport: viewport_projection(world, dataset),
    }
}

fn path_tree_entries(world: &World, dataset: Entity) -> Vec<PathTreeEntry> {
    let elements = world
        .get::<DatasetCollection>(dataset)
        .map(|collection| collection.0.as_slice())
        .unwrap_or_default();
    elements
        .iter()
        .copied()
        .map(|element| {
            let kind = world.get::<DatasetType>(element.entity).map(|kind| kind.0);
            PathTreeEntry {
                entity: element.entity,
                depth: element.depth,
                directory: kind == Some(DatasetKind::FileTreeDirectory),
                file_leaf: matches!(kind, Some(DatasetKind::File | DatasetKind::WorkingTreeFile)),
            }
        })
        .collect()
}

fn file_path(world: &World, entity: Entity) -> Option<&pitui_core::GitPath> {
    if let Some(metadata) = world.get::<FileTreeDirectoryMetadata>(entity) {
        return Some(&metadata.0);
    }
    if let Some(metadata) = world.get::<FileMetadata>(entity) {
        return Some(&metadata.0.path);
    }
    world
        .get::<WorkingTreeFileMetadata>(entity)
        .map(|metadata| &metadata.0.path)
}

fn file_name(path: &pitui_core::GitPath) -> String {
    path.as_bytes()
        .rsplit(|byte| *byte == b'/')
        .find(|component| !component.is_empty())
        .map_or_else(
            || path.as_str().to_owned(),
            |component| String::from_utf8_lossy(component).into_owned(),
        )
}

fn project_detail(world: &World, dataset: Entity, spec: &RenderProxySpec) -> DetailProjection {
    DetailProjection {
        fields: project_fields(world, dataset, &spec.fields),
        viewport: viewport_projection(world, dataset),
    }
}

fn project_fields(world: &World, entity: Entity, specs: &[FieldSpec]) -> Vec<CellProjection> {
    specs
        .iter()
        .filter_map(|spec| {
            field_value(world, entity, spec.field).and_then(|value| {
                format_value(value, &spec.format).map(|text| CellProjection {
                    label: spec.label.clone(),
                    text,
                })
            })
        })
        .collect()
}

enum RawFieldValue {
    Text(String),
    Many(Vec<String>),
}

fn field_value(world: &World, entity: Entity, field: FieldId) -> Option<RawFieldValue> {
    use FieldId as Field;

    let text = |value: String| Some(RawFieldValue::Text(value));
    match field {
        Field::DatasetLabel => dataset_label(world, entity).map(RawFieldValue::Text),
        Field::RepositoryName => world
            .get::<RepositoryMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.0.name.clone())),
        Field::RepositoryPath => world
            .get::<RepositoryMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.0.root.display().to_string())),
        Field::RepositoryCurrentBranch => world
            .get::<RepositoryMetadata>(entity)
            .and_then(|metadata| metadata.0.current_branch.as_ref())
            .map(|branch| RawFieldValue::Text(branch.0.clone())),
        Field::BranchCurrentMarker => world.get::<BranchMetadata>(entity).map(|metadata| {
            RawFieldValue::Text(if metadata.0.is_current { "*" } else { " " }.into())
        }),
        Field::BranchName => world
            .get::<BranchMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.0.name.0.clone())),
        Field::BranchHead => world
            .get::<BranchMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.0.head.0.clone())),
        Field::BranchAuthoredAt => world
            .get::<BranchMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.0.commit_date.clone())),
        Field::BranchSubject => world
            .get::<BranchMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.0.subject.clone())),
        Field::CommitHash => world
            .get::<CommitMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.summary.hash.0.clone())),
        Field::CommitAuthor => world
            .get::<CommitMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.summary.author.clone())),
        Field::CommitAuthoredAt => world
            .get::<CommitMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.summary.authored_at.clone())),
        Field::CommitTags => world
            .get::<CommitMetadata>(entity)
            .map(|metadata| RawFieldValue::Many(metadata.tags.clone())),
        Field::CommitSubject => world
            .get::<CommitMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.summary.subject.clone())),
        Field::CommitMessage => world
            .get::<CommitMetadata>(entity)
            .and_then(|metadata| metadata.message.clone())
            .map(RawFieldValue::Text),
        Field::CommitFieldLabel => world
            .get::<CommitFieldMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.field.label().into())),
        Field::CommitFieldValue => world.get::<CommitFieldMetadata>(entity).map(|metadata| {
            RawFieldValue::Text(metadata.value.lines().collect::<Vec<_>>().join(" ↵ "))
        }),
        Field::CommitCreationStagedFiles => {
            world.get::<CommitCreationMetadata>(entity).map(|metadata| {
                RawFieldValue::Many(
                    metadata
                        .staged_paths
                        .iter()
                        .map(|path| path.as_str().to_owned())
                        .collect(),
                )
            })
        }
        Field::CommitCreationMessage => world
            .get::<CommitCreationMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.message.clone())),
        Field::CommitCreationError => world
            .get::<CommitCreationMetadata>(entity)
            .and_then(|metadata| metadata.error.clone())
            .map(RawFieldValue::Text),
        Field::FileStatus => {
            if let Some(metadata) = world.get::<FileMetadata>(entity) {
                text(metadata.0.kind.marker().into())
            } else {
                world
                    .get::<WorkingTreeFileMetadata>(entity)
                    .map(|metadata| RawFieldValue::Text(metadata.0.status_code()))
            }
        }
        Field::FilePath => {
            if let Some(metadata) = world.get::<FileMetadata>(entity) {
                text(metadata.0.path.as_str().into())
            } else if let Some(metadata) = world.get::<FileTreeDirectoryMetadata>(entity) {
                text(metadata.0.as_str().into())
            } else {
                world
                    .get::<WorkingTreeFileMetadata>(entity)
                    .map(|metadata| RawFieldValue::Text(metadata.0.path.as_str().into()))
            }
        }
        Field::FileOldPath => {
            if let Some(metadata) = world.get::<FileMetadata>(entity) {
                metadata
                    .0
                    .old_path
                    .as_ref()
                    .map(|path| RawFieldValue::Text(path.as_str().into()))
            } else {
                world
                    .get::<WorkingTreeFileMetadata>(entity)
                    .and_then(|metadata| metadata.0.old_path.as_ref())
                    .map(|path| RawFieldValue::Text(path.as_str().into()))
            }
        }
        Field::FileAdditions => world
            .get::<FileMetadata>(entity)
            .and_then(|metadata| metadata.0.additions)
            .map(|count| RawFieldValue::Text(format!("+{count}"))),
        Field::FileDeletions => world
            .get::<FileMetadata>(entity)
            .and_then(|metadata| metadata.0.deletions)
            .map(|count| RawFieldValue::Text(format!("-{count}"))),
        Field::FileBinary => world.get::<FileMetadata>(entity).and_then(|metadata| {
            metadata
                .0
                .is_binary
                .then(|| RawFieldValue::Text("binary".into()))
        }),
        Field::ReflogSelector => reflog_text(world, entity, |entry| entry.selector.clone()),
        Field::ReflogHash => reflog_text(world, entity, |entry| entry.hash.0.clone()),
        Field::ReflogAction => reflog_text(world, entity, |entry| entry.action.clone()),
        Field::ReflogMessage => reflog_text(world, entity, |entry| entry.message.clone()),
        Field::ReflogAuthor => reflog_text(world, entity, |entry| entry.author.clone()),
        Field::ReflogAuthoredAt => reflog_text(world, entity, |entry| entry.authored_at.clone()),
        Field::RemoteName => remote_text(world, entity, |remote| remote.name.clone()),
        Field::RemoteFetchUrls => world
            .get::<RemoteMetadata>(entity)
            .map(|metadata| RawFieldValue::Many(metadata.0.fetch_urls.clone())),
        Field::RemotePushUrls => world
            .get::<RemoteMetadata>(entity)
            .map(|metadata| RawFieldValue::Many(metadata.0.push_urls.clone())),
        Field::RemoteUpstream => world.get::<RemoteMetadata>(entity).and_then(|metadata| {
            metadata
                .0
                .is_upstream
                .then(|| RawFieldValue::Text("upstream".into()))
        }),
        Field::RemotePushTarget => world.get::<RemoteMetadata>(entity).and_then(|metadata| {
            metadata
                .0
                .is_push_target
                .then(|| RawFieldValue::Text("push target".into()))
        }),
        Field::RemotePolicy => world.get::<RemoteMetadata>(entity).map(|metadata| {
            RawFieldValue::Text(
                if metadata.0.urls_match() {
                    "shared URL"
                } else {
                    "blocked: fetch/push URL mismatch"
                }
                .into(),
            )
        }),
        Field::GitOperationStartedAt => world
            .get::<GitOperationLogEntryMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.started_at_utc.clone())),
        Field::GitOperationName => world
            .get::<GitOperationLogEntryMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.operation.clone())),
        Field::GitOperationRepository => {
            world
                .get::<GitOperationLogEntryMetadata>(entity)
                .map(|metadata| {
                    RawFieldValue::Text(metadata.repository.as_path().display().to_string())
                })
        }
        Field::GitOperationDuration => world
            .get::<GitOperationLogEntryMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(format!("{} ms", metadata.duration_ms))),
        Field::GitOperationStatus => world
            .get::<GitOperationLogEntryMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.status.label().into())),
        Field::GitOperationMessage => world
            .get::<GitOperationLogEntryMetadata>(entity)
            .map(|metadata| RawFieldValue::Text(metadata.message.clone())),
        Field::GitOperationAbort => {
            world
                .get::<GitOperationLogEntryMetadata>(entity)
                .map(|metadata| {
                    RawFieldValue::Text(if metadata.abort_attempted {
                        metadata
                            .abort_result
                            .clone()
                            .unwrap_or_else(|| "attempted".into())
                    } else {
                        "not attempted".into()
                    })
                })
        }
    }
}

fn reflog_text(
    world: &World,
    entity: Entity,
    value: impl FnOnce(&pitui_core::ReflogEntry) -> String,
) -> Option<RawFieldValue> {
    world
        .get::<ReflogEntryMetadata>(entity)
        .map(|metadata| RawFieldValue::Text(value(&metadata.0)))
}

fn remote_text(
    world: &World,
    entity: Entity,
    value: impl FnOnce(&pitui_core::RemoteInfo) -> String,
) -> Option<RawFieldValue> {
    world
        .get::<RemoteMetadata>(entity)
        .map(|metadata| RawFieldValue::Text(value(&metadata.0)))
}

fn format_value(value: RawFieldValue, format: &FieldFormat) -> Option<String> {
    let raw = match (value, format) {
        (RawFieldValue::Many(values), FieldFormat::Joined { separator }) => values.join(separator),
        (RawFieldValue::Many(values), _) => values.join(", "),
        (RawFieldValue::Text(value), _) => value,
    };
    if raw.is_empty() {
        return None;
    }

    let formatted = match format {
        FieldFormat::Plain | FieldFormat::Joined { .. } => raw,
        FieldFormat::Hash { length } => raw.chars().take(*length).collect(),
        FieldFormat::DateTime { precision } => format_datetime(&raw, *precision),
    };
    (!formatted.is_empty()).then_some(formatted)
}

fn format_datetime(value: &str, precision: DateTimePrecision) -> String {
    let end = match precision {
        DateTimePrecision::Date => 10,
        DateTimePrecision::Minute => 16,
        DateTimePrecision::Second => 19,
        DateTimePrecision::Raw => return value.into(),
    };
    if value.len() < end {
        return value.into();
    }
    value[..end].replace('T', " ")
}

fn dataset_label(world: &World, entity: Entity) -> Option<String> {
    if let Some(metadata) = world.get::<RepositoryMetadata>(entity) {
        return Some(metadata.0.name.clone());
    }
    if let Some(metadata) = world.get::<BranchMetadata>(entity) {
        return Some(metadata.0.name.0.clone());
    }
    if let Some(metadata) = world.get::<CommitFieldMetadata>(entity) {
        return Some(metadata.field.label().into());
    }
    if let Some(metadata) = world.get::<FileTreeDirectoryMetadata>(entity) {
        return Some(metadata.0.as_str().into());
    }
    if let Some(metadata) = world.get::<WorkingTreeFileMetadata>(entity) {
        return Some(metadata.0.path.as_str().into());
    }
    if let Some(metadata) = world.get::<GitOperationLogEntryMetadata>(entity) {
        return Some(format!(
            "{} · {}",
            metadata.status.label(),
            metadata.operation
        ));
    }
    match &world.get::<DatasetKey>(entity)?.0 {
        DatasetIdentity::WorkingTreeFiles { boundary, .. } => Some(
            match boundary {
                ChangeBoundary::Staged => "Staged",
                ChangeBoundary::Unstaged => "Unstaged",
            }
            .into(),
        ),
        DatasetIdentity::FileDirectory { path, .. }
        | DatasetIdentity::WorkingTreeDirectory { path, .. }
        | DatasetIdentity::WorkingTreeFile { path, .. }
        | DatasetIdentity::WorkingTreeFileChanges { path, .. }
        | DatasetIdentity::File { path, .. }
        | DatasetIdentity::FileChanges { path, .. } => Some(path.as_str().into()),
        DatasetIdentity::Branch { name, .. } => Some(name.0.clone()),
        DatasetIdentity::Commit { hash, .. } => Some(hash.short().into()),
        _ => None,
    }
}

fn project_unified_diff(world: &World, dataset: Entity) -> RenderContentProjection {
    let (path, header, binary, hunks) =
        if let Some(metadata) = world.get::<FileChangesMetadata>(dataset) {
            (
                metadata.0.path.as_str().to_string(),
                metadata.0.header.clone(),
                metadata.0.is_binary,
                metadata.0.hunks.clone(),
            )
        } else if let Some(metadata) = world.get::<WorkingTreeFileChangesMetadata>(dataset) {
            (
                metadata.0.path.as_str().to_string(),
                Vec::new(),
                false,
                working_tree_hunks(&metadata.0),
            )
        } else {
            return RenderContentProjection::Empty;
        };
    RenderContentProjection::UnifiedDiff(UnifiedDiffProjection {
        path,
        header,
        binary,
        hunks: hunks
            .into_iter()
            .map(|hunk| UnifiedDiffHunkProjection {
                header: hunk.header,
                lines: hunk
                    .lines
                    .into_iter()
                    .map(|line| DiffLineProjection {
                        old_line_no: line.old_line_no,
                        new_line_no: line.new_line_no,
                        kind: map_diff_line_kind(line.kind),
                        text: line.text,
                    })
                    .collect(),
            })
            .collect(),
        viewport: viewport_projection(world, dataset),
    })
}

fn project_side_by_side_diff(world: &World, dataset: Entity) -> RenderContentProjection {
    let (path, header, binary, hunks) =
        if let Some(metadata) = world.get::<FileChangesMetadata>(dataset) {
            (
                metadata.0.path.as_str().to_string(),
                metadata.0.header.clone(),
                metadata.0.is_binary,
                metadata.0.hunks.clone(),
            )
        } else if let Some(metadata) = world.get::<WorkingTreeFileChangesMetadata>(dataset) {
            (
                metadata.0.path.as_str().to_string(),
                Vec::new(),
                false,
                working_tree_hunks(&metadata.0),
            )
        } else {
            return RenderContentProjection::Empty;
        };
    RenderContentProjection::SideBySideDiff(SideBySideDiffProjection {
        path,
        header,
        binary,
        hunks: hunks
            .iter()
            .map(|hunk| SideBySideHunkProjection {
                header: hunk.header.clone(),
                rows: side_by_side_rows(hunk)
                    .into_iter()
                    .map(|row| SideBySideRowProjection {
                        left_line_no: row.left_line_no,
                        right_line_no: row.right_line_no,
                        left_text: row.left_text,
                        right_text: row.right_text,
                        left_kind: map_diff_cell_kind(row.left_kind),
                        right_kind: map_diff_cell_kind(row.right_kind),
                    })
                    .collect(),
            })
            .collect(),
        viewport: viewport_projection(world, dataset),
    })
}

fn map_diff_line_kind(kind: DiffLineKind) -> DiffLineKindProjection {
    match kind {
        DiffLineKind::Context => DiffLineKindProjection::Context,
        DiffLineKind::Addition => DiffLineKindProjection::Addition,
        DiffLineKind::Deletion => DiffLineKindProjection::Deletion,
        DiffLineKind::Metadata => DiffLineKindProjection::Metadata,
    }
}

fn map_diff_cell_kind(kind: pitui_core::DiffCellKind) -> DiffCellKindProjection {
    match kind {
        pitui_core::DiffCellKind::Added => DiffCellKindProjection::Added,
        pitui_core::DiffCellKind::Deleted => DiffCellKindProjection::Deleted,
        pitui_core::DiffCellKind::Modified => DiffCellKindProjection::Modified,
        pitui_core::DiffCellKind::Empty | pitui_core::DiffCellKind::Context => {
            DiffCellKindProjection::Context
        }
    }
}

fn working_tree_hunks(diff: &WorkingTreeDiff) -> Vec<DiffHunk> {
    diff.sections
        .iter()
        .map(|section| DiffHunk {
            header: match section.kind {
                WorkingTreeDiffKind::Staged => "Staged".into(),
                WorkingTreeDiffKind::Worktree => "Unstaged".into(),
                WorkingTreeDiffKind::Untracked => "Untracked".into(),
            },
            old_start: 0,
            old_count: 0,
            new_start: 0,
            new_count: 0,
            lines: section
                .lines
                .iter()
                .map(|line| {
                    let (kind, text) = if line.starts_with("+++") || line.starts_with("---") {
                        (DiffLineKind::Metadata, line.as_str())
                    } else if let Some(text) = line.strip_prefix('+') {
                        (DiffLineKind::Addition, text)
                    } else if let Some(text) = line.strip_prefix('-') {
                        (DiffLineKind::Deletion, text)
                    } else if let Some(text) = line.strip_prefix(' ') {
                        (DiffLineKind::Context, text)
                    } else {
                        (DiffLineKind::Metadata, line.as_str())
                    };
                    pitui_core::DiffLine {
                        old_line_no: None,
                        new_line_no: None,
                        kind,
                        text: text.into(),
                    }
                })
                .collect(),
        })
        .collect()
}

fn viewport_projection(world: &World, dataset: Entity) -> ViewportProjection {
    world
        .get::<DatasetViewport>(dataset)
        .map(|viewport| ViewportProjection {
            offset: viewport.offset,
            page_size: viewport.page_size,
            content_length: viewport.content_length,
        })
        .unwrap_or_default()
}

fn dataset_title(world: &World, dataset: Entity, kind: DatasetKind) -> String {
    match kind {
        DatasetKind::RepositoriesBranches => "Repositories / Branches".into(),
        DatasetKind::Repository => world
            .get::<RepositoryMetadata>(dataset)
            .map(|metadata| metadata.0.name.clone())
            .unwrap_or_else(|| "Repository".into()),
        DatasetKind::Branch => dataset_label(world, dataset).unwrap_or_else(|| "Branch".into()),
        DatasetKind::Commits => match world.get::<DatasetKey>(dataset).map(|key| &key.0) {
            Some(DatasetIdentity::Commits { branch, .. }) => format!("Commits · {branch}"),
            _ => "Commits".into(),
        },
        DatasetKind::Commit => match world.get::<DatasetKey>(dataset).map(|key| &key.0) {
            Some(DatasetIdentity::Commit { hash, .. }) => format!("Commit · {}", hash.short()),
            _ => "Commit".into(),
        },
        DatasetKind::CommitField => {
            dataset_label(world, dataset).unwrap_or_else(|| "Commit Field".into())
        }
        DatasetKind::Files => world
            .get::<DatasetViewState>(dataset)
            .and_then(|state| state.0.as_ref())
            .map_or_else(|| "Files".into(), |view| format!("Files · {}", view.0)),
        DatasetKind::FileTreeDirectory => {
            dataset_label(world, dataset).unwrap_or_else(|| "Directory".into())
        }
        DatasetKind::File => dataset_label(world, dataset).unwrap_or_else(|| "File".into()),
        DatasetKind::FileChanges | DatasetKind::WorkingTreeFileChanges => {
            dataset_label(world, dataset).unwrap_or_else(|| "Diff".into())
        }
        DatasetKind::Changes => "Changes".into(),
        DatasetKind::WorkingTreeFiles => dataset_label(world, dataset).unwrap_or_default(),
        DatasetKind::WorkingTreeFile => dataset_label(world, dataset).unwrap_or_default(),
        DatasetKind::CommitCreation => "Create Commit".into(),
        DatasetKind::Reflog => "Reflog".into(),
        DatasetKind::ReflogEntry => "Reflog Entry".into(),
        DatasetKind::Remotes => "Remotes".into(),
        DatasetKind::Remote => dataset_label(world, dataset).unwrap_or_else(|| "Remote".into()),
        DatasetKind::InteractionContext => String::new(),
        DatasetKind::GitOperationLog => "Git Operation Log".into(),
        DatasetKind::GitOperationLogEntry => "Git Operation".into(),
    }
}

fn project_status(world: &World) -> StatusProjection {
    let repository = world
        .get_resource::<ActiveUiContext>()
        .and_then(|context| {
            context
                .render_bindings
                .get(&RenderBindingId::CurrentRepository)
        })
        .and_then(|entity| world.get::<RepositoryMetadata>(entity));
    let Some(repository) = repository else {
        return StatusProjection::default();
    };

    let mut items = vec![repository.0.name.clone()];
    if let Some(branch) = &repository.0.current_branch {
        items.push(branch.0.clone());
    }
    let status = &repository.0.status;
    if !status.is_clean() {
        items.push(format!(
            "staged:{} modified:{} untracked:{} conflicted:{}",
            status.staged, status.modified, status.untracked, status.conflicted
        ));
    }
    if status.ahead > 0 || status.behind > 0 {
        items.push(format!("ahead:{} behind:{}", status.ahead, status.behind));
    }
    StatusProjection { items }
}
