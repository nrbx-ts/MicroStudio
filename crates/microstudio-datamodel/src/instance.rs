use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::attributes::AttributeValue;
use crate::signal::Signal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InstanceId(pub u64);

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InstanceId({})", self.0)
    }
}

impl InstanceId {
    // reserved range: instances outside the DataModel tree (game, world root)
    pub const RESERVED_START: u64 = u64::MAX - 1_000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EventKind {
    ChildAdded,
    ChildRemoved,
    DescendantAdded,
    DescendantRemoving,
    AncestryChanged,
    AttributeChanged,
    Destroying,
    PlayerAdded,
    PlayerRemoving,
    Heartbeat,
    Stepped,
    RenderStepped,
    PreSimulation,
    PostSimulation,
    CharacterAdded,
    ServiceAdded,
}

impl EventKind {
    pub const fn name(self) -> &'static str {
        match self {
            EventKind::ChildAdded => "ChildAdded",
            EventKind::ChildRemoved => "ChildRemoved",
            EventKind::DescendantAdded => "DescendantAdded",
            EventKind::DescendantRemoving => "DescendantRemoving",
            EventKind::AncestryChanged => "AncestryChanged",
            EventKind::AttributeChanged => "AttributeChanged",
            EventKind::Destroying => "Destroying",
            EventKind::PlayerAdded => "PlayerAdded",
            EventKind::PlayerRemoving => "PlayerRemoving",
            EventKind::Heartbeat => "Heartbeat",
            EventKind::Stepped => "Stepped",
            EventKind::RenderStepped => "RenderStepped",
            EventKind::PreSimulation => "PreSimulation",
            EventKind::PostSimulation => "PostSimulation",
            EventKind::CharacterAdded => "CharacterAdded",
            EventKind::ServiceAdded => "ServiceAdded",
        }
    }

    pub const fn all() -> &'static [EventKind] {
        &[
            EventKind::ChildAdded,
            EventKind::ChildRemoved,
            EventKind::DescendantAdded,
            EventKind::DescendantRemoving,
            EventKind::AncestryChanged,
            EventKind::AttributeChanged,
            EventKind::Destroying,
            EventKind::PlayerAdded,
            EventKind::PlayerRemoving,
            EventKind::Heartbeat,
            EventKind::Stepped,
            EventKind::RenderStepped,
            EventKind::PreSimulation,
            EventKind::PostSimulation,
            EventKind::CharacterAdded,
            EventKind::ServiceAdded,
        ]
    }
}

#[derive(Debug)]
pub struct InstanceData {
    pub name: String,
    pub class_name: String,
    pub parent: Option<InstanceId>,
    pub children: Vec<InstanceId>,
    pub attributes: HashMap<String, AttributeValue>,
    pub properties: HashMap<String, AttributeValue>,
    pub signals: HashMap<EventKind, Signal>,
    // events declared only in the generated table, and topics subscribed by name
    pub named_signals: HashMap<String, Signal>,
    pub tags: BTreeSet<String>,
    pub source: Option<String>,
    // set by Destroy; slot is freed right after
    pub destroyed: bool,
}

impl InstanceData {
    pub fn new(name: impl Into<String>, class_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            class_name: class_name.into(),
            parent: None,
            children: Vec::new(),
            attributes: HashMap::new(),
            properties: HashMap::new(),
            signals: HashMap::new(),
            named_signals: HashMap::new(),
            tags: BTreeSet::new(),
            source: None,
            destroyed: false,
        }
    }

    pub fn is_a(&self, class_name: &str) -> bool {
        crate::classes::is_a(&self.class_name, class_name)
    }

    pub fn is_script_container(&self) -> bool {
        crate::classes::descriptor(&self.class_name)
            .map(|c| c.kind == crate::classes::ClassKind::ScriptContainer)
            .unwrap_or(false)
    }

    pub fn is_service(&self) -> bool {
        crate::classes::descriptor(&self.class_name)
            .map(|c| c.is_service())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_names_are_unique() {
        let kinds = EventKind::all();
        for (i, k) in kinds.iter().enumerate() {
            for other in &kinds[i + 1..] {
                assert_ne!(k.name(), other.name());
            }
        }
    }

    #[test]
    fn instance_data_reports_is_a() {
        let part = InstanceData::new("P", "Part");
        assert!(part.is_a("BasePart"));
        assert!(part.is_a("Instance"));
        assert!(!part.is_a("Model"));
    }
}
