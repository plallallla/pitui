// Crate-internal tests live separately so production modules remain navigable.

use std::{fs, process::Command};

use pitui_data::{
    ActiveRenderMode, ActiveUiContext, ClipboardContentKind, CommitCreationMetadata, ContextStack,
    DatasetChildren, DatasetCursor, DatasetIdentity, DatasetKey, DatasetSelection, FieldId,
    GitOperationLogEntryMetadata, GitOperationStatus, InteractionContextKind,
    InteractionContextMetadata, KeyCode, KeyModifiers, KeyStroke, PendingChordState,
    ReflogEntryMetadata, RenderContentProjection, RepositoryMetadata, ResolvedKeyAction,
    ResolvedOperationSet, UiFrame, UiLayoutProjection,
};
use pitui_ecs::{
    ClipboardRequests, CommandExecution, CommandExecutionLog, GitExecutionFailures,
    GitMutationSuccesses, OperationNotices, PendingInteractionNotices, ProjectionDiagnostics,
    RenderReconcileDiagnostics,
};

use super::*;

#[test]
fn repository_arguments_preserve_order_and_remove_duplicates() {
    let cwd = Path::new("/work");
    assert_eq!(
        repository_paths_from_args(
            cwd,
            [
                OsString::from("one"),
                OsString::from("/two"),
                OsString::from("one"),
            ],
        ),
        vec![PathBuf::from("/work/one"), PathBuf::from("/two")]
    );
    assert_eq!(
        repository_paths_from_args(cwd, Vec::<OsString>::new()),
        vec![cwd]
    );
}

