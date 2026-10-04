use std::collections::HashMap;

use microstudio_datamodel::{
    DataModel, DataModelError, EventArg, InstanceId, PendingEvent, Signal,
};

// tags live on InstanceData::tags; this holds the per-tag added/removed signals
#[derive(Debug, Default)]
pub struct CollectionState {
    added: HashMap<String, Signal>,
    removed: HashMap<String, Signal>,
}

impl CollectionState {
    pub fn added_signal(
        &mut self,
        dm: &mut DataModel,
        tag: &str,
    ) -> Result<Signal, DataModelError> {
        Self::signal_for(dm, &mut self.added, tag, "GetInstanceAddedSignal")
    }

    pub fn removed_signal(
        &mut self,
        dm: &mut DataModel,
        tag: &str,
    ) -> Result<Signal, DataModelError> {
        Self::signal_for(dm, &mut self.removed, tag, "GetInstanceRemovedSignal")
    }

    fn signal_for(
        dm: &mut DataModel,
        map: &mut HashMap<String, Signal>,
        tag: &str,
        label: &'static str,
    ) -> Result<Signal, DataModelError> {
        if let Some(signal) = map.get(tag) {
            return Ok(signal.clone());
        }
        let signal = dm.new_signal(label);
        map.insert(tag.to_string(), signal.clone());
        Ok(signal)
    }
}

pub fn add_tag(
    dm: &mut DataModel,
    state: &mut CollectionState,
    instance: InstanceId,
    tag: &str,
) -> Result<Vec<PendingEvent>, DataModelError> {
    let inserted = dm.get_mut(instance)?.tags.insert(tag.to_string());
    if !inserted {
        return Ok(Vec::new());
    }
    let signal = state.added_signal(dm, tag)?;
    Ok(vec![PendingEvent {
        signal,
        args: vec![EventArg::Instance(instance), EventArg::Text(tag.to_string())],
    }])
}

pub fn remove_tag(
    dm: &mut DataModel,
    state: &mut CollectionState,
    instance: InstanceId,
    tag: &str,
) -> Result<Vec<PendingEvent>, DataModelError> {
    let removed = dm.get_mut(instance)?.tags.remove(tag);
    if !removed {
        return Ok(Vec::new());
    }
    let signal = state.removed_signal(dm, tag)?;
    Ok(vec![PendingEvent {
        signal,
        args: vec![EventArg::Instance(instance), EventArg::Text(tag.to_string())],
    }])
}

pub fn has_tag(dm: &DataModel, instance: InstanceId, tag: &str) -> bool {
    dm.get(instance)
        .map(|data| data.tags.contains(tag))
        .unwrap_or(false)
}

pub fn get_tags(dm: &DataModel, instance: InstanceId) -> Vec<String> {
    dm.get(instance)
        .map(|data| data.tags.iter().cloned().collect())
        .unwrap_or_default()
}

pub fn get_tagged(dm: &DataModel, tag: &str) -> Vec<InstanceId> {
    dm.all_ids()
        .into_iter()
        .filter(|id| has_tag(dm, *id, tag))
        .collect()
}

pub fn get_all_tags(dm: &DataModel) -> Vec<String> {
    let mut tags: Vec<String> = dm
        .all_ids()
        .into_iter()
        .flat_map(|id| {
            dm.get(id)
                .map(|data| data.tags.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

impl crate::world::World {
    pub fn add_tag(
        &mut self,
        instance: InstanceId,
        tag: &str,
    ) -> Result<Vec<PendingEvent>, DataModelError> {
        add_tag(&mut self.dm, &mut self.collection, instance, tag)
    }

    pub fn remove_tag(
        &mut self,
        instance: InstanceId,
        tag: &str,
    ) -> Result<Vec<PendingEvent>, DataModelError> {
        remove_tag(&mut self.dm, &mut self.collection, instance, tag)
    }

    pub fn collection_added_signal(&mut self, tag: &str) -> Result<Signal, DataModelError> {
        self.collection.added_signal(&mut self.dm, tag)
    }

    pub fn collection_removed_signal(&mut self, tag: &str) -> Result<Signal, DataModelError> {
        self.collection.removed_signal(&mut self.dm, tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagging_is_idempotent_and_reports_through_signals() {
        let mut world = crate::world::World::new();
        world.bootstrap().unwrap();
        let part = world.dm.create_instance("Part").unwrap();

        let signal = world.collection_added_signal("Checkpoint").unwrap();
        let hits = std::rc::Rc::new(std::cell::Cell::new(0));
        let hits2 = hits.clone();
        signal.connect(move |_| hits2.set(hits2.get() + 1));

        let events = world.add_tag(part, "Checkpoint").unwrap();
        for event in events {
            event.signal.fire(&event.args);
        }
        assert_eq!(hits.get(), 1);
        assert!(has_tag(&world.dm, part, "Checkpoint"));
        assert_eq!(get_tags(&world.dm, part), vec!["Checkpoint".to_string()]);

        assert!(world.add_tag(part, "Checkpoint").unwrap().is_empty());
        assert_eq!(hits.get(), 1);
    }

    #[test]
    fn remove_fires_removed_signal() {
        let mut world = crate::world::World::new();
        world.bootstrap().unwrap();
        let part = world.dm.create_instance("Part").unwrap();
        world.add_tag(part, "A").unwrap();

        let signal = world.collection_removed_signal("A").unwrap();
        let hits = std::rc::Rc::new(std::cell::Cell::new(0));
        let hits2 = hits.clone();
        signal.connect(move |_| hits2.set(hits2.get() + 1));

        let events = world.remove_tag(part, "A").unwrap();
        for event in events {
            event.signal.fire(&event.args);
        }
        assert_eq!(hits.get(), 1);
        assert!(!has_tag(&world.dm, part, "A"));
        assert!(world.remove_tag(part, "A").unwrap().is_empty());
    }

    #[test]
    fn get_tagged_and_get_all_tags_scan_the_tree() {
        let mut world = crate::world::World::new();
        world.bootstrap().unwrap();
        let a = world.dm.create_instance("Part").unwrap();
        let b = world.dm.create_instance("Folder").unwrap();
        world.add_tag(a, "Zed").unwrap();
        world.add_tag(b, "Alpha").unwrap();
        world.add_tag(b, "Zed").unwrap();

        assert_eq!(get_tagged(&world.dm, "Zed"), vec![a, b]);
        assert_eq!(
            get_all_tags(&world.dm),
            vec!["Alpha".to_string(), "Zed".to_string()]
        );
    }
}
