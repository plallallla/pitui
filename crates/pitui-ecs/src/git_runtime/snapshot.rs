use super::*;

#[derive(Clone, Debug)]
struct PlannedNode {
    identity: DatasetIdentity,
    kind: DatasetKind,
    template: DatasetTemplateId,
}

#[derive(Clone, Debug)]
struct PlannedChildren {
    parent: DatasetIdentity,
    children: Vec<DatasetIdentity>,
}

#[derive(Clone, Debug)]
enum MetadataUpdate {
    Branch(DatasetIdentity, Branch),
    Commit(DatasetIdentity, CommitMetadata),
    CommitField(DatasetIdentity, CommitFieldMetadata),
    File(DatasetIdentity, ChangedFile),
    FileTreeDirectory(DatasetIdentity, GitPath),
    FileChanges(DatasetIdentity, FileDiff),
    WorkingTreeFile(DatasetIdentity, WorkingTreeChange),
    WorkingTreeFileChanges(DatasetIdentity, WorkingTreeDiff),
    ReflogEntry(DatasetIdentity, ReflogEntry),
}

#[derive(Clone, Debug, Default)]
struct DatasetSnapshotPlan {
    nodes: Vec<PlannedNode>,
    children: Vec<PlannedChildren>,
    metadata: Vec<MetadataUpdate>,
    snapshots: Vec<DatasetIdentity>,
    invalidated_snapshots: Vec<DatasetIdentity>,
}

impl DatasetSnapshotPlan {
    fn add_node(
        &mut self,
        identity: DatasetIdentity,
        kind: DatasetKind,
        template: DatasetTemplateId,
    ) {
        self.nodes.push(PlannedNode {
            identity,
            kind,
            template,
        });
    }

    fn replace_children(&mut self, parent: DatasetIdentity, children: Vec<DatasetIdentity>) {
        self.snapshots.push(parent.clone());
        self.children.push(PlannedChildren { parent, children });
    }

    fn mark_snapshot(&mut self, identity: DatasetIdentity) {
        self.snapshots.push(identity);
    }

    fn invalidate_snapshot(&mut self, identity: DatasetIdentity) {
        self.invalidated_snapshots.push(identity);
    }
}

pub(super) fn apply_payload(
    world: &mut World,
    request_id: GitRequestId,
    data: &GitCommandData,
    payload: ParsedGitPayload,
) -> Result<(), GitDataError> {
    require_dataset(world, data.repository_dataset)?;
    let repository = repository_key(world, data.repository_dataset)?;
    let plan = match (&data.command, payload) {
        (
            GitCommand::StagePaths { .. }
            | GitCommand::UnstagePaths { .. }
            | GitCommand::Commit { .. }
            | GitCommand::CherryPick { .. }
            | GitCommand::Reset { .. },
            ParsedGitPayload::CommandSucceeded { message },
        ) => {
            let limit = world.resource::<GitRuntimeRetention>().mutation_entries;
            push_bounded(
                &mut world.resource_mut::<GitMutationSuccesses>().0,
                GitMutationSuccess {
                    request_id,
                    command: data.command.clone(),
                    message,
                },
                limit,
            );
            return Ok(());
        }
        (GitCommand::CherryPick { .. }, ParsedGitPayload::ConflictAborted { .. }) => return Ok(()),
        (GitCommand::LoadRepository, ParsedGitPayload::Repository(metadata)) => {
            return apply_repository_metadata(world, data.repository_dataset, metadata);
        }
        (GitCommand::LoadBranches, ParsedGitPayload::Branches(branches)) => {
            branches_plan(world, repository, branches)?
        }
        (
            GitCommand::LoadCommits { branch, .. },
            ParsedGitPayload::Commits {
                branch: payload_branch,
                commits,
            },
        ) if branch == &payload_branch => commits_plan(world, repository, payload_branch, commits)?,
        (GitCommand::LoadCommitDetail { commit }, ParsedGitPayload::CommitDetail(detail))
            if commit == &detail.commit.hash =>
        {
            commit_detail_plan(world, repository, detail)?
        }
        (GitCommand::LoadFileDiff { commit, path, .. }, ParsedGitPayload::FileDiff(diff))
            if commit == &diff.commit && path == &diff.path =>
        {
            file_diff_plan(world, repository, diff)?
        }
        (GitCommand::LoadWorkingTree, ParsedGitPayload::WorkingTree(changes)) => {
            working_tree_plan(world, repository, changes)?
        }
        (GitCommand::LoadReflog { .. }, ParsedGitPayload::Reflog(entries)) => {
            reflog_plan(world, repository, entries)?
        }
        (
            GitCommand::LoadWorkingTreeDiff {
                path,
                include_staged,
                include_worktree,
                untracked,
                ..
            },
            ParsedGitPayload::WorkingTreeDiff(diff),
        ) if path == &diff.path => {
            let boundary = if *include_staged && !*include_worktree && !*untracked {
                ChangeBoundary::Staged
            } else if !*include_staged && (*include_worktree || *untracked) {
                ChangeBoundary::Unstaged
            } else {
                return Err(GitDataError::PayloadDoesNotMatchCommand);
            };
            working_tree_diff_plan(world, repository, boundary, diff)?
        }
        _ => return Err(GitDataError::PayloadDoesNotMatchCommand),
    };

    validate_plan(world, &plan)?;
    apply_plan(world, plan)
}