#[test]
fn composition_bootstraps_history_from_dataset_ecs() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(output.status.success());
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("README.md"), "pitui\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-m", "initial"]);
    git(&["tag", "v1.0.0"]);
    fs::write(repository.path().join("README.md"), "pitui next\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-m", "second"]);

    let mut app = App::open_from(repository.path(), Vec::new()).unwrap();

    assert!(app.runtime().validate_registration_contracts().is_empty());

    assert_eq!(app.repositories().len(), 1);
    assert!(
        app.runtime()
            .world()
            .get::<RepositoryMetadata>(app.repositories()[0])
            .is_some()
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset()
    );
    assert!(
        app.runtime()
            .world()
            .resource::<ActiveRenderMode>()
            .layout
            .is_focus_owner(app.root_dataset())
    );
    assert!(
        app.runtime()
            .world()
            .get::<DatasetCursor>(app.root_dataset())
            .is_some()
    );
    assert!(
        !app.runtime()
            .world()
            .get::<DatasetChildren>(app.root_dataset())
            .unwrap()
            .0
            .is_empty()
    );

    let frame = app.runtime().world().resource::<UiFrame>();
    let UiLayoutProjection::Row(columns) = &frame.layout else {
        panic!("history mode must project a row layout");
    };
    assert_eq!(columns.len(), 2);
    let UiLayoutProjection::Dataset { panel: tree, .. } = &columns[0] else {
        panic!("history left column must be a Dataset panel");
    };
    assert!(tree.active);
    let RenderContentProjection::Rows(tree_rows) = &tree.content else {
        panic!("repositories/branches proxy must project rows");
    };
    assert_eq!(tree_rows.rows.len(), 2);
    assert_eq!(tree_rows.rows[0].depth, 0);
    assert_eq!(tree_rows.rows[1].depth, 1);

    let UiLayoutProjection::Dataset { panel: commits, .. } = &columns[1] else {
        panic!("history right column must be a Dataset panel");
    };
    assert!(!commits.active);
    let RenderContentProjection::Rows(commit_rows) = &commits.content else {
        panic!("commits proxy must project rows");
    };
    assert_eq!(commit_rows.rows.len(), 2);
    assert!(commit_rows.rows.iter().all(|row| {
        row.cells
            .iter()
            .any(|cell| cell.field == FieldId::CommitAuthor)
    }));
    assert!(commit_rows.rows.iter().all(|row| {
        row.cells
            .iter()
            .find(|cell| cell.field == FieldId::CommitAuthoredAt)
            .is_some_and(|cell| cell.text.len() == 16 && cell.text.contains(' '))
    }));
    assert_eq!(
        commit_rows
            .rows
            .iter()
            .filter(|row| row
                .cells
                .iter()
                .any(|cell| cell.field == FieldId::CommitTags))
            .count(),
        1
    );
    assert!(commit_rows.rows.iter().any(|row| {
        row.cells
            .iter()
            .any(|cell| cell.field == FieldId::CommitTags && cell.text == "v1.0.0")
    }));
    assert!(
        app.runtime()
            .world()
            .resource::<ProjectionDiagnostics>()
            .0
            .is_empty()
    );

    let generation = frame.generation;
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime().world().resource::<UiFrame>().generation,
        generation,
        "an unchanged schedule must not create a new frame or cause flicker"
    );

    let operations = app.runtime().world().resource::<ResolvedOperationSet>();
    for stroke in [
        KeyStroke::character('q'),
        KeyStroke::character('w'),
        KeyStroke::character('a'),
        KeyStroke::character('s'),
        KeyStroke::character('d'),
        KeyStroke::plain(KeyCode::Up),
        KeyStroke::plain(KeyCode::Down),
        KeyStroke::plain(KeyCode::Left),
        KeyStroke::plain(KeyCode::Right),
        KeyStroke::character('h'),
    ] {
        assert!(operations.key_bindings.contains_key(&stroke));
    }
    let control_space = KeyStroke {
        code: KeyCode::Space,
        modifiers: KeyModifiers::control(),
    };
    assert!(!operations.key_bindings.contains_key(&control_space));

    let repository_row = app.repositories()[0];
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('s')));
    app.runtime_mut().run_schedule();
    assert_ne!(
        app.runtime()
            .world()
            .get::<DatasetCursor>(app.root_dataset())
            .unwrap()
            .0,
        Some(repository_row)
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset(),
        "row navigation must not steal focus or change Active Dataset"
    );
    assert!(matches!(
        app.runtime()
            .world()
            .resource::<CommandExecutionLog>()
            .0
            .last(),
        Some((_, CommandExecution::Completed))
    ));

    let (repository_key, root_path, branch) = {
        let world = app.runtime().world();
        let DatasetIdentity::Repository(repository_key) =
            &world.get::<DatasetKey>(repository_row).unwrap().0
        else {
            panic!("repository row must have repository identity");
        };
        let metadata = world.get::<RepositoryMetadata>(repository_row).unwrap();
        (
            repository_key.clone(),
            metadata.0.root.clone(),
            metadata.0.current_branch.clone().unwrap(),
        )
    };
    let (commits_dataset, commit_dataset, head) = {
        let world = app.runtime().world();
        let commits = world
            .resource::<DatasetIndex>()
            .get(&DatasetIdentity::Commits {
                repository: repository_key.clone(),
                branch: branch.clone(),
            })
            .unwrap();
        let commit = world.get::<DatasetCursor>(commits).unwrap().0.unwrap();
        let DatasetIdentity::Commit { hash, .. } = &world.get::<DatasetKey>(commit).unwrap().0
        else {
            panic!("commit cursor must reference a Commit Dataset");
        };
        (commits, commit, hash.clone())
    };
    app.runtime_mut()
        .enqueue_git_command(GitCommandData {
            repository_dataset: repository_row,
            cwd: root_path,
            command: GitCommand::LoadCommitDetail {
                commit: head.clone(),
            },
        })
        .unwrap();
    app.runtime_mut().run_schedule();

    let files_dataset = {
        let world = app.runtime().world();
        let index = world.resource::<DatasetIndex>();
        index
            .get(&DatasetIdentity::Files {
                repository: repository_key,
                commit: head,
            })
            .unwrap()
    };
    let mut bindings = app
        .runtime()
        .world()
        .resource::<ActiveUiContext>()
        .render_bindings
        .clone();
    bindings.bind(RenderBindingId::CurrentCommits, commits_dataset);
    bindings.bind(RenderBindingId::CurrentCommit, commit_dataset);
    bindings.bind(RenderBindingId::CurrentFiles, files_dataset);
    app.runtime_mut()
        .replace_context_from_mode(
            commits_dataset,
            RenderModeId::from("history"),
            bindings.clone(),
            ResolvedOperationSet::default(),
        )
        .unwrap();
    app.runtime_mut().run_schedule();

    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .render_bindings
            .get(&RenderBindingId::CurrentCommit),
        Some(commit_dataset)
    );
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('s')));
    app.runtime_mut().run_schedule();
    let previewed_commit = app
        .runtime()
        .world()
        .resource::<ActiveUiContext>()
        .render_bindings
        .get(&RenderBindingId::CurrentCommit)
        .unwrap();
    assert_ne!(previewed_commit, commit_dataset);
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        commits_dataset,
        "commit preview reconciliation must not move keyboard focus"
    );
    assert!(
        app.runtime()
            .world()
            .resource::<ActiveRenderMode>()
            .layout
            .is_focus_owner(commits_dataset)
    );
    let previewed_files = app
        .runtime()
        .world()
        .resource::<ActiveUiContext>()
        .render_bindings
        .get(&RenderBindingId::CurrentFiles)
        .unwrap();
    assert_ne!(previewed_files, files_dataset);
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('w')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .render_bindings
            .get(&RenderBindingId::CurrentCommit),
        Some(commit_dataset)
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .render_bindings
            .get(&RenderBindingId::CurrentFiles),
        Some(files_dataset)
    );

    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::plain(
            KeyCode::Space,
        )));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .get::<pitui_data::DatasetSelection>(commits_dataset)
            .unwrap()
            .0,
        vec![commit_dataset]
    );
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::plain(
            KeyCode::Space,
        )));
    app.runtime_mut().run_schedule();
    assert!(
        app.runtime()
            .world()
            .get::<pitui_data::DatasetSelection>(commits_dataset)
            .unwrap()
            .0
            .is_empty()
    );

    let copy_prefix = KeyStroke::control('c');
    assert!(matches!(
        app.runtime()
            .world()
            .resource::<ResolvedOperationSet>()
            .key_bindings
            .get(&copy_prefix)
            .map(|binding| &binding.action),
        Some(ResolvedKeyAction::EnterChord(_))
    ));
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(copy_prefix.clone()));
    app.runtime_mut().run_schedule();
    let commit_chord = app.runtime().world().resource::<ResolvedOperationSet>();
    assert_eq!(
        commit_chord
            .key_bindings
            .keys()
            .filter_map(|stroke| match stroke.code {
                KeyCode::Character(character) => Some(character),
                _ => None,
            })
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from(['h', 'i', 'm'])
    );
    assert!(
        app.runtime()
            .world()
            .resource::<UiFrame>()
            .footer
            .bindings
            .iter()
            .all(|binding| commit_chord.key_bindings.contains_key(&binding.stroke))
    );

    for (suffix, expected_kind) in [
        ('h', ClipboardContentKind::CommitHashes),
        ('i', ClipboardContentKind::CommitInfo),
        ('m', ClipboardContentKind::CommitMessage),
    ] {
        if suffix != 'h' {
            app.runtime_mut()
                .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::control('c')));
            app.runtime_mut().run_schedule();
        }
        app.runtime_mut()
            .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character(suffix)));
        app.runtime_mut().run_schedule();
        let request = app
            .runtime()
            .world()
            .resource::<ClipboardRequests>()
            .0
            .last()
            .unwrap();
        assert_eq!(request.kind, expected_kind);
        assert!(!request.text.is_empty());
    }
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ClipboardRequests>()
            .0
            .last()
            .unwrap()
            .text,
        "second"
    );

    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::CancelChord);
    app.runtime_mut().run_schedule();

    // Right/Left traverse logical Dataset depth without wrapping. Crossing
    // a mode edge pushes/pops the complete context; within a mode only the
    // Active Dataset changes.
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('d')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .render_mode,
        RenderModeId::from("commit")
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        commits_dataset
    );
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 1);
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('d')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        files_dataset
    );
    assert_eq!(
        app.runtime().world().resource::<ContextStack>().0.len(),
        1,
        "moving between focusable leaves must not push another context"
    );

    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(copy_prefix));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ResolvedOperationSet>()
            .key_bindings
            .keys()
            .filter_map(|stroke| match stroke.code {
                KeyCode::Character(character) => Some(character),
                _ => None,
            })
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from(['a', 'n', 'r'])
    );
    assert_eq!(
        app.runtime().world().resource::<PendingChordState>().prefix,
        vec![KeyStroke::control('c')]
    );
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('n')));
    app.runtime_mut().run_schedule();
    let file_name = app
        .runtime()
        .world()
        .resource::<ClipboardRequests>()
        .0
        .last()
        .unwrap();
    assert_eq!(file_name.kind, ClipboardContentKind::FileName);
    assert_eq!(file_name.text, "README.md");
    for (suffix, expected_kind) in [
        ('a', ClipboardContentKind::FileAbsolutePath),
        ('r', ClipboardContentKind::FileRelativePath),
    ] {
        app.runtime_mut()
            .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::control('c')));
        app.runtime_mut().run_schedule();
        app.runtime_mut()
            .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character(suffix)));
        app.runtime_mut().run_schedule();
        let request = app
            .runtime()
            .world()
            .resource::<ClipboardRequests>()
            .0
            .last()
            .unwrap();
        assert_eq!(request.kind, expected_kind);
        assert!(request.text.ends_with("README.md"));
    }
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::CancelChord);
    app.runtime_mut().run_schedule();

    let diff_dataset = app
        .runtime()
        .world()
        .resource::<ActiveUiContext>()
        .render_bindings
        .get(&RenderBindingId::CurrentFileChanges)
        .unwrap();
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('d')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .render_mode,
        RenderModeId::from("file-diff.unified")
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        files_dataset
    );
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 2);
    let UiLayoutProjection::Row(file_diff_columns) =
        &app.runtime().world().resource::<UiFrame>().layout
    else {
        panic!("FileDiff mode must remain a row");
    };
    assert!(matches!(
        file_diff_columns.first(),
        Some(UiLayoutProjection::Column(_))
    ));

    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('d')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        diff_dataset
    );
    assert!(
        app.runtime()
            .world()
            .resource::<ResolvedOperationSet>()
            .key_bindings
            .contains_key(&KeyStroke::control('c')),
        "FileChanges focus must reuse the CurrentFiles copy operations"
    );
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::control('c')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ResolvedOperationSet>()
            .key_bindings
            .keys()
            .filter_map(|stroke| match stroke.code {
                KeyCode::Character(character) => Some(character),
                _ => None,
            })
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from(['a', 'n', 'r'])
    );
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('r')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ClipboardRequests>()
            .0
            .last()
            .unwrap()
            .kind,
        ClipboardContentKind::FileRelativePath
    );
    let executions_before_deepest_right = app
        .runtime()
        .world()
        .resource::<CommandExecutionLog>()
        .0
        .len();
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('d')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        diff_dataset,
        "Right at the deepest leaf must not wrap"
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<CommandExecutionLog>()
            .0
            .len(),
        executions_before_deepest_right,
        "an unavailable deepest Right operation must not be invoked"
    );

    for expected in [
        (files_dataset, RenderModeId::from("file-diff.unified"), 2),
        (files_dataset, RenderModeId::from("commit"), 1),
        (commits_dataset, RenderModeId::from("commit"), 1),
        (commits_dataset, RenderModeId::from("history"), 0),
        (app.root_dataset(), RenderModeId::from("history"), 0),
    ] {
        app.runtime_mut()
            .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('a')));
        app.runtime_mut().run_schedule();
        let context = app.runtime().world().resource::<ActiveUiContext>();
        assert_eq!(context.active_dataset, expected.0);
        assert_eq!(context.render_mode, expected.1);
        assert_eq!(
            app.runtime().world().resource::<ContextStack>().0.len(),
            expected.2
        );
    }
    app.runtime_mut()
        .enqueue_input_intent(pitui_data::InputIntent::Key(KeyStroke::character('a')));
    app.runtime_mut().run_schedule();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset(),
        "Left at the outermost leaf must not wrap"
    );

    fs::write(repository.path().join("README.md"), "pitui working tree\n").unwrap();
    fs::write(repository.path().join("NEW.md"), "untracked\n").unwrap();
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::control('g')));
    let changes = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalChanges)
        .unwrap();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        changes
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .render_mode,
        RenderModeId::from("changes.unified")
    );
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 1);
    assert_eq!(
        app.runtime()
            .world()
            .get::<pitui_data::DatasetNavigationOrder>(changes)
            .unwrap()
            .0
            .len(),
        4,
        "Changes owns Staged/Unstaged group rows and third-level files"
    );
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('s')));
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('s')));
    let selected_change = app
        .runtime()
        .world()
        .get::<DatasetCursor>(changes)
        .unwrap()
        .0
        .unwrap();
    assert!(matches!(
        app.runtime()
            .world()
            .get::<DatasetKey>(selected_change)
            .map(|key| &key.0),
        Some(DatasetIdentity::WorkingTreeFile { .. })
    ));
    let changes_diff = app
        .runtime()
        .world()
        .resource::<ActiveUiContext>()
        .render_bindings
        .get(&RenderBindingId::CurrentChangesFileChanges)
        .unwrap();
    assert!(
        app.runtime()
            .world()
            .get::<pitui_data::WorkingTreeFileChangesMetadata>(changes_diff)
            .is_some(),
        "moving the Changes cursor synchronously refreshes the right diff"
    );
    let UiLayoutProjection::Row(changes_columns) = &app.ui_frame().layout else {
        panic!("Changes must render as a reusable two-column mode");
    };
    assert_eq!(changes_columns.len(), 2);
    let UiLayoutProjection::Dataset { panel: diff, .. } = &changes_columns[1] else {
        panic!("Changes right column must be a Dataset proxy");
    };
    assert!(matches!(
        diff.content,
        RenderContentProjection::UnifiedDiff(_)
    ));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        changes,
        "automatic diff refresh must not steal focus"
    );
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('d')));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        changes_diff
    );
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('a')));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        changes
    );
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::control('r')));
    assert!(
        app.runtime()
            .world()
            .get::<pitui_data::HasSnapshot>(changes_diff)
            .unwrap()
            .0,
        "manual refresh reloads the selected Changes diff after invalidation"
    );

    // Selection is owned by the global Changes Dataset even when the
    // reusable diff panel is focused. Whole-file stage/unstage and commit
    // all travel through the same CommandInvocation -> GitCommand path.
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('d')));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        changes_diff
    );
    let (change_repository, change_path) = match &app
        .runtime()
        .world()
        .get::<DatasetKey>(selected_change)
        .unwrap()
        .0
    {
        DatasetIdentity::WorkingTreeFile {
            repository,
            boundary: pitui_data::ChangeBoundary::Unstaged,
            path,
        } => (repository.clone(), path.clone()),
        identity => panic!("expected an unstaged working-tree file, got {identity:?}"),
    };
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::plain(
        KeyCode::Space,
    )));
    assert_eq!(
        app.runtime()
            .world()
            .get::<DatasetSelection>(changes)
            .unwrap()
            .0,
        vec![selected_change]
    );

    let shifted = |character| {
        let mut stroke = KeyStroke::character(character);
        stroke.modifiers.shift = true;
        stroke
    };
    app.dispatch_input(pitui_data::InputIntent::Key(shifted('s')));
    let staged_file = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::WorkingTreeFile {
            repository: change_repository.clone(),
            boundary: pitui_data::ChangeBoundary::Staged,
            path: change_path.clone(),
        })
        .expect("stage refresh must move the file into the staged group");
    let staged_diff = app
        .runtime()
        .world()
        .get::<DatasetChildren>(staged_file)
        .unwrap()
        .0[0];
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        staged_diff,
        "stage from the diff panel preserves the focused logical column"
    );
    assert!(
        app.runtime()
            .world()
            .get::<DatasetSelection>(changes)
            .unwrap()
            .0
            .is_empty(),
        "snapshot reconciliation removes the old boundary entity from selection"
    );

    for _ in 0..8 {
        if app
            .runtime()
            .world()
            .get::<DatasetCursor>(changes)
            .unwrap()
            .0
            == Some(staged_file)
        {
            break;
        }
        app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('s')));
    }
    assert_eq!(
        app.runtime()
            .world()
            .get::<DatasetCursor>(changes)
            .unwrap()
            .0,
        Some(staged_file)
    );
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::plain(
        KeyCode::Space,
    )));
    app.dispatch_input(pitui_data::InputIntent::Key(shifted('u')));
    let unstaged_file = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::WorkingTreeFile {
            repository: change_repository.clone(),
            boundary: pitui_data::ChangeBoundary::Unstaged,
            path: change_path.clone(),
        })
        .expect("unstage refresh must move the file into the unstaged group");
    let unstaged_diff = app
        .runtime()
        .world()
        .get::<DatasetChildren>(unstaged_file)
        .unwrap()
        .0[0];
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        unstaged_diff,
        "unstage from the diff panel preserves the focused logical column"
    );

    for _ in 0..8 {
        if app
            .runtime()
            .world()
            .get::<DatasetCursor>(changes)
            .unwrap()
            .0
            == Some(unstaged_file)
        {
            break;
        }
        app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('s')));
    }
    assert_eq!(
        app.runtime()
            .world()
            .get::<DatasetCursor>(changes)
            .unwrap()
            .0,
        Some(unstaged_file)
    );
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::plain(
        KeyCode::Space,
    )));
    app.dispatch_input(pitui_data::InputIntent::Key(shifted('s')));

    app.dispatch_input(pitui_data::InputIntent::Key(shifted('c')));
    let commit_creation = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::CommitCreation(change_repository.clone()))
        .unwrap();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        commit_creation
    );
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 2);
    let creation = app
        .runtime()
        .world()
        .get::<CommitCreationMetadata>(commit_creation)
        .unwrap();
    assert_eq!(creation.repository, change_repository);
    assert!(creation.message.is_empty());
    assert_eq!(creation.staged_paths, vec![change_path.clone()]);
    let commit_keys = app
        .runtime()
        .world()
        .resource::<ResolvedOperationSet>()
        .key_bindings
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        commit_keys,
        std::collections::HashSet::from([
            KeyStroke::character('h'),
            KeyStroke::plain(KeyCode::Escape),
            KeyStroke::plain(KeyCode::Enter),
        ]),
        "Commit Creation owns an exclusive semantic Operation Set"
    );
    let UiLayoutProjection::Overlay(layers) = &app.ui_frame().layout else {
        panic!("Commit Creation must be rendered as a data-backed overlay");
    };
    let UiLayoutProjection::Dataset { panel, .. } = layers.last().unwrap() else {
        panic!("Commit Creation overlay must bind its own Render Proxy");
    };
    assert_eq!(panel.proxy, RenderProxyId::from("commit-creation.editor"));
    assert_eq!(panel.renderer, pitui_data::RendererKind::CommitCreation);
    assert!(matches!(
        panel.content,
        RenderContentProjection::Interaction(_)
    ));

    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::plain(
        KeyCode::Enter,
    )));
    assert_eq!(
        app.runtime()
            .world()
            .get::<CommitCreationMetadata>(commit_creation)
            .unwrap()
            .error
            .as_deref(),
        Some("Commit message cannot be empty")
    );
    app.dispatch_input(pitui_data::InputIntent::Paste(
        "created from the Changes Context".into(),
    ));
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::plain(
        KeyCode::Enter,
    )));
    let remaining_diff = app
        .runtime()
        .world()
        .resource::<ActiveUiContext>()
        .render_bindings
        .get(&RenderBindingId::CurrentChangesFileChanges)
        .expect("the remaining unstaged file keeps the Changes diff panel populated");
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        remaining_diff,
        "commit restores the same logical diff focus on the next remaining file"
    );
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 1);

    let output = Command::new("git")
        .args(["log", "-1", "--pretty=%s"])
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "created from the Changes Context"
    );
    let output = Command::new("git")
        .args(["show", "--format=", "--name-only", "HEAD"])
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        change_path.as_str()
    );
    assert!(
        app.runtime()
            .world()
            .get::<pitui_data::DatasetNavigationOrder>(changes)
            .unwrap()
            .0
            .iter()
            .all(|entity| !matches!(
                app.runtime()
                    .world()
                    .get::<DatasetKey>(*entity)
                    .map(|key| &key.0),
                Some(DatasetIdentity::WorkingTreeFile {
                    boundary: pitui_data::ChangeBoundary::Staged,
                    ..
                })
            )),
        "commit refresh must leave no staged file rows"
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<GitMutationSuccesses>()
            .0
            .iter()
            .filter(|success| matches!(
                success.command,
                GitCommand::StagePaths { .. }
                    | GitCommand::UnstagePaths { .. }
                    | GitCommand::Commit { .. }
            ))
            .count(),
        4
    );

    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::plain(
        KeyCode::Escape,
    )));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset()
    );
    assert!(
        app.runtime()
            .world()
            .resource::<ContextStack>()
            .0
            .is_empty()
    );

    let normal_shortcuts = app
        .runtime()
        .world()
        .resource::<ResolvedOperationSet>()
        .key_bindings
        .len();
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('h')));
    let interaction = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalInteractionContext)
        .unwrap();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        interaction
    );
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 1);
    let help = app
        .runtime()
        .world()
        .get::<InteractionContextMetadata>(interaction)
        .unwrap();
    let InteractionContextKind::Help { entries } = &help.kind else {
        panic!("h must open the data-backed Help Context");
    };
    assert_eq!(entries.len(), normal_shortcuts);
    assert!(matches!(
        app.ui_frame().layout,
        UiLayoutProjection::Overlay(_)
    ));
    let help_operations = app.runtime().world().resource::<ResolvedOperationSet>();
    assert_eq!(help_operations.key_bindings.len(), 2);
    assert!(
        help_operations
            .key_bindings
            .contains_key(&KeyStroke::plain(KeyCode::Escape))
    );
    assert!(
        help_operations
            .key_bindings
            .contains_key(&KeyStroke::character('q'))
    );

    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('q')));
    assert!(!app.quit_requested(), "q closes Help instead of quitting");
    assert!(
        app.runtime()
            .world()
            .resource::<ContextStack>()
            .0
            .is_empty()
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset()
    );

    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::control('p')));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        interaction
    );
    app.dispatch_input(pitui_data::InputIntent::Paste("qui".into()));
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::character('t')));
    let palette = app
        .runtime()
        .world()
        .get::<InteractionContextMetadata>(interaction)
        .unwrap();
    assert!(matches!(
        &palette.kind,
        InteractionContextKind::CommandPalette { query, .. } if query == "quit"
    ));
    app.dispatch_input(pitui_data::InputIntent::Key(KeyStroke::plain(
        KeyCode::Enter,
    )));
    assert!(
        !app.quit_requested(),
        "palette commands run only after Context pop"
    );
    app.runtime_mut().run_schedule();
    assert!(
        app.quit_requested(),
        "the palette emits the same deferred quit CommandInvocation"
    );
}

