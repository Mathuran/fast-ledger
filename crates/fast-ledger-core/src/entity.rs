use crate::types::{AccountId, Currency, EntityId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A legal entity or subsidiary within an organization (or the sole entity for a startup).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub code: String,
    pub name: String,
    pub functional_currency: Currency,
    pub parent_entity_id: Option<EntityId>,
}

/// Input specification for registering a new entity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateEntityInput {
    pub code: String,
    pub name: String,
    pub functional_currency: Currency,
    #[serde(default)]
    pub parent_entity_id: Option<EntityId>,
}

/// Intercompany Due-From / Due-To mapping for a pair of entities `(from_entity, to_entity)`.
///
/// When a journal entry spans multiple entities, the ledger automatically injects
/// `Due From` / `Due To` balancing lines so each entity remains independently balanced.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct IntercompanyRule {
    pub from_entity: EntityId,
    pub to_entity: EntityId,
    /// Asset account on `from_entity`'s books (Due From `to_entity`).
    pub due_from_account: AccountId,
    /// Liability account on `to_entity`'s books (Due To `from_entity`).
    pub due_to_account: AccountId,
}

/// Registry of intercompany balancing rules indexed by `(from_entity, to_entity)`.
#[derive(Clone, Debug, Default)]
pub struct IntercompanyConfig {
    rules: HashMap<(EntityId, EntityId), IntercompanyRule>,
}

impl IntercompanyConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_rule(&mut self, rule: IntercompanyRule) {
        self.rules.insert((rule.from_entity, rule.to_entity), rule);
    }

    pub fn get_rule(&self, from_entity: EntityId, to_entity: EntityId) -> Option<&IntercompanyRule> {
        self.rules.get(&(from_entity, to_entity))
    }

    pub fn all_rules(&self) -> impl Iterator<Item = &IntercompanyRule> {
        self.rules.values()
    }
}
