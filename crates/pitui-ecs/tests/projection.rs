use std::{fs, path::Path, process::Command};

use pitui_core::{BranchName, CommitHash, GitPath};
use pitui_data::{
    CellProjection, DatasetIdentity, DatasetIndex, DatasetKind, DatasetTemplateId, DatasetViewport,
    DetailProjection, FieldId, InputIntent, KeyCode, KeyStroke, RenderBindingId,
    RenderContentProjection, RenderContextBindings, RenderModeId, ResolvedOperationSet,
    ResolvedOperationSetId, SideBySideDiffProjection, UiFrame, UiLayoutProjection,
    UnifiedDiffProjection, ViewportMeasurement,
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

fn detail_fields(layout: &UiLayoutProjection) -> &DetailProjection {
    let UiLayoutProjection::Dataset { panel, .. } = layout else {
        panic!("expected Dataset projection");
    };
    let RenderContentProjection::Detail(detail) = &panel.content else {
        panic!("expected Detail projection");
    };
    detail
}

fn cell(fields: &[CellProjection], id: FieldId) -> &str {
    &fields
        .iter()
        .find(|field| field.field == id)
        .unwrap_or_else(|| panic!("missing projected field {id:?}"))
        .text
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
    git(&cwd, &["add", "file.txt"]);
    git(&cwd, &["commit", "-m", "second\n\nbody"]);
    let head = CommitHash(git(&cwd, &["rev-parse", "HEAD"]));

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
    runtime.set_navigation_modes(pitui_config::builtin_navigation_modes());
    runtime.register_builtin_interaction_systems().unwrap();

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

    let (commit, files, diff) = {
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
            files,
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
    let detail = detail_fields(&left[0]);
    assert_eq!(cell(&detail.fields, FieldId::CommitSubject), "second");
    assert_eq!(
        cell(&detail.fields, FieldId::CommitMessage),
        "second\n\nbody"
    );
    assert_eq!(cell(&detail.fields, FieldId::CommitAuthoredAt).len(), 16);
    let UiLayoutProjection::Dataset {
        panel: files_panel, ..
    } = &left[1]
    else {
        panic!("files must project as a Dataset panel");
    };
    assert!(files_panel.active);
    let UiLayoutProjection::Dataset {
        panel: diff_panel, ..
    } = &columns[1]
    else {
        panic!("diff must project as a Dataset panel");
    };
    assert!(!diff_panel.active);
    let RenderContentProjection::UnifiedDiff(UnifiedDiffProjection { hunks, .. }) =
        &diff_panel.content
    else {
        panic!("unified proxy must produce structured unified diff data");
    };
    assert!(!hunks.is_empty());
    assert!(
        hunks
            .iter()
            .any(|hunk| hunk.lines.iter().any(|line| line.text == "two"))
    );
    assert!(
        runtime
            .world()
            .resource::<ProjectionDiagnostics>()
            .0
            .is_empty()
    );

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