#[test]
fn changes_supports_clean_repositories_and_committing_the_last_visible_file() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("file.txt"), "initial\n").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-m", "initial"]);

    let mut app = App::open_from(repository.path(), Vec::new()).unwrap();
    let changes = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalChanges)
        .unwrap();

    app.dispatch_input(InputIntent::Key(KeyStroke::control('g')));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        changes
    );
    assert!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .render_bindings
            .get(&RenderBindingId::CurrentChangesFileChanges)
            .is_none()
    );
    assert!(
        app.runtime()
            .world()
            .resource::<RenderReconcileDiagnostics>()
            .last_render_error
            .is_none(),
        "a valid empty Changes snapshot projects a blank detail area"
    );
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Escape)));

    fs::write(repository.path().join("file.txt"), "changed\n").unwrap();
    app.dispatch_input(InputIntent::Key(KeyStroke::control('g')));
    app.dispatch_input(InputIntent::Key(KeyStroke::character('s')));
    app.dispatch_input(InputIntent::Key(KeyStroke::character('s')));
    let file = app
        .runtime()
        .world()
        .get::<DatasetCursor>(changes)
        .unwrap()
        .0
        .unwrap();
    assert!(matches!(
        app.runtime()
            .world()
            .get::<DatasetKey>(file)
            .map(|key| &key.0),
        Some(DatasetIdentity::WorkingTreeFile {
            boundary: pitui_data::ChangeBoundary::Unstaged,
            ..
        })
    ));
    app.dispatch_input(InputIntent::Key(KeyStroke::character('d')));
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Space)));
    let mut stage = KeyStroke::character('s');
    stage.modifiers.shift = true;
    app.dispatch_input(InputIntent::Key(stage));
    let mut commit = KeyStroke::character('c');
    commit.modifiers.shift = true;
    app.dispatch_input(InputIntent::Key(commit));
    app.dispatch_input(InputIntent::Paste("last visible change".into()));
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Enter)));

    let context = app.runtime().world().resource::<ActiveUiContext>();
    assert_eq!(context.active_dataset, changes);
    assert!(
        context
            .render_bindings
            .get(&RenderBindingId::CurrentChangesFileChanges)
            .is_none()
    );
    assert!(
        app.runtime()
            .world()
            .resource::<ActiveRenderMode>()
            .layout
            .is_focus_owner(changes),
        "the removed diff entity must never remain as stale focus"
    );
    assert!(app.runtime_mut().validate().is_empty());
    let output = Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn failed_commit_restores_changes_then_opens_a_redacted_notice_context() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("file.txt"), "initial\n").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-m", "initial"]);

    let mut app = App::open_from(repository.path(), Vec::new()).unwrap();
    fs::write(repository.path().join("file.txt"), "changed\n").unwrap();
    app.dispatch_input(InputIntent::Key(KeyStroke::control('g')));
    let changes = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalChanges)
        .unwrap();
    app.dispatch_input(InputIntent::Key(KeyStroke::character('s')));
    app.dispatch_input(InputIntent::Key(KeyStroke::character('s')));
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Space)));
    let mut stage = KeyStroke::character('s');
    stage.modifiers.shift = true;
    app.dispatch_input(InputIntent::Key(stage));
    let mut commit = KeyStroke::character('c');
    commit.modifiers.shift = true;
    app.dispatch_input(InputIntent::Key(commit));

    // Simulate an external change while the Commit Creation Dataset owns
    // input. The pending commit now has no staged snapshot and must fail.
    git(&["reset", "--hard", "HEAD"]);
    let secret = "secret message must not reach the Notice";
    app.dispatch_input(InputIntent::Paste(secret.into()));
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Enter)));

    let interaction = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalInteractionContext)
        .unwrap();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        interaction
    );
    let metadata = app
        .runtime()
        .world()
        .get::<InteractionContextMetadata>(interaction)
        .unwrap();
    let InteractionContextKind::Notice { title, message } = &metadata.kind else {
        panic!("Git failure must be materialized as a Notice Context");
    };
    assert_eq!(title, "Git operation failed");
    assert!(message.starts_with("commit:"));
    assert!(!message.contains(secret));
    assert!(matches!(
        app.ui_frame().layout,
        UiLayoutProjection::Overlay(_)
    ));
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 2);
    assert!(
        app.runtime()
            .world()
            .resource::<PendingInteractionNotices>()
            .0
            .is_empty()
    );
    assert_eq!(
        app.runtime()
            .world()
            .resource::<GitExecutionFailures>()
            .0
            .len(),
        1
    );
    let log = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalGitOperationLog)
        .unwrap();
    let failed_commit = app
        .runtime()
        .world()
        .get::<DatasetChildren>(log)
        .unwrap()
        .0
        .iter()
        .filter_map(|entry| {
            app.runtime()
                .world()
                .get::<GitOperationLogEntryMetadata>(*entry)
        })
        .find(|entry| entry.operation == "commit" && entry.status == GitOperationStatus::Failure)
        .expect("failed Git command must be retained in the session log Dataset");
    assert!(!failed_commit.message.contains(secret));
    assert!(!failed_commit.started_at_utc.is_empty());
    let notice_keys = app
        .runtime()
        .world()
        .resource::<ResolvedOperationSet>()
        .key_bindings
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        notice_keys,
        std::collections::HashSet::from([
            KeyStroke::plain(KeyCode::Escape),
            KeyStroke::plain(KeyCode::Enter),
            KeyStroke::character('q'),
        ])
    );

    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Enter)));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        changes
    );
    assert_eq!(app.runtime().world().resource::<ContextStack>().0.len(), 1);
}

