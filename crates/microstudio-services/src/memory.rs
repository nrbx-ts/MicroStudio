// MemoryStoreService state: session scoped, so it never reaches the state directory
//
// roblox drops memory store data when the last server of a session shuts down, and the
// runtime has exactly one session, so keeping this in memory matches the real lifetime.
// expiry is measured against the virtual clock, which makes it testable.

use std::collections::HashMap;

use serde_json::Value as Json;

// roblox caps a memory store key at 128 bytes
const MAX_KEY_BYTES: usize = 128;

#[derive(Debug, Clone)]
struct Entry {
    value: Json,
    // absolute virtual time the entry stops being visible
    expires: Option<f64>,
}

impl Entry {
    fn is_live(&self, now: f64) -> bool {
        self.expires.is_none_or(|at| at > now)
    }
}

#[derive(Debug, Clone)]
struct Queued {
    id: u64,
    value: Json,
    priority: f64,
    expires: Option<f64>,
}

impl Queued {
    fn is_live(&self, now: f64) -> bool {
        self.expires.is_none_or(|at| at > now)
    }
}

#[derive(Debug, Clone, Default)]
struct SortedMap {
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Clone, Default)]
struct Queue {
    items: Vec<Queued>,
    next_id: u64,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryStoreState {
    maps: HashMap<String, SortedMap>,
    queues: HashMap<String, Queue>,
}

// roblox orders a sorted map by value, so values need a total order across json types
fn rank(value: &Json) -> u8 {
    match value {
        Json::Number(_) => 0,
        Json::String(_) => 1,
        Json::Bool(_) => 2,
        Json::Array(_) => 3,
        Json::Object(_) => 4,
        Json::Null => 5,
    }
}

fn compare_values(left: &Json, right: &Json) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (left, right) {
        (Json::Number(a), Json::Number(b)) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(Ordering::Equal),
        (Json::String(a), Json::String(b)) => a.cmp(b),
        (Json::Bool(a), Json::Bool(b)) => a.cmp(b),
        _ => rank(left)
            .cmp(&rank(right))
            // equal ranks fall back to the text form, which is stable
            .then_with(|| left.to_string().cmp(&right.to_string())),
    }
}

impl MemoryStoreState {
    pub fn sorted_map_set(
        &mut self,
        name: &str,
        key: &str,
        value: Json,
        expires: Option<f64>,
    ) -> Result<(), String> {
        if key.len() > MAX_KEY_BYTES {
            return Err(format!(
                "key is {} bytes, the limit is {MAX_KEY_BYTES}",
                key.len()
            ));
        }
        self.maps
            .entry(name.to_string())
            .or_default()
            .entries
            .insert(
                key.to_string(),
                Entry {
                    value,
                    expires,
                },
            );
        Ok(())
    }