fn repository_key(world: &World, entity: Entity) -> Result<RepositoryKey, GitDataError> {
    match world.get::<DatasetKey>(entity).map(|key| &key.0) {
        Some(DatasetIdentity::Repository(repository)) => Ok(repository.clone()),
        _ => Err(GitDataError::TargetIsNotRepository(entity)),
    }
}

fn template_for(world: &World, kind: DatasetKind) -> Result<DatasetTemplateId, GitDataError> {
    world
        .resource::<DefaultDatasetTemplates>()
        .get(kind)
        .cloned()
        .ok_or(GitDataError::MissingDefaultTemplate(kind))
}

fn existing_node(world: &World, entity: Entity) -> Result<PlannedNode, GitDataError> {
    Ok(PlannedNode {
        identity: world
            .get::<DatasetKey>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0
            .clone(),
        kind: world
            .get::<DatasetType>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0,
        template: world
            .get::<DatasetTemplateRef>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0
            .clone(),
    })
}

fn apply_repository_metadata(
    world: &mut World,
    repository_entity: Entity,
    metadata: Repository,
) -> Result<(), GitDataError> {
    let old_identity = world
        .get::<DatasetKey>(repository_entity)
        .ok_or(KernelError::MissingDataset(repository_entity))?
        .0
        .clone();
    let DatasetIdentity::Repository(old_repository) = &old_identity else {
        return Err(GitDataError::TargetIsNotRepository(repository_entity));
    };
    let actual_repository = RepositoryKey::new(metadata.root.clone());
    let actual_identity = DatasetIdentity::Repository(actual_repository.clone());
    if let Some(existing) = world.resource::<DatasetIndex>().get(&actual_identity)
        && existing != repository_entity
    {
        return Err(GitDataError::RepositoryIdentityCollision(actual_repository));
    }

    // All fallible checks finish before identity and metadata are committed.
    if old_repository != &actual_repository {
        let mut index = world.resource_mut::<DatasetIndex>();
        index.by_key.remove(&old_identity);
        index
            .by_key
            .insert(actual_identity.clone(), repository_entity);
        world
            .entity_mut(repository_entity)
            .insert(DatasetKey(actual_identity));
    }
    world
        .entity_mut(repository_entity)
        .insert(RepositoryMetadata(metadata));
    mark_snapshot(world, repository_entity);
    Ok(())
}

fn branches_plan(
    world: &World,
    repository: RepositoryKey,
    mut branches: Vec<Branch>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let branch_template = template_for(world, DatasetKind::Branch)?;
    let commits_template = template_for(world, DatasetKind::Commits)?;
    let repository_identity = DatasetIdentity::Repository(repository.clone());
    let repository_entity = world
        .resource::<DatasetIndex>()
        .get(&repository_identity)
        .ok_or_else(|| GitDataError::MissingPlannedIdentity(repository_identity.clone()))?;
    let repository_node = existing_node(world, repository_entity)?;
    if let Some(metadata) = world.get::<RepositoryMetadata>(repository_entity)
        && let Some(current) = &metadata.0.current_branch
        && !branches.iter().any(|branch| &branch.name == current)
    {
        branches.insert(
            0,
            Branch {
                name: current.clone(),
                full_ref: format!("refs/heads/{current}"),
                kind: BranchKind::Local,
                head: metadata.0.head.clone(),
                short_head: metadata.0.head.short().to_string(),
                commit_date: String::new(),
                subject: String::new(),
                is_current: true,
            },
        );
    }
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(
        repository_node.identity.clone(),
        repository_node.kind,
        repository_node.template,
    );

    let mut branch_ids = Vec::with_capacity(branches.len());
    for branch in branches {
        let branch_id = DatasetIdentity::Branch {
            repository: repository.clone(),
            name: branch.name.clone(),
        };
        let commits_id = DatasetIdentity::Commits {
            repository: repository.clone(),
            branch: branch.name.clone(),
        };
        plan.add_node(
            branch_id.clone(),
            DatasetKind::Branch,
            branch_template.clone(),
        );
        plan.add_node(
            commits_id.clone(),
            DatasetKind::Commits,
            commits_template.clone(),
        );
        plan.metadata
            .push(MetadataUpdate::Branch(branch_id.clone(), branch));
        plan.replace_children(branch_id.clone(), vec![commits_id]);
        branch_ids.push(branch_id);
    }
    plan.replace_children(repository_identity, branch_ids);
    Ok(plan)
}