#[test]
fn initial_non_repository_failure_keeps_a_blank_snapshot_and_presents_notice_later() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::open_from(directory.path(), Vec::new()).unwrap();
    let interaction = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalInteractionContext)
        .unwrap();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        interaction
    );
    assert!(matches!(
        &app
            .runtime()
            .world()
            .get::<InteractionContextMetadata>(interaction)
            .unwrap()
            .kind,
        InteractionContextKind::Notice { title, message }
            if title == "Git operation failed"
                && message.starts_with("load_repository:")
    ));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<GitExecutionFailures>()
            .0
            .len(),
        1
    );

    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Escape)));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset()
    );
    assert!(app.runtime_mut().validate().is_empty());
}

#[test]
fn git_operation_log_is_a_navigable_data_backed_two_column_context() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(output.status.success());
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("file.txt"), "initial\n").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-m", "initial"]);

    let mut app = App::open_from(repository.path(), Vec::new()).unwrap();
    let log = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalGitOperationLog)
        .unwrap();
    let entries = app
        .runtime()
        .world()
        .get::<DatasetChildren>(log)
        .unwrap()
        .0
        .clone();
    assert!(entries.len() >= 3);
    let canonical_repository = repository.path().canonicalize().unwrap();
    assert!(entries.iter().all(|entry| {
        app.runtime()
            .world()
            .get::<GitOperationLogEntryMetadata>(*entry)
            .is_some_and(|metadata| {
                metadata.status == GitOperationStatus::Success
                    && !metadata.operation.is_empty()
                    && metadata.repository.as_path() == canonical_repository
            })
    }));
    let newest = entries[0];
    assert_eq!(
        app.runtime().world().get::<DatasetCursor>(log).unwrap().0,
        Some(newest)
    );

    app.dispatch_input(InputIntent::CommandLine("logs".into()));
    let context = app.runtime().world().resource::<ActiveUiContext>();
    let displayed_entry = app
        .runtime()
        .world()
        .get::<DatasetCursor>(log)
        .unwrap()
        .0
        .unwrap();
    assert_eq!(context.active_dataset, log);
    assert_eq!(context.render_mode, RenderModeId::from("git-operation-log"));
    assert_eq!(
        context
            .render_bindings
            .get(&RenderBindingId::CurrentGitOperationLogEntry),
        Some(displayed_entry)
    );
    let UiLayoutProjection::Row(columns) = &app.ui_frame().layout else {
        panic!("Git operation log must use its configured two-column Mode");
    };
    assert_eq!(columns.len(), 2);
    let UiLayoutProjection::Dataset { panel: list, .. } = &columns[0] else {
        panic!("left log column must be a Dataset panel");
    };
    let RenderContentProjection::Rows(rows) = &list.content else {
        panic!("log list proxy must project typed rows");
    };
    assert_eq!(
        rows.rows.len(),
        app.runtime()
            .world()
            .get::<DatasetChildren>(log)
            .unwrap()
            .0
            .len()
    );
    assert!(
        rows.rows[0]
            .cells
            .iter()
            .any(|cell| cell.field == FieldId::GitOperationName)
    );
    let UiLayoutProjection::Dataset { panel: detail, .. } = &columns[1] else {
        panic!("right log column must be a Dataset detail panel");
    };
    assert!(matches!(detail.content, RenderContentProjection::Detail(_)));

    if entries.len() > 1 {
        app.dispatch_input(InputIntent::Key(KeyStroke::character('s')));
        assert_ne!(
            app.runtime()
                .world()
                .resource::<ActiveUiContext>()
                .render_bindings
                .get(&RenderBindingId::CurrentGitOperationLogEntry),
            Some(displayed_entry)
        );
        assert_eq!(
            app.runtime()
                .world()
                .resource::<ActiveUiContext>()
                .active_dataset,
            log,
            "log detail preview must not steal list focus"
        );
    }
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Escape)));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset()
    );
}

