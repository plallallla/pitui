use std::{fs, path::Path, process::Command};

use pitui_core::{BranchName, CommitHash, GitPath};
use pitui_data::{
    CellProjection, DatasetChildren, DatasetIdentity, DatasetIndex, DatasetKind, DatasetTemplateId,
    DatasetType, DatasetViewId, DatasetViewState, DatasetViewport, FieldId, InputIntent, KeyCode,
    KeyStroke, RenderBindingId, RenderContentProjection, RenderContextBindings, RenderModeId,
    RendererKind, ResolvedOperationSet, ResolvedOperationSetId, RowProjectionKind, RowsProjection,
    SideBySideDiffProjection, UiFrame, UiLayoutProjection, UnifiedDiffProjection,
    ViewportMeasurement,
};
use pitui_ecs::{DatasetRuntime, GitCommandData, ProjectionDiagnostics};
use pitui_git::GitCommand;

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}

fn enqueue(
    runtime: &mut DatasetRuntime,
    repository: bevy_ecs::prelude::Entity,
    cwd: &Path,
    command: GitCommand,
) {
    runtime
        .enqueue_git_command(GitCommandData {
            repository_dataset: repository,
            cwd: cwd.into(),
            command,
        })
        .unwrap();
    runtime.run_schedule();
}

fn operations() -> ResolvedOperationSet {
    ResolvedOperationSet {
        id: ResolvedOperationSetId::from("projection-test"),
        ..ResolvedOperationSet::default()
    }
}

fn configured_runtime() -> DatasetRuntime {
    let mut runtime = DatasetRuntime::new();
    for template in pitui_config::builtin_dataset_templates() {
        runtime.register_default_template(template).unwrap();
    }
    for proxy in pitui_config::builtin_render_proxies() {
        runtime.register_render_proxy(proxy).unwrap();
    }
    for mode in pitui_config::builtin_render_modes() {
        runtime.register_render_mode(mode).unwrap();
    }
    for (id, rule) in pitui_config::builtin_availability_rules() {
        runtime.register_availability_rule(id, rule).unwrap();
    }
    for command in pitui_config::builtin_command_specs() {
        runtime.register_command(command).unwrap();
    }
    for operation in pitui_config::builtin_operation_specs() {
        runtime.register_operation(operation).unwrap();
    }
    runtime.set_global_operations(pitui_config::builtin_global_operations());
    runtime.set_active_handoffs(pitui_config::builtin_active_handoffs());
    runtime.register_builtin_interaction_systems().unwrap();
    runtime
}

fn row_fields(layout: &UiLayoutProjection) -> &RowsProjection {
    let UiLayoutProjection::Dataset { panel, .. } = layout else {
        panic!("expected Dataset projection");
    };
    let RenderContentProjection::Rows(rows) = &panel.content else {
        panic!("expected Rows projection");
    };
    rows
}

fn cell(fields: &[CellProjection], id: FieldId) -> &str {
    &fields
        .iter()
        .find(|field| field.field == id)
        .unwrap_or_else(|| panic!("missing projected field {id:?}"))
        .text
}

fn commit_field<'a>(rows: &'a RowsProjection, label: &str) -> &'a str {
    let row = rows
        .rows
        .iter()
        .find(|row| cell(&row.cells, FieldId::CommitFieldLabel) == label)
        .unwrap_or_else(|| panic!("missing Commit field {label}"));
    cell(&row.cells, FieldId::CommitFieldValue)
}

