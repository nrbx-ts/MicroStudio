use std::collections::HashMap;

use microstudio_types::{Color3, Vector3};

use crate::attributes::AttributeValue;
use crate::classes::{self, ClassKind};
use crate::instance::{EventKind, InstanceData, InstanceId};
use crate::signal::{EventArg, Signal, SignalId};

// mutations collect these; caller dispatches after the borrow drops, so listeners may re-enter
#[derive(Debug)]
pub struct PendingEvent {
    pub signal: Signal,
    pub args: Vec<EventArg>,
}

type RawEvent = (InstanceId, EventKind, Vec<EventArg>);

#[derive(Debug, thiserror::Error)]
pub enum DataModelError {
    #[error("instance is no longer available (it was destroyed)")]
    DeadInstance,
    #[error("'{0}' is not a valid class name")]
    UnknownClass(String),
    #[error("'{class}' cannot be created with Instance.new")]
    NotCreatable { class: String },
    #[error("attempt to set an instance ({child}) as a descendant of itself")]
    CyclicParent { child: InstanceId },
    #[error("'{name}' is a service and cannot be reparented")]
    ServiceReparent { name: String },
    #[error("'{name}' is not a member of {class}")]
    UnknownProperty { name: String, class: String },
    #[error("cannot assign {actual} to {class}.{name}")]
    PropertyType {
        class: String,
        name: String,
        actual: &'static str,
    },
}

#[derive(Debug)]
pub struct DataModel {
    instances: Vec<Option<InstanceData>>,
    next_signal_id: u64,
    game: InstanceId,
}

impl Default for DataModel {
    fn default() -> Self {
        Self::new()
    }
}

impl DataModel {
    // no services yet: they materialise lazily like Roblox
    pub fn new() -> Self {
        let mut model = DataModel {
            instances: Vec::new(),
            next_signal_id: 1,
            game: InstanceId(0),
        };
        let game = model.alloc(InstanceData::new("game", "DataModel"));
        model.game = game;
        model
    }

    #[inline]
    pub fn game(&self) -> InstanceId {
        self.game
    }

    fn index(id: InstanceId) -> Result<usize, DataModelError> {
        if id.0 == 0 {
            return Err(DataModelError::DeadInstance);
        }
        usize::try_from(id.0 - 1).map_err(|_| DataModelError::DeadInstance)
    }

    pub fn contains(&self, id: InstanceId) -> bool {
        Self::index(id)
            .ok()
            .and_then(|i| self.instances.get(i))
            .is_some_and(|slot| slot.is_some())
    }

    pub fn get(&self, id: InstanceId) -> Result<&InstanceData, DataModelError> {
        Self::index(id)
            .ok()
            .and_then(|i| self.instances.get(i))
            .and_then(|slot| slot.as_ref())
            .ok_or(DataModelError::DeadInstance)
    }

    pub fn get_mut(&mut self, id: InstanceId) -> Result<&mut InstanceData, DataModelError> {
        Self::index(id)
            .ok()
            .and_then(|i| self.instances.get_mut(i))
            .and_then(|slot| slot.as_mut())
            .ok_or(DataModelError::DeadInstance)
    }

    pub fn name_of(&self, id: InstanceId) -> Option<&str> {
        self.get(id).ok().map(|d| d.name.as_str())
    }

    pub fn class_of(&self, id: InstanceId) -> Option<&str> {
        self.get(id).ok().map(|d| d.class_name.as_str())
    }

    pub fn parent_of(&self, id: InstanceId) -> Option<InstanceId> {
        self.get(id).ok().and_then(|d| d.parent)
    }

    pub fn children_of(&self, id: InstanceId) -> &[InstanceId] {
        self.get(id).map(|d| d.children.as_slice()).unwrap_or(&[])
    }