#[test]
fn injected_jsonl_sink_and_session_log_receive_the_same_git_results() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(output.status.success());
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("file.txt"), "initial\n").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-m", "initial"]);

    let logs = tempfile::tempdir().unwrap();
    let path = logs.path().join("git.jsonl");
    let sink =
        pitui_git::logging::JsonlGitOperationLogSink::open(pitui_git::logging::JsonlGitLogConfig {
            path: path.clone(),
            level: pitui_git::logging::GitLogLevel::Info,
            max_bytes: 1024 * 1024,
            keep_files: 2,
            rotate_on_start: false,
            flush_interval: Duration::ZERO,
            buffer_capacity: 1024,
            max_message_chars: 4096,
        })
        .unwrap();
    let app =
        App::open_from_with_log_sink(repository.path(), Vec::new(), Arc::new(sink.clone()), None)
            .unwrap();
    pitui_git::logging::GitOperationLogSink::flush(&sink);

    let log = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalGitOperationLog)
        .unwrap();
    let session_count = app
        .runtime()
        .world()
        .get::<DatasetChildren>(log)
        .unwrap()
        .0
        .len();
    let contents = fs::read_to_string(path).unwrap();
    assert_eq!(contents.lines().count(), session_count);
    assert!(contents.lines().all(|line| {
        line.starts_with('{')
            && line.ends_with('}')
            && line.contains("\"operation\":")
            && line.contains("\"duration_ms\":")
            && line.contains("\"status\":\"success\"")
    }));
}