    pub fn sorted_map_get(&mut self, name: &str, key: &str, now: f64) -> Option<Json> {
        let map = self.maps.get_mut(name)?;
        match map.entries.get(key) {
            Some(entry) if entry.is_live(now) => Some(entry.value.clone()),
            // an expired entry is gone, not returned as nil
            Some(_) => {
                map.entries.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn sorted_map_remove(&mut self, name: &str, key: &str, now: f64) -> Option<Json> {
        let map = self.maps.get_mut(name)?;
        let entry = map.entries.remove(key)?;
        entry.is_live(now).then_some(entry.value)
    }

    // sorted by value; bounds are exclusive, as roblox documents them
    pub fn sorted_map_range(
        &mut self,
        name: &str,
        descending: bool,
        count: usize,
        lower: Option<&Json>,
        upper: Option<&Json>,
        now: f64,
    ) -> Vec<(String, Json)> {
        let Some(map) = self.maps.get_mut(name) else {
            return Vec::new();
        };
        map.entries.retain(|_, entry| entry.is_live(now));

        let mut items: Vec<(&String, &Json)> = map
            .entries
            .iter()
            .map(|(key, entry)| (key, &entry.value))
            .filter(|(_, value)| {
                lower.is_none_or(|bound| compare_values(value, bound).is_gt())
                    && upper.is_none_or(|bound| compare_values(value, bound).is_lt())
            })
            .collect();

        items.sort_by(|(left_key, left_value), (right_key, right_value)| {
            compare_values(left_value, right_value).then_with(|| left_key.cmp(right_key))
        });
        if descending {
            items.reverse();
        }

        items
            .into_iter()
            .take(count)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn queue_add(
        &mut self,
        name: &str,
        value: Json,
        priority: f64,
        expires: Option<f64>,
    ) -> u64 {
        let queue = self.queues.entry(name.to_string()).or_default();
        let id = queue.next_id;
        queue.next_id += 1;
        queue.items.push(Queued {
            id,
            value,
            priority,
            expires,
        });
        id
    }

    // highest priority first, then oldest, and nothing is removed by a read
    pub fn queue_read(
        &mut self,
        name: &str,
        count: usize,
        now: f64,
    ) -> Vec<(u64, Json)> {
        let Some(queue) = self.queues.get_mut(name) else {
            return Vec::new();
        };
        queue.items.retain(|item| item.is_live(now));

        let mut items: Vec<&Queued> = queue.items.iter().collect();
        items.sort_by(|left, right| {
            right
                .priority
                .partial_cmp(&left.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });

        items
            .into_iter()
            .take(count)
            .map(|item| (item.id, item.value.clone()))
            .collect()
    }

    pub fn queue_remove(&mut self, name: &str, id: u64, now: f64) -> bool {
        let Some(queue) = self.queues.get_mut(name) else {
            return false;
        };
        let before = queue.items.len();
        queue.items.retain(|item| item.id != id || !item.is_live(now));
        queue.items.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_sorted_map_round_trips() {
        let mut state = MemoryStoreState::default();
        state
            .sorted_map_set("scores", "a", json!(1), None)
            .unwrap();
        assert_eq!(state.sorted_map_get("scores", "a", 0.0), Some(json!(1)));
        assert_eq!(state.sorted_map_get("scores", "b", 0.0), None);
        assert_eq!(state.sorted_map_remove("scores", "a", 0.0), Some(json!(1)));
        assert_eq!(state.sorted_map_get("scores", "a", 0.0), None);
    }

    #[test]
    fn an_expired_entry_is_gone() {
        let mut state = MemoryStoreState::default();
        state
            .sorted_map_set("scores", "a", json!("x"), Some(10.0))
            .unwrap();
        assert_eq!(state.sorted_map_get("scores", "a", 9.0), Some(json!("x")));
        // an expired entry is dropped rather than reported as present
        assert_eq!(state.sorted_map_get("scores", "a", 11.0), None);
        assert_eq!(state.sorted_map_get("scores", "a", 9.0), None);
    }

    #[test]
    fn long_keys_are_rejected() {
        let mut state = MemoryStoreState::default();
        let error = state
            .sorted_map_set("m", &"k".repeat(MAX_KEY_BYTES + 1), json!(1), None)
            .unwrap_err();
        assert!(error.contains("limit"), "{error}");
    }

    #[test]
    fn a_range_comes_back_sorted_by_value() {
        let mut state = MemoryStoreState::default();
        for (key, value) in [("a", 3), ("b", 1), ("c", 2)] {
            state
                .sorted_map_set("scores", key, json!(value), None)
                .unwrap();
        }
        let ascending = state.sorted_map_range("scores", false, 10, None, None, 0.0);
        assert_eq!(
            ascending,
            vec![
                ("b".to_string(), json!(1)),
                ("c".to_string(), json!(2)),
                ("a".to_string(), json!(3)),
            ]
        );

        let descending = state.sorted_map_range("scores", true, 2, None, None, 0.0);
        assert_eq!(
            descending,
            vec![("a".to_string(), json!(3)), ("c".to_string(), json!(2))]
        );

        // bounds are exclusive on both ends
        let bounded = state.sorted_map_range("scores", false, 10, Some(&json!(1)), Some(&json!(3)), 0.0);
        assert_eq!(bounded, vec![("c".to_string(), json!(2))]);
    }

    #[test]
    fn strings_and_numbers_sort_without_mixing() {
        let mut state = MemoryStoreState::default();
        state
            .sorted_map_set("m", "number", json!(1), None)
            .unwrap();
        state.sorted_map_set("m", "text", json!("a"), None).unwrap();
        let range = state.sorted_map_range("m", false, 10, None, None, 0.0);
        // numbers rank before strings, and the order is total
        assert_eq!(range[0].0, "number");
        assert_eq!(range[1].0, "text");
    }

    #[test]
    fn a_queue_reads_highest_priority_first() {
        let mut state = MemoryStoreState::default();
        let low = state.queue_add("jobs", json!("low"), 0.0, None);
        let high = state.queue_add("jobs", json!("high"), 5.0, None);
        assert_eq!(state.queue_read("jobs", 10, 0.0).len(), 2);

        let read = state.queue_read("jobs", 2, 0.0);
        assert_eq!(read, vec![(high, json!("high")), (low, json!("low"))]);

        // a read does not consume: the item stays until it is removed
        assert_eq!(state.queue_read("jobs", 10, 0.0).len(), 2);
        assert!(state.queue_remove("jobs", high, 0.0));
        assert!(!state.queue_remove("jobs", high, 0.0));
        assert_eq!(state.queue_read("jobs", 10, 0.0).len(), 1);
    }

    #[test]
    fn a_queue_item_expires() {
        let mut state = MemoryStoreState::default();
        state.queue_add("jobs", json!(1), 0.0, Some(5.0));
        assert_eq!(state.queue_read("jobs", 10, 4.0).len(), 1);
        assert_eq!(state.queue_read("jobs", 10, 6.0).len(), 0);
    }
}
