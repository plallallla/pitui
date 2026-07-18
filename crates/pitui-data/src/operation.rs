use std::collections::HashMap;

use bevy_ecs::prelude::{Entity, Message, Resource};

use crate::{
    AvailabilityRuleId, CommandId, CommandSystemId, DatasetKind, OperationId, RenderBindingId,
    ResolvedOperationSetId,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandScope {
    Global,
    Dataset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub id: CommandId,
    pub name: String,
    pub scope: CommandScope,
    pub system: CommandSystemId,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct CommandRegistry {
    pub commands: HashMap<CommandId, CommandSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandRegistryError {
    DuplicateCommand(CommandId),
    EmptyCommandName(CommandId),
    CommandNameContainsWhitespace(CommandId),
}

impl CommandRegistry {
    pub fn register(&mut self, spec: CommandSpec) -> Result<(), CommandRegistryError> {
        if spec.name.is_empty() {
            return Err(CommandRegistryError::EmptyCommandName(spec.id));
        }
        if spec.name.chars().any(char::is_whitespace) {
            return Err(CommandRegistryError::CommandNameContainsWhitespace(spec.id));
        }
        if self.commands.contains_key(&spec.id) {
            return Err(CommandRegistryError::DuplicateCommand(spec.id));
        }
        self.commands.insert(spec.id.clone(), spec);
        Ok(())
    }

    pub fn get(&self, id: &CommandId) -> Option<&CommandSpec> {
        self.commands.get(id)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeyModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
}

impl KeyModifiers {
    pub const fn control() -> Self {
        Self {
            control: true,
            alt: false,
            shift: false,
            super_key: false,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum KeyCode {
    Character(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Escape,
    Space,
    Backspace,
    Tab,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeyStroke {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyStroke {
    pub fn plain(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::default(),
        }
    }

    pub fn character(character: char) -> Self {
        Self::plain(KeyCode::Character(character.to_ascii_lowercase()))
    }

    pub fn control(character: char) -> Self {
        Self {
            code: KeyCode::Character(character.to_ascii_lowercase()),
            modifiers: KeyModifiers::control(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct KeySequence(pub Vec<KeyStroke>);

impl KeySequence {
    pub fn single(stroke: KeyStroke) -> Self {
        Self(vec![stroke])
    }

    pub fn chord(strokes: impl IntoIterator<Item = KeyStroke>) -> Self {
        Self(strokes.into_iter().collect())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationSpec {
    pub id: OperationId,
    pub label: String,
    pub command: CommandId,
    pub bindings: Vec<KeySequence>,
    pub target_source: TargetSource,
    pub availability: AvailabilityRuleId,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct OperationRegistry {
    pub operations: HashMap<OperationId, OperationSpec>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationRegistryError {
    DuplicateOperation(OperationId),
    EmptyKeySequence(OperationId),
}

impl OperationRegistry {
    pub fn register(&mut self, spec: OperationSpec) -> Result<(), OperationRegistryError> {
        if spec.bindings.iter().any(|binding| binding.0.is_empty()) {
            return Err(OperationRegistryError::EmptyKeySequence(spec.id));
        }
        if self.operations.contains_key(&spec.id) {
            return Err(OperationRegistryError::DuplicateOperation(spec.id));
        }
        self.operations.insert(spec.id.clone(), spec);
        Ok(())
    }

    pub fn get(&self, id: &OperationId) -> Option<&OperationSpec> {
        self.operations.get(id)
    }
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct GlobalOperationSet(pub Vec<OperationId>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetSource {
    None,
    ActiveDataset,
    ActiveElement,
    Selection,
    SelectionOrActiveElement,
    ContextActiveElement(RenderBindingId),
    ContextSelectionOrActiveElement(RenderBindingId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AvailabilityRule {
    Always,
    ActiveDatasetKind(DatasetKind),
    HasActiveElement,
    HasSelection,
    HasSelectionOrActiveElement,
    ContextHasActiveElement(RenderBindingId),
    ContextHasSelectionOrActiveElement(RenderBindingId),
    ContextActiveElementKind(RenderBindingId, DatasetKind),
    ContextTargetsBoundary(RenderBindingId, crate::ChangeBoundary),
    ChangesHasStagedFiles(RenderBindingId),
    InteractionContextType(crate::InteractionContextType),
    All(Vec<AvailabilityRule>),
    Any(Vec<AvailabilityRule>),
    Not(Box<AvailabilityRule>),
}

#[derive(Resource, Clone, Debug, Default)]
pub struct AvailabilityRuleRegistry {
    pub rules: HashMap<AvailabilityRuleId, AvailabilityRule>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AvailabilityRegistryError {
    DuplicateRule(AvailabilityRuleId),
}

impl AvailabilityRuleRegistry {
    pub fn register(
        &mut self,
        id: AvailabilityRuleId,
        rule: AvailabilityRule,
    ) -> Result<(), AvailabilityRegistryError> {
        if self.rules.contains_key(&id) {
            return Err(AvailabilityRegistryError::DuplicateRule(id));
        }
        self.rules.insert(id, rule);
        Ok(())
    }

    pub fn get(&self, id: &AvailabilityRuleId) -> Option<&AvailabilityRule> {
        self.rules.get(id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedOperation {
    pub id: OperationId,
    pub label: String,
    pub command: CommandId,
    pub target_source: TargetSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedKeyAction {
    Invoke(OperationId),
    EnterChord(Vec<KeyStroke>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedKeyBinding {
    pub stroke: KeyStroke,
    pub label: String,
    pub action: ResolvedKeyAction,
}

#[derive(Resource, Clone, Debug, Eq, PartialEq)]
pub struct ResolvedOperationSet {
    pub id: ResolvedOperationSetId,
    pub operations: Vec<ResolvedOperation>,
    pub key_bindings: HashMap<KeyStroke, ResolvedKeyBinding>,
    pub commands: HashMap<String, OperationId>,
    pub generation: u64,
}

impl Default for ResolvedOperationSet {
    fn default() -> Self {
        Self {
            id: ResolvedOperationSetId::from("unresolved"),
            operations: Vec::new(),
            key_bindings: HashMap::new(),
            commands: HashMap::new(),
            generation: 0,
        }
    }
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct PendingChordState {
    pub prefix: Vec<KeyStroke>,
}

/// Process lifecycle is data as well: the quit command requests exit and the
/// composition root decides when to tear down the terminal adapter.
#[derive(Resource, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QuitRequested(pub bool);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvocationSource {
    KeyBinding,
    CommandPalette,
}

#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub struct CommandInvocation {
    pub command: CommandId,
    pub source_dataset: Entity,
    pub targets: Vec<Entity>,
    pub source: InvocationSource,
}

#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub enum InputIntent {
    Key(KeyStroke),
    Paste(String),
    CommandLine(String),
    CancelChord,
}

#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub enum OperationNotice {
    UnknownCommand(String),
    CommandArgumentsUnsupported(String),
    TargetUnavailable(OperationId),
    CommandSystemUnavailable(CommandId),
    CommandRejected { command: CommandId, message: String },
    ContextTransitionRejected(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardContentKind {
    CommitHashes,
    CommitInfo,
    CommitMessage,
    ReflogHash,
    FileName,
    FileAbsolutePath,
    FileRelativePath,
}

#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub struct ClipboardRequest {
    pub kind: ClipboardContentKind,
    pub text: String,
    pub source_entities: Vec<Entity>,
}