#[test]
fn reflog_command_opens_a_data_backed_context_and_copies_the_current_hash() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("file.txt"), "first\n").unwrap();
    git(&["add", "file.txt"]);
    git(&["commit", "-m", "first"]);
    fs::write(repository.path().join("file.txt"), "second\n").unwrap();
    git(&["commit", "-am", "second"]);

    let mut app = App::open_from(repository.path(), Vec::new()).unwrap();
    app.dispatch_input(InputIntent::CommandLine("reflog".into()));

    let context = app.runtime().world().resource::<ActiveUiContext>();
    assert_eq!(
        context.render_mode,
        RenderModeId::from("reflog"),
        "failures={:?}; notices={:?}; reconcile={:?}",
        app.runtime().world().resource::<GitExecutionFailures>(),
        app.runtime().world().resource::<OperationNotices>(),
        app.runtime()
            .world()
            .resource::<RenderReconcileDiagnostics>()
    );
    let reflog = context.active_dataset;
    assert!(matches!(
        app.runtime().world().get::<DatasetKey>(reflog),
        Some(DatasetKey(DatasetIdentity::Reflog(_)))
    ));
    let entries = app
        .runtime()
        .world()
        .get::<DatasetChildren>(reflog)
        .unwrap();
    assert!(entries.0.len() >= 2);
    let first = app
        .runtime()
        .world()
        .get::<DatasetCursor>(reflog)
        .unwrap()
        .0
        .unwrap();
    let first_metadata = app
        .runtime()
        .world()
        .get::<ReflogEntryMetadata>(first)
        .unwrap()
        .clone();
    assert_eq!(
        context
            .render_bindings
            .get(&RenderBindingId::CurrentReflogEntry),
        Some(first)
    );

    let UiLayoutProjection::Row(columns) = &app.ui_frame().layout else {
        panic!("Reflog mode must project a row");
    };
    assert_eq!(columns.len(), 2);
    let UiLayoutProjection::Dataset { panel: list, .. } = &columns[0] else {
        panic!("Reflog list must be the left Dataset panel");
    };
    assert!(list.active);
    let RenderContentProjection::Rows(rows) = &list.content else {
        panic!("Reflog list must project rows");
    };
    assert_eq!(rows.rows.len(), entries.0.len());
    let UiLayoutProjection::Dataset { panel: detail, .. } = &columns[1] else {
        panic!("Reflog detail must be the right Dataset panel");
    };
    assert!(matches!(detail.content, RenderContentProjection::Detail(_)));

    app.dispatch_input(InputIntent::Key(KeyStroke::control('c')));
    app.dispatch_input(InputIntent::Key(KeyStroke::character('h')));
    let copied = app.take_clipboard_requests().pop().unwrap();
    assert_eq!(copied.kind, ClipboardContentKind::ReflogHash);
    assert_eq!(copied.text, first_metadata.0.hash.0);
    assert_eq!(copied.source_entities, vec![first]);

    app.dispatch_input(InputIntent::Key(KeyStroke::character('s')));
    let context = app.runtime().world().resource::<ActiveUiContext>();
    assert_eq!(context.active_dataset, reflog);
    assert_ne!(
        context
            .render_bindings
            .get(&RenderBindingId::CurrentReflogEntry),
        Some(first),
        "moving the Reflog cursor must refresh detail without stealing focus"
    );
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Escape)));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        app.root_dataset()
    );
}