#[test]
fn commit_and_file_diff_modes_project_complete_immutable_data() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    git(&cwd, &["init", "-b", "main"]);
    git(&cwd, &["config", "user.name", "Pitui Test"]);
    git(&cwd, &["config", "user.email", "pitui@example.invalid"]);
    fs::write(cwd.join("file.txt"), "one\n").unwrap();
    git(&cwd, &["add", "file.txt"]);
    git(&cwd, &["commit", "-m", "initial"]);
    fs::write(cwd.join("file.txt"), "one\ntwo\n").unwrap();
    fs::create_dir_all(cwd.join("docs")).unwrap();
    fs::create_dir_all(cwd.join("src/nested")).unwrap();
    fs::write(cwd.join("docs/guide.md"), "guide\n").unwrap();
    fs::write(cwd.join("src/lib.rs"), "pub mod nested;\n").unwrap();
    fs::write(cwd.join("src/nested/mod.rs"), "pub fn value() {}\n").unwrap();
    git(&cwd, &["add", "-A"]);
    git(&cwd, &["commit", "-m", "second\n\nbody"]);
    let head = CommitHash(git(&cwd, &["rev-parse", "HEAD"]));

    let mut runtime = configured_runtime();

    let repository_key = pitui_data::RepositoryKey::new(cwd.clone());
    let repository = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(repository_key.clone()),
            DatasetKind::Repository,
            DatasetTemplateId::from("repository"),
        )
        .unwrap();
    runtime.add_root(repository).unwrap();
    enqueue(&mut runtime, repository, &cwd, GitCommand::LoadRepository);
    enqueue(&mut runtime, repository, &cwd, GitCommand::LoadBranches);
    enqueue(
        &mut runtime,
        repository,
        &cwd,
        GitCommand::LoadCommits {
            branch: BranchName("main".into()),
            limit: 50,
        },
    );
    enqueue(
        &mut runtime,
        repository,
        &cwd,
        GitCommand::LoadCommitDetail {
            commit: head.clone(),
        },
    );
    enqueue(
        &mut runtime,
        repository,
        &cwd,
        GitCommand::LoadFileDiff {
            commit: head.clone(),
            path: GitPath::from("file.txt"),
            old_path: None,
        },
    );

    let (commit, files, docs_directory, file_txt, nested_file, diff) = {
        let index = runtime.world().resource::<DatasetIndex>();
        (
            index
                .get(&DatasetIdentity::Commit {
                    repository: repository_key.clone(),
                    hash: head.clone(),
                })
                .unwrap(),
            index
                .get(&DatasetIdentity::Files {
                    repository: repository_key.clone(),
                    commit: head.clone(),
                })
                .unwrap(),
            index
                .get(&DatasetIdentity::FileDirectory {
                    repository: repository_key.clone(),
                    commit: head.clone(),
                    path: GitPath::from("docs"),
                })
                .unwrap(),
            index
                .get(&DatasetIdentity::File {
                    repository: repository_key.clone(),
                    commit: head.clone(),
                    path: GitPath::from("file.txt"),
                })
                .unwrap(),
            index
                .get(&DatasetIdentity::File {
                    repository: repository_key.clone(),
                    commit: head.clone(),
                    path: GitPath::from("src/nested/mod.rs"),
                })
                .unwrap(),
            index
                .get(&DatasetIdentity::FileChanges {
                    repository: repository_key,
                    commit: head,
                    path: GitPath::from("file.txt"),
                })
                .unwrap(),
        )
    };
    let mut bindings = RenderContextBindings::default();
    bindings.bind(RenderBindingId::CurrentRepository, repository);
    bindings.bind(RenderBindingId::CurrentCommit, commit);
    bindings.bind(RenderBindingId::CurrentFiles, files);
    bindings.bind(RenderBindingId::CurrentFileChanges, diff);
    runtime
        .initialize_ui_from_mode(
            diff,
            RenderModeId::from("file-diff.unified"),
            bindings.clone(),
            operations(),
        )
        .unwrap();
    runtime.run_schedule();

    let frame = runtime.world().resource::<UiFrame>();
    let UiLayoutProjection::Row(columns) = &frame.layout else {
        panic!("file diff mode must remain a two-column row");
    };
    assert_eq!(columns.len(), 2);
    let UiLayoutProjection::Column(left) = &columns[0] else {
        panic!("commit detail and files must remain in the left column");
    };
    assert_eq!(left.len(), 2);
    let commit_rows = row_fields(&left[0]);
    assert_eq!(commit_field(commit_rows, "Subject"), "second");
    assert_eq!(commit_field(commit_rows, "Message"), "second ↵  ↵ body");
    assert_eq!(commit_field(commit_rows, "Date").len(), 16);
    let UiLayoutProjection::Dataset {
        panel: files_panel, ..
    } = &left[1]
    else {
        panic!("files must project as a Dataset panel");
    };
    assert!(!files_panel.active);
    assert_eq!(files_panel.proxy.0, "files.tree");
    assert_eq!(files_panel.renderer, RendererKind::PathTree);
    let RenderContentProjection::Rows(file_rows) = &files_panel.content else {
        panic!("Files below Commit must project as a path tree");
    };
    let tree = file_rows
        .rows
        .iter()
        .map(|row| {
            let label = row
                .cells
                .iter()
                .find(|cell| cell.field == FieldId::FilePath)
                .map(|cell| cell.text.as_str())
                .unwrap_or_default();
            (row.kind, row.depth, label, row.active)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tree,
        vec![
            (RowProjectionKind::Directory, 0, "docs/", false),
            (RowProjectionKind::Item, 1, "guide.md", false),
            (RowProjectionKind::Item, 0, "file.txt", false),
            (RowProjectionKind::Directory, 0, "src/", false),
            (RowProjectionKind::Item, 1, "lib.rs", false),
            (RowProjectionKind::Directory, 1, "nested/", false),
            (RowProjectionKind::Item, 2, "mod.rs", false),
        ]
    );
    assert!(
        file_rows
            .rows
            .iter()
            .filter(|row| row.kind == RowProjectionKind::Directory)
            .all(|row| runtime
                .world()
                .get::<DatasetType>(row.entity)
                .is_some_and(|kind| kind.0 == DatasetKind::FileTreeDirectory))
    );
    assert_eq!(file_rows.rows[0].entity, docs_directory);
    assert_eq!(file_rows.rows[6].entity, nested_file);
    assert_eq!(file_rows.viewport.content_length, 7);
    let UiLayoutProjection::Dataset {
        panel: diff_panel, ..
    } = &columns[1]
    else {
        panic!("diff must project as a Dataset panel");
    };
    assert!(diff_panel.active);
    let RenderContentProjection::UnifiedDiff(UnifiedDiffProjection { hunks, .. }) =
        &diff_panel.content
    else {
        panic!("unified proxy must produce structured unified diff data");
    };
    assert!(!hunks.is_empty());
    assert!(
        hunks
            .iter()
            .any(|hunk| hunk.lines.iter().any(|line| line.text == "guide"))
    );
    assert!(
        runtime
            .world()
            .resource::<ProjectionDiagnostics>()
            .0
            .is_empty()
    );

    let ownership_before = runtime
        .world()
        .get::<DatasetChildren>(files)
        .unwrap()
        .0
        .clone();
    runtime.set_selection(files, vec![docs_directory]).unwrap();
    runtime.run_schedule();
    assert_eq!(
        runtime
            .world()
            .get::<pitui_data::DatasetSelection>(files)
            .unwrap()
            .0
            .len(),
        2,
        "Tree parent selection must include its File descendant"
    );
    runtime
        .world_mut()
        .entity_mut(files)
        .insert(DatasetViewState(Some(DatasetViewId::from("list"))));
    runtime.run_schedule();
    let UiLayoutProjection::Row(columns) = &runtime.world().resource::<UiFrame>().layout else {
        panic!("file diff mode must remain a row after switching Files View");
    };
    let UiLayoutProjection::Column(left) = &columns[0] else {
        panic!("left side must retain Commit and Files");
    };
    let UiLayoutProjection::Dataset { panel, .. } = &left[1] else {
        panic!("Files must remain a Dataset panel");
    };
    assert_eq!(panel.proxy.0, "files.list");
    assert_eq!(panel.renderer, RendererKind::List);
    let RenderContentProjection::Rows(rows) = &panel.content else {
        panic!("flat Files View must project rows");
    };
    assert_eq!(rows.rows.len(), 4);
    assert!(rows.rows.iter().all(|row| {
        row.kind == RowProjectionKind::Item
            && row.depth == 0
            && runtime
                .world()
                .get::<DatasetType>(row.entity)
                .is_some_and(|kind| kind.0 == DatasetKind::File)
    }));
    assert_eq!(
        runtime
            .world()
            .get::<pitui_data::DatasetSelection>(files)
            .unwrap()
            .0
            .len(),
        1,
        "List View must preserve the selected File while hiding its directory row"
    );
    assert_eq!(
        runtime.world().get::<DatasetChildren>(files).unwrap().0,
        ownership_before,
        "switching Files View must not rewrite the ownership DAG"
    );
    runtime
        .world_mut()
        .entity_mut(files)
        .insert(DatasetViewState(Some(DatasetViewId::from("tree"))));
    runtime.run_schedule();
    assert!(
        runtime
            .world()
            .get::<pitui_data::DatasetSelection>(files)
            .unwrap()
            .0
            .contains(&docs_directory),
        "returning to Tree View must derive the selected parent from selected descendants"
    );

    runtime.enqueue_viewport_measurement(ViewportMeasurement {
        dataset: files,
        page_size: 2,
    });
    runtime
        .set_active_element(files, Some(nested_file))
        .unwrap();
    runtime.run_schedule();
    let frame = runtime.world().resource::<UiFrame>();
    let UiLayoutProjection::Row(columns) = &frame.layout else {
        panic!("file diff mode must remain a row");
    };
    let UiLayoutProjection::Column(left) = &columns[0] else {
        panic!("left side must keep Commit and Files");
    };
    let UiLayoutProjection::Dataset { panel, .. } = &left[1] else {
        panic!("Files tree must remain visible");
    };
    let RenderContentProjection::Rows(rows) = &panel.content else {
        panic!("Files must remain a path tree");
    };
    assert_eq!(rows.viewport.offset, 5);
    assert_eq!(rows.viewport.page_size, 2);
    assert!(
        !rows.rows[6].active,
        "an inactive Dataset retains its active element without highlighting it"
    );

    runtime.set_active_element(files, Some(file_txt)).unwrap();
    runtime.run_schedule();

    runtime
        .replace_context_from_mode(
            diff,
            RenderModeId::from("file-diff.side-by-side"),
            bindings,
            operations(),
        )
        .unwrap();
    runtime.run_schedule();

    let frame = runtime.world().resource::<UiFrame>();
    let UiLayoutProjection::Row(columns) = &frame.layout else {
        panic!("side-by-side mode must remain a row");
    };
    let UiLayoutProjection::Dataset { panel, .. } = &columns[1] else {
        panic!("right column must remain the diff panel");
    };
    assert!(panel.active);
    let RenderContentProjection::SideBySideDiff(SideBySideDiffProjection { hunks, .. }) =
        &panel.content
    else {
        panic!("side-by-side proxy must produce aligned rows");
    };
    assert!(!hunks.is_empty());
    assert!(hunks.iter().any(|hunk| {
        hunk.rows
            .iter()
            .any(|row| row.right_text.as_deref() == Some("two"))
    }));

    runtime.enqueue_viewport_measurement(ViewportMeasurement {
        dataset: diff,
        page_size: 1,
    });
    runtime.run_schedule();
    let content_length = runtime
        .world()
        .get::<DatasetViewport>(diff)
        .unwrap()
        .content_length;
    assert!(content_length > 1);
    runtime.enqueue_input_intent(InputIntent::Key(KeyStroke::plain(KeyCode::End)));
    runtime.run_schedule();
    assert_eq!(
        runtime.world().get::<DatasetViewport>(diff).unwrap().offset,
        content_length - 1
    );
    runtime.enqueue_input_intent(InputIntent::Key(KeyStroke::plain(KeyCode::PageUp)));
    runtime.run_schedule();
    assert_eq!(
        runtime.world().get::<DatasetViewport>(diff).unwrap().offset,
        content_length - 2
    );
    runtime.enqueue_input_intent(InputIntent::Key(KeyStroke::plain(KeyCode::Home)));
    runtime.run_schedule();
    assert_eq!(
        runtime.world().get::<DatasetViewport>(diff).unwrap().offset,
        0
    );
    let frame = runtime.world().resource::<UiFrame>();
    let UiLayoutProjection::Row(columns) = &frame.layout else {
        panic!("side-by-side mode must remain a row after scrolling");
    };
    let UiLayoutProjection::Dataset { panel, .. } = &columns[1] else {
        panic!("right column must remain the diff panel after scrolling");
    };
    let RenderContentProjection::SideBySideDiff(diff_projection) = &panel.content else {
        panic!("right panel must retain side-by-side projection");
    };
    assert_eq!(diff_projection.viewport.page_size, 1);
    assert_eq!(diff_projection.viewport.offset, 0);
    assert!(runtime.validate().is_empty());
}

