use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

use pitui_data::{
    AvailabilityRule, DatasetKind, DatasetTemplate, DatasetTemplateId, GlobalOperationSet,
    InteractionContextType, KeyCode, KeyModifiers, KeySequence, KeyStroke, OperationHotkeyBinding,
    OperationHotkeyTable, OperationId,
};

pub const HOTKEY_PROFILE_VERSION: i64 = 1;
pub const HOTKEY_CONFIG_ENV: &str = "PITUI_HOTKEY_CONFIG";

/// The only externally configurable behavior boundary. Operations and Git
/// commands remain compiled Rust; a profile can only replace key sequences of
/// already-declared Operations.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HotkeyProfile {
    pub global: BTreeMap<OperationId, Vec<KeySequence>>,
    pub datasets: BTreeMap<DatasetTemplateId, BTreeMap<OperationId, Vec<KeySequence>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HotkeyProfileError {
    Read {
        path: PathBuf,
        message: String,
    },
    Parse(String),
    MissingVersion,
    UnsupportedVersion(i64),
    UnknownTopLevel(String),
    ExpectedTable(String),
    ExpectedStringArray(String),
    InvalidKeySequence {
        location: String,
        message: String,
    },
    UnknownDatasetTemplate(DatasetTemplateId),
    OperationNotDeclared {
        scope: String,
        operation: OperationId,
    },
    DuplicateKeySequence {
        scope: String,
        sequence: KeySequence,
        first: OperationId,
        second: OperationId,
    },
    AmbiguousKeyPrefix {
        scope: String,
        shorter: KeySequence,
        longer: KeySequence,
    },
}

pub fn hotkey_profile_path_from_env() -> Option<PathBuf> {
    env::var_os(HOTKEY_CONFIG_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn load_hotkey_profile(path: &Path) -> Result<HotkeyProfile, HotkeyProfileError> {
    let source = fs::read_to_string(path).map_err(|error| HotkeyProfileError::Read {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    parse_hotkey_profile(&source)
}

/// Parses the deliberately small TOML schema:
///
/// ```text
/// version = 1
/// [global]
/// "global.help" = ["h"]
/// [datasets.commits]
/// "copy.commit.hash" = ["ctrl+c h"]
/// ```
///
/// An empty array disables an Operation's shortcut without removing the
/// Operation from its command palette.
pub fn parse_hotkey_profile(source: &str) -> Result<HotkeyProfile, HotkeyProfileError> {
    #[derive(Clone)]
    enum Section {
        Root,
        Global,
        Dataset(DatasetTemplateId),
    }

    let mut profile = HotkeyProfile::default();
    let mut version = None;
    let mut section = Section::Root;
    for (line_index, raw_line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let line = strip_comment(raw_line)
            .map_err(|message| parse_line_error(line_number, message))?
            .trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') || line.starts_with("[[") || line.ends_with("]]") {
                return Err(parse_line_error(line_number, "invalid table header"));
            }
            let name = line[1..line.len() - 1].trim();
            section = if name == "global" {
                Section::Global
            } else if let Some(template) = name.strip_prefix("datasets.") {
                if template.is_empty() || !valid_bare_name(template) {
                    return Err(parse_line_error(
                        line_number,
                        "Dataset template names must be bare identifiers",
                    ));
                }
                let template = DatasetTemplateId::from(template);
                profile.datasets.entry(template.clone()).or_default();
                Section::Dataset(template)
            } else {
                return Err(HotkeyProfileError::UnknownTopLevel(name.into()));
            };
            continue;
        }

        let (raw_key, raw_value) = split_assignment(line)
            .ok_or_else(|| parse_line_error(line_number, "expected key = value"))?;
        match &section {
            Section::Root => {
                if raw_key.trim() != "version" {
                    return Err(HotkeyProfileError::UnknownTopLevel(raw_key.trim().into()));
                }
                if version.is_some() {
                    return Err(parse_line_error(line_number, "duplicate version"));
                }
                version =
                    Some(raw_value.trim().parse::<i64>().map_err(|_| {
                        parse_line_error(line_number, "version must be an integer")
                    })?);
            }
            Section::Global => insert_operation_binding(
                &mut profile.global,
                raw_key,
                raw_value,
                "global",
                line_number,
            )?,
            Section::Dataset(template) => {
                let scope = format!("datasets.{}", template.0);
                insert_operation_binding(
                    profile
                        .datasets
                        .get_mut(template)
                        .expect("section inserted"),
                    raw_key,
                    raw_value,
                    &scope,
                    line_number,
                )?;
            }
        }
    }
    let version = version.ok_or(HotkeyProfileError::MissingVersion)?;
    if version != HOTKEY_PROFILE_VERSION {
        return Err(HotkeyProfileError::UnsupportedVersion(version));
    }
    Ok(profile)
}

fn parse_line_error(line: usize, message: impl Into<String>) -> HotkeyProfileError {
    HotkeyProfileError::Parse(format!("line {line}: {}", message.into()))
}

fn valid_bare_name(name: &str) -> bool {
    name.chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn strip_comment(line: &str) -> Result<&str, String> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return Ok(&line[..index]),
            _ => {}
        }
    }
    if quoted {
        Err("unterminated string".into())
    } else {
        Ok(line)
    }
}

