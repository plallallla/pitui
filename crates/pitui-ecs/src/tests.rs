// Crate-internal tests live separately so production modules remain navigable.

use super::*;
use pitui_data::{RenderBindingId, RenderProxyId, ResolvedOperationSetId};

fn register(runtime: &mut DatasetRuntime, id: &str, kind: DatasetKind) -> DatasetTemplateId {
    let id = DatasetTemplateId::from(id);
    runtime
        .register_template(DatasetTemplate {
            id: id.clone(),
            kind,
            operations: Vec::new(),
            render_proxies: vec![RenderProxyId::from("test")],
        })
        .unwrap();
    id
}

fn operations() -> ResolvedOperationSet {
    ResolvedOperationSet {
        id: ResolvedOperationSetId::from("test"),
        ..ResolvedOperationSet::default()
    }
}

#[test]
fn canonical_identity_is_shared_by_multiple_parents() {
    let mut runtime = DatasetRuntime::new();
    let root_template = register(&mut runtime, "root", DatasetKind::RepositoriesBranches);
    let commits_template = register(&mut runtime, "commits", DatasetKind::Commits);
    let commit_template = register(&mut runtime, "commit", DatasetKind::Commit);
    let repository = pitui_data::RepositoryKey::new("/repo");
    let root = runtime
        .ensure_dataset(
            DatasetIdentity::GlobalRepositoriesBranches,
            DatasetKind::RepositoriesBranches,
            root_template,
        )
        .unwrap();
    let left = runtime
        .ensure_dataset(
            DatasetIdentity::Commits {
                repository: repository.clone(),
                branch: pitui_core::BranchName("main".into()),
            },
            DatasetKind::Commits,
            commits_template.clone(),
        )
        .unwrap();
    let right = runtime
        .ensure_dataset(
            DatasetIdentity::Commits {
                repository: repository.clone(),
                branch: pitui_core::BranchName("feature".into()),
            },
            DatasetKind::Commits,
            commits_template,
        )
        .unwrap();
    let identity = DatasetIdentity::Commit {
        repository,
        hash: pitui_core::CommitHash("abc".into()),
    };
    let commit = runtime
        .ensure_dataset(
            identity.clone(),
            DatasetKind::Commit,
            commit_template.clone(),
        )
        .unwrap();
    let same_commit = runtime
        .ensure_dataset(identity, DatasetKind::Commit, commit_template)
        .unwrap();
    assert_eq!(commit, same_commit);

    runtime.replace_children(left, vec![commit], true).unwrap();
    runtime.replace_children(right, vec![commit], true).unwrap();
    runtime
        .replace_children(root, vec![left, right], true)
        .unwrap();
    runtime.add_root(root).unwrap();
    runtime.run_schedule();

    assert!(runtime.world().get_entity(commit).is_ok());
    assert!(runtime.validate().is_empty());
}

#[test]
fn rejects_cycles_before_mutating_children() {
    let mut runtime = DatasetRuntime::new();
    let template = register(&mut runtime, "repository", DatasetKind::Repository);
    let a = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/a")),
            DatasetKind::Repository,
            template.clone(),
        )
        .unwrap();
    let b = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/b")),
            DatasetKind::Repository,
            template,
        )
        .unwrap();
    runtime.replace_children(a, vec![b], true).unwrap();

    assert!(matches!(
        runtime.replace_children(b, vec![a], true),
        Err(KernelError::Cycle { .. })
    ));
    assert!(
        runtime
            .world()
            .get::<DatasetChildren>(b)
            .unwrap()
            .0
            .is_empty()
    );
}