#[test]
fn commits_selection_is_the_only_cherry_pick_source_and_replays_oldest_first() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("base.txt"), "base\n").unwrap();
    git(&["add", "base.txt"]);
    git(&["commit", "-m", "base"]);
    git(&["switch", "-c", "feature"]);
    fs::write(repository.path().join("one.txt"), "one\n").unwrap();
    git(&["add", "one.txt"]);
    git(&["commit", "-m", "feature one"]);
    let one = git(&["rev-parse", "HEAD"]);
    fs::write(repository.path().join("two.txt"), "two\n").unwrap();
    git(&["add", "two.txt"]);
    git(&["commit", "-m", "feature two"]);
    let two = git(&["rev-parse", "HEAD"]);
    git(&["switch", "main"]);

    let mut app = App::open_from(repository.path(), Vec::new()).unwrap();
    let repository_entity = app.repositories()[0];
    let DatasetIdentity::Repository(repository_key) = app
        .runtime()
        .world()
        .get::<DatasetKey>(repository_entity)
        .unwrap()
        .0
        .clone()
    else {
        panic!("composition repository must have a Repository identity");
    };
    let (feature, feature_name) = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .by_key
        .iter()
        .find_map(|(identity, entity)| match identity {
            DatasetIdentity::Branch { repository, name }
                if repository == &repository_key && name.0 == "feature" =>
            {
                Some((*entity, name.clone()))
            }
            _ => None,
        })
        .unwrap();
    let feature_commits_identity = DatasetIdentity::Commits {
        repository: repository_key,
        branch: feature_name.clone(),
    };
    let feature_commits = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&feature_commits_identity)
        .unwrap();
    app.runtime_mut()
        .enqueue_git_command(GitCommandData {
            repository_dataset: repository_entity,
            cwd: repository.path().to_path_buf(),
            command: GitCommand::LoadCommits {
                branch: feature_name,
                limit: 50,
            },
        })
        .unwrap();
    app.runtime_mut().run_schedule();
    let root = app.root_dataset();
    app.runtime_mut().set_cursor(root, Some(feature)).unwrap();
    app.runtime_mut().run_schedule();
    app.dispatch_input(InputIntent::Key(KeyStroke::character('d')));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        feature_commits
    );
    assert!(
        !app.runtime()
            .world()
            .resource::<ResolvedOperationSet>()
            .commands
            .contains_key("cherry-pick"),
        "cherry-pick must be unavailable before Commits.Selection exists"
    );

    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Space)));
    app.dispatch_input(InputIntent::Key(KeyStroke::character('s')));
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Space)));
    let selected = &app
        .runtime()
        .world()
        .get::<DatasetSelection>(feature_commits)
        .unwrap()
        .0;
    let selected_hashes = selected
        .iter()
        .filter_map(
            |entity| match &app.runtime().world().get::<DatasetKey>(*entity).unwrap().0 {
                DatasetIdentity::Commit { hash, .. } => Some(hash.0.as_str()),
                _ => None,
            },
        )
        .collect::<Vec<_>>();
    assert_eq!(selected_hashes, vec![two.as_str(), one.as_str()]);
    assert!(
        app.runtime()
            .world()
            .resource::<ResolvedOperationSet>()
            .commands
            .contains_key("cherry-pick")
    );

    app.dispatch_input(InputIntent::CommandLine("cherry-pick".into()));
    assert_eq!(
        git(&["log", "-2", "--reverse", "--pretty=%s"]),
        "feature one\nfeature two"
    );
    assert!(
        app.runtime()
            .world()
            .resource::<GitMutationSuccesses>()
            .0
            .iter()
            .any(|success| matches!(success.command, GitCommand::CherryPick { .. }))
    );
    let log = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalGitOperationLog)
        .unwrap();
    assert!(
        app.runtime()
            .world()
            .get::<DatasetChildren>(log)
            .unwrap()
            .0
            .iter()
            .filter_map(|entry| app
                .runtime()
                .world()
                .get::<GitOperationLogEntryMetadata>(*entry))
            .any(|entry| {
                entry.operation == "cherry_pick" && entry.status == GitOperationStatus::Success
            })
    );
}

