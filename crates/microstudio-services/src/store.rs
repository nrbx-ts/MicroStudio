// persistent state for simulated services: plain json under a state directory
//
// where that directory lives:
//   MICROSTUDIO_STATE_DIR     explicit, wins
//   MICROSTUDIO_PROJECT_DIR   <project>/.microstudio
//   otherwise                 nowhere: a store with no root is ephemeral
//
// nothing is written unless a place opted in, so running a script at a prompt cannot
// leave data behind. `Store::at` is the explicit way in.

use std::env;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub const STATE_DIR_NAME: &str = ".microstudio";

#[derive(Debug, Clone, Default)]
pub struct Store {
    root: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} is not valid JSON: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl Store {
    // nothing is read from or written to disk, for tests and one-off runs
    pub fn ephemeral() -> Self {
        Self { root: None }
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    pub fn from_env() -> Self {
        Self::resolve(
            env::var_os("MICROSTUDIO_STATE_DIR"),
            env::var_os("MICROSTUDIO_PROJECT_DIR"),
        )
    }

    // split out of from_env so the rules can be tested without touching the environment
    pub fn resolve(
        state_dir: Option<std::ffi::OsString>,
        project_dir: Option<std::ffi::OsString>,
    ) -> Self {
        if let Some(dir) = state_dir {
            return Self::at(expand_home(PathBuf::from(dir)));
        }
        if let Some(dir) = project_dir {
            return Self::at(PathBuf::from(dir).join(STATE_DIR_NAME));
        }
        // no place to put it and nobody asked for one, so keep it in memory
        Self::ephemeral()
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn is_persistent(&self) -> bool {
        self.root.is_some()
    }

    // absolute path of a state file, or none when ephemeral
    pub fn path(&self, relative: &str) -> Option<PathBuf> {
        self.root.as_ref().map(|root| root.join(relative))
    }

    pub fn read_json(&self, relative: &str) -> Result<Option<Value>, StoreError> {
        let Some(path) = self.path(relative) else {
            return Ok(None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(StoreError::Io {
                    action: "read",
                    path,
                    source,
                })
            }
        };
        // an empty file is how a user clears a store by hand
        if text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|source| StoreError::Json { path, source })
    }

    pub fn write_json(&self, relative: &str, value: &Value) -> Result<(), StoreError> {
        let Some(path) = self.path(relative) else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                action: "create directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string());
        text.push('\n');
        // written beside the target then moved, so a crash never leaves half a file
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text).map_err(|source| StoreError::Io {
            action: "write",
            path: temp.clone(),
            source,
        })?;
        std::fs::rename(&temp, &path).map_err(|source| StoreError::Io {
            action: "replace",
            path,
            source,
        })
    }

    // read, change, write: the shape the persisted services all use
    pub fn update_json(
        &self,
        relative: &str,
        edit: impl FnOnce(Option<Value>) -> Value,
    ) -> Result<Option<Value>, StoreError> {
        let current = self.read_json(relative)?;
        let next = edit(current);
        self.write_json(relative, &next)?;
        Ok(self.read_json(relative)?)
    }

    // every `<dir>/<name>.json`, name-sorted, ephemeral stores come back empty
    pub fn list_json(&self, dir: &str) -> Result<Vec<(String, Value)>, StoreError> {
        let Some(root) = self.root.as_ref() else {
            return Ok(Vec::new());
        };
        let folder = root.join(dir);
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(StoreError::Io {
                    action: "list",
                    path: folder,
                    source,
                })
            }
        };

        let mut files = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|source| StoreError::Io {
                    action: "list",
                    path: folder.clone(),
                    source,
                })?
                .path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let name = name.to_string();
            if let Some(value) = self.read_json(&format!("{dir}/{name}.json"))? {
                files.push((name, value));
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(files)
    }

    // store names reach the filesystem, so keep only characters a path can hold
    pub fn file_name(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                    out.push(byte as char)
                }
                _ => out.push_str(&format!("%{byte:02X}")),
            }
        }
        // windows caps the whole path, so long names keep a readable prefix plus a checksum
        if out.len() > 80 {
            let digest = fnv1a(value);
            out.truncate(64);
            out.push_str(&format!("-{digest:016x}"));
        }
        if out.is_empty() {
            out.push_str("unnamed");
        }
        // never leave a name that resolves to the state directory itself
        if out.starts_with('.') {
            out.replace_range(..1, "%2E");
        }
        out
    }
}

fn fnv1a(value: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

// a leading ~ is the shell's shorthand, and a shell does not always expand it for a
// program it starts, so accept it here as well as expanding it in the cli
fn expand_home(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" {
        return home_dir().unwrap_or(path);
    }
    for prefix in ["~/", "~\\"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return match home_dir() {
                Some(home) => home.join(rest),
                None => path,
            };
        }
    }
    path
}