#[test]
fn cursor_repair_does_not_move_active_dataset() {
    let mut runtime = DatasetRuntime::new();
    let commits_template = register(&mut runtime, "commits", DatasetKind::Commits);
    let commit_template = register(&mut runtime, "commit", DatasetKind::Commit);
    let repository = pitui_data::RepositoryKey::new("/repo");
    let commits = runtime
        .ensure_dataset(
            DatasetIdentity::Commits {
                repository: repository.clone(),
                branch: pitui_core::BranchName("main".into()),
            },
            DatasetKind::Commits,
            commits_template,
        )
        .unwrap();
    let first = runtime
        .ensure_dataset(
            DatasetIdentity::Commit {
                repository: repository.clone(),
                hash: pitui_core::CommitHash("one".into()),
            },
            DatasetKind::Commit,
            commit_template.clone(),
        )
        .unwrap();
    let second = runtime
        .ensure_dataset(
            DatasetIdentity::Commit {
                repository,
                hash: pitui_core::CommitHash("two".into()),
            },
            DatasetKind::Commit,
            commit_template,
        )
        .unwrap();
    runtime
        .replace_children(commits, vec![first, second], true)
        .unwrap();
    runtime.add_root(commits).unwrap();
    runtime.set_cursor(commits, Some(first)).unwrap();
    runtime.set_selection(commits, vec![first, second]).unwrap();
    runtime
        .initialize_ui(
            commits,
            RenderModeId::from("history"),
            RenderContextBindings::default(),
            ResolvedRenderLayout::Dataset {
                dataset: commits,
                proxy: RenderProxyId::from("commits.detailed"),
                constraint: pitui_data::LayoutConstraint::Fill(1),
                focusable: true,
            },
            operations(),
        )
        .unwrap();

    runtime
        .replace_children(commits, vec![second], true)
        .unwrap();
    runtime.run_schedule();

    assert_eq!(
        runtime.world().get::<DatasetCursor>(commits).unwrap().0,
        Some(second)
    );
    assert_eq!(
        runtime.world().get::<DatasetSelection>(commits).unwrap().0,
        vec![second]
    );
    assert_eq!(
        runtime.world().resource::<ActiveUiContext>().active_dataset,
        commits
    );
    assert!(runtime.validate().is_empty());
}

#[test]
fn repository_tree_navigation_exposes_repositories_and_branches_only() {
    let mut runtime = DatasetRuntime::new();
    let root_template = register(
        &mut runtime,
        "repositories-branches",
        DatasetKind::RepositoriesBranches,
    );
    let repository_template = register(&mut runtime, "repository", DatasetKind::Repository);
    let branch_template = register(&mut runtime, "branch", DatasetKind::Branch);
    let commits_template = register(&mut runtime, "commits", DatasetKind::Commits);
    let repository_key = pitui_data::RepositoryKey::new("/repo");
    let root = runtime
        .ensure_dataset(
            DatasetIdentity::GlobalRepositoriesBranches,
            DatasetKind::RepositoriesBranches,
            root_template,
        )
        .unwrap();
    let repository = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(repository_key.clone()),
            DatasetKind::Repository,
            repository_template,
        )
        .unwrap();
    let branch = runtime
        .ensure_dataset(
            DatasetIdentity::Branch {
                repository: repository_key.clone(),
                name: pitui_core::BranchName("main".into()),
            },
            DatasetKind::Branch,
            branch_template,
        )
        .unwrap();
    let commits = runtime
        .ensure_dataset(
            DatasetIdentity::Commits {
                repository: repository_key,
                branch: pitui_core::BranchName("main".into()),
            },
            DatasetKind::Commits,
            commits_template,
        )
        .unwrap();

    runtime
        .replace_children(branch, vec![commits], true)
        .unwrap();
    runtime
        .replace_children(repository, vec![branch], true)
        .unwrap();
    runtime
        .replace_children(root, vec![repository], true)
        .unwrap();
    runtime.add_root(root).unwrap();
    runtime.run_schedule();

    assert_eq!(
        runtime
            .world()
            .get::<DatasetNavigationOrder>(root)
            .unwrap()
            .0,
        vec![repository, branch]
    );
    runtime.set_cursor(root, Some(branch)).unwrap();
    assert!(matches!(
        runtime.set_cursor(root, Some(commits)),
        Err(KernelError::CursorOutsideDataset { .. })
    ));

    runtime
        .replace_children(repository, Vec::new(), true)
        .unwrap();
    runtime.run_schedule();
    assert_eq!(
        runtime
            .world()
            .get::<DatasetNavigationOrder>(root)
            .unwrap()
            .0,
        vec![repository]
    );
    assert_eq!(
        runtime.world().get::<DatasetCursor>(root).unwrap().0,
        Some(repository)
    );
    assert!(runtime.validate().is_empty());
}

