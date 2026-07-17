use std::{
    collections::{BTreeMap, HashMap},
    env,
    error::Error,
    ffi::OsString,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use crate::app::{
    Action, AppState, CommandId, CommandMount, DiffViewMode, FooterGroup, ShortcutContext,
};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
const DEFAULT_LOG_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyStroke {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyStroke {
    pub fn from_event(event: KeyEvent) -> Self {
        let mut modifiers = event.modifiers;
        let mut code = event.code;
        if let KeyCode::Char(character) = code {
            modifiers.remove(KeyModifiers::SHIFT);
            code = KeyCode::Char(if modifiers.contains(KeyModifiers::CONTROL) {
                character.to_ascii_lowercase()
            } else {
                character
            });
        }
        if matches!(code, KeyCode::BackTab) {
            modifiers.remove(KeyModifiers::SHIFT);
        }
        Self { code, modifiers }
    }

    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ConfigError::new("key stroke cannot be empty"));
        }

        let mut modifiers = KeyModifiers::NONE;
        let parts = value.split('+').map(str::trim).collect::<Vec<_>>();
        let Some(key_name) = parts.last().copied() else {
            return Err(ConfigError::new(format!("invalid key stroke `{value}`")));
        };
        for modifier in &parts[..parts.len().saturating_sub(1)] {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.insert(KeyModifiers::CONTROL),
                "alt" => modifiers.insert(KeyModifiers::ALT),
                "shift" => modifiers.insert(KeyModifiers::SHIFT),
                _ => {
                    return Err(ConfigError::new(format!(
                        "unknown modifier `{modifier}` in `{value}`"
                    )));
                }
            }
        }

        let lower = key_name.to_ascii_lowercase();
        let code = match lower.as_str() {
            "enter" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdown" => KeyCode::PageDown,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            _ if key_name.chars().count() == 1 => {
                KeyCode::Char(key_name.chars().next().expect("one character"))
            }
            _ => {
                return Err(ConfigError::new(format!(
                    "unknown key `{key_name}` in `{value}`"
                )));
            }
        };

        if matches!(code, KeyCode::BackTab)
            || (matches!(code, KeyCode::Tab) && modifiers.contains(KeyModifiers::SHIFT))
        {
            return Ok(Self {
                code: KeyCode::BackTab,
                modifiers: KeyModifiers::NONE,
            });
        }
        if let KeyCode::Char(character) = code {
            modifiers.remove(KeyModifiers::SHIFT);
            return Ok(Self {
                code: KeyCode::Char(if modifiers.contains(KeyModifiers::CONTROL) {
                    character.to_ascii_lowercase()
                } else {
                    character
                }),
                modifiers,
            });
        }
        Ok(Self { code, modifiers })
    }

    pub fn display(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("Alt".to_string());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("Shift".to_string());
        }
        parts.push(match self.code {
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(character) => {
                if self.modifiers.contains(KeyModifiers::CONTROL) {
                    character.to_ascii_uppercase().to_string()
                } else {
                    character.to_string()
                }
            }
            KeyCode::Tab => "Tab".into(),
            KeyCode::BackTab => "BackTab".into(),
            KeyCode::Up => "Up".into(),
            KeyCode::Down => "Down".into(),
            KeyCode::Left => "Left".into(),
            KeyCode::Right => "Right".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PageUp".into(),
            KeyCode::PageDown => "PageDown".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Delete".into(),
            other => format!("{other:?}"),
        });
        parts.join("+")
    }
}

pub type KeySequence = Vec<KeyStroke>;

fn parse_sequence(value: &str) -> Result<KeySequence, ConfigError> {
    let sequence = value
        .split_ascii_whitespace()
        .map(KeyStroke::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if sequence.is_empty() {
        return Err(ConfigError::new("key sequence cannot be empty"));
    }
    if sequence.len() > 3 {
        return Err(ConfigError::new(format!(
            "key sequence `{value}` has more than three strokes"
        )));
    }
    Ok(sequence)
}

fn display_sequence(sequence: &[KeyStroke]) -> String {
    sequence
        .iter()
        .copied()
        .map(KeyStroke::display)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Debug)]
