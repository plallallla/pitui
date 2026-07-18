use super::*;

pub(super) fn payload_summary(payload: &ParsedGitPayload) -> String {
    match payload {
        ParsedGitPayload::Repository(repository) => format!(
            "branch={} staged={} modified={} untracked={} conflicted={}",
            repository
                .current_branch
                .as_ref()
                .map_or("detached", |branch| branch.0.as_str()),
            repository.status.staged,
            repository.status.modified,
            repository.status.untracked,
            repository.status.conflicted
        ),
        ParsedGitPayload::Branches(branches) => format!("branches={}", branches.len()),
        ParsedGitPayload::Commits { branch, commits } => {
            format!("branch={} commits={}", branch.0, commits.len())
        }
        ParsedGitPayload::CommitDetail(detail) => format!(
            "commit={} files={}",
            detail.commit.hash.short(),
            detail.files.len()
        ),
        ParsedGitPayload::FileDiff(diff) => format!(
            "path={} hunks={} binary={}",
            diff.path,
            diff.hunks.len(),
            diff.is_binary
        ),
        ParsedGitPayload::Reflog(entries) => format!("entries={}", entries.len()),
        ParsedGitPayload::WorkingTree(changes) => format!("changes={}", changes.len()),
        ParsedGitPayload::WorkingTreeDiff(diff) => {
            format!("path={} sections={}", diff.path, diff.sections.len())
        }
        ParsedGitPayload::CommandSucceeded { message } => message.clone(),
        ParsedGitPayload::ConflictAborted { message, .. } => message.clone(),
    }
}

pub(super) struct GitOperationOutcome {
    pub(super) status: GitOperationStatus,
    pub(super) message: String,
    pub(super) abort_attempted: bool,
    pub(super) abort_result: Option<String>,
}

pub(super) fn record_git_operation(
    world: &mut World,
    data: &GitCommandData,
    started_at: SystemTime,
    duration: Duration,
    outcome: GitOperationOutcome,
) {
    let GitOperationOutcome {
        status,
        message,
        abort_attempted,
        abort_result,
    } = outcome;
    let repository = match world.get::<DatasetKey>(data.repository_dataset) {
        Some(DatasetKey(DatasetIdentity::Repository(repository))) => repository.clone(),
        _ => RepositoryKey::new(data.cwd.clone()),
    };
    world
        .resource::<GitOperationLogSinkResource>()
        .0
        .record(&GitOperationRecord {
            operation: data.command.operation_name().into(),
            repository: repository.as_path().to_path_buf(),
            started_at,
            duration,
            status: match status {
                GitOperationStatus::Success => GitLogStatus::Success,
                GitOperationStatus::Failure => GitLogStatus::Failure,
                GitOperationStatus::ConflictAborted => GitLogStatus::ConflictAborted,
            },
            message: message.clone(),
            abort_attempted,
            abort_result: abort_result.clone(),
        });

    let Some(log) = world
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalGitOperationLog)
    else {
        return;
    };
    let Some(template) = world
        .resource::<DefaultDatasetTemplates>()
        .get(DatasetKind::GitOperationLogEntry)
        .cloned()
    else {
        return;
    };
    let sequence = {
        let mut next = world.resource_mut::<NextGitOperationLogSequence>();
        let sequence = next.0;
        next.0 = next.0.wrapping_add(1);
        sequence
    };
    let identity = DatasetIdentity::GitOperationLogEntry(sequence);
    let Ok(entry) =
        ensure_dataset_in_world(world, identity, DatasetKind::GitOperationLogEntry, template)
    else {
        return;
    };
    world
        .entity_mut(entry)
        .insert(GitOperationLogEntryMetadata {
            sequence,
            operation: data.command.operation_name().into(),
            repository,
            started_at_utc: format_system_time_utc(started_at),
            duration_ms: duration.as_millis(),
            status,
            message: pitui_git::sanitize_log_text(&message, 4096),
            abort_attempted,
            abort_result: abort_result.map(|result| pitui_git::sanitize_log_text(&result, 4096)),
        });
    let mut children = world
        .get::<DatasetChildren>(log)
        .map(|children| children.0.clone())
        .unwrap_or_default();
    children.insert(0, entry);
    let session_limit = world.resource::<GitRuntimeRetention>().session_log_entries;
    children.truncate(session_limit);
    if replace_children_in_world(world, log, children, true).is_ok() {
        let log_is_active = world
            .get_resource::<ActiveUiContext>()
            .is_some_and(|context| context.active_dataset == log);
        if let Some(mut active) = world.get_mut::<pitui_data::DatasetActiveElement>(log)
            && (!log_is_active || active.0.is_none())
        {
            active.0 = Some(entry);
        }
    }
}

pub(super) fn format_system_time_utc(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let millis = duration.subsec_millis();
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = second_of_day % 3_600 / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

pub(super) fn request_interaction_notice(world: &mut World, title: &str, message: String) {
    world
        .resource_mut::<Messages<InteractionNoticeRequest>>()
        .write(InteractionNoticeRequest {
            title: title.into(),
            message,
        });
}