#[test]
fn changes_tree_navigation_reaches_third_level_files_without_exposing_diffs() {
    let mut runtime = DatasetRuntime::new();
    let changes_template = register(&mut runtime, "changes", DatasetKind::Changes);
    let group_template = register(
        &mut runtime,
        "working-tree-files",
        DatasetKind::WorkingTreeFiles,
    );
    let file_template = register(
        &mut runtime,
        "working-tree-file",
        DatasetKind::WorkingTreeFile,
    );
    let diff_template = register(
        &mut runtime,
        "working-tree-file-changes",
        DatasetKind::WorkingTreeFileChanges,
    );
    let repository = pitui_data::RepositoryKey::new("/repo");
    let root = runtime
        .ensure_dataset(
            DatasetIdentity::GlobalChanges,
            DatasetKind::Changes,
            changes_template,
        )
        .unwrap();
    let group = runtime
        .ensure_dataset(
            DatasetIdentity::WorkingTreeFiles {
                repository: repository.clone(),
                boundary: pitui_data::ChangeBoundary::Unstaged,
            },
            DatasetKind::WorkingTreeFiles,
            group_template,
        )
        .unwrap();
    let file = runtime
        .ensure_dataset(
            DatasetIdentity::WorkingTreeFile {
                repository: repository.clone(),
                boundary: pitui_data::ChangeBoundary::Unstaged,
                path: pitui_core::GitPath::from("src/main.rs"),
            },
            DatasetKind::WorkingTreeFile,
            file_template,
        )
        .unwrap();
    let diff = runtime
        .ensure_dataset(
            DatasetIdentity::WorkingTreeFileChanges {
                repository,
                boundary: pitui_data::ChangeBoundary::Unstaged,
                path: pitui_core::GitPath::from("src/main.rs"),
            },
            DatasetKind::WorkingTreeFileChanges,
            diff_template,
        )
        .unwrap();

    runtime.replace_children(file, vec![diff], true).unwrap();
    runtime.replace_children(group, vec![file], true).unwrap();
    runtime.replace_children(root, vec![group], true).unwrap();
    runtime.add_root(root).unwrap();
    runtime.run_schedule();

    assert_eq!(
        runtime
            .world()
            .get::<DatasetNavigationOrder>(root)
            .unwrap()
            .0,
        vec![group, file]
    );
    runtime.set_cursor(root, Some(file)).unwrap();
    runtime.set_selection(root, vec![file]).unwrap();
    assert!(matches!(
        runtime.set_cursor(root, Some(diff)),
        Err(KernelError::CursorOutsideDataset { .. })
    ));
    assert_eq!(
        runtime.world().get::<DatasetSelection>(root).unwrap().0,
        vec![file]
    );
    assert!(runtime.validate().is_empty());
}

#[test]
fn gc_removes_only_unreachable_datasets() {
    let mut runtime = DatasetRuntime::new();
    let template = register(&mut runtime, "repository", DatasetKind::Repository);
    let root = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/root")),
            DatasetKind::Repository,
            template.clone(),
        )
        .unwrap();
    let child = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/child")),
            DatasetKind::Repository,
            template.clone(),
        )
        .unwrap();
    let orphan_identity = DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/orphan"));
    let orphan = runtime
        .ensure_dataset(orphan_identity.clone(), DatasetKind::Repository, template)
        .unwrap();
    runtime.replace_children(root, vec![child], true).unwrap();
    runtime.add_root(root).unwrap();

    runtime.run_schedule();

    assert!(runtime.world().get_entity(root).is_ok());
    assert!(runtime.world().get_entity(child).is_ok());
    assert!(runtime.world().get_entity(orphan).is_err());
    assert_eq!(
        runtime
            .world()
            .resource::<DatasetIndex>()
            .get(&orphan_identity),
        None
    );
}