fn split_assignment(line: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '=' if !quoted => return Some((&line[..index], &line[index + 1..])),
            _ => {}
        }
    }
    None
}

fn insert_operation_binding(
    table: &mut BTreeMap<OperationId, Vec<KeySequence>>,
    raw_key: &str,
    raw_value: &str,
    scope: &str,
    line_number: usize,
) -> Result<(), HotkeyProfileError> {
    let operation = parse_string_or_bare(raw_key.trim())
        .map_err(|message| parse_line_error(line_number, message))?;
    if operation.is_empty() {
        return Err(parse_line_error(
            line_number,
            "Operation ID cannot be empty",
        ));
    }
    let location = format!("{scope}.{operation}");
    let bindings = parse_string_array(raw_value.trim())
        .map_err(|_| HotkeyProfileError::ExpectedStringArray(location.clone()))?
        .into_iter()
        .map(|binding| {
            parse_key_sequence(&binding).map_err(|message| HotkeyProfileError::InvalidKeySequence {
                location: location.clone(),
                message,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let operation = OperationId::from(operation);
    if table.insert(operation.clone(), bindings).is_some() {
        return Err(parse_line_error(
            line_number,
            format!("duplicate Operation {}", operation.0),
        ));
    }
    Ok(())
}

fn parse_string_or_bare(source: &str) -> Result<String, String> {
    if source.starts_with('"') {
        let (value, consumed) = parse_quoted_string(source)?;
        if !source[consumed..].trim().is_empty() {
            return Err("unexpected text after quoted key".into());
        }
        Ok(value)
    } else if !source.is_empty()
        && source.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        Ok(source.into())
    } else {
        Err("invalid key".into())
    }
}

fn parse_string_array(source: &str) -> Result<Vec<String>, String> {
    if !source.starts_with('[') || !source.ends_with(']') {
        return Err("expected an array".into());
    }
    let inner = &source[1..source.len() - 1];
    let mut values = Vec::new();
    let mut cursor = 0;
    loop {
        cursor += leading_whitespace_bytes(&inner[cursor..]);
        if cursor == inner.len() {
            return Ok(values);
        }
        let (value, consumed) = parse_quoted_string(&inner[cursor..])?;
        values.push(value);
        cursor += consumed;
        cursor += leading_whitespace_bytes(&inner[cursor..]);
        if cursor == inner.len() {
            return Ok(values);
        }
        if !inner[cursor..].starts_with(',') {
            return Err("array entries must be comma-separated".into());
        }
        cursor += 1;
        if inner[cursor..].trim().is_empty() {
            return Ok(values);
        }
    }
}

fn leading_whitespace_bytes(source: &str) -> usize {
    source
        .chars()
        .take_while(|character| character.is_whitespace())
        .map(char::len_utf8)
        .sum()
}

fn parse_quoted_string(source: &str) -> Result<(String, usize), String> {
    if !source.starts_with('"') {
        return Err("expected a quoted string".into());
    }
    let mut value = String::new();
    let mut escaped = false;
    for (offset, character) in source[1..].char_indices() {
        if escaped {
            value.push(match character {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                _ => return Err(format!("unsupported escape \\{character}")),
            });
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => return Ok((value, offset + 2)),
            _ => value.push(character),
        }
    }
    Err("unterminated quoted string".into())
}

pub fn parse_key_sequence(source: &str) -> Result<KeySequence, String> {
    let strokes = source
        .split_whitespace()
        .map(parse_key_stroke)
        .collect::<Result<Vec<_>, _>>()?;
    if strokes.is_empty() {
        return Err("a key sequence must contain at least one stroke".into());
    }
    Ok(KeySequence(strokes))
}

fn parse_key_stroke(source: &str) -> Result<KeyStroke, String> {
    if source.is_empty() {
        return Err("key stroke cannot be empty".into());
    }
    let parts = source.split('+').collect::<Vec<_>>();
    let (key, modifiers) = parts
        .split_last()
        .ok_or_else(|| "key stroke cannot be empty".to_owned())?;
    if key.is_empty() {
        return Err(format!("{source:?} has no key after its modifiers"));
    }
    let mut parsed = KeyModifiers::default();
    for modifier in modifiers {
        let slot = match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => &mut parsed.control,
            "alt" => &mut parsed.alt,
            "shift" => &mut parsed.shift,
            "super" | "cmd" => &mut parsed.super_key,
            _ => return Err(format!("unknown modifier {modifier:?}")),
        };
        if *slot {
            return Err(format!("duplicate modifier {modifier:?}"));
        }
        *slot = true;
    }
    let normalized = key.to_ascii_lowercase();
    let code = match normalized.as_str() {
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "page-up" => KeyCode::PageUp,
        "pagedown" | "page-down" => KeyCode::PageDown,
        "enter" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "space" => KeyCode::Space,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        _ => {
            let mut characters = normalized.chars();
            let Some(character) = characters.next() else {
                return Err("key cannot be empty".into());
            };
            if characters.next().is_some() {
                return Err(format!("unknown key {key:?}"));
            }
            KeyCode::Character(character)
        }
    };
    Ok(KeyStroke {
        code,
        modifiers: parsed,
    })
}

pub fn apply_hotkey_profile(
    templates: &mut [DatasetTemplate],
    global: &mut GlobalOperationSet,
    profile: &HotkeyProfile,
) -> Result<(), HotkeyProfileError> {
    let mut next_templates = templates.to_vec();
    let mut next_global = global.clone();
    apply_hotkey_profile_in_place(&mut next_templates, &mut next_global, profile)?;
    validate_effective_hotkeys(&next_templates, &next_global)?;
    templates.clone_from_slice(&next_templates);
    *global = next_global;
    Ok(())
}

fn apply_hotkey_profile_in_place(
    templates: &mut [DatasetTemplate],
    global: &mut GlobalOperationSet,
    profile: &HotkeyProfile,
) -> Result<(), HotkeyProfileError> {
    validate_overrides("global", &global.operations, &profile.global)?;
    validate_hotkey_overrides("global", &profile.global)?;
    apply_overrides(&mut global.hotkeys, &global.operations, &profile.global);

    for (template_id, overrides) in &profile.datasets {
        let Some(template) = templates
            .iter_mut()
            .find(|template| &template.id == template_id)
        else {
            return Err(HotkeyProfileError::UnknownDatasetTemplate(
                template_id.clone(),
            ));
        };
        validate_overrides(&template_id.0, &template.operations, overrides)?;
        validate_hotkey_overrides(&template_id.0, overrides)?;
        apply_overrides(&mut template.hotkeys, &template.operations, overrides);
    }
    Ok(())
}

fn validate_overrides(
    scope: &str,
    declared: &[OperationId],
    overrides: &BTreeMap<OperationId, Vec<KeySequence>>,
) -> Result<(), HotkeyProfileError> {
    for operation in overrides.keys() {
        if !declared.contains(operation) {
            return Err(HotkeyProfileError::OperationNotDeclared {
                scope: scope.into(),
                operation: operation.clone(),
            });
        }
    }
    Ok(())
}

fn apply_overrides(
    table: &mut OperationHotkeyTable,
    declared: &[OperationId],
    overrides: &BTreeMap<OperationId, Vec<KeySequence>>,
) {
    table
        .0
        .retain(|entry| !overrides.contains_key(&entry.operation));
    for operation in declared {
        let Some(bindings) = overrides.get(operation) else {
            continue;
        };
        if !bindings.is_empty() {
            table.0.push(OperationHotkeyBinding {
                operation: operation.clone(),
                bindings: bindings.clone(),
            });
        }
    }
    table.0.sort_by_key(|entry| {
        declared
            .iter()
            .position(|operation| operation == &entry.operation)
            .unwrap_or(usize::MAX)
    });
}

fn validate_hotkey_overrides(
    scope: &str,
    overrides: &BTreeMap<OperationId, Vec<KeySequence>>,
) -> Result<(), HotkeyProfileError> {
    let mut sequences = Vec::<(KeySequence, OperationId)>::new();
    for (operation, bindings) in overrides {
        let mut own = HashSet::new();
        for sequence in bindings {
            if !own.insert(sequence.clone()) {
                return Err(HotkeyProfileError::DuplicateKeySequence {
                    scope: scope.into(),
                    sequence: sequence.clone(),
                    first: operation.clone(),
                    second: operation.clone(),
                });
            }
            if let Some((_, first)) = sequences.iter().find(|(existing, _)| existing == sequence) {
                return Err(HotkeyProfileError::DuplicateKeySequence {
                    scope: scope.into(),
                    sequence: sequence.clone(),
                    first: first.clone(),
                    second: operation.clone(),
                });
            }
            sequences.push((sequence.clone(), operation.clone()));
        }
    }
    for (index, (left, _)) in sequences.iter().enumerate() {
        for (right, _) in sequences.iter().skip(index + 1) {
            let (shorter, longer) = if left.0.len() <= right.0.len() {
                (left, right)
            } else {
                (right, left)
            };
            if shorter.0.len() < longer.0.len() && longer.0.starts_with(&shorter.0) {
                return Err(HotkeyProfileError::AmbiguousKeyPrefix {
                    scope: scope.into(),
                    shorter: shorter.clone(),
                    longer: longer.clone(),
                });
            }
        }
    }
    Ok(())
}

fn validate_effective_hotkeys(
    templates: &[DatasetTemplate],
    global: &GlobalOperationSet,
) -> Result<(), HotkeyProfileError> {
    let operations = crate::builtin_operation_specs()
        .into_iter()
        .map(|operation| (operation.id.clone(), operation))
        .collect::<BTreeMap<_, _>>();
    let rules = crate::builtin_availability_rules()
        .into_iter()
        .collect::<BTreeMap<_, _>>();

    for template in templates {
        let context_types = if template.kind == DatasetKind::InteractionContext {
            vec![
                Some(InteractionContextType::Help),
                Some(InteractionContextType::CommandPalette),
                Some(InteractionContextType::Confirmation),
                Some(InteractionContextType::TextInput),
                Some(InteractionContextType::Notice),
            ]
        } else {
            vec![None]
        };
        for context_type in context_types {
            let scope = format!("{}:{context_type:?}", template.id.0);
            let mut seen_operations = HashSet::new();
            let mut sequences = Vec::<(KeySequence, OperationId)>::new();
            for (operation, bindings) in global
                .operations
                .iter()
                .map(|operation| (operation, global.hotkeys.bindings_for(operation)))
                .chain(
                    template
                        .operations
                        .iter()
                        .map(|operation| (operation, template.hotkeys.bindings_for(operation))),
                )
            {
                if !seen_operations.insert(operation.clone()) {
                    continue;
                }
                let Some(spec) = operations.get(operation) else {
                    continue;
                };
                let Some(rule) = rules.get(&spec.availability) else {
                    continue;
                };
                if !availability_can_match(rule, template.kind, context_type) {
                    continue;
                }
                for sequence in bindings {
                    if let Some((_, first)) =
                        sequences.iter().find(|(existing, _)| existing == sequence)
                    {
                        return Err(HotkeyProfileError::DuplicateKeySequence {
                            scope,
                            sequence: sequence.clone(),
                            first: first.clone(),
                            second: operation.clone(),
                        });
                    }
                    sequences.push((sequence.clone(), operation.clone()));
                }
            }
            for (index, (left, _)) in sequences.iter().enumerate() {
                for (right, _) in sequences.iter().skip(index + 1) {
                    let (shorter, longer) = if left.0.len() <= right.0.len() {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    if shorter.0.len() < longer.0.len() && longer.0.starts_with(&shorter.0) {
                        return Err(HotkeyProfileError::AmbiguousKeyPrefix {
                            scope,
                            shorter: shorter.clone(),
                            longer: longer.clone(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn availability_can_match(
    rule: &AvailabilityRule,
    kind: DatasetKind,
    context_type: Option<InteractionContextType>,
) -> bool {
    match rule {
        AvailabilityRule::Always
        | AvailabilityRule::HasActiveElement
        | AvailabilityRule::HasSelection
        | AvailabilityRule::HasSelectionOrActiveElement
        | AvailabilityRule::ContextHasActiveElement(_)
        | AvailabilityRule::ContextHasSelectionOrActiveElement(_)
        | AvailabilityRule::ContextActiveElementKind(_, _)
        | AvailabilityRule::ContextTargetsBoundary(_, _)
        | AvailabilityRule::ChangesHasStagedFiles(_) => true,
        AvailabilityRule::ActiveDatasetKind(expected) => *expected == kind,
        AvailabilityRule::InteractionContextType(expected) => context_type == Some(*expected),
        AvailabilityRule::All(rules) => rules
            .iter()
            .all(|rule| availability_can_match(rule, kind, context_type)),
        AvailabilityRule::Any(rules) => rules
            .iter()
            .any(|rule| availability_can_match(rule, kind, context_type)),
        AvailabilityRule::Not(rule) => !availability_can_match(rule, kind, context_type),
    }
}
