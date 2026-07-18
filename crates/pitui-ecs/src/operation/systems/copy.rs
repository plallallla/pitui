use super::*;

pub fn copy_commit_hashes(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let hashes = invocation
        .targets
        .iter()
        .filter_map(|entity| match keys.get(*entity).ok().map(|key| &key.0) {
            Some(DatasetIdentity::Commit { hash, .. }) => Some(hash.0.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if hashes.len() != invocation.targets.len() || hashes.is_empty() {
        return OperationExecution::Rejected("copy hash target is not a Commit Dataset".into());
    }
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitHashes,
        text: hashes.join("\n"),
        source_entities: invocation.targets,
    });
    OperationExecution::Completed
}

pub fn copy_reflog_hash(
    In(invocation): In<OperationInvocation>,
    entries: Query<&ReflogEntryMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no Reflog entry target".into());
    };
    let Ok(metadata) = entries.get(target) else {
        return OperationExecution::Rejected(
            "copy hash target is not a Reflog entry Dataset".into(),
        );
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::ReflogHash,
        text: metadata.0.hash.0.clone(),
        source_entities: vec![target],
    });
    OperationExecution::Completed
}

pub fn copy_commit_info(
    In(invocation): In<OperationInvocation>,
    commits: Query<&CommitMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no Commit target".into());
    };
    let Ok(metadata) = commits.get(target) else {
        return OperationExecution::Rejected("copy info target has no Commit metadata".into());
    };
    let mut refs = metadata.summary.decorations.clone();
    if refs.is_empty() && !metadata.tags.is_empty() {
        refs = metadata.tags.join(", ");
    }
    let refs = if refs.is_empty() {
        String::new()
    } else {
        format!("\nRefs: {refs}")
    };
    let message = metadata
        .message
        .as_deref()
        .unwrap_or(&metadata.summary.subject);
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitInfo,
        text: format!(
            "commit {}\nAuthor: {}\nDate:   {}{}\n\n{}",
            metadata.summary.hash.0,
            metadata.summary.author,
            metadata.summary.authored_at,
            refs,
            message
        ),
        source_entities: vec![target],
    });
    OperationExecution::Completed
}

pub fn copy_commit_message(
    In(invocation): In<OperationInvocation>,
    commits: Query<&CommitMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no Commit target".into());
    };
    let Some(message) = commits
        .get(target)
        .ok()
        .and_then(|metadata| metadata.message.clone())
    else {
        return OperationExecution::Rejected("full commit message is not loaded".into());
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitMessage,
        text: message,
        source_entities: vec![target],
    });
    OperationExecution::Completed
}

pub fn copy_commit_field_values(
    In(invocation): In<OperationInvocation>,
    fields: Query<&CommitFieldMetadata>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let values = invocation
        .targets
        .iter()
        .map(|target| fields.get(*target).cloned())
        .collect::<Result<Vec<_>, _>>();
    let Ok(values) = values else {
        return OperationExecution::Rejected(
            "copy value target is not a Commit field Dataset".into(),
        );
    };
    if values.is_empty() {
        return OperationExecution::Rejected("no Commit field target".into());
    }
    let text = if values.len() == 1 {
        values[0].value.clone()
    } else {
        values
            .iter()
            .map(|metadata| format!("{}: {}", metadata.field.label(), metadata.value))
            .collect::<Vec<_>>()
            .join("\n")
    };
    clipboard.write(ClipboardRequest {
        kind: ClipboardContentKind::CommitFieldValues,
        text,
        source_entities: invocation.targets,
    });
    OperationExecution::Completed
}

pub fn copy_file_name(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    copy_file_path(
        invocation,
        &keys,
        ClipboardContentKind::FileName,
        |_, path| {
            PathBuf::from(path.to_os_string())
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        },
        &mut clipboard,
    )
}

pub fn copy_file_absolute_path(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    copy_file_path(
        invocation,
        &keys,
        ClipboardContentKind::FileAbsolutePath,
        |repository, path| {
            Some(
                repository
                    .as_path()
                    .join(PathBuf::from(path.to_os_string()))
                    .to_string_lossy()
                    .into_owned(),
            )
        },
        &mut clipboard,
    )
}

pub fn copy_file_relative_path(
    In(invocation): In<OperationInvocation>,
    keys: Query<&DatasetKey>,
    mut clipboard: MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    copy_file_path(
        invocation,
        &keys,
        ClipboardContentKind::FileRelativePath,
        |_, path| Some(path.as_str().into()),
        &mut clipboard,
    )
}

fn copy_file_path(
    invocation: OperationInvocation,
    keys: &Query<&DatasetKey>,
    kind: ClipboardContentKind,
    value: impl FnOnce(&pitui_data::RepositoryKey, &pitui_core::GitPath) -> Option<String>,
    clipboard: &mut MessageWriter<ClipboardRequest>,
) -> OperationExecution {
    let Some(target) = invocation.targets.first().copied() else {
        return OperationExecution::Rejected("no File target".into());
    };
    let Ok(key) = keys.get(target) else {
        return OperationExecution::Rejected("File target no longer exists".into());
    };
    let (repository, path) = match &key.0 {
        DatasetIdentity::FileDirectory {
            repository, path, ..
        }
        | DatasetIdentity::File {
            repository, path, ..
        }
        | DatasetIdentity::WorkingTreeDirectory {
            repository, path, ..
        }
        | DatasetIdentity::WorkingTreeFile {
            repository, path, ..
        } => (repository, path),
        _ => {
            return OperationExecution::Rejected(
                "copy path target is not a file or directory Dataset".into(),
            );
        }
    };
    let Some(text) = value(repository, path) else {
        return OperationExecution::Rejected("File path has no copyable name".into());
    };
    clipboard.write(ClipboardRequest {
        kind,
        text,
        source_entities: vec![target],
    });
    OperationExecution::Completed
}