#[test]
fn context_push_and_pop_restore_active_mode_and_bindings_atomically() {
    let mut runtime = DatasetRuntime::new();
    let template = register(&mut runtime, "repository", DatasetKind::Repository);
    let history = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/history")),
            DatasetKind::Repository,
            template.clone(),
        )
        .unwrap();
    let detail = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(pitui_data::RepositoryKey::new("/detail")),
            DatasetKind::Repository,
            template,
        )
        .unwrap();
    let history_layout = ResolvedRenderLayout::Dataset {
        dataset: history,
        proxy: RenderProxyId::from("history"),
        constraint: pitui_data::LayoutConstraint::Fill(1),
        focusable: true,
    };
    let detail_layout = ResolvedRenderLayout::Dataset {
        dataset: detail,
        proxy: RenderProxyId::from("detail"),
        constraint: pitui_data::LayoutConstraint::Fill(1),
        focusable: true,
    };
    let mut history_bindings = RenderContextBindings::default();
    history_bindings.bind(RenderBindingId::CurrentRepository, history);
    let mut detail_bindings = RenderContextBindings::default();
    detail_bindings.bind(RenderBindingId::CurrentRepository, detail);

    runtime
        .initialize_ui(
            history,
            RenderModeId::from("history"),
            history_bindings,
            history_layout.clone(),
            operations(),
        )
        .unwrap();
    runtime
        .push_context(
            detail,
            RenderModeId::from("detail"),
            detail_bindings,
            detail_layout,
            operations(),
        )
        .unwrap();
    assert_eq!(
        runtime.world().resource::<ActiveUiContext>().active_dataset,
        detail
    );
    assert_eq!(runtime.world().resource::<ContextStack>().0.len(), 1);

    runtime.pop_context(history_layout, operations()).unwrap();

    let restored = runtime.world().resource::<ActiveUiContext>();
    assert_eq!(restored.active_dataset, history);
    assert_eq!(restored.render_mode, RenderModeId::from("history"));
    assert_eq!(
        restored
            .render_bindings
            .get(&RenderBindingId::CurrentRepository),
        Some(history)
    );
    assert_eq!(restored.generation, 2);
    assert!(runtime.world().resource::<ContextStack>().0.is_empty());
}

#[test]
fn stable_dataset_identity_rejects_a_caller_selected_wrong_kind() {
    let mut runtime = DatasetRuntime::new();
    let template = register(&mut runtime, "commit", DatasetKind::Commit);
    let error = runtime
        .ensure_dataset(
            DatasetIdentity::GlobalChanges,
            DatasetKind::Commit,
            template,
        )
        .unwrap_err();
    assert_eq!(
        error,
        KernelError::IdentityKindMismatch {
            identity: Box::new(DatasetIdentity::GlobalChanges),
            expected: DatasetKind::Changes,
            actual: DatasetKind::Commit,
        }
    );
}

#[test]
fn registration_contracts_reject_dangling_proxy_and_command_system_references() {
    let mut runtime = DatasetRuntime::new();
    runtime
        .register_default_template(DatasetTemplate {
            id: DatasetTemplateId::from("changes"),
            kind: DatasetKind::Changes,
            operations: Vec::new(),
            render_proxies: vec![RenderProxyId::from("missing.proxy")],
        })
        .unwrap();
    runtime
        .register_command(CommandSpec {
            id: CommandId::from("missing-system"),
            name: "missing-system".into(),
            scope: pitui_data::CommandScope::Dataset,
            system: CommandSystemId::from("missing-system"),
        })
        .unwrap();

    let errors = runtime.validate_registration_contracts();
    assert!(
        errors.contains(&RegistrationContractError::MissingTemplateProxy {
            template: DatasetTemplateId::from("changes"),
            proxy: RenderProxyId::from("missing.proxy"),
        })
    );
    assert!(
        errors.contains(&RegistrationContractError::CommandSystemMissing {
            command: CommandId::from("missing-system"),
            system: CommandSystemId::from("missing-system"),
        })
    );
}