    // allocation order
    pub fn all_ids(&self) -> Vec<InstanceId> {
        self.instances
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| InstanceId(i as u64 + 1)))
            .collect()
    }

    fn alloc(&mut self, data: InstanceData) -> InstanceId {
        self.instances.push(Some(data));
        InstanceId(self.instances.len() as u64)
    }

    // mirrors Instance.new: rejects abstract and service classes
    pub fn create_instance(&mut self, class_name: &str) -> Result<InstanceId, DataModelError> {
        self.create(class_name, false)
    }

    // bypasses creatability: runtime builds what scripts cannot (Player, Character)
    pub fn create_internal(&mut self, class_name: &str) -> Result<InstanceId, DataModelError> {
        self.create(class_name, true)
    }

    fn create(&mut self, class_name: &str, internal: bool) -> Result<InstanceId, DataModelError> {
        let descriptor =
            classes::descriptor(class_name).ok_or_else(|| DataModelError::UnknownClass(class_name.to_string()))?;
        if !internal && !descriptor.is_creatable() {
            return Err(DataModelError::NotCreatable {
                class: class_name.to_string(),
            });
        }

        let mut data = InstanceData::new(class_name, class_name);
        data.properties = default_properties(class_name);
        Ok(self.alloc(data))
    }

    fn next_signal_id(&mut self) -> SignalId {
        let id = self.next_signal_id;
        self.next_signal_id += 1;
        SignalId(id)
    }

    // detached signal, e.g. per-tag CollectionService signals
    pub fn new_signal(&mut self, name: &str) -> Signal {
        Signal::new(self.next_signal_id(), name)
    }

    pub fn signal(&mut self, id: InstanceId, kind: EventKind) -> Result<Signal, DataModelError> {
        let existing = self.get(id)?.signals.get(&kind).cloned();
        if let Some(signal) = existing {
            return Ok(signal);
        }
        let signal = self.new_signal(kind.name());
        self.get_mut(id)?.signals.insert(kind, signal.clone());
        Ok(signal)
    }

    // an event that only the generated table knows about, e.g. GuiService.MenuOpened
    // an event the generated table declares, or a named topic such as a messaging topic
    pub fn named_signal(
        &mut self,
        id: InstanceId,
        name: &str,
    ) -> Result<Signal, DataModelError> {
        let existing = self.get(id)?.named_signals.get(name).cloned();
        if let Some(signal) = existing {
            return Ok(signal);
        }
        let signal = self.new_signal(name);
        self.get_mut(id)?
            .named_signals
            .insert(name.to_string(), signal.clone());
        Ok(signal)
    }

    fn materialize(&mut self, raw: Vec<RawEvent>) -> Vec<PendingEvent> {
        raw.into_iter()
            .filter_map(|(id, kind, args)| {
                self.signal(id, kind).ok().map(|signal| PendingEvent { signal, args })
            })
            .collect()
    }

    pub fn get_children(&self, id: InstanceId) -> Vec<InstanceId> {
        self.children_of(id).to_vec()
    }

    // depth-first pre-order, like GetDescendants
    pub fn get_descendants(&self, id: InstanceId) -> Vec<InstanceId> {
        let mut out = Vec::new();
        self.collect_descendants(id, &mut out);
        out
    }

    fn collect_descendants(&self, id: InstanceId, out: &mut Vec<InstanceId>) {
        for child in self.children_of(id) {
            out.push(*child);
            self.collect_descendants(*child, out);
        }
    }

    pub fn ancestors(&self, id: InstanceId) -> Vec<InstanceId> {
        let mut out = Vec::new();
        let mut current = self.parent_of(id);
        while let Some(parent) = current {
            out.push(parent);
            current = self.parent_of(parent);
        }
        out
    }

    pub fn find_first_child(&self, id: InstanceId, name: &str) -> Option<InstanceId> {
        self.find_first(id, name, false, None)
    }

    pub fn find_first_child_of_class(&self, id: InstanceId, class: &str) -> Option<InstanceId> {
        self.children_of(id)
            .iter()
            .copied()
            .find(|c| self.class_of(*c) == Some(class))
    }

    pub fn find_first_child_which_is_a(&self, id: InstanceId, class: &str) -> Option<InstanceId> {
        self.children_of(id)
            .iter()
            .copied()
            .find(|c| self.is_a(*c, class))
    }

    pub fn find_first_descendant(&self, id: InstanceId, name: &str) -> Option<InstanceId> {
        self.find_first(id, name, true, None)
    }

    fn find_first(
        &self,
        id: InstanceId,
        name: &str,
        recursive: bool,
        class: Option<&str>,
    ) -> Option<InstanceId> {
        for child in self.children_of(id) {
            let matches = self.name_of(*child) == Some(name)
                && class.map_or(true, |c| self.is_a(*child, c));
            if matches {
                return Some(*child);
            }
            if recursive {
                if let Some(found) = self.find_first(*child, name, true, class) {
                    return Some(found);
                }
            }
        }
        None
    }

    pub fn is_a(&self, id: InstanceId, class: &str) -> bool {
        self.get(id).map(|d| d.is_a(class)).unwrap_or(false)
    }

    pub fn is_ancestor_of(&self, id: InstanceId, candidate_ancestor: InstanceId) -> bool {
        if id == candidate_ancestor {
            return true;
        }
        self.ancestors(id).contains(&candidate_ancestor)
    }

    pub fn path(&self, id: InstanceId, include_game: bool) -> String {
        let mut parts = Vec::new();
        let mut current = Some(id);
        while let Some(node) = current {
            if node == self.game {
                if include_game {
                    parts.push("game".to_string());
                }
                break;
            }
            parts.push(self.name_of(node).unwrap_or("<destroyed>").to_string());
            current = self.parent_of(node);
        }
        parts.reverse();
        parts.join(".")
    }

    pub fn set_name(&mut self, id: InstanceId, name: &str) -> Result<(), DataModelError> {
        self.get_mut(id)?.name = name.to_string();
        Ok(())
    }

    // None reparents to top level
    // event order: ChildRemoved, DescendantRemoving, AncestryChanged, ChildAdded, DescendantAdded
    pub fn set_parent(
        &mut self,
        child: InstanceId,
        parent: Option<InstanceId>,
    ) -> Result<Vec<PendingEvent>, DataModelError> {
        let old_parent = self.get(child)?.parent;
        if old_parent == parent {
            return Ok(Vec::new());
        }
        if self.get(child)?.is_service() {
            return Err(DataModelError::ServiceReparent {
                name: self.name_of(child).unwrap_or("<unknown>").to_string(),
            });
        }
        if let Some(parent) = parent {
            self.get(parent)?;
            if child == self.game {
                return Err(DataModelError::ServiceReparent {
                    name: "game".to_string(),
                });
            }
            if self.is_ancestor_of(parent, child) {
                return Err(DataModelError::CyclicParent { child });
            }
        }

        let subtree = {
            let mut subtree = vec![child];
            subtree.extend(self.get_descendants(child));
            subtree
        };
        let mut raw: Vec<RawEvent> = Vec::new();

        if let Some(old) = old_parent {
            self.get_mut(old)?.children.retain(|c| *c != child);
            raw.push((old, EventKind::ChildRemoved, vec![EventArg::Instance(child)]));
            for ancestor in self.ancestors(old) {
                for node in &subtree {
                    raw.push((
                        ancestor,
                        EventKind::DescendantRemoving,
                        vec![EventArg::Instance(*node)],
                    ));
                }
            }
        }

        self.get_mut(child)?.parent = parent;

        for node in &subtree {
            raw.push((
                *node,
                EventKind::AncestryChanged,
                vec![
                    EventArg::Instance(*node),
                    EventArg::Parent(parent),
                ],
            ));
        }

        if let Some(new_parent) = parent {
            let insert_at = self.get(new_parent)?.children.len();
            self.get_mut(new_parent)?.children.insert(insert_at, child);
            raw.push((
                new_parent,
                EventKind::ChildAdded,
                vec![EventArg::Instance(child)],
            ));
            for ancestor in self.ancestors(new_parent) {
                for node in &subtree {
                    raw.push((
                        ancestor,
                        EventKind::DescendantAdded,
                        vec![EventArg::Instance(*node)],
                    ));
                }
            }
        }

        Ok(self.materialize(raw))
    }

    pub fn destroy(&mut self, id: InstanceId) -> Result<Vec<PendingEvent>, DataModelError> {
        if !self.contains(id) {
            // Roblox tolerates double destroy
            return Ok(Vec::new());
        }
        if id == self.game {
            return Err(DataModelError::ServiceReparent {
                name: "game".to_string(),
            });
        }

        let subtree = {
            let mut subtree = vec![id];
            subtree.extend(self.get_descendants(id));
            subtree
        };
        let parent = self.parent_of(id);

        let mut raw: Vec<RawEvent> = Vec::new();
        if let Some(parent) = parent {
            raw.push((parent, EventKind::ChildRemoved, vec![EventArg::Instance(id)]));
        }
        // deepest-first, like Roblox teardown
        for node in subtree.iter().rev() {
            raw.push((*node, EventKind::Destroying, Vec::new()));
        }

        let events = self.materialize(raw);

        // detach before freeing: traversal never sees half-removed nodes
        if let Some(parent) = parent {
            if let Ok(index) = Self::index(parent) {
                if let Some(slot) = self.instances.get_mut(index).and_then(|s| s.as_mut()) {
                    slot.children.retain(|c| *c != id);
                }
            }
        }
        for node in &subtree {
            if let Ok(index) = Self::index(*node) {
                if let Some(slot) = self.instances.get_mut(index) {
                    *slot = None;
                }
            }
        }
        Ok(events)
    }

    // Archivable == false clones to None, like Roblox
    pub fn clone_instance(
        &mut self,
        id: InstanceId,
        deep: bool,
    ) -> Result<Option<InstanceId>, DataModelError> {
        let source = self.get(id)?;
        if source.properties.get("Archivable") == Some(&AttributeValue::Bool(false)) {
            return Ok(None);
        }
        let class_name = source.class_name.clone();
        let name = source.name.clone();
        let properties = source.properties.clone();
        let attributes = source.attributes.clone();
        let tags = source.tags.clone();
        let source_code = source.source.clone();
        let child_ids = source.children.clone();

        let copy = self.alloc({
            let mut data = InstanceData::new(name, class_name);
            data.properties = properties;
            data.attributes = attributes;
            data.tags = tags;
            data.source = source_code;
            data
        });

        if deep {
            for child in child_ids {
                if let Some(child_copy) = self.clone_instance(child, true)? {
                    self.attach_silently(child_copy, copy)?;
                }
            }
        }
        Ok(Some(copy))
    }

    // no events: Clone builds a detached subtree
    fn attach_silently(&mut self, child: InstanceId, parent: InstanceId) -> Result<(), DataModelError> {
        self.get_mut(child)?.parent = Some(parent);
        self.get_mut(parent)?.children.push(child);
        Ok(())
    }

    pub fn get_attribute(&self, id: InstanceId, name: &str) -> Option<AttributeValue> {
        self.get(id).ok()?.attributes.get(name).cloned()
    }

    pub fn get_attributes(&self, id: InstanceId) -> Vec<(String, AttributeValue)> {
        match self.get(id) {
            Ok(data) => {
                let mut items: Vec<_> = data
                    .attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                items
            }
            Err(_) => Vec::new(),
        }
    }

    pub fn set_attribute(
        &mut self,
        id: InstanceId,
        name: &str,
        value: Option<AttributeValue>,
    ) -> Result<Vec<PendingEvent>, DataModelError> {
        match value {
            Some(value) => {
                self.get_mut(id)?.attributes.insert(name.to_string(), value.clone());
                Ok(self.materialize(vec![(
                    id,
                    EventKind::AttributeChanged,
                    vec![EventArg::Text(name.to_string()), EventArg::Attribute(name.to_string(), value)],
                )]))
            }
            None => {
                self.get_mut(id)?.attributes.remove(name);
                Ok(Vec::new())
            }
        }
    }

    pub fn get_property(&self, id: InstanceId, name: &str) -> Option<AttributeValue> {
        self.get(id).ok()?.properties.get(name).cloned()
    }

    // Name/Parent live on the tree and are rejected here; class must declare the property
    pub fn set_property(
        &mut self,
        id: InstanceId,
        name: &str,
        value: AttributeValue,
    ) -> Result<(), DataModelError> {
        let class = self.get(id)?.class_name.clone();
        if name == "Name" || name == "Parent" {
            return Err(DataModelError::UnknownProperty {
                name: name.to_string(),
                class,
            });
        }
        let expected = property_type(&class, name).ok_or_else(|| DataModelError::UnknownProperty {
            name: name.to_string(),
            class: class.clone(),
        })?;
        if !expected.matches(&value) {
            return Err(DataModelError::PropertyType {
                class,
                name: name.to_string(),
                actual: value.type_name(),
            });
        }
        self.get_mut(id)?.properties.insert(name.to_string(), value);
        Ok(())
    }

    pub fn get_service(&mut self, name: &str) -> Result<InstanceId, DataModelError> {
        if let Some(existing) = self.find_first_child(self.game, name) {
            return Ok(existing);
        }
        let descriptor =
            classes::descriptor(name).ok_or_else(|| DataModelError::UnknownClass(name.to_string()))?;
        if !descriptor.is_service() {
            return Err(DataModelError::UnknownClass(name.to_string()));
        }
        let data = InstanceData::new(name, name);
        let id = self.alloc(data);
        self.get_mut(self.game)?.children.push(id);
        self.get_mut(id)?.parent = Some(self.game);
        Ok(id)
    }

    pub fn has_service(&self, name: &str) -> bool {
        self.find_first_child(self.game, name).is_some()
    }

    // up-front creation so service ids are stable from the first script
    pub fn bootstrap_default_services(&mut self) -> Result<Vec<(String, InstanceId)>, DataModelError> {
        let mut out = Vec::new();
        for name in DEFAULT_SERVICES {
            let id = self.get_service(name)?;
            out.push((name.to_string(), id));
        }
        Ok(out)
    }
}

