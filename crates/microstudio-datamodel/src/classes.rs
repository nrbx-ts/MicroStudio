// hand-written table; a generated one can replace it later

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    Abstract,
    Concrete,
    Service,
    ScriptContainer,
    Player,
}

#[derive(Debug, Clone, Copy)]
pub struct ClassDescriptor {
    pub name: &'static str,
    pub superclass: Option<&'static str>,
    pub kind: ClassKind,
}

impl ClassDescriptor {
    pub fn is_creatable(&self) -> bool {
        matches!(self.kind, ClassKind::Concrete | ClassKind::ScriptContainer)
    }

    pub fn is_service(&self) -> bool {
        self.kind == ClassKind::Service
    }
}

macro_rules! classes {
    ($($name:literal : $super:expr, $kind:expr;)*) => {
        const CLASSES: &[ClassDescriptor] = &[
            $(ClassDescriptor { name: $name, superclass: $super, kind: $kind },)*
        ];
    };
}

use ClassKind::{Abstract, Concrete, ScriptContainer, Service};

classes! {
    "Instance": None, Abstract;
    "PVInstance": Some("Instance"), Abstract;
    "BasePart": Some("PVInstance"), Abstract;
    "Model": Some("PVInstance"), Concrete;
    "Workspace": Some("Model"), Service;
    "Folder": Some("Instance"), Concrete;
    "Part": Some("BasePart"), Concrete;
    "MeshPart": Some("BasePart"), Concrete;
    "SpawnLocation": Some("BasePart"), Concrete;
    "Humanoid": Some("Instance"), Concrete;
    "LuaSourceContainer": Some("Instance"), Abstract;
    "BaseScript": Some("LuaSourceContainer"), Abstract;
    "Script": Some("BaseScript"), ScriptContainer;
    "LocalScript": Some("BaseScript"), ScriptContainer;
    "ModuleScript": Some("LuaSourceContainer"), ScriptContainer;
    "Player": Some("Instance"), ClassKind::Player;
    "Players": Some("Instance"), Service;
    "RunService": Some("Instance"), Service;
    "ReplicatedStorage": Some("Instance"), Service;
    "ServerStorage": Some("Instance"), Service;
    "ServerScriptService": Some("Instance"), Service;
    "CollectionService": Some("Instance"), Service;
    "StarterPlayer": Some("Instance"), Service;
    "StarterPlayerScripts": Some("Instance"), Service;
    "StarterCharacterScripts": Some("Instance"), Service;
    "StarterGui": Some("Instance"), Service;
    "Lighting": Some("Instance"), Service;
    "Teams": Some("Instance"), Service;
    "SoundService": Some("Instance"), Service;
    "TestService": Some("Instance"), Service;
    "DataModel": Some("Instance"), Abstract;

    // objects the simulated services hand back; scripts cannot create these either
    "GlobalDataStore": Some("Instance"), Abstract;
    "DataStore": Some("GlobalDataStore"), Abstract;
    "OrderedDataStore": Some("DataStore"), Abstract;
    "MemoryStoreQueue": Some("Instance"), Abstract;
    "MemoryStoreSortedMap": Some("Instance"), Abstract;
    "Tween": Some("Instance"), Abstract;
}

pub fn descriptor(name: &str) -> Option<&'static ClassDescriptor> {
    CLASSES
        .iter()
        .find(|c| c.name == name)
        .or_else(|| generated().iter().find(|c| c.name == name))
}

// every other Roblox service, from the generated table: present, not creatable
fn generated() -> &'static [ClassDescriptor] {
    static GENERATED: std::sync::OnceLock<Vec<ClassDescriptor>> = std::sync::OnceLock::new();

    GENERATED.get_or_init(|| {
        crate::api::services()
            .iter()
            .filter(|service| descriptor_hand_written(service.name).is_none())
            .map(|service| ClassDescriptor {
                name: service.name,
                superclass: Some(service.superclass),
                kind: ClassKind::Service,
            })
            .collect()
    })
}

fn descriptor_hand_written(name: &str) -> Option<&'static ClassDescriptor> {
    CLASSES.iter().find(|c| c.name == name)
}

#[inline]
pub fn is_known(name: &str) -> bool {
    descriptor(name).is_some()
}

pub fn is_a(class_name: &str, ancestor: &str) -> bool {
    if class_name == ancestor {
        return true;
    }
    let mut current = descriptor(class_name);
    while let Some(c) = current {
        match c.superclass {
            Some(parent) if parent == ancestor => return true,
            Some(parent) => current = descriptor(parent),
            None => return false,
        }
    }
    false
}

// declaration order: superclasses before subclasses
pub fn all() -> &'static [ClassDescriptor] {
    CLASSES
}

pub fn service_names() -> impl Iterator<Item = &'static str> {
    CLASSES.iter().filter(|c| c.is_service()).map(|c| c.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitively_reports_is_a() {
        assert!(is_a("Part", "Instance"));
        assert!(is_a("Part", "BasePart"));
        assert!(is_a("Part", "PVInstance"));
        assert!(is_a("Part", "Part"));
        assert!(!is_a("Part", "Model"));
        assert!(is_a("Workspace", "Model"));
        assert!(is_a("Script", "LuaSourceContainer"));
    }

    #[test]
    fn unknown_classes_are_not_a() {
        assert!(!is_a("NotAClass", "Instance"));
        assert!(is_a("Instance", "Instance"));
    }

    #[test]
    fn creatability_follows_kind() {
        assert!(descriptor("Part").unwrap().is_creatable());
        assert!(descriptor("Script").unwrap().is_creatable());
        assert!(!descriptor("Instance").unwrap().is_creatable());
        assert!(!descriptor("BasePart").unwrap().is_creatable());
        assert!(!descriptor("Players").unwrap().is_creatable());
    }

    #[test]
    fn every_superclass_exists() {
        for class in all() {
            if let Some(parent) = class.superclass {
                assert!(is_known(parent), "{} has unknown superclass {parent}", class.name);
            }
        }
    }

    #[test]
    fn declared_once() {
        for (i, class) in all().iter().enumerate() {
            for other in &all()[i + 1..] {
                assert_ne!(class.name, other.name);
            }
        }
    }
}