fn home_dir() -> Option<PathBuf> {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = env::var_os(key) {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    let drive = env::var_os("HOMEDRIVE")?;
    let path = env::var_os("HOMEPATH")?;
    let mut home = PathBuf::from(drive);
    home.push(path);
    Some(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("microstudio-store-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_explicit_state_dir_wins() {
        let store = Store::resolve(Some("C:/state".into()), Some("C:/project".into()));
        assert_eq!(store.root().unwrap(), Path::new("C:/state"));
    }

    #[test]
    fn a_project_state_dir_sits_next_to_the_project() {
        let store = Store::resolve(None, Some("C:/project".into()));
        assert_eq!(store.root().unwrap(), Path::new("C:/project/.microstudio"));
    }

    #[test]
    fn no_project_and_no_state_dir_keeps_everything_in_memory() {
        // a script run at a prompt must leave nothing behind, not even in the home directory
        let store = Store::resolve(None, None);
        assert!(!store.is_persistent());
        assert!(store.root().is_none());
    }

    #[test]
    fn a_leading_tilde_is_the_home_directory() {
        std::env::set_var("HOME", "C:/home/tester");
        let store = Store::resolve(Some("~/.microstudio".into()), None);
        assert_eq!(store.root().unwrap(), Path::new("C:/home/tester/.microstudio"));

        let bare = Store::resolve(Some("~".into()), None);
        assert_eq!(bare.root().unwrap(), Path::new("C:/home/tester"));

        // anything else is left alone, so a relative path stays relative to the caller
        let plain = Store::resolve(Some("state".into()), None);
        assert_eq!(plain.root().unwrap(), Path::new("state"));
    }

    #[test]
    fn an_ephemeral_store_round_trips_nowhere() {
        let store = Store::ephemeral();
        assert!(!store.is_persistent());
        store.write_json("datastores/a.json", &json!({"x": 1})).unwrap();
        assert!(store.read_json("datastores/a.json").unwrap().is_none());
        assert!(store.list_json("datastores").unwrap().is_empty());
    }

    #[test]
    fn json_survives_a_round_trip() {
        let store = Store::at(scratch("round-trip"));
        assert!(store.read_json("datastores/a.json").unwrap().is_none());
        store
            .write_json("datastores/a.json", &json!({"global": {"k": 1}}))
            .unwrap();
        let read = store.read_json("datastores/a.json").unwrap().unwrap();
        assert_eq!(read["global"]["k"], json!(1));
    }

    #[test]
    fn update_json_sees_the_previous_value() {
        let store = Store::at(scratch("update"));
        store.update_json("memory.json", |_| json!({"hits": 1})).unwrap();
        let next = store
            .update_json("memory.json", |current| {
                let hits = current.unwrap()["hits"].as_i64().unwrap();
                json!({ "hits": hits + 1 })
            })
            .unwrap()
            .unwrap();
        assert_eq!(next["hits"], json!(2));
    }

    #[test]
    fn an_empty_file_reads_as_absent() {
        let root = scratch("empty");
        std::fs::write(root.join("empty.json"), "").unwrap();
        let store = Store::at(&root);
        assert!(store.read_json("empty.json").unwrap().is_none());
    }

    #[test]
    fn a_broken_file_reports_the_path() {
        let root = scratch("broken");
        std::fs::write(root.join("broken.json"), "{ nope").unwrap();
        let store = Store::at(&root);
        let error = store.read_json("broken.json").unwrap_err();
        assert!(error.to_string().contains("broken.json"));
    }

    #[test]
    fn list_json_is_sorted_and_skips_other_files() {
        let store = Store::at(scratch("list"));
        store.write_json("stores/b.json", &json!({"b": true})).unwrap();
        store.write_json("stores/a.json", &json!({"a": true})).unwrap();
        std::fs::write(store.path("stores/notes.txt").unwrap(), "hi").unwrap();
        let names: Vec<String> = store
            .list_json("stores")
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn file_names_stay_inside_the_state_directory() {
        assert_eq!(Store::file_name("player_1"), "player_1");
        // a key that would otherwise escape the directory
        let escaped = Store::file_name("../../etc/passwd");
        assert!(!escaped.contains('/'));
        assert!(!escaped.contains('\\'));
        assert!(!escaped.starts_with('.'));

        let long = Store::file_name(&"k".repeat(400));
        assert!(long.len() <= 81, "{}", long.len());
        // the same name always maps to the same file
        assert_eq!(long, Store::file_name(&"k".repeat(400)));
        assert_ne!(long, Store::file_name(&"j".repeat(400)));
    }
}