fn commits_plan(
    world: &World,
    repository: RepositoryKey,
    branch: pitui_core::BranchName,
    commits: Vec<Commit>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let commits_template = template_for(world, DatasetKind::Commits)?;
    let commit_template = template_for(world, DatasetKind::Commit)?;
    let commit_field_template = template_for(world, DatasetKind::CommitField)?;
    let files_template = template_for(world, DatasetKind::Files)?;
    let commits_id = DatasetIdentity::Commits {
        repository: repository.clone(),
        branch,
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(commits_id.clone(), DatasetKind::Commits, commits_template);

    let mut commit_ids = Vec::with_capacity(commits.len());
    for commit in commits {
        let commit_id = DatasetIdentity::Commit {
            repository: repository.clone(),
            hash: commit.hash.clone(),
        };
        let files_id = DatasetIdentity::Files {
            repository: repository.clone(),
            commit: commit.hash.clone(),
        };
        let tags = tags_from_decorations(&commit.decorations);
        let message = world
            .resource::<DatasetIndex>()
            .get(&commit_id)
            .and_then(|entity| world.get::<CommitMetadata>(entity))
            .and_then(|metadata| metadata.message.clone());
        plan.add_node(
            commit_id.clone(),
            DatasetKind::Commit,
            commit_template.clone(),
        );
        plan.add_node(files_id.clone(), DatasetKind::Files, files_template.clone());
        let metadata = CommitMetadata {
            summary: commit,
            message,
            tags,
        };
        let mut commit_children =
            append_commit_fields(&mut plan, &repository, &metadata, &commit_field_template);
        commit_children.push(files_id);
        plan.metadata
            .push(MetadataUpdate::Commit(commit_id.clone(), metadata));
        plan.replace_children(commit_id.clone(), commit_children);
        commit_ids.push(commit_id);
    }
    plan.replace_children(commits_id, commit_ids);
    Ok(plan)
}

/// Materializes Commit detail fields as independently addressable Dataset
/// rows. The Commit remains their collection owner, while Files stays a sibling
/// in the canonical ownership DAG and is filtered out by Commit's List Manager.
fn append_commit_fields(
    plan: &mut DatasetSnapshotPlan,
    repository: &RepositoryKey,
    metadata: &CommitMetadata,
    template: &DatasetTemplateId,
) -> Vec<DatasetIdentity> {
    let value = |field| match field {
        CommitFieldKind::Hash => Some(metadata.summary.hash.0.clone()),
        CommitFieldKind::Author => Some(metadata.summary.author.clone()),
        CommitFieldKind::AuthoredAt => Some(
            metadata
                .summary
                .authored_at
                .chars()
                .take(16)
                .collect::<String>()
                .replace('T', " "),
        ),
        CommitFieldKind::Tags => (!metadata.tags.is_empty()).then(|| metadata.tags.join(", ")),
        CommitFieldKind::Subject => Some(metadata.summary.subject.clone()),
        CommitFieldKind::Message => metadata.message.clone().filter(|value| !value.is_empty()),
    };

    CommitFieldKind::ALL
        .into_iter()
        .filter_map(|field| {
            let value = value(field).filter(|value| !value.is_empty())?;
            let identity = DatasetIdentity::CommitField {
                repository: repository.clone(),
                commit: metadata.summary.hash.clone(),
                field,
            };
            plan.add_node(identity.clone(), DatasetKind::CommitField, template.clone());
            plan.metadata.push(MetadataUpdate::CommitField(
                identity.clone(),
                CommitFieldMetadata { field, value },
            ));
            Some(identity)
        })
        .collect()
}

#[derive(Default)]
struct FileTreePlanState {
    directory_ids: HashSet<DatasetIdentity>,
    relations: HashMap<DatasetIdentity, Vec<DatasetIdentity>>,
}

fn attach_file_to_tree(
    plan: &mut DatasetSnapshotPlan,
    tree: &mut FileTreePlanState,
    root: &DatasetIdentity,
    file: DatasetIdentity,
    path: &GitPath,
    directory_template: &DatasetTemplateId,
    directory_identity: impl Fn(GitPath) -> DatasetIdentity,
) {
    let components = path
        .as_bytes()
        .split(|byte| *byte == b'/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let mut parent = root.clone();
    let mut directory_path = Vec::new();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        if !directory_path.is_empty() {
            directory_path.push(b'/');
        }
        directory_path.extend_from_slice(component);
        let path = GitPath::from_bytes(directory_path.clone());
        let directory = directory_identity(path.clone());
        if tree.directory_ids.insert(directory.clone()) {
            plan.add_node(
                directory.clone(),
                DatasetKind::FileTreeDirectory,
                directory_template.clone(),
            );
            plan.metadata
                .push(MetadataUpdate::FileTreeDirectory(directory.clone(), path));
        }
        tree.relations
            .entry(parent)
            .or_default()
            .push(directory.clone());
        tree.relations.entry(directory.clone()).or_default();
        parent = directory;
    }
    tree.relations.entry(parent).or_default().push(file);
}

fn apply_file_tree_relations(
    plan: &mut DatasetSnapshotPlan,
    relations: HashMap<DatasetIdentity, Vec<DatasetIdentity>>,
) {
    let mut relations = relations.into_iter().collect::<Vec<_>>();
    relations.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (parent, mut children) in relations {
        children.sort();
        children.dedup();
        plan.replace_children(parent, children);
    }
}

fn commit_detail_plan(
    world: &World,
    repository: RepositoryKey,
    detail: CommitDetail,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let commit_template = template_for(world, DatasetKind::Commit)?;
    let commit_field_template = template_for(world, DatasetKind::CommitField)?;
    let files_template = template_for(world, DatasetKind::Files)?;
    let directory_template = template_for(world, DatasetKind::FileTreeDirectory)?;
    let file_template = template_for(world, DatasetKind::File)?;
    let changes_template = template_for(world, DatasetKind::FileChanges)?;
    let commit_id = DatasetIdentity::Commit {
        repository: repository.clone(),
        hash: detail.commit.hash.clone(),
    };
    let files_id = DatasetIdentity::Files {
        repository: repository.clone(),
        commit: detail.commit.hash.clone(),
    };
    let tags = world
        .resource::<DatasetIndex>()
        .get(&commit_id)
        .and_then(|entity| world.get::<CommitMetadata>(entity))
        .map(|metadata| metadata.tags.clone())
        .unwrap_or_default();

    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(commit_id.clone(), DatasetKind::Commit, commit_template);
    plan.add_node(files_id.clone(), DatasetKind::Files, files_template);
    let metadata = CommitMetadata {
        summary: detail.commit.clone(),
        message: Some(detail.message),
        tags,
    };
    let mut commit_children =
        append_commit_fields(&mut plan, &repository, &metadata, &commit_field_template);
    plan.metadata
        .push(MetadataUpdate::Commit(commit_id.clone(), metadata));

    let mut tree = FileTreePlanState::default();
    tree.relations.insert(files_id.clone(), Vec::new());
    for file in detail.files {
        let file_path = file.path.clone();
        let file_id = DatasetIdentity::File {
            repository: repository.clone(),
            commit: detail.commit.hash.clone(),
            path: file.path.clone(),
        };
        let changes_id = DatasetIdentity::FileChanges {
            repository: repository.clone(),
            commit: detail.commit.hash.clone(),
            path: file.path.clone(),
        };
        plan.add_node(file_id.clone(), DatasetKind::File, file_template.clone());
        plan.add_node(
            changes_id.clone(),
            DatasetKind::FileChanges,
            changes_template.clone(),
        );
        plan.metadata
            .push(MetadataUpdate::File(file_id.clone(), file));
        plan.replace_children(file_id.clone(), vec![changes_id]);
        attach_file_to_tree(
            &mut plan,
            &mut tree,
            &files_id,
            file_id,
            &file_path,
            &directory_template,
            |path| DatasetIdentity::FileDirectory {
                repository: repository.clone(),
                commit: detail.commit.hash.clone(),
                path,
            },
        );
    }
    apply_file_tree_relations(&mut plan, tree.relations);
    commit_children.push(files_id);
    plan.replace_children(commit_id, commit_children);
    Ok(plan)
}

fn file_diff_plan(
    world: &World,
    repository: RepositoryKey,
    diff: FileDiff,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let changes_template = template_for(world, DatasetKind::FileChanges)?;
    let changes_id = DatasetIdentity::FileChanges {
        repository,
        commit: diff.commit.clone(),
        path: diff.path.clone(),
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(
        changes_id.clone(),
        DatasetKind::FileChanges,
        changes_template,
    );
    plan.metadata
        .push(MetadataUpdate::FileChanges(changes_id.clone(), diff));
    plan.mark_snapshot(changes_id);
    Ok(plan)
}

fn working_tree_plan(
    world: &World,
    repository: RepositoryKey,
    changes: Vec<WorkingTreeChange>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let changes_template = template_for(world, DatasetKind::Changes)?;
    let groups_template = template_for(world, DatasetKind::WorkingTreeFiles)?;
    let directory_template = template_for(world, DatasetKind::FileTreeDirectory)?;
    let file_template = template_for(world, DatasetKind::WorkingTreeFile)?;
    let diff_template = template_for(world, DatasetKind::WorkingTreeFileChanges)?;
    let changes_id = DatasetIdentity::Changes(repository.clone());
    let staged_group = DatasetIdentity::WorkingTreeFiles {
        repository: repository.clone(),
        boundary: ChangeBoundary::Staged,
    };
    let unstaged_group = DatasetIdentity::WorkingTreeFiles {
        repository: repository.clone(),
        boundary: ChangeBoundary::Unstaged,
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(changes_id.clone(), DatasetKind::Changes, changes_template);
    for group in [&staged_group, &unstaged_group] {
        plan.add_node(
            group.clone(),
            DatasetKind::WorkingTreeFiles,
            groups_template.clone(),
        );
    }

    let mut tree = FileTreePlanState::default();
    tree.relations.insert(staged_group.clone(), Vec::new());
    tree.relations.insert(unstaged_group.clone(), Vec::new());
    for change in changes {
        for boundary in [ChangeBoundary::Staged, ChangeBoundary::Unstaged] {
            let included = match boundary {
                ChangeBoundary::Staged => change.has_staged_changes(),
                ChangeBoundary::Unstaged => change.has_worktree_changes() || change.is_untracked(),
            };
            if !included {
                continue;
            }
            let file_id = DatasetIdentity::WorkingTreeFile {
                repository: repository.clone(),
                boundary,
                path: change.path.clone(),
            };
            let diff_id = DatasetIdentity::WorkingTreeFileChanges {
                repository: repository.clone(),
                boundary,
                path: change.path.clone(),
            };
            plan.add_node(
                file_id.clone(),
                DatasetKind::WorkingTreeFile,
                file_template.clone(),
            );
            plan.add_node(
                diff_id.clone(),
                DatasetKind::WorkingTreeFileChanges,
                diff_template.clone(),
            );
            plan.invalidate_snapshot(diff_id.clone());
            plan.metadata.push(MetadataUpdate::WorkingTreeFile(
                file_id.clone(),
                change.clone(),
            ));
            plan.replace_children(file_id.clone(), vec![diff_id]);
            let root = match boundary {
                ChangeBoundary::Staged => &staged_group,
                ChangeBoundary::Unstaged => &unstaged_group,
            };
            attach_file_to_tree(
                &mut plan,
                &mut tree,
                root,
                file_id,
                &change.path,
                &directory_template,
                |path| DatasetIdentity::WorkingTreeDirectory {
                    repository: repository.clone(),
                    boundary,
                    path,
                },
            );
        }
    }
    apply_file_tree_relations(&mut plan, tree.relations);
    plan.replace_children(changes_id, vec![staged_group, unstaged_group]);
    Ok(plan)
}

fn reflog_plan(
    world: &World,
    repository: RepositoryKey,
    entries: Vec<ReflogEntry>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let reflog_template = template_for(world, DatasetKind::Reflog)?;
    let entry_template = template_for(world, DatasetKind::ReflogEntry)?;
    let reflog_id = DatasetIdentity::Reflog(repository.clone());
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(reflog_id.clone(), DatasetKind::Reflog, reflog_template);

    let mut entry_ids = Vec::with_capacity(entries.len());
    for entry in entries {
        let entry_id = DatasetIdentity::ReflogEntry {
            repository: repository.clone(),
            selector: entry.selector.clone(),
        };
        plan.add_node(
            entry_id.clone(),
            DatasetKind::ReflogEntry,
            entry_template.clone(),
        );
        plan.metadata
            .push(MetadataUpdate::ReflogEntry(entry_id.clone(), entry));
        entry_ids.push(entry_id);
    }
    plan.replace_children(reflog_id, entry_ids);
    Ok(plan)
}

fn working_tree_diff_plan(
    world: &World,
    repository: RepositoryKey,
    boundary: ChangeBoundary,
    diff: WorkingTreeDiff,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let template = template_for(world, DatasetKind::WorkingTreeFileChanges)?;
    let identity = DatasetIdentity::WorkingTreeFileChanges {
        repository,
        boundary,
        path: diff.path.clone(),
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(
        identity.clone(),
        DatasetKind::WorkingTreeFileChanges,
        template,
    );
    plan.metadata.push(MetadataUpdate::WorkingTreeFileChanges(
        identity.clone(),
        diff,
    ));
    plan.mark_snapshot(identity);
    Ok(plan)
}

fn tags_from_decorations(decorations: &str) -> Vec<String> {
    decorations
        .split(',')
        .map(str::trim)
        .filter_map(|decoration| decoration.strip_prefix("tag: "))
        .map(str::to_string)
        .collect()
}

fn validate_plan(world: &World, plan: &DatasetSnapshotPlan) -> Result<(), GitDataError> {
    let mut planned = HashMap::<DatasetIdentity, (&DatasetKind, &DatasetTemplateId)>::new();
    for node in &plan.nodes {
        if let Some((kind, template)) =
            planned.insert(node.identity.clone(), (&node.kind, &node.template))
            && (*kind != node.kind || *template != node.template)
        {
            return Err(GitDataError::DuplicatePlannedIdentity(
                node.identity.clone(),
            ));
        }
        let registered_kind = world
            .resource::<DatasetTemplateRegistry>()
            .get(&node.template)
            .map(|template| template.kind)
            .ok_or_else(|| KernelError::MissingTemplate(node.template.clone()))?;
        if registered_kind != node.kind {
            return Err(KernelError::TemplateKindMismatch {
                template: node.template.clone(),
                expected: node.kind,
                actual: registered_kind,
            }
            .into());
        }
        if let Some(entity) = world.resource::<DatasetIndex>().get(&node.identity) {
            let kind = world
                .get::<DatasetType>(entity)
                .ok_or(KernelError::MissingDataset(entity))?
                .0;
            let template = world
                .get::<DatasetTemplateRef>(entity)
                .ok_or(KernelError::MissingDataset(entity))?
                .0
                .clone();
            if kind != node.kind {
                return Err(KernelError::IdentityKindMismatch {
                    identity: Box::new(node.identity.clone()),
                    expected: node.kind,
                    actual: kind,
                }
                .into());
            }
            if template != node.template {
                return Err(KernelError::IdentityTemplateMismatch {
                    identity: Box::new(node.identity.clone()),
                    expected: node.template.clone(),
                    actual: template,
                }
                .into());
            }
        }
    }

    let mut graph = current_identity_graph(world);
    for relation in &plan.children {
        if !planned.contains_key(&relation.parent)
            && world
                .resource::<DatasetIndex>()
                .get(&relation.parent)
                .is_none()
        {
            return Err(GitDataError::MissingPlannedIdentity(
                relation.parent.clone(),
            ));
        }
        let mut seen = HashSet::new();
        for child in &relation.children {
            if !seen.insert(child) {
                return Err(GitDataError::DuplicatePlannedIdentity(child.clone()));
            }
            if !planned.contains_key(child) && world.resource::<DatasetIndex>().get(child).is_none()
            {
                return Err(GitDataError::MissingPlannedIdentity(child.clone()));
            }
        }
        graph.insert(relation.parent.clone(), relation.children.clone());
    }
    if let Some(identity) = graph_cycle(&graph) {
        return Err(GitDataError::PlannedCycle(identity));
    }
    Ok(())
}

fn current_identity_graph(world: &World) -> HashMap<DatasetIdentity, Vec<DatasetIdentity>> {
    let index = world.resource::<DatasetIndex>();
    let mut graph = HashMap::new();
    for (identity, entity) in &index.by_key {
        let children = world
            .get::<DatasetChildren>(*entity)
            .map(|children| {
                children
                    .0
                    .iter()
                    .filter_map(|child| world.get::<DatasetKey>(*child))
                    .map(|key| key.0.clone())
                    .collect()
            })
            .unwrap_or_default();
        graph.insert(identity.clone(), children);
    }
    graph
}

fn graph_cycle(graph: &HashMap<DatasetIdentity, Vec<DatasetIdentity>>) -> Option<DatasetIdentity> {
    fn visit(
        node: &DatasetIdentity,
        graph: &HashMap<DatasetIdentity, Vec<DatasetIdentity>>,
        visiting: &mut HashSet<DatasetIdentity>,
        visited: &mut HashSet<DatasetIdentity>,
    ) -> Option<DatasetIdentity> {
        if visiting.contains(node) {
            return Some(node.clone());
        }
        if !visited.insert(node.clone()) {
            return None;
        }
        visiting.insert(node.clone());
        if let Some(children) = graph.get(node) {
            for child in children {
                if let Some(cycle) = visit(child, graph, visiting, visited) {
                    return Some(cycle);
                }
            }
        }
        visiting.remove(node);
        None
    }

    let mut visited = HashSet::new();
    for node in graph.keys() {
        if let Some(cycle) = visit(node, graph, &mut HashSet::new(), &mut visited) {
            return Some(cycle);
        }
    }
    None
}

fn apply_plan(world: &mut World, plan: DatasetSnapshotPlan) -> Result<(), GitDataError> {
    for node in &plan.nodes {
        ensure_dataset_in_world(
            world,
            node.identity.clone(),
            node.kind,
            node.template.clone(),
        )?;
    }

    for identity in &plan.invalidated_snapshots {
        let entity = entity_for(world, identity)?;
        world
            .entity_mut(entity)
            .remove::<WorkingTreeFileChangesMetadata>()
            .insert(HasSnapshot(false));
    }

    for update in plan.metadata {
        match update {
            MetadataUpdate::Branch(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world.entity_mut(entity).insert(BranchMetadata(metadata));
            }
            MetadataUpdate::Commit(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world.entity_mut(entity).insert(metadata);
            }
            MetadataUpdate::CommitField(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world.entity_mut(entity).insert(metadata);
            }
            MetadataUpdate::File(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world.entity_mut(entity).insert(FileMetadata(metadata));
            }
            MetadataUpdate::FileTreeDirectory(identity, path) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(FileTreeDirectoryMetadata(path));
            }
            MetadataUpdate::FileChanges(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(FileChangesMetadata(metadata));
            }
            MetadataUpdate::WorkingTreeFile(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(WorkingTreeFileMetadata(metadata));
            }
            MetadataUpdate::WorkingTreeFileChanges(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(WorkingTreeFileChangesMetadata(metadata));
            }
            MetadataUpdate::ReflogEntry(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(ReflogEntryMetadata(metadata));
            }
        }
    }

    let relation_parents = plan
        .children
        .iter()
        .map(|relation| relation.parent.clone())
        .collect::<HashSet<_>>();
    for relation in plan.children {
        let parent = entity_for(world, &relation.parent)?;
        let children = relation
            .children
            .iter()
            .map(|identity| entity_for(world, identity))
            .collect::<Result<Vec<_>, _>>()?;
        replace_children_in_world(world, parent, children, true)?;
    }
    for identity in plan.snapshots {
        if !relation_parents.contains(&identity) {
            mark_snapshot(world, entity_for(world, &identity)?);
        }
    }
    Ok(())
}

fn entity_for(world: &World, identity: &DatasetIdentity) -> Result<Entity, GitDataError> {
    world
        .resource::<DatasetIndex>()
        .get(identity)
        .ok_or_else(|| GitDataError::MissingPlannedIdentity(identity.clone()))
}

fn mark_snapshot(world: &mut World, entity: Entity) {
    world.entity_mut(entity).insert(HasSnapshot(true));
    world
        .get_mut::<DatasetRevision>(entity)
        .expect("validated Dataset must have a revision")
        .0 += 1;
}