pub struct ResolvedKeymap {
    bindings: HashMap<CommandId, Vec<KeySequence>>,
    pub chord_timeout: Option<Duration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyResolution {
    Action(Action),
    Chord(Vec<KeyStroke>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FooterItem {
    pub key: String,
    pub label: String,
    pub group: FooterGroup,
    pub priority: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutHelpItem {
    pub key: String,
    pub label: String,
    /// Stable operation id used by the design document and configuration.
    pub operation: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShortcutHelpSection {
    pub title: String,
    pub context: Option<ShortcutContext>,
    pub items: Vec<ShortcutHelpItem>,
}

#[derive(Clone, Debug)]
struct Transition {
    stroke: KeyStroke,
    leaves: Vec<CommandId>,
    descendants: Vec<CommandId>,
}

impl ResolvedKeymap {
    pub fn default_keymap() -> Self {
        let bindings = CommandId::ALL
            .iter()
            .copied()
            .map(|command| {
                let sequences = command
                    .default_bindings()
                    .iter()
                    .map(|binding| {
                        parse_sequence(binding)
                            .unwrap_or_else(|error| panic!("invalid default binding: {error}"))
                    })
                    .collect();
                (command, sequences)
            })
            .collect();
        Self {
            bindings,
            chord_timeout: None,
        }
    }

    pub fn bindings_for(&self, command: CommandId) -> &[KeySequence] {
        self.bindings.get(&command).map_or(&[], Vec::as_slice)
    }

    pub fn resolve(
        &self,
        app: &AppState,
        prefix: &[KeyStroke],
        stroke: KeyStroke,
    ) -> Option<KeyResolution> {
        let transition = self
            .transitions(app, prefix, false)
            .into_iter()
            .find(|transition| transition.stroke == stroke)?;
        let mut next_prefix = prefix.to_vec();
        next_prefix.push(stroke);
        if !transition.descendants.is_empty() {
            return Some(KeyResolution::Chord(next_prefix));
        }
        transition
            .leaves
            .first()
            .and_then(|command| command.action(app))
            .map(KeyResolution::Action)
    }

    pub fn footer_items(&self, app: &AppState, footer: &ResolvedFooterConfig) -> Vec<FooterItem> {
        if footer.mode == FooterMode::Hidden {
            return Vec::new();
        }
        let prefix = match &app.mode {
            crate::app::GlobalMode::Chord { prefix, .. } => prefix.as_slice(),
            crate::app::GlobalMode::Normal => &[],
            _ => return Vec::new(),
        };
        let mut items = self
            .transitions(
                app,
                prefix,
                prefix.is_empty() && !footer.show_alternative_bindings,
            )
            .into_iter()
            .filter_map(|transition| {
                if !transition.descendants.is_empty() {
                    let footer_group = transition
                        .descendants
                        .first()
                        .map(|command| command.footer_group())
                        .filter(|group| {
                            transition
                                .descendants
                                .iter()
                                .all(|command| command.footer_group() == *group)
                        })
                        .unwrap_or(FooterGroup::Primary);
                    let semantic_group = transition
                        .descendants
                        .first()
                        .and_then(|command| command.chord_group())
                        .filter(|group| {
                            transition
                                .descendants
                                .iter()
                                .all(|command| command.chord_group() == Some(*group))
                        });
                    let presentation = if let Some(group_id) = semantic_group {
                        footer.group_presentation(group_id)
                    } else if transition.descendants.len() == 1 {
                        let mut presentation =
                            footer.command_presentation(transition.descendants[0]);
                        presentation.label.push('…');
                        presentation
                    } else {
                        footer.group_presentation("more")
                    };
                    if !presentation.visible {
                        return None;
                    }
                    return Some(FooterItem {
                        key: transition.stroke.display(),
                        label: presentation.label,
                        group: footer_group,
                        priority: presentation.priority,
                    });
                }

                let command = *transition.leaves.first()?;
                let presentation = footer.command_presentation(command);
                presentation.visible.then_some(FooterItem {
                    key: transition.stroke.display(),
                    label: presentation.label,
                    group: command.footer_group(),
                    priority: presentation.priority,
                })
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            left.group
                .rank()
                .cmp(&right.group.rank())
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| left.key.cmp(&right.key))
        });
        items
    }

    fn transitions(
        &self,
        app: &AppState,
        prefix: &[KeyStroke],
        primary_only: bool,
    ) -> Vec<Transition> {
        let mut transitions = HashMap::<KeyStroke, Transition>::new();
        for command in CommandId::ALL.iter().copied() {
            if command.action(app).is_none() {
                continue;
            }
            let Some(sequences) = self.bindings.get(&command) else {
                continue;
            };
            for (index, sequence) in sequences.iter().enumerate() {
                if primary_only && index > 0 {
                    continue;
                }
                if sequence.len() <= prefix.len() || !sequence.starts_with(prefix) {
                    continue;
                }
                let stroke = sequence[prefix.len()];
                let transition = transitions.entry(stroke).or_insert_with(|| Transition {
                    stroke,
                    leaves: Vec::new(),
                    descendants: Vec::new(),
                });
                if sequence.len() == prefix.len() + 1 {
                    if !transition.leaves.contains(&command) {
                        transition.leaves.push(command);
                    }
                } else if !transition.descendants.contains(&command) {
                    transition.descendants.push(command);
                }
            }
        }
        let mut transitions = transitions.into_values().collect::<Vec<_>>();
        transitions.sort_by_key(|transition| transition.stroke.display());
        transitions
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FooterMode {
    Contextual,
    Compact,
    Hidden,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FooterVisibilityMode {
    Registry,
    All,
    Allowlist,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FooterOverflow {
    Count,
    Ellipsis,
}

#[derive(Clone, Debug)]
pub struct FooterPresentation {
    pub visible: bool,
    pub label: String,
    pub priority: u16,
}

#[derive(Clone, Debug)]
pub struct ResolvedFooterConfig {
    pub mode: FooterMode,
    pub max_rows: u16,
    pub show_global: bool,
    pub show_alternative_bindings: bool,
    pub default_visibility: FooterVisibilityMode,
    pub separator: String,
    pub overflow: FooterOverflow,
    commands: HashMap<CommandId, FooterPresentationOverride>,
    groups: HashMap<String, FooterPresentationOverride>,
}

#[derive(Clone, Debug, Default)]
struct FooterPresentationOverride {
    visible: Option<bool>,
    label: Option<String>,
    priority: Option<u16>,
}

impl Default for ResolvedFooterConfig {
    fn default() -> Self {
        Self {
            mode: FooterMode::Contextual,
            max_rows: 1,
            show_global: true,
            show_alternative_bindings: false,
            default_visibility: FooterVisibilityMode::Registry,
            separator: " | ".into(),
            overflow: FooterOverflow::Count,
            commands: HashMap::new(),
            groups: HashMap::new(),
        }
    }
}

impl ResolvedFooterConfig {
    fn command_presentation(&self, command: CommandId) -> FooterPresentation {
        let defaults_visible = match self.default_visibility {
            FooterVisibilityMode::Registry => command.default_visible(),
            FooterVisibilityMode::All => true,
            FooterVisibilityMode::Allowlist => false,
        } && (self.show_global
            || command.footer_group() != FooterGroup::Global)
            && (self.mode != FooterMode::Compact
                || matches!(
                    command.footer_group(),
                    FooterGroup::Safety | FooterGroup::Global | FooterGroup::Primary
                ));
        let custom = self.commands.get(&command);
        FooterPresentation {
            visible: custom
                .and_then(|presentation| presentation.visible)
                .unwrap_or(defaults_visible),
            label: custom
                .and_then(|presentation| presentation.label.clone())
                .unwrap_or_else(|| command.default_label().into()),
            priority: custom
                .and_then(|presentation| presentation.priority)
                .unwrap_or_else(|| command.default_priority()),
        }
    }

    fn group_presentation(&self, group: &str) -> FooterPresentation {
        let default_visible = self.default_visibility != FooterVisibilityMode::Allowlist;
        let custom = self.groups.get(group);
        FooterPresentation {
            visible: custom
                .and_then(|presentation| presentation.visible)
                .unwrap_or(default_visible),
            label: custom
                .and_then(|presentation| presentation.label.clone())
                .unwrap_or_else(|| match group {
                    "commit.copy" => "copy…".into(),
                    "file.copy" => "copy file…".into(),
                    _ => "more…".into(),
                }),
            priority: custom
                .and_then(|presentation| presentation.priority)
                .unwrap_or(100),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

impl FromStr for LogLevel {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" | "warning" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err(ConfigError::new(format!(
                "invalid log level `{value}`; expected error, warn, info, debug, or trace"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogRotationConfig {
    pub enabled: bool,
    pub max_bytes: u64,
    pub keep_files: usize,
    pub rotate_on_start: bool,
}

#[derive(Clone, Debug)]
pub struct ResolvedLoggingConfig {
    pub enabled: bool,
    pub level: LogLevel,
    pub path: PathBuf,
    pub flush_interval: Duration,
    pub buffer_capacity: usize,
    pub max_detail_chars: usize,
    pub fail_on_open_error: bool,
    pub rotation: LogRotationConfig,
    pub targets: HashMap<String, LogLevel>,
}

impl ResolvedLoggingConfig {
    pub fn target_level(&self, target: &str) -> LogLevel {
        self.targets.get(target).copied().unwrap_or(self.level)
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedDiffConfig {
    pub default_mode: DiffViewMode,
}

#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    pub source_path: Option<PathBuf>,
    pub keymap: ResolvedKeymap,
    pub footer: ResolvedFooterConfig,
    pub diff: ResolvedDiffConfig,
    pub logging: ResolvedLoggingConfig,
}

impl Default for ResolvedConfig {
    fn default() -> Self {
        Self {
            source_path: None,
            keymap: ResolvedKeymap::default_keymap(),
            footer: ResolvedFooterConfig::default(),
            diff: ResolvedDiffConfig {
                default_mode: DiffViewMode::Unified,
            },
            logging: ResolvedLoggingConfig {
                enabled: true,
                level: LogLevel::Info,
                path: default_backend_log_path(),
                flush_interval: Duration::ZERO,
                buffer_capacity: 1024,
                max_detail_chars: 4096,
                fail_on_open_error: false,
                rotation: LogRotationConfig {
                    enabled: true,
                    max_bytes: DEFAULT_LOG_BYTES,
                    keep_files: 1,
                    rotate_on_start: false,
                },
                targets: HashMap::new(),
            },
        }
    }
}

impl ResolvedConfig {
    pub fn shared_default() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Builds the shortcut reference for one originating normal-mode focus.
    /// Only global commands and that focus's mounted operation table are
    /// included. Empty bindings remain visible as `(unbound)` so the popup is
    /// also an effective-configuration inspector for the current interface.
    pub fn shortcut_help_sections(
        &self,
        context: Option<ShortcutContext>,
    ) -> Vec<ShortcutHelpSection> {
        let command_item = |command: CommandId| ShortcutHelpItem {
            key: {
                let bindings = self.keymap.bindings_for(command);
                if bindings.is_empty() {
                    "(unbound)".into()
                } else {
                    bindings
                        .iter()
                        .map(|binding| display_sequence(binding))
                        .collect::<Vec<_>>()
                        .join(" / ")
                }
            },
            label: self.footer.command_presentation(command).label,
            operation: command.as_str().into(),
        };

        let mut sections = Vec::new();
        sections.push(ShortcutHelpSection {
            title: "Global · mounted in every normal focus".into(),
            context: None,
            items: CommandId::ALL
                .iter()
                .copied()
                .filter(|command| command.mount() == CommandMount::Global)
                .map(command_item)
                .collect(),
        });
        if let Some(context) = context {
            sections.push(ShortcutHelpSection {
                title: format!("{}  [{}]", context.title(), context.id()),
                context: Some(context),
                items: CommandId::ALL
                    .iter()
                    .copied()
                    .filter(|command| command.mount() == CommandMount::Focus)
                    .filter(|command| command.context_mask() & context.mask() != 0)
                    .map(command_item)
                    .collect(),
            });
        }
        sections
    }

    pub fn shortcut_help_line_count(&self, context: Option<ShortcutContext>) -> usize {
        3 + self
            .shortcut_help_sections(context)
            .iter()
            .map(|section| section.items.len() + 2)
            .sum::<usize>()
    }

    pub fn effective_toml(&self) -> String {
        let mut output = format!(
            "schema_version = {CONFIG_SCHEMA_VERSION}\n\n[ui.footer]\nmode = {}\nmax_rows = {}\nshow_global = {}\nshow_alternative_bindings = {}\ndefault_visibility = {}\nseparator = {}\noverflow = {}\n",
            quote(match self.footer.mode {
                FooterMode::Contextual => "contextual",
                FooterMode::Compact => "compact",
                FooterMode::Hidden => "hidden",
            }),
            self.footer.max_rows,
            self.footer.show_global,
            self.footer.show_alternative_bindings,
            quote(match self.footer.default_visibility {
                FooterVisibilityMode::Registry => "registry",
                FooterVisibilityMode::All => "all",
                FooterVisibilityMode::Allowlist => "allowlist",
            }),
            quote(&self.footer.separator),
            quote(match self.footer.overflow {
                FooterOverflow::Count => "count",
                FooterOverflow::Ellipsis => "ellipsis",
            })
        );
        for group_id in ["commit.copy", "file.copy", "more"] {
            let group = self.footer.group_presentation(group_id);
            output.push_str(&format!(
                "\n[ui.footer.groups.{}]\nvisible = {}\nlabel = {}\npriority = {}\n",
                quote(group_id),
                group.visible,
                quote(&group.label),
                group.priority
            ));
        }
        for command in CommandId::ALL.iter().copied() {
            let presentation = self.footer.command_presentation(command);
            output.push_str(&format!(
                "\n[ui.footer.commands.{}]\nvisible = {}\nlabel = {}\npriority = {}\n",
                quote(command.as_str()),
                presentation.visible,
                quote(&presentation.label),
                presentation.priority
            ));
        }
        output.push_str(&format!(
            "\n[keybindings]\nchord_timeout_ms = {}\n",
            self.keymap
                .chord_timeout
                .map_or(0, |duration| duration.as_millis())
        ));
        for command in CommandId::ALL.iter().copied() {
            output.push_str(&format!(
                "\n[keybindings.commands.{}]\nbindings = [{}]\n",
                quote(command.as_str()),
                self.keymap
                    .bindings_for(command)
                    .iter()
                    .map(|sequence| quote(&display_sequence(sequence)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        output.push_str(&format!(
            "\n[diff]\ndefault_mode = {}\n",
            quote(match self.diff.default_mode {
                DiffViewMode::Unified => "unified",
                DiffViewMode::SideBySide => "side-by-side",
            })
        ));
        output.push_str(&format!(
            "\n[logging]\nenabled = {}\nlevel = {}\npath = {}\nformat = \"jsonl\"\nflush_interval_ms = {}\nbuffer_capacity = {}\nmax_detail_chars = {}\nfail_on_open_error = {}\n",
            self.logging.enabled,
            quote(self.logging.level.as_str()),
            quote(&self.logging.path.to_string_lossy()),
            self.logging.flush_interval.as_millis(),
            self.logging.buffer_capacity,
            self.logging.max_detail_chars,
            self.logging.fail_on_open_error
        ));
        output.push_str(&format!(
            "\n[logging.rotation]\nenabled = {}\nmax_size = {}\nkeep_files = {}\nrotate_on_start = {}\n",
            self.logging.rotation.enabled,
            quote(&format!("{} B", self.logging.rotation.max_bytes)),
            self.logging.rotation.keep_files,
            self.logging.rotation.rotate_on_start
        ));
        if !self.logging.targets.is_empty() {
            output.push_str("\n[logging.targets]\n");
            let mut targets = self.logging.targets.iter().collect::<Vec<_>>();
            targets.sort_by_key(|(name, _)| *name);
            for (name, level) in targets {
                output.push_str(&format!("{name} = {}\n", quote(level.as_str())));
            }
        }
        output
    }
}

#[derive(Clone, Debug, Default)]
pub struct ConfigLoadOptions {
    pub path: Option<PathBuf>,
    pub no_config: bool,
    pub log_path_override: Option<PathBuf>,
}

impl ConfigLoadOptions {
    pub fn from_environment() -> Self {
        Self {
            path: env::var_os("PITUI_CONFIG")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            no_config: false,
            log_path_override: env::var_os("PITUI_LOG")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
        }
    }
}

pub fn selected_config_path(options: &ConfigLoadOptions) -> Result<Option<PathBuf>, ConfigError> {
    if options.no_config {
        Ok(None)
    } else {
        let selected = options.path.clone().unwrap_or_else(default_config_path);
        resolve_user_path(&selected, None).map(Some)
    }
}

pub fn load(options: &ConfigLoadOptions) -> Result<ResolvedConfig, ConfigError> {
    let path = selected_config_path(options)?;
    let explicit = !options.no_config && options.path.is_some();
    let raw = match path.as_ref() {
        Some(path) => match fs::read_to_string(path) {
            Ok(contents) => Some(
                toml::from_str::<RawConfig>(&contents)
                    .map_err(|error| ConfigError::at(path, error.to_string()))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit => None,
            Err(error) => return Err(ConfigError::at(path, error.to_string())),
        },
        None => None,
    };

    let mut resolved = ResolvedConfig {
        source_path: raw.as_ref().and(path.clone()),
        ..ResolvedConfig::default()
    };
    if let Some(raw) = raw {
        let loaded_path = path.as_deref().expect("loaded path");
        apply_raw_config(&mut resolved, raw, loaded_path)
            .map_err(|error| error.with_path(loaded_path))?;
    }
    if let Some(path) = options.log_path_override.as_ref() {
        resolved.logging.path = resolve_user_path(path, resolved.source_path.as_deref())?;
    }
    validate_keymap(&resolved.keymap).map_err(|error| {
        if let Some(path) = resolved.source_path.as_deref() {
            error.with_path(path)
        } else {
            error
        }
    })?;
    Ok(resolved)
}

fn apply_raw_config(
    resolved: &mut ResolvedConfig,
    raw: RawConfig,
    source_path: &Path,
) -> Result<(), ConfigError> {
    if raw.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(ConfigError::at(
            source_path,
            format!(
                "unsupported schema_version {}; expected {CONFIG_SCHEMA_VERSION}",
                raw.schema_version
            ),
        ));
    }

    apply_footer(&mut resolved.footer, raw.ui.footer)?;
    apply_keybindings(&mut resolved.keymap, raw.keybindings)?;
    if let Some(mode) = raw.diff.default_mode {
        resolved.diff.default_mode = match mode.as_str() {
            "unified" => DiffViewMode::Unified,
            "side-by-side" => DiffViewMode::SideBySide,
            _ => {
                return Err(ConfigError::new(format!(
                    "invalid diff.default_mode `{mode}`; expected unified or side-by-side"
                )));
            }
        };
    }
    apply_logging(&mut resolved.logging, raw.logging, source_path)?;
    Ok(())
}

fn apply_footer(resolved: &mut ResolvedFooterConfig, raw: RawFooter) -> Result<(), ConfigError> {
    if let Some(mode) = raw.mode {
        resolved.mode = match mode.as_str() {
            "contextual" => FooterMode::Contextual,
            "compact" => FooterMode::Compact,
            "hidden" => FooterMode::Hidden,
            _ => return Err(ConfigError::new(format!("invalid ui.footer.mode `{mode}`"))),
        };
    }
    if let Some(max_rows) = raw.max_rows {
        if !(1..=3).contains(&max_rows) {
            return Err(ConfigError::new(
                "ui.footer.max_rows must be between 1 and 3",
            ));
        }
        resolved.max_rows = max_rows;
    }
    if let Some(value) = raw.show_global {
        resolved.show_global = value;
    }
    if let Some(value) = raw.show_alternative_bindings {
        resolved.show_alternative_bindings = value;
    }
    if let Some(value) = raw.default_visibility {
        resolved.default_visibility = match value.as_str() {
            "registry" => FooterVisibilityMode::Registry,
            "all" => FooterVisibilityMode::All,
            "allowlist" => FooterVisibilityMode::Allowlist,
            _ => {
                return Err(ConfigError::new(format!(
                    "invalid ui.footer.default_visibility `{value}`"
                )));
            }
        };
    }
    if let Some(separator) = raw.separator {
        validate_display_text("ui.footer.separator", &separator, 16)?;
        resolved.separator = separator;
    }
    if let Some(value) = raw.overflow {
        resolved.overflow = match value.as_str() {
            "count" => FooterOverflow::Count,
            "ellipsis" => FooterOverflow::Ellipsis,
            _ => {
                return Err(ConfigError::new(format!(
                    "invalid ui.footer.overflow `{value}`"
                )));
            }
        };
    }
    for (id, presentation) in raw.commands {
        let command = parse_command_id(&id)?;
        resolved.commands.insert(
            command,
            resolve_presentation(format!("ui.footer.commands.{id}"), presentation)?,
        );
    }
    for (id, presentation) in raw.groups {
        if !matches!(id.as_str(), "commit.copy" | "file.copy" | "more") {
            return Err(ConfigError::new(format!(
                "unknown chord group `{id}`; known groups: commit.copy, file.copy, more"
            )));
        }
        resolved.groups.insert(
            id.clone(),
            resolve_presentation(format!("ui.footer.groups.{id}"), presentation)?,
        );
    }
    Ok(())
}

fn resolve_presentation(
    field: String,
    raw: RawPresentation,
) -> Result<FooterPresentationOverride, ConfigError> {
    if let Some(label) = raw.label.as_ref() {
        validate_display_text(&format!("{field}.label"), label, 40)?;
    }
    Ok(FooterPresentationOverride {
        visible: raw.visible,
        label: raw.label,
        priority: raw.priority,
    })
}

fn apply_keybindings(
    resolved: &mut ResolvedKeymap,
    raw: RawKeybindings,
) -> Result<(), ConfigError> {
    if let Some(timeout) = raw.chord_timeout_ms {
        if timeout > 60_000 {
            return Err(ConfigError::new(
                "keybindings.chord_timeout_ms must be 0..=60000",
            ));
        }
        resolved.chord_timeout = (timeout > 0).then(|| Duration::from_millis(timeout));
    }
    for (id, command_config) in raw.commands {
        let command = parse_command_id(&id)?;
        if let Some(bindings) = command_config.bindings {
            let mut parsed = Vec::new();
            for binding in bindings {
                let sequence = parse_sequence(&binding)
                    .map_err(|error| error.context(format!("keybindings.commands.{id}")))?;
                if sequence
                    .iter()
                    .skip(1)
                    .any(|stroke| stroke.code == KeyCode::Esc)
                {
                    return Err(ConfigError::new(format!(
                        "keybindings.commands.{id}: `Esc` is reserved for cancelling an active chord"
                    )));
                }
                if parsed.contains(&sequence) {
                    return Err(ConfigError::new(format!(
                        "duplicate binding `{binding}` for `{id}`"
                    )));
                }
                parsed.push(sequence);
            }
            resolved.bindings.insert(command, parsed);
        }
    }
    Ok(())
}

fn apply_logging(
    resolved: &mut ResolvedLoggingConfig,
    raw: RawLogging,
    source_path: &Path,
) -> Result<(), ConfigError> {
    if let Some(enabled) = raw.enabled {
        resolved.enabled = enabled;
    }
    if let Some(level) = raw.level {
        resolved.level = level.parse()?;
    }
    if let Some(format) = raw.format
        && format != "jsonl"
    {
        return Err(ConfigError::new(
            "logging.format currently supports only `jsonl`",
        ));
    }
    if let Some(path) = raw.path.filter(|path| !path.is_empty()) {
        resolved.path = resolve_user_path(Path::new(&path), Some(source_path))?;
    }
    if let Some(interval) = raw.flush_interval_ms {
        if interval != 0 && !(50..=60_000).contains(&interval) {
            return Err(ConfigError::new(
                "logging.flush_interval_ms must be 0 or 50..=60000",
            ));
        }
        resolved.flush_interval = Duration::from_millis(interval);
    }
    if let Some(capacity) = raw.buffer_capacity {
        if !(16..=1_000_000).contains(&capacity) {
            return Err(ConfigError::new(
                "logging.buffer_capacity must be 16..=1000000",
            ));
        }
        resolved.buffer_capacity = capacity;
    }
    if let Some(chars) = raw.max_detail_chars {
        if !(256..=65_536).contains(&chars) {
            return Err(ConfigError::new(
                "logging.max_detail_chars must be 256..=65536",
            ));
        }
        resolved.max_detail_chars = chars;
    }
    if let Some(value) = raw.fail_on_open_error {
        resolved.fail_on_open_error = value;
    }
    if let Some(value) = raw.rotation.enabled {
        resolved.rotation.enabled = value;
    }
    if let Some(value) = raw.rotation.max_size {
        resolved.rotation.max_bytes = parse_size(&value)?;
    }
    if let Some(value) = raw.rotation.keep_files {
        if value > 100 {
            return Err(ConfigError::new(
                "logging.rotation.keep_files must be 0..=100",
            ));
        }
        resolved.rotation.keep_files = value;
    }
    if let Some(value) = raw.rotation.rotate_on_start {
        resolved.rotation.rotate_on_start = value;
    }
    for (target, level) in raw.targets {
        if !matches!(target.as_str(), "git_worker" | "app" | "config") {
            return Err(ConfigError::new(format!(
                "unknown logging target `{target}`; expected git_worker, app, or config"
            )));
        }
        resolved.targets.insert(target, level.parse()?);
    }
    Ok(())
}

fn validate_keymap(keymap: &ResolvedKeymap) -> Result<(), ConfigError> {
    let quit_bindings = keymap.bindings_for(CommandId::AppQuit);
    if quit_bindings.is_empty() {
        return Err(ConfigError::new(
            "`app.quit` must keep at least one binding so Normal mode cannot trap the user",
        ));
    }
    let commands = CommandId::ALL;
    let has_unshadowed_quit = quit_bindings.iter().any(|quit| {
        !commands
            .iter()
            .copied()
            .filter(|command| *command != CommandId::AppQuit)
            .filter(|command| command.context_mask() & CommandId::AppQuit.context_mask() != 0)
            .flat_map(|command| keymap.bindings_for(command))
            .any(|binding| binding.len() > quit.len() && binding.starts_with(quit))
    });
    if !has_unshadowed_quit {
        return Err(ConfigError::new(
            "`app.quit` must keep one binding that is not shadowed by a chord prefix",
        ));
    }
    for command in commands.iter().copied() {
        let sequences = keymap.bindings_for(command);
        for (index, left) in sequences.iter().enumerate() {
            for right in sequences.iter().skip(index + 1) {
                if left.starts_with(right) || right.starts_with(left) {
                    return Err(ConfigError::new(format!(
                        "keybinding prefix conflict within `{}`: `{}` and `{}`",
                        command.as_str(),
                        display_sequence(left),
                        display_sequence(right)
                    )));
                }
            }
        }
    }
    for (left_index, left) in commands.iter().copied().enumerate() {
        for right in commands.iter().copied().skip(left_index + 1) {
            let overlap = left.context_mask() & right.context_mask();
            if overlap == 0 {
                continue;
            }
            for left_sequence in keymap.bindings_for(left) {
                for right_sequence in keymap.bindings_for(right) {
                    if left_sequence == right_sequence {
                        return Err(binding_conflict(left, right, left_sequence));
                    }
                    let (shorter, shorter_command, longer, longer_command) =
                        if left_sequence.len() < right_sequence.len() {
                            (left_sequence, left, right_sequence, right)
                        } else {
                            (right_sequence, right, left_sequence, left)
                        };
                    if longer.starts_with(shorter) {
                        let shorter_scope = shorter_command.context_mask();
                        let longer_scope = longer_command.context_mask();
                        // A broad leaf may act as a fallback outside a narrower
                        // chord context. This preserves Ctrl+C quit vs copy.
                        if longer_scope != shorter_scope && longer_scope & !shorter_scope == 0 {
                            continue;
                        }
                        return Err(binding_conflict(left, right, shorter));
                    }
                }
            }
        }
    }
    Ok(())
}

fn binding_conflict(left: CommandId, right: CommandId, sequence: &[KeyStroke]) -> ConfigError {
    ConfigError::new(format!(
        "keybinding conflict `{}` between `{}` and `{}` in overlapping contexts",
        display_sequence(sequence),
        left.as_str(),
        right.as_str()
    ))
}

fn parse_command_id(id: &str) -> Result<CommandId, ConfigError> {
    CommandId::parse(id).ok_or_else(|| {
        let candidates = CommandId::ALL
            .iter()
            .map(|command| command.as_str())
            .filter(|candidate| candidate.starts_with(id.split('.').next().unwrap_or_default()))
            .take(5)
            .collect::<Vec<_>>();
        ConfigError::new(if candidates.is_empty() {
            format!("unknown command id `{id}`")
        } else {
            format!(
                "unknown command id `{id}`; possible values: {}",
                candidates.join(", ")
            )
        })
    })
}

fn parse_size(value: &str) -> Result<u64, ConfigError> {
    let compact = value.trim();
    let split = compact
        .find(|character: char| !character.is_ascii_digit() && character != '_')
        .unwrap_or(compact.len());
    let number = compact[..split].replace('_', "");
    let amount = number.parse::<u64>().map_err(|_| {
        ConfigError::new(format!(
            "invalid size `{value}`; expected for example `5 MiB`"
        ))
    })?;
    let unit = compact[split..].trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1,
        "kib" => 1024,
        "mib" => 1024 * 1024,
        "gib" => 1024 * 1024 * 1024,
        _ => {
            return Err(ConfigError::new(format!(
                "invalid size unit in `{value}`; expected B, KiB, MiB, or GiB"
            )));
        }
    };
    amount
        .checked_mul(multiplier)
        .filter(|value| *value > 0)
        .ok_or_else(|| ConfigError::new(format!("size `{value}` is zero or too large")))
}

fn validate_display_text(
    field: &str,
    value: &str,
    maximum_chars: usize,
) -> Result<(), ConfigError> {
    if value.chars().count() > maximum_chars {
        return Err(ConfigError::new(format!(
            "{field} must be at most {maximum_chars} characters"
        )));
    }
    if value.chars().any(|character| {
        character.is_control()
            || matches!(
                character,
                '\u{202A}'
                    ..='\u{202E}' | '\u{2066}'
                    ..='\u{2069}' | '\u{001B}'
            )
    }) {
        return Err(ConfigError::new(format!(
            "{field} contains terminal control or bidi override characters"
        )));
    }
    Ok(())
}

fn resolve_user_path(path: &Path, config_path: Option<&Path>) -> Result<PathBuf, ConfigError> {
    let text = path.to_string_lossy();
    if text.starts_with('~') && text != "~" && !text.starts_with("~/") && !text.starts_with("~\\") {
        return Err(ConfigError::new(
            "only `~` or a path beginning with `~/` can use home expansion",
        ));
    }
    let expanded = if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .ok_or_else(|| ConfigError::new("cannot expand `~`: home directory is unavailable"))?;
        PathBuf::from(home).join(text.trim_start_matches('~').trim_start_matches(['/', '\\']))
    } else {
        path.to_path_buf()
    };
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        let base = config_path
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or(env::current_dir().map_err(|error| ConfigError::new(error.to_string()))?);
        Ok(base.join(expanded))
    }
}

pub fn default_config_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("pitui")
            .join("config.toml");
    }
    #[cfg(windows)]
    if let Some(app_data) = env::var_os("APPDATA") {
        return PathBuf::from(app_data).join("pitui").join("config.toml");
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(config_home).join("pitui").join("config.toml");
        }
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("pitui")
                .join("config.toml");
        }
    }
    env::temp_dir().join("pitui").join("config.toml")
}

pub fn default_backend_log_path() -> PathBuf {
    if let Some(path) = env::var_os("PITUI_LOG")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
    {
        return resolve_user_path(&path, None).unwrap_or(path);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join("pitui")
            .join("pitui.jsonl");
    }
    #[cfg(windows)]
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local_app_data)
            .join("pitui")
            .join("pitui.jsonl");
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        if let Some(state_home) = env::var_os("XDG_STATE_HOME") {
            return PathBuf::from(state_home).join("pitui").join("pitui.jsonl");
        }
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("pitui")
                .join("pitui.jsonl");
        }
    }
    env::temp_dir().join("pitui").join("pitui.jsonl")
}

fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('\"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

#[derive(Debug)]
pub struct ConfigError {
    path: Option<PathBuf>,
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            path: None,
            message: message.into(),
        }
    }

    fn at(path: &Path, message: impl Into<String>) -> Self {
        Self {
            path: Some(path.to_path_buf()),
            message: message.into(),
        }
    }

    fn context(mut self, context: String) -> Self {
        self.message = format!("{context}: {}", self.message);
        self
    }

    fn with_path(mut self, path: &Path) -> Self {
        if self.path.is_none() {
            self.path = Some(path.to_path_buf());
        }
        self
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = self.path.as_ref() {
            write!(formatter, "{}: {}", path.display(), self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl Error for ConfigError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    schema_version: u32,
    #[serde(default)]
    ui: RawUi,
    #[serde(default)]
    keybindings: RawKeybindings,
    #[serde(default)]
    diff: RawDiff,
    #[serde(default)]
    logging: RawLogging,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawUi {
    footer: RawFooter,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFooter {
    mode: Option<String>,
    max_rows: Option<u16>,
    show_global: Option<bool>,
    show_alternative_bindings: Option<bool>,
    default_visibility: Option<String>,
    separator: Option<String>,
    overflow: Option<String>,
    commands: BTreeMap<String, RawPresentation>,
    groups: BTreeMap<String, RawPresentation>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawPresentation {
    visible: Option<bool>,
    label: Option<String>,
    priority: Option<u16>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawKeybindings {
    chord_timeout_ms: Option<u64>,
    commands: BTreeMap<String, RawCommandBinding>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawCommandBinding {
    bindings: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawDiff {
    default_mode: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLogging {
    enabled: Option<bool>,
    level: Option<String>,
    path: Option<String>,
    format: Option<String>,
    flush_interval_ms: Option<u64>,
    buffer_capacity: Option<usize>,
    max_detail_chars: Option<usize>,
    fail_on_open_error: Option<bool>,
    rotation: RawRotation,
    targets: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRotation {
    enabled: Option<bool>,
    max_size: Option<String>,
    keep_files: Option<usize>,
    rotate_on_start: Option<bool>,
}

pub fn repository_args_without_config_flags(
    args: impl IntoIterator<Item = OsString>,
) -> Result<(Vec<OsString>, ConfigCli), ConfigError> {
    let mut repositories = Vec::new();
    let mut cli = ConfigCli::default();
    let mut arguments = args.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--config") => {
                let path = arguments
                    .next()
                    .ok_or_else(|| ConfigError::new("--config requires a path"))?;
                cli.path = Some(PathBuf::from(path));
            }
            Some("--no-config") => cli.no_config = true,
            Some("--check-config") => cli.check = true,
            Some("--print-config-path") => cli.print_path = true,
            Some("--print-effective-config") => cli.print_effective = true,
            Some("-h" | "--help") => cli.help = true,
            Some("--") => {
                repositories.extend(&mut arguments);
                break;
            }
            Some(value) if value.starts_with('-') => {
                return Err(ConfigError::new(format!("unknown option `{value}`")));
            }
            _ => repositories.push(argument),
        }
    }
    if cli.no_config && cli.path.is_some() {
        return Err(ConfigError::new(
            "--config and --no-config cannot be used together",
        ));
    }
    Ok((repositories, cli))
}

#[derive(Clone, Debug, Default)]
pub struct ConfigCli {
    pub path: Option<PathBuf>,
    pub no_config: bool,
    pub check: bool,
    pub print_path: bool,
    pub print_effective: bool,
    pub help: bool,
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc, time::Instant};

    use super::*;
    use crate::{
        app::{FocusPanel, GlobalMode, Screen, ShortcutContext},
        domain::{ChangedFile, Commit, CommitDetail, CommitHash, FileChangeKind, GitPath},
    };

    #[test]
    fn parses_and_formats_key_sequences() {
        let sequence = parse_sequence("Ctrl+C h").unwrap();
        assert_eq!(display_sequence(&sequence), "Ctrl+C h");
        assert_eq!(KeyStroke::parse("Shift+Tab").unwrap().display(), "BackTab");
        assert!(parse_sequence("Ctrl+K r x y").is_err());
    }

    #[test]
    fn parses_sizes_strictly() {
        assert_eq!(parse_size("5 MiB").unwrap(), 5 * 1024 * 1024);
        assert_eq!(parse_size("1024 B").unwrap(), 1024);
        assert!(parse_size("5 MB").is_err());
        assert!(parse_size("0 B").is_err());
    }

    #[test]
    fn loads_key_footer_diff_and_logging_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"
schema_version = 1

[ui.footer]
default_visibility = "allowlist"
max_rows = 2

[ui.footer.groups."commit.copy"]
visible = true
label = "copy"

[ui.footer.commands."app.refresh"]
visible = true
label = "reload"

[keybindings.commands."app.refresh"]
bindings = ["Alt+R"]

[diff]
default_mode = "side-by-side"

[logging]
level = "debug"
path = "logs/backend.jsonl"
flush_interval_ms = 250

[logging.rotation]
max_size = "2 MiB"
keep_files = 4
"#,
        )
        .unwrap();

        let config = load(&ConfigLoadOptions {
            path: Some(path.clone()),
            ..ConfigLoadOptions::default()
        })
        .unwrap();
        assert_eq!(config.footer.max_rows, 2);
        assert_eq!(config.diff.default_mode, DiffViewMode::SideBySide);
        assert_eq!(config.logging.level, LogLevel::Debug);
        assert_eq!(
            config.logging.path,
            directory.path().join("logs/backend.jsonl")
        );
        assert_eq!(config.logging.rotation.max_bytes, 2 * 1024 * 1024);
        assert_eq!(config.logging.rotation.keep_files, 4);
        assert_eq!(
            config
                .keymap
                .bindings_for(CommandId::AppRefresh)
                .first()
                .map(|sequence| display_sequence(sequence)),
            Some("Alt+R".into())
        );
        toml::from_str::<toml::Value>(&config.effective_toml()).unwrap();
    }

    #[test]
    fn rejects_unknown_fields_commands_and_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        for (name, contents) in [
            ("unknown", "schema_version=1\nwat=true\n"),
            (
                "command",
                "schema_version=1\n[keybindings.commands.\"wat.nope\"]\nbindings=[\"x\"]\n",
            ),
            (
                "conflict",
                "schema_version=1\n[keybindings.commands.\"changes.stage\"]\nbindings=[\"x\"]\n[keybindings.commands.\"changes.commit\"]\nbindings=[\"x\"]\n",
            ),
            (
                "prefix-conflict",
                "schema_version=1\n[keybindings.commands.\"changes.stage\"]\nbindings=[\"x\"]\n[keybindings.commands.\"changes.commit\"]\nbindings=[\"x y\"]\n",
            ),
            (
                "quit-unbound",
                "schema_version=1\n[keybindings.commands.\"app.quit\"]\nbindings=[]\n",
            ),
            (
                "quit-shadowed",
                "schema_version=1\n[keybindings.commands.\"app.quit\"]\nbindings=[\"Ctrl+C\"]\n",
            ),
            (
                "reserved-chord-escape",
                "schema_version=1\n[keybindings.commands.\"app.refresh\"]\nbindings=[\"Ctrl+K Esc\"]\n",
            ),
        ] {
            let path = directory.path().join(format!("{name}.toml"));
            fs::write(&path, contents).unwrap();
            assert!(
                load(&ConfigLoadOptions {
                    path: Some(path),
                    ..ConfigLoadOptions::default()
                })
                .is_err(),
                "{name} should fail"
            );
        }
    }

    #[test]
    fn disjoint_commit_and_file_focus_tables_may_reuse_the_same_binding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("disjoint.toml");
        fs::write(
            &path,
            r#"
schema_version = 1
[keybindings.commands."commit.copy.hash"]
bindings = ["Ctrl+K x"]
[keybindings.commands."file.copy.name"]
bindings = ["Ctrl+K x"]
"#,
        )
        .unwrap();
        load(&ConfigLoadOptions {
            path: Some(path),
            ..ConfigLoadOptions::default()
        })
        .expect("disjoint focus tables must not conflict");
    }

    #[test]
    fn configurable_binding_and_footer_share_the_same_actionability() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"
schema_version = 1

[ui.footer]
default_visibility = "allowlist"

[ui.footer.commands."app.refresh"]
visible = true
label = "reload"

[keybindings.commands."app.refresh"]
bindings = ["Alt+R"]
"#,
        )
        .unwrap();
        let config = Arc::new(
            load(&ConfigLoadOptions {
                path: Some(path),
                ..ConfigLoadOptions::default()
            })
            .unwrap(),
        );
        let state = AppState::with_config(vec![PathBuf::from("/repo")], config.clone());

        assert_eq!(
            config
                .keymap
                .resolve(&state, &[], KeyStroke::parse("Alt+R").unwrap()),
            Some(KeyResolution::Action(Action::RefreshRepository))
        );
        assert_eq!(
            config
                .keymap
                .resolve(&state, &[], KeyStroke::parse("Ctrl+R").unwrap()),
            None
        );
        assert_eq!(
            config
                .keymap
                .footer_items(&state, &config.footer)
                .iter()
                .map(|item| format!("{} {}", item.key, item.label))
                .collect::<Vec<_>>(),
            vec!["Alt+R reload"]
        );

        let empty = AppState::with_config(Vec::new(), config.clone());
        assert!(
            config
                .keymap
                .footer_items(&empty, &config.footer)
                .is_empty(),
            "an allowlisted but currently inactive refresh must not be shown"
        );
    }

    #[test]
    fn chord_footer_reveals_only_the_current_level() {
        let mut state = AppState {
            focus: FocusPanel::CommitList,
            ..AppState::default()
        };
        state.branch_commits.items.push(Commit {
            hash: CommitHash("0123456789abcdef".into()),
            short_hash: "01234567".into(),
            author: "Ada".into(),
            authored_at: "2026-07-16".into(),
            decorations: String::new(),
            subject: "copy me".into(),
        });
        state.ensure_valid_commit_selection();

        let root = state
            .config
            .keymap
            .footer_items(&state, &state.config.footer);
        assert!(
            root.iter()
                .any(|item| item.key == "Ctrl+C" && item.label == "copy…")
        );
        assert!(
            root.iter()
                .any(|item| item.key == "h" && item.label == "help")
        );
        assert!(
            !root
                .iter()
                .any(|item| matches!(item.label.as_str(), "hash" | "info" | "message"))
        );

        let prefix = vec![KeyStroke::parse("Ctrl+C").unwrap()];
        state.mode = GlobalMode::Chord {
            prefix: prefix.clone(),
            started_at: Instant::now(),
        };
        let second = state
            .config
            .keymap
            .footer_items(&state, &state.config.footer);
        assert_eq!(
            second
                .iter()
                .map(|item| item.key.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["h", "i", "m"])
        );
        assert!(!second.iter().any(|item| item.key == "Ctrl+C"));
        assert_eq!(
            state
                .config
                .keymap
                .resolve(&state, &prefix, KeyStroke::parse("h").unwrap()),
            Some(KeyResolution::Action(Action::CopySelectedCommitHashes))
        );
    }

    #[test]
    fn focused_file_table_replaces_commit_copy_palette() {
        let commit = Commit {
            hash: CommitHash("0123456789abcdef".into()),
            short_hash: "01234567".into(),
            author: "Ada".into(),
            authored_at: "2026-07-16".into(),
            decorations: String::new(),
            subject: "file palette".into(),
        };
        let mut state = AppState {
            screen: Screen::CommitDetail,
            focus: FocusPanel::CommitFileList,
            current_commit_detail: Some(CommitDetail {
                commit,
                author_email: "ada@example.invalid".into(),
                committer: "Ada".into(),
                committer_email: "ada@example.invalid".into(),
                committed_at: "2026-07-16".into(),
                message: "file palette".into(),
                files: vec![ChangedFile {
                    kind: FileChangeKind::Modified,
                    path: GitPath::from("src/main.rs"),
                    old_path: None,
                    additions: Some(1),
                    deletions: Some(1),
                    hunks: Vec::new(),
                    is_binary: false,
                }],
            }),
            ..AppState::default()
        };
        state.ensure_valid_file_selection();
        let root = state
            .config
            .keymap
            .footer_items(&state, &state.config.footer);
        assert!(
            root.iter()
                .any(|item| item.key == "Ctrl+C" && item.label == "copy file…")
        );

        let prefix = vec![KeyStroke::parse("Ctrl+C").unwrap()];
        state.mode = GlobalMode::Chord {
            prefix,
            started_at: Instant::now(),
        };
        assert_eq!(
            state
                .config
                .keymap
                .footer_items(&state, &state.config.footer)
                .iter()
                .map(|item| item.key.as_str())
                .collect::<HashSet<_>>(),
            HashSet::from(["a", "n", "r"])
        );
    }

    #[test]
    fn shortcut_reference_contains_only_global_and_current_focus_operations() {
        let config = ResolvedConfig::default();
        let sections = config.shortcut_help_sections(Some(ShortcutContext::DetailCommits));
        assert_eq!(sections.len(), 2);
        let commit = sections
            .iter()
            .find(|section| section.context == Some(ShortcutContext::DetailCommits))
            .unwrap();
        assert!(
            commit
                .items
                .iter()
                .any(|item| item.operation == "commit.copy.message")
        );
        assert!(
            commit
                .items
                .iter()
                .all(|item| !item.operation.starts_with("file.copy."))
        );
        assert!(sections.iter().all(|section| {
            section.context.is_none() || section.context == Some(ShortcutContext::DetailCommits)
        }));

        let sections = config.shortcut_help_sections(Some(ShortcutContext::CommitFiles));
        assert_eq!(sections.len(), 2);
        let files = sections
            .iter()
            .find(|section| section.context == Some(ShortcutContext::CommitFiles))
            .unwrap();
        assert!(
            files
                .items
                .iter()
                .any(|item| item.operation == "file.copy.absolute_path")
        );
        assert!(
            files
                .items
                .iter()
                .all(|item| !item.operation.starts_with("commit.copy."))
        );
        assert!(sections.iter().all(|section| {
            section.context.is_none() || section.context == Some(ShortcutContext::CommitFiles)
        }));
        assert_eq!(config.shortcut_help_sections(None).len(), 1);
    }

    #[test]
    fn configured_three_stroke_chords_reveal_one_transition_at_a_time() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"
schema_version = 1

[ui.footer]
default_visibility = "allowlist"

[ui.footer.commands."app.refresh"]
visible = true
label = "reload"

[keybindings.commands."app.refresh"]
bindings = ["Ctrl+K r x"]
"#,
        )
        .unwrap();
        let config = Arc::new(
            load(&ConfigLoadOptions {
                path: Some(path),
                ..ConfigLoadOptions::default()
            })
            .unwrap(),
        );
        let mut state = AppState::with_config(vec![PathBuf::from("/repo")], config.clone());

        let labels = |state: &AppState| {
            config
                .keymap
                .footer_items(state, &config.footer)
                .into_iter()
                .map(|item| format!("{} {}", item.key, item.label))
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(&state), vec!["Ctrl+K reload…"]);

        let first = vec![KeyStroke::parse("Ctrl+K").unwrap()];
        state.mode = GlobalMode::Chord {
            prefix: first.clone(),
            started_at: Instant::now(),
        };
        assert_eq!(labels(&state), vec!["r reload…"]);

        let second = vec![
            KeyStroke::parse("Ctrl+K").unwrap(),
            KeyStroke::parse("r").unwrap(),
        ];
        state.mode = GlobalMode::Chord {
            prefix: second.clone(),
            started_at: Instant::now(),
        };
        assert_eq!(labels(&state), vec!["x reload"]);
        assert_eq!(
            config
                .keymap
                .resolve(&state, &second, KeyStroke::parse("x").unwrap()),
            Some(KeyResolution::Action(Action::RefreshRepository))
        );
    }

    #[test]
    fn hiding_a_footer_hint_does_not_disable_its_binding() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            r#"
schema_version = 1

[ui.footer.commands."app.refresh"]
visible = false

[keybindings.commands."app.refresh"]
bindings = ["Alt+R"]
"#,
        )
        .unwrap();
        let config = Arc::new(
            load(&ConfigLoadOptions {
                path: Some(path),
                ..ConfigLoadOptions::default()
            })
            .unwrap(),
        );
        let state = AppState::with_config(vec![PathBuf::from("/repo")], config.clone());
        assert_eq!(
            config
                .keymap
                .resolve(&state, &[], KeyStroke::parse("Alt+R").unwrap()),
            Some(KeyResolution::Action(Action::RefreshRepository))
        );
        assert!(
            config
                .keymap
                .footer_items(&state, &config.footer)
                .iter()
                .all(|item| item.key != "Alt+R")
        );
    }

    #[test]
    fn configured_diff_mode_initializes_both_shared_diff_views() {
        let mut config = ResolvedConfig::default();
        config.diff.default_mode = DiffViewMode::SideBySide;
        let state = AppState::with_config(Vec::new(), Arc::new(config));
        assert_eq!(state.diff_mode, DiffViewMode::SideBySide);
    }

    #[test]
    fn diff_default_is_unified_and_invalid_values_fail_before_startup() {
        let defaults = load(&ConfigLoadOptions {
            no_config: true,
            ..ConfigLoadOptions::default()
        })
        .unwrap();
        assert_eq!(defaults.diff.default_mode, DiffViewMode::Unified);

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid-diff.toml");
        fs::write(
            &path,
            "schema_version = 1\n[diff]\ndefault_mode = \"side_by_side\"\n",
        )
        .unwrap();
        let error = load(&ConfigLoadOptions {
            path: Some(path),
            ..ConfigLoadOptions::default()
        })
        .unwrap_err();
        assert!(error.to_string().contains("invalid diff.default_mode"));
        assert!(error.to_string().contains("invalid-diff.toml"));
    }

    #[test]
    fn selected_config_paths_are_normalized_before_diagnostics() {
        let selected = selected_config_path(&ConfigLoadOptions {
            path: Some(PathBuf::from("relative/config.toml")),
            ..ConfigLoadOptions::default()
        })
        .unwrap()
        .unwrap();
        assert!(selected.is_absolute());
        assert!(selected.ends_with("relative/config.toml"));
        assert_eq!(
            selected_config_path(&ConfigLoadOptions {
                no_config: true,
                ..ConfigLoadOptions::default()
            })
            .unwrap(),
            None
        );
        assert!(resolve_user_path(Path::new("~another-user/config.toml"), None).is_err());
    }

    #[test]
    fn parses_diagnostic_cli_without_consuming_repository_arguments() {
        let (repositories, cli) = repository_args_without_config_flags([
            OsString::from("--config"),
            OsString::from("custom.toml"),
            OsString::from("repo-one"),
            OsString::from("--"),
            OsString::from("-repo-two"),
        ])
        .unwrap();
        assert_eq!(cli.path, Some(PathBuf::from("custom.toml")));
        assert_eq!(
            repositories,
            vec![OsString::from("repo-one"), OsString::from("-repo-two")]
        );
    }
}