#[test]
fn changes_proxy_builds_a_path_tree_inside_each_boundary() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    git(&cwd, &["init", "-b", "main"]);
    git(&cwd, &["config", "user.name", "Pitui Test"]);
    git(&cwd, &["config", "user.email", "pitui@example.invalid"]);
    fs::write(cwd.join("README.md"), "base\n").unwrap();
    git(&cwd, &["add", "README.md"]);
    git(&cwd, &["commit", "-m", "initial"]);

    fs::create_dir_all(cwd.join("src/nested")).unwrap();
    fs::create_dir_all(cwd.join("docs")).unwrap();
    fs::write(cwd.join("src/lib.rs"), "pub mod nested;\n").unwrap();
    fs::write(cwd.join("src/nested/mod.rs"), "pub fn value() {}\n").unwrap();
    fs::write(cwd.join("docs/guide.md"), "guide\n").unwrap();
    git(&cwd, &["add", "src"]);

    let mut runtime = configured_runtime();
    let repository_key = pitui_data::RepositoryKey::new(cwd.clone());
    let repository = runtime
        .ensure_dataset(
            DatasetIdentity::Repository(repository_key.clone()),
            DatasetKind::Repository,
            DatasetTemplateId::from("repository"),
        )
        .unwrap();
    runtime.add_root(repository).unwrap();
    let changes = runtime
        .ensure_dataset(
            DatasetIdentity::GlobalChanges,
            DatasetKind::Changes,
            DatasetTemplateId::from("changes"),
        )
        .unwrap();
    runtime.add_root(changes).unwrap();
    enqueue(&mut runtime, repository, &cwd, GitCommand::LoadRepository);
    enqueue(&mut runtime, repository, &cwd, GitCommand::LoadWorkingTree);

    let (staged_directory, staged_diff) = {
        let index = runtime.world().resource::<DatasetIndex>();
        (
            index
                .get(&DatasetIdentity::WorkingTreeDirectory {
                    repository: repository_key.clone(),
                    boundary: pitui_data::ChangeBoundary::Staged,
                    path: GitPath::from("src"),
                })
                .unwrap(),
            index
                .get(&DatasetIdentity::WorkingTreeFileChanges {
                    repository: repository_key,
                    boundary: pitui_data::ChangeBoundary::Staged,
                    path: GitPath::from("src/lib.rs"),
                })
                .unwrap(),
        )
    };
    runtime
        .set_active_element(changes, Some(staged_directory))
        .unwrap();
    runtime
        .set_selection(changes, vec![staged_directory])
        .unwrap();
    let mut bindings = RenderContextBindings::default();
    bindings.bind(RenderBindingId::CurrentRepository, repository);
    bindings.bind(RenderBindingId::Changes, changes);
    bindings.bind(RenderBindingId::CurrentChangesFileChanges, staged_diff);
    runtime
        .initialize_ui_from_mode(
            changes,
            RenderModeId::from("changes.unified"),
            bindings,
            operations(),
        )
        .unwrap();
    runtime.run_schedule();

    let frame = runtime.world().resource::<UiFrame>();
    let UiLayoutProjection::Row(columns) = &frame.layout else {
        panic!("Changes mode must be a row");
    };
    let UiLayoutProjection::Dataset { panel, .. } = &columns[0] else {
        panic!("left column must be Changes");
    };
    assert_eq!(panel.renderer, RendererKind::PathTree);
    let RenderContentProjection::Rows(rows) = &panel.content else {
        panic!("Changes must project rows");
    };
    let tree = rows
        .rows
        .iter()
        .map(|row| {
            let label = row
                .cells
                .iter()
                .find(|cell| cell.field == FieldId::DatasetLabel)
                .map(|cell| cell.text.as_str())
                .unwrap_or_default();
            (row.kind, row.depth, label)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tree,
        vec![
            (RowProjectionKind::Item, 0, "Staged"),
            (RowProjectionKind::Directory, 1, "src/"),
            (RowProjectionKind::Item, 2, "lib.rs"),
            (RowProjectionKind::Directory, 2, "nested/"),
            (RowProjectionKind::Item, 3, "mod.rs"),
            (RowProjectionKind::Item, 0, "Unstaged"),
            (RowProjectionKind::Directory, 1, "docs/"),
            (RowProjectionKind::Item, 2, "guide.md"),
        ]
    );
    assert_eq!(rows.viewport.content_length, tree.len());
    assert!(
        rows.rows
            .iter()
            .filter(|row| row.kind == RowProjectionKind::Directory)
            .all(|row| runtime
                .world()
                .get::<DatasetType>(row.entity)
                .is_some_and(|kind| kind.0 == DatasetKind::FileTreeDirectory))
    );
    let staged_directory_row = rows
        .rows
        .iter()
        .find(|row| row.entity == staged_directory)
        .unwrap();
    assert!(staged_directory_row.active);
    assert!(staged_directory_row.selected);
}
