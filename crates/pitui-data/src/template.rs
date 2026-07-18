use std::collections::HashMap;

use bevy_ecs::prelude::Resource;

use crate::DatasetKind;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

string_id!(DatasetTemplateId);
string_id!(OperationId);
string_id!(CommandId);
string_id!(CommandSystemId);
string_id!(AvailabilityRuleId);
string_id!(RenderProxyId);
string_id!(RenderModeId);
string_id!(ResolvedOperationSetId);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetTemplate {
    pub id: DatasetTemplateId,
    pub kind: DatasetKind,
    pub operations: Vec<OperationId>,
    pub render_proxies: Vec<RenderProxyId>,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct DatasetTemplateRegistry {
    pub templates: HashMap<DatasetTemplateId, DatasetTemplate>,
}

impl DatasetTemplateRegistry {
    pub fn register(&mut self, template: DatasetTemplate) -> Result<(), DatasetTemplate> {
        if self.templates.contains_key(&template.id) {
            return Err(template);
        }
        self.templates.insert(template.id.clone(), template);
        Ok(())
    }

    pub fn get(&self, id: &DatasetTemplateId) -> Option<&DatasetTemplate> {
        self.templates.get(id)
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct DefaultDatasetTemplates {
    pub by_kind: HashMap<DatasetKind, DatasetTemplateId>,
}

impl DefaultDatasetTemplates {
    pub fn bind(&mut self, kind: DatasetKind, template: DatasetTemplateId) {
        self.by_kind.insert(kind, template);
    }

    pub fn get(&self, kind: DatasetKind) -> Option<&DatasetTemplateId> {
        self.by_kind.get(&kind)
    }
}