#[test]
fn cherry_pick_conflict_is_aborted_noticed_and_logged_as_typed_data() {
    let repository = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Pitui Test"]);
    git(&["config", "user.email", "pitui@example.invalid"]);
    fs::write(repository.path().join("conflict.txt"), "base\n").unwrap();
    git(&["add", "conflict.txt"]);
    git(&["commit", "-m", "base"]);
    git(&["switch", "-c", "feature"]);
    fs::write(repository.path().join("conflict.txt"), "feature\n").unwrap();
    git(&["commit", "-am", "feature conflict"]);
    git(&["switch", "main"]);
    fs::write(repository.path().join("conflict.txt"), "main\n").unwrap();
    git(&["commit", "-am", "main conflict"]);
    let head_before = git(&["rev-parse", "HEAD"]);

    let mut app = App::open_from(repository.path(), Vec::new()).unwrap();
    let repository_entity = app.repositories()[0];
    let DatasetIdentity::Repository(repository_key) = app
        .runtime()
        .world()
        .get::<DatasetKey>(repository_entity)
        .unwrap()
        .0
        .clone()
    else {
        panic!("composition repository must have a Repository identity");
    };
    let (feature, feature_name) = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .by_key
        .iter()
        .find_map(|(identity, entity)| match identity {
            DatasetIdentity::Branch { repository, name }
                if repository == &repository_key && name.0 == "feature" =>
            {
                Some((*entity, name.clone()))
            }
            _ => None,
        })
        .unwrap();
    let feature_commits = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::Commits {
            repository: repository_key,
            branch: feature_name.clone(),
        })
        .unwrap();
    app.runtime_mut()
        .enqueue_git_command(GitCommandData {
            repository_dataset: repository_entity,
            cwd: repository.path().to_path_buf(),
            command: GitCommand::LoadCommits {
                branch: feature_name,
                limit: 50,
            },
        })
        .unwrap();
    app.runtime_mut().run_schedule();
    let root = app.root_dataset();
    app.runtime_mut().set_cursor(root, Some(feature)).unwrap();
    app.runtime_mut().run_schedule();
    app.dispatch_input(InputIntent::Key(KeyStroke::character('d')));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        feature_commits
    );
    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Space)));
    app.dispatch_input(InputIntent::CommandLine("cherry-pick".into()));

    assert_eq!(git(&["rev-parse", "HEAD"]), head_before);
    assert!(git(&["status", "--porcelain=v1"]).is_empty());
    assert!(!repository.path().join(".git/CHERRY_PICK_HEAD").exists());
    assert!(
        app.runtime()
            .world()
            .resource::<GitExecutionFailures>()
            .0
            .is_empty()
    );
    assert!(
        !app.runtime()
            .world()
            .resource::<GitMutationSuccesses>()
            .0
            .iter()
            .any(|success| matches!(success.command, GitCommand::CherryPick { .. }))
    );

    let interaction = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalInteractionContext)
        .unwrap();
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        interaction
    );
    let notice = app
        .runtime()
        .world()
        .get::<InteractionContextMetadata>(interaction)
        .unwrap();
    assert!(matches!(
        &notice.kind,
        InteractionContextKind::Notice { title, message }
            if title == "Git conflict aborted"
                && message.contains("restored the pre-operation state")
    ));

    let log = app
        .runtime()
        .world()
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalGitOperationLog)
        .unwrap();
    let conflict = app
        .runtime()
        .world()
        .get::<DatasetChildren>(log)
        .unwrap()
        .0
        .iter()
        .filter_map(|entry| {
            app.runtime()
                .world()
                .get::<GitOperationLogEntryMetadata>(*entry)
        })
        .find(|entry| entry.operation == "cherry_pick")
        .unwrap();
    assert_eq!(conflict.status, GitOperationStatus::ConflictAborted);
    assert!(conflict.abort_attempted);
    assert_eq!(
        conflict.abort_result.as_deref(),
        Some("git cherry-pick --abort completed")
    );

    app.dispatch_input(InputIntent::Key(KeyStroke::plain(KeyCode::Enter)));
    assert_eq!(
        app.runtime()
            .world()
            .resource::<ActiveUiContext>()
            .active_dataset,
        feature_commits
    );
}