pub const DEFAULT_SERVICES: &[&str] = &[
    "Workspace",
    "Players",
    "ReplicatedStorage",
    "ServerStorage",
    "ServerScriptService",
    "CollectionService",
    "StarterPlayer",
    "StarterGui",
    "Lighting",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyType {
    Bool,
    Number,
    String,
    Vector2,
    Vector3,
    CFrame,
    Color3,
    BrickColor,
    Instance,
    Enum,
    // declared but not modelled: accepts anything, defaults to nil
    Any,
}

impl PropertyType {
    fn matches(self, value: &AttributeValue) -> bool {
        if self == PropertyType::Any {
            return true;
        }
        matches!(
            (self, value),
            (PropertyType::Bool, AttributeValue::Bool(_))
                | (PropertyType::Number, AttributeValue::Number(_))
                | (PropertyType::String, AttributeValue::String(_))
                | (PropertyType::Vector2, AttributeValue::Vector2(_))
                | (PropertyType::Vector3, AttributeValue::Vector3(_))
                | (PropertyType::CFrame, AttributeValue::CFrame(_))
                | (PropertyType::Color3, AttributeValue::Color3(_))
                | (PropertyType::BrickColor, AttributeValue::BrickColor(_))
                | (PropertyType::Instance, AttributeValue::Instance(_))
                | (PropertyType::Enum, AttributeValue::String(_))
        )
    }
}

// absent means unimplemented, not accepted: typos surface loudly
pub fn property_type(class: &str, name: &str) -> Option<PropertyType> {
    hand_written_property_type(class, name).or_else(|| {
        // a service declares its whole surface, so every property it has is real
        let member = crate::api::member(class, name)?;
        if member.kind != crate::api::MemberKind::Property {
            return None;
        }
        Some(declared_property_type(member.type_name))
    })
}

// roblox documents a few properties as on until a place turns them off; every other
// unset property starts at the zero value its type implies
pub fn default_property_value(class: &str, name: &str) -> Option<AttributeValue> {
    let on = matches!(
        (class, name),
        ("HttpService", "HttpEnabled")
            | ("Players", "CharacterAutoLoads")
            | ("Lighting", "GlobalShadows")
            | ("Lighting", "Outlines")
    );
    on.then_some(AttributeValue::Bool(true))
}

// the dump's type names, mapped onto what the runtime can store
fn declared_property_type(name: &str) -> PropertyType {
    match name {
        "bool" => PropertyType::Bool,
        "number" => PropertyType::Number,
        "string" => PropertyType::String,
        "vector2" => PropertyType::Vector2,
        "vector3" => PropertyType::Vector3,
        "cframe" => PropertyType::CFrame,
        "color3" => PropertyType::Color3,
        "brickcolor" => PropertyType::BrickColor,
        "instance" => PropertyType::Instance,
        "enum" => PropertyType::Enum,
        _ => PropertyType::Any,
    }
}

fn hand_written_property_type(class: &str, name: &str) -> Option<PropertyType> {
    use PropertyType::*;

    // walk the class chain so Part inherits BasePart
    let mut current = Some(class);
    while let Some(klass) = current {
        let found = match (klass, name) {
            (_, "Archivable") => Some(Bool),
            ("Instance", _) => None,
            ("PVInstance", "CFrame") => Some(CFrame),
            ("PVInstance", "Position") => Some(Vector3),
            ("PVInstance", "Orientation") => Some(Vector3),
            ("BasePart", "Anchored") => Some(Bool),
            ("BasePart", "CanCollide") => Some(Bool),
            ("BasePart", "Size") => Some(Vector3),
            ("BasePart", "Color") => Some(Color3),
            ("BasePart", "Transparency") => Some(Number),
            ("BasePart", "Material") => Some(Enum),
            ("BasePart", "Shape") => Some(Enum),
            ("BasePart", "Massless") => Some(Bool),
            ("BasePart", "Locked") => Some(Bool),
            ("BasePart", "CollisionGroup") => Some(String),
            ("Model", "PrimaryPart") => Some(Instance),
            ("Folder", _) => None,
            ("LuaSourceContainer", "Source") => Some(String),
            ("BaseScript", "Enabled") => Some(Bool),
            ("BaseScript", "Disabled") => Some(Bool),
            ("Script", "RunContext") => Some(Enum),
            ("Player", "UserId") => Some(Number),
            ("Player", "DisplayName") => Some(String),
            ("Player", "AccountAge") => Some(Number),
            ("Player", "Character") => Some(Instance),
            ("Humanoid", "Health") => Some(Number),
            ("Humanoid", "MaxHealth") => Some(Number),
            ("Humanoid", "WalkSpeed") => Some(Number),
            ("Humanoid", "JumpPower") => Some(Number),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        current = classes::descriptor(klass).and_then(|c| c.superclass);
    }
    None
}

fn default_properties(class: &str) -> HashMap<String, AttributeValue> {
    let mut map = HashMap::new();
    map.insert("Archivable".into(), AttributeValue::Bool(true));

    if classes::is_a(class, "BasePart") {
        map.insert(
            "Size".into(),
            AttributeValue::Vector3(Vector3::new(4.0, 1.0, 2.0)),
        );
        map.insert("Position".into(), AttributeValue::Vector3(Vector3::zero()));
        map.insert(
            "CFrame".into(),
            AttributeValue::CFrame(microstudio_types::CFrame::identity()),
        );
        map.insert("Anchored".into(), AttributeValue::Bool(false));
        map.insert("CanCollide".into(), AttributeValue::Bool(true));
        map.insert("Transparency".into(), AttributeValue::Number(0.0));
        map.insert("Massless".into(), AttributeValue::Bool(false));
        map.insert("Locked".into(), AttributeValue::Bool(false));
        map.insert("Material".into(), AttributeValue::String("Plastic".into()));
        map.insert(
            "Color".into(),
            AttributeValue::Color3(Color3::from_rgb(163, 162, 165)),
        );
    }
    if classes::is_a(class, "LuaSourceContainer") {
        map.insert("Source".into(), AttributeValue::String(String::new()));
    }
    if classes::is_a(class, "BaseScript") {
        map.insert("Enabled".into(), AttributeValue::Bool(true));
        map.insert("Disabled".into(), AttributeValue::Bool(false));
    }
    map
}

pub fn class_kind(class: &str) -> Option<ClassKind> {
    classes::descriptor(class).map(|c| c.kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> DataModel {
        let mut model = DataModel::new();
        model.bootstrap_default_services().unwrap();
        model
    }

    #[test]
    fn services_are_created_lazily_and_once() {
        let mut model = DataModel::new();
        assert!(!model.has_service("Workspace"));
        let ws = model.get_service("Workspace").unwrap();
        assert_eq!(ws, model.get_service("Workspace").unwrap());
        assert_eq!(model.name_of(ws), Some("Workspace"));
        assert_eq!(model.parent_of(ws), Some(model.game()));
        assert!(model.get_service("NotAService").is_err());
    }

    #[test]
    fn instance_new_applies_class_defaults() {
        let mut model = model();
        let part = model.create_instance("Part").unwrap();
        assert_eq!(model.class_of(part), Some("Part"));
        assert!(model.is_a(part, "BasePart"));
        assert_eq!(
            model.get_property(part, "Size"),
            Some(AttributeValue::Vector3(Vector3::new(4.0, 1.0, 2.0)))
        );
        assert!(model.create_instance("BasePart").is_err());
        assert!(model.create_instance("Nope").is_err());
    }

    #[test]
    fn parenting_maintains_the_tree() {
        let mut model = model();
        let ws = model.get_service("Workspace").unwrap();
        let folder = model.create_instance("Folder").unwrap();
        model.set_name(folder, "Train").unwrap();
        model.set_parent(folder, Some(ws)).unwrap();

        let part = model.create_instance("Part").unwrap();
        model.set_parent(part, Some(folder)).unwrap();

        assert_eq!(model.find_first_child(ws, "Train"), Some(folder));
        assert_eq!(model.find_first_descendant(ws, "Part"), Some(part));
        assert_eq!(model.path(part, false), "Workspace.Train.Part");
        assert_eq!(model.get_children(ws), vec![folder]);
        assert_eq!(model.get_descendants(ws), vec![folder, part]);
    }

    #[test]
    fn parenting_fires_events_in_order() {
        let mut model = model();
        let ws = model.get_service("Workspace").unwrap();
        let part = model.create_instance("Part").unwrap();
        let folder = model.create_instance("Folder").unwrap();

        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        for (signal, tag) in [
            (model.signal(ws, EventKind::ChildAdded).unwrap(), "ws.ChildAdded"),
            (model.signal(ws, EventKind::ChildRemoved).unwrap(), "ws.ChildRemoved"),
            (model.signal(part, EventKind::AncestryChanged).unwrap(), "part.AncestryChanged"),
        ] {
            let log = log.clone();
            signal.connect(move |_| log.borrow_mut().push(tag.to_string()));
        }

        let events = model.set_parent(part, Some(folder)).unwrap();
        for event in events {
            event.signal.fire(&event.args);
        }
        assert_eq!(*log.borrow(), vec!["part.AncestryChanged"]);

        log.borrow_mut().clear();
        let events = model.set_parent(part, Some(ws)).unwrap();
        for event in events {
            event.signal.fire(&event.args);
        }
        assert_eq!(*log.borrow(), vec!["part.AncestryChanged", "ws.ChildAdded"]);
    }

    #[test]
    fn cyclic_parenting_is_rejected() {
        let mut model = model();
        let parent = model.create_instance("Folder").unwrap();
        let child = model.create_instance("Folder").unwrap();
        model.set_parent(child, Some(parent)).unwrap();
        assert!(matches!(
            model.set_parent(parent, Some(child)),
            Err(DataModelError::CyclicParent { .. })
        ));
    }

    #[test]
    fn services_cannot_be_reparented() {
        let mut model = model();
        let ws = model.get_service("Workspace").unwrap();
        let folder = model.create_instance("Folder").unwrap();
        assert!(matches!(
            model.set_parent(ws, Some(folder)),
            Err(DataModelError::ServiceReparent { .. })
        ));
    }

    #[test]
    fn destroy_removes_the_subtree_and_is_idempotent() {
        let mut model = model();
        let ws = model.get_service("Workspace").unwrap();
        let folder = model.create_instance("Folder").unwrap();
        let part = model.create_instance("Part").unwrap();
        model.set_parent(folder, Some(ws)).unwrap();
        model.set_parent(part, Some(folder)).unwrap();

        let events = model.destroy(folder).unwrap();
        assert!(!events.is_empty());
        assert!(!model.contains(folder));
        assert!(!model.contains(part));
        assert_eq!(model.get_children(ws), vec![]);
        assert!(model.destroy(folder).unwrap().is_empty());
    }

    #[test]
    fn destroyed_ids_are_never_reused() {
        let mut model = model();
        let a = model.create_instance("Part").unwrap();
        model.destroy(a).unwrap();
        let b = model.create_instance("Part").unwrap();
        assert_ne!(a, b);
        assert!(!model.contains(a));
        assert!(model.contains(b));
    }

    #[test]
    fn clone_copies_properties_and_children() {
        let mut model = model();
        let ws = model.get_service("Workspace").unwrap();
        let source = model.create_instance("Model").unwrap();
        model.set_name(source, "Train").unwrap();
        let part = model.create_instance("Part").unwrap();
        model.set_name(part, "Chassis").unwrap();
        model.set_parent(part, Some(source)).unwrap();
        model.set_parent(source, Some(ws)).unwrap();
        model
            .set_attribute(source, "speed", Some(AttributeValue::Number(12.0)))
            .unwrap();

        let copy = model.clone_instance(source, true).unwrap().unwrap();
        assert_eq!(model.name_of(copy), Some("Train"));
        assert_eq!(model.parent_of(copy), None);
        assert_eq!(
            model.get_attribute(copy, "speed"),
            Some(AttributeValue::Number(12.0))
        );
        assert_eq!(model.find_first_child(copy, "Chassis").is_some(), true);
        assert_ne!(copy, source);
    }

    #[test]
    fn unarchivable_instances_do_not_clone() {
        let mut model = model();
        let part = model.create_instance("Part").unwrap();
        model.set_property(part, "Archivable", AttributeValue::Bool(false)).unwrap();
        assert_eq!(model.clone_instance(part, true).unwrap(), None);
    }

    #[test]
    fn attributes_round_trip_and_fire() {
        let mut model = model();
        let part = model.create_instance("Part").unwrap();
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let signal = model.signal(part, EventKind::AttributeChanged).unwrap();
        let log2 = log.clone();
        signal.connect(move |args| log2.borrow_mut().push(args.to_vec()));

        let events = model
            .set_attribute(part, "speed", Some(AttributeValue::Number(5.0)))
            .unwrap();
        for event in events {
            event.signal.fire(&event.args);
        }
        assert_eq!(
            model.get_attribute(part, "speed"),
            Some(AttributeValue::Number(5.0))
        );
        assert_eq!(log.borrow().len(), 1);

        model.set_attribute(part, "speed", None).unwrap();
        assert_eq!(model.get_attribute(part, "speed"), None);
    }

    #[test]
    fn property_writes_are_type_checked() {
        let mut model = model();
        let part = model.create_instance("Part").unwrap();
        model
            .set_property(part, "Size".into(), AttributeValue::Vector3(Vector3::one()))
            .unwrap();
        assert!(matches!(
            model.set_property(part, "Size".into(), AttributeValue::Bool(true)),
            Err(DataModelError::PropertyType { .. })
        ));
        assert!(matches!(
            model.set_property(part, "NotAProperty".into(), AttributeValue::Bool(true)),
            Err(DataModelError::UnknownProperty { .. })
        ));
        assert!(matches!(
            model.set_property(part, "Name".into(), AttributeValue::String("x".into())),
            Err(DataModelError::UnknownProperty { .. })
        ));
    }

    #[test]
    fn inherited_properties_resolve_through_the_chain() {
        assert_eq!(property_type("Part", "Anchored"), Some(PropertyType::Bool));
        assert_eq!(property_type("Part", "Size"), Some(PropertyType::Vector3));
        assert_eq!(property_type("Script", "Source"), Some(PropertyType::String));
        assert_eq!(property_type("Script", "Enabled"), Some(PropertyType::Bool));
        assert_eq!(property_type("Part", "Source"), None);
    }

    #[test]
    fn a_collision_group_is_a_string_on_every_base_part() {
        assert_eq!(
            property_type("Part", "CollisionGroup"),
            Some(PropertyType::String)
        );
        assert_eq!(
            property_type("MeshPart", "CollisionGroup"),
            Some(PropertyType::String)
        );
        assert_eq!(property_type("Folder", "CollisionGroup"), None);
    }

    #[test]
    fn a_handful_of_properties_start_on() {
        assert_eq!(
            default_property_value("HttpService", "HttpEnabled"),
            Some(AttributeValue::Bool(true))
        );
        assert_eq!(
            default_property_value("Players", "CharacterAutoLoads"),
            Some(AttributeValue::Bool(true))
        );
        // everything else starts at the zero value its type implies
        assert_eq!(default_property_value("Part", "Anchored"), None);
        assert_eq!(default_property_value("HttpService", "SomethingElse"), None);
    }

    #[test]
    fn find_first_child_which_is_a_matches_subclasses() {
        let mut model = model();
        let ws = model.get_service("Workspace").unwrap();
        let folder = model.create_instance("Folder").unwrap();
        let part = model.create_instance("Part").unwrap();
        model.set_parent(folder, Some(ws)).unwrap();
        model.set_parent(part, Some(ws)).unwrap();
        assert_eq!(model.find_first_child_which_is_a(ws, "BasePart"), Some(part));
        assert_eq!(model.find_first_child_of_class(ws, "Folder"), Some(folder));
        assert_eq!(model.find_first_child_of_class(ws, "BasePart"), None);
    }
}
