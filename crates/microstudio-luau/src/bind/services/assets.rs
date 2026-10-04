// InsertService: model assets seeded from the state directory
//
// assets.json (under the state directory):
// {
//   "assets": {
//     "12345": {
//       "name": "Crate",
//       "class": "Model",
//       "properties": { "Anchored": true, "Name": "Crate" },
//       "children": [
//         { "name": "Body", "class": "Part", "properties": { "Size": [4, 4, 4] } }
//       ]
//     }
//   }
// }
//
// a node takes name (defaults to class), class (defaults to Model), an optional properties
// object and an optional children array. property values are a bool, a number, a string,
// or three numbers standing for a Vector3. LoadLocalAsset reads the node shape from
// <state dir>/assets/<path>; a path without an extension gets .json appended.

use std::path::{Component, Path};

use mlua::{AnyUserData, Error as LuaError, Lua, Result as LuaResult, Table, Value};
use microstudio_datamodel::{AttributeValue, DataModel, InstanceId};
use microstudio_services::{Store, StoreError};
use microstudio_types::Vector3;
use serde_json::Value as Json;

use crate::bind::instance::{instance_of, LuaInstance};
use crate::bind::services::{methods, require_class};
use crate::convert::lua_err;
use crate::vm::{self, Ctx};

const ASSETS_FILE: &str = "assets.json";
const REGISTRY: &str = "microstudio.insert_service";

// deviation: a real server fetches the model from Roblox asset delivery
const LOCAL_ASSETS: &str =
    "InsertService serves assets from assets.json, not Roblox asset delivery";
// deviation: a real server resolves the id in the asset's version history
const VERSION: &str =
    "InsertService:LoadAssetVersion ignores version history and loads the seeded asset";
// deviation: a real server reads a file from Roblox's local content folder
const LOCAL_FILE: &str =
    "InsertService:LoadLocalAsset reads from <state dir>/assets, not Roblox's local folder";
// deviation: a real server downloads and builds a real package
const PACKAGE: &str = "InsertService:LoadPackageAsset returns the seeded tree, not a real package";
// deviation: a real server converts the asset to the requested format
const FORMAT: &str =
    "InsertService:LoadAssetWithFormat ignores the format and returns the seeded tree";
// deviation: a real server reads the latest version from the asset's history
const LATEST_VERSION: &str =
    "InsertService:GetLatestAssetVersionAsync returns 1, there is no version history locally";
// deviation: a real server loads the mesh from Roblox's CDN
const MESH: &str = "InsertService:CreateMeshPartAsync has no local mesh loading";

pub fn install(lua: &Lua, _ctx: &Ctx) -> LuaResult<()> {
    // one note per deviation, not per call
    lua.set_named_registry_value(REGISTRY, lua.create_table()?)?;
    let methods = methods(lua)?;

    methods.set(
        "LoadAsset",
        lua.create_function(|lua, (ud, asset_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "InsertService")?;
            load_seeded(lua, &this, "local-assets", LOCAL_ASSETS, asset_id)
        })?,
    )?;

    methods.set(
        "LoadAssetVersion",
        lua.create_function(|lua, (ud, asset_version_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "InsertService")?;
            load_seeded(lua, &this, "version", VERSION, asset_version_id)
        })?,
    )?;

    // these return Objects on Roblox, so hand back a single-model list
    for (name, key, warning) in [
        ("LoadAssetWithFormat", "format", FORMAT),
        ("LoadPackageAsset", "package", PACKAGE),
        ("LoadPackageAssetAsync", "package", PACKAGE),
    ] {
        methods.set(
            name,
            lua.create_function(
                move |lua, (ud, asset_id, _options): (AnyUserData, f64, Option<Value>)| {
                    let this = instance_of(&ud)?;
                    require_class(&this, "InsertService")?;
                    let instance = load_seeded(lua, &this, key, warning, asset_id)?;
                    let objects = lua.create_table()?;
                    objects.set(1, instance)?;
                    Ok(Value::Table(objects))
                },
            )?,
        )?;
    }

    methods.set(
        "LoadLocalAsset",
        lua.create_function(|lua, (ud, path): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "InsertService")?;
            load_local_seeded(lua, &this, &path)
        })?,
    )?;

    methods.set(
        "CreateMeshPartAsync",
        lua.create_function(
            |lua, (ud, _mesh_id, _options): (AnyUserData, f64, Option<Table>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "InsertService")?;
                warn_once(lua, &this.ctx, "mesh", MESH)?;
                Err::<Value, LuaError>(LuaError::runtime(
                    "InsertService:CreateMeshPartAsync is not supported locally: \
                     MicroStudio has no mesh loading",
                ))
            },
        )?,
    )?;

    methods.set(
        "GetLatestAssetVersionAsync",
        lua.create_function(|lua, (ud, asset_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "InsertService")?;
            let key = asset_key(asset_id);
            let (spec, dir) = lookup(&this.ctx, &key)?;
            warn_once(lua, &this.ctx, "latest-version", LATEST_VERSION)?;
            if spec.is_none() {
                return Err(asset_not_found(&key, &dir));
            }
            Ok(1i64)
        })?,
    )?;

    Ok(())
}

fn load_seeded(
    lua: &Lua,
    this: &LuaInstance,
    warning_key: &str,
    warning: &str,
    asset_id: f64,
) -> LuaResult<Value> {
    let key = asset_key(asset_id);
    let (spec, dir) = lookup(&this.ctx, &key)?;
    warn_once(lua, &this.ctx, warning_key, warning)?;
    let Some(spec) = spec else {
        return Err(asset_not_found(&key, &dir));
    };
    instance_from_spec(lua, &this.ctx, &spec)
}

fn load_local_seeded(lua: &Lua, this: &LuaInstance, path: &str) -> LuaResult<Value> {
    let key = local_asset_key(path)?;
    let (spec, dir) = {
        let state = this.ctx.state.borrow();
        let store = &state.world.store;
        let spec = store
            .read_json(&key)
            .map_err(|error| LuaError::runtime(format!("LoadLocalAsset: {error}")))?;
        (spec, state_dir(store))
    };
    warn_once(lua, &this.ctx, "local-file", LOCAL_FILE)?;
    let Some(spec) = spec else {
        return Err(LuaError::runtime(format!(
            "LoadLocalAsset: {key} was not found under {dir}"
        )));
    };
    instance_from_spec(lua, &this.ctx, &spec)
}

// reads the seeded entry plus the state dir, so a miss can name the file to edit
fn lookup(ctx: &Ctx, key: &str) -> LuaResult<(Option<Json>, String)> {
    let state = ctx.state.borrow();
    let store = &state.world.store;
    let spec = read_asset(store, key).map_err(lua_err)?;
    Ok((spec, state_dir(store)))
}

fn read_asset(store: &Store, key: &str) -> Result<Option<Json>, StoreError> {
    match store.read_json(ASSETS_FILE)? {
        Some(file) => Ok(file.get("assets").and_then(|assets| assets.get(key)).cloned()),
        None => Ok(None),
    }
}

fn instance_from_spec(lua: &Lua, ctx: &Ctx, spec: &Json) -> LuaResult<Value> {
    let root = {
        let mut state = ctx.state.borrow_mut();
        build_model(&mut state.world.dm, spec)?
    };
    // the model comes back unparented; the caller decides where it goes
    vm::instance_value(lua, ctx, root)
}

fn build_model(dm: &mut DataModel, spec: &Json) -> LuaResult<InstanceId> {
    let class = spec.get("class").and_then(Json::as_str).unwrap_or("Model");
    let name = spec
        .get("name")
        .and_then(Json::as_str)
        .unwrap_or(class)
        .to_string();
    let id = dm.create_instance(class).map_err(lua_err)?;
    dm.set_name(id, &name).map_err(lua_err)?;

    if let Some(properties) = spec.get("properties") {
        apply_properties(dm, id, properties)?;
    }

    if let Some(children) = spec.get("children") {
        let children = children
            .as_array()
            .ok_or_else(|| LuaError::runtime("children must be an array in a model file"))?;
        for child in children {
            let child_id = build_model(dm, child)?;
            dm.set_parent(child_id, Some(id)).map_err(lua_err)?;
        }
    }

    Ok(id)
}

fn apply_properties(dm: &mut DataModel, id: InstanceId, properties: &Json) -> LuaResult<()> {
    let values = properties
        .as_object()
        .ok_or_else(|| LuaError::runtime("properties must be an object in a model file"))?;
    for (name, value) in values {
        // Name lives on the tree, so set_property would reject it
        if name == "Name" {
            let text = value.as_str().ok_or_else(|| unsupported_property(name))?;
            dm.set_name(id, text).map_err(lua_err)?;
            continue;
        }
        let converted = property_value(name, value)?;
        dm.set_property(id, name, converted).map_err(lua_err)?;
    }
    Ok(())
}

fn property_value(name: &str, value: &Json) -> LuaResult<AttributeValue> {
    let unsupported = || unsupported_property(name);
    match value {
        Json::Bool(inner) => Ok(AttributeValue::Bool(*inner)),
        Json::Number(inner) => inner
            .as_f64()
            .map(AttributeValue::Number)
            .ok_or_else(unsupported),
        Json::String(inner) => Ok(AttributeValue::String(inner.clone())),
        Json::Array(items) if items.len() == 3 => {
            let mut parts = [0.0f64; 3];
            for (index, item) in items.iter().enumerate() {
                parts[index] = item.as_f64().ok_or_else(unsupported)?;
            }
            Ok(AttributeValue::Vector3(Vector3::new(
                parts[0], parts[1], parts[2],
            )))
        }
        _ => Err(unsupported()),
    }
}

fn unsupported_property(name: &str) -> LuaError {
    LuaError::runtime(format!("{name} is not supported in a model file"))
}

// local paths reach the filesystem, so keep them under the assets folder
fn local_asset_key(path: &str) -> LuaResult<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => {
                return Err(LuaError::runtime(format!(
                    "LoadLocalAsset refuses {path:?}: paths must stay inside the assets directory"
                )))
            }
        }
    }
    if parts.is_empty() {
        return Err(LuaError::runtime("LoadLocalAsset expects a file path"));
    }
    let mut key = parts.join("/");
    if Path::new(&key).extension().is_none() {
        key.push_str(".json");
    }
    Ok(format!("assets/{key}"))
}

fn asset_key(asset_id: f64) -> String {
    if asset_id.is_finite() && asset_id.fract() == 0.0 {
        (asset_id as i64).to_string()
    } else {
        asset_id.to_string()
    }
}

fn asset_not_found(id: &str, dir: &str) -> LuaError {
    LuaError::runtime(format!(
        "Asset {id} was not found. Add it to {dir}/assets.json, e.g. \"assets\": \
         {{ \"{id}\": {{ \"name\": \"Crate\", \"class\": \"Model\" }} }}"
    ))
}

fn state_dir(store: &Store) -> String {
    store
        .root()
        .map(|root| root.display().to_string())
        .unwrap_or_else(|| "<state dir>".to_string())
}

fn warn_once(lua: &Lua, ctx: &Ctx, key: &str, message: &str) -> LuaResult<()> {
    let noted: Table = lua.named_registry_value(REGISTRY)?;
    if noted.get::<bool>(key)? {
        return Ok(());
    }
    noted.set(key, true)?;
    ctx.state.borrow_mut().scheduler.warn(message.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{Vm, VmOptions};

    fn seeded(case: &str, seed: Json) -> (Vm, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("microstudio-assets-{case}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = microstudio_services::Store::at(&dir);
        store.write_json("assets.json", &seed).unwrap();
        let vm = Vm::new(VmOptions {
            store,
            ..Default::default()
        })
        .expect("vm boots");
        (vm, dir)
    }

    fn crate_seed() -> Json {
        serde_json::json!({
            "assets": {
                "12345": {
                    "name": "Crate",
                    "class": "Model",
                    "properties": { "Name": "Crate" },
                    "children": [
                        {
                            "name": "Body",
                            "class": "Part",
                            "properties": { "Anchored": true, "Size": [4, 4, 4] }
                        }
                    ]
                }
            }
        })
    }

    fn eval_string(vm: &Vm, code: &str) -> String {
        match vm.eval(code).unwrap() {
            Value::String(text) => text.to_str().unwrap().to_string(),
            other => panic!("expected a string, got {}", other.type_name()),
        }
    }

    // a runtime error is reported rather than raised at the top level, so ask for it
    fn eval_error(vm: &Vm, code: &str) -> String {
        let source = format!(
            "local ok, message = pcall(function() return {code} end)\n\
             assert(not ok, 'expected an error')\n\
             return tostring(message)"
        );
        eval_string(vm, &source)
    }

    #[test]
    fn load_asset_builds_a_model_with_children_and_properties() {
        let (vm, _dir) = seeded("load", crate_seed());
        let value = eval_string(
            &vm,
            r#"
            local model = game:GetService("InsertService"):LoadAsset(12345)
            assert(model.Name == "Crate")
            assert(model.ClassName == "Model")
            assert(model.Parent == nil)
            local children = model:GetChildren()
            assert(#children == 1)
            local body = children[1]
            assert(body.Name == "Body")
            assert(body.ClassName == "Part")
            assert(body.Anchored == true)
            assert(body.Size == Vector3.new(4, 4, 4))
            return "ok"
            "#,
        );
        assert_eq!(value, "ok");
    }

    #[test]
    fn unknown_asset_raises_a_seed_hint() {
        let (vm, _dir) = seeded("unknown", crate_seed());
        let message = eval_error(&vm, r#"game:GetService("InsertService"):LoadAsset(999)"#);
        assert!(message.contains("was not found"), "{message}");
        assert!(message.contains("assets.json"), "{message}");
        assert!(message.contains("\"999\""), "{message}");
    }

    #[test]
    fn load_local_asset_reads_a_file_on_disk() {
        let (vm, dir) = seeded("local", serde_json::json!({ "assets": {} }));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let tree = serde_json::json!({
            "name": "Local",
            "class": "Model",
            "children": [
                { "name": "Body", "class": "Part", "properties": { "Anchored": true } }
            ]
        });
        std::fs::write(
            dir.join("assets").join("crate.json"),
            serde_json::to_string(&tree).unwrap(),
        )
        .unwrap();

        let value = eval_string(
            &vm,
            r#"
            local model = game:GetService("InsertService"):LoadLocalAsset("crate")
            assert(model.Name == "Local")
            assert(#model:GetChildren() == 1)
            return "ok"
            "#,
        );
        assert_eq!(value, "ok");
    }

    #[test]
    fn load_local_asset_refuses_a_path_that_escapes() {
        let (vm, _dir) = seeded("escape", serde_json::json!({ "assets": {} }));
        let message = eval_error(
            &vm,
            r#"game:GetService("InsertService"):LoadLocalAsset("../secret")"#,
        );
        assert!(message.contains("stay inside"), "{message}");
    }

    #[test]
    fn an_unsupported_property_value_raises() {
        let seed = serde_json::json!({
            "assets": {
                "7": {
                    "name": "Odd",
                    "class": "Model",
                    "children": [
                        {
                            "name": "Body",
                            "class": "Part",
                            "properties": { "Transparency": { "nested": true } }
                        }
                    ]
                }
            }
        });
        let (vm, _dir) = seeded("unsupported", seed);
        let message = eval_error(&vm, r#"game:GetService("InsertService"):LoadAsset(7)"#);
        assert!(
            message.contains("Transparency is not supported in a model file"),
            "{message}"
        );
    }

    #[test]
    fn latest_asset_version_is_one_for_a_seeded_asset() {
        let (vm, _dir) = seeded("version", crate_seed());
        let value = eval_string(
            &vm,
            r#"local service = game:GetService("InsertService")
               return tostring(service:GetLatestAssetVersionAsync(12345))"#,
        );
        assert_eq!(value, "1");

        let message = eval_error(
            &vm,
            r#"game:GetService("InsertService"):GetLatestAssetVersionAsync(999)"#,
        );
        assert!(message.contains("was not found"), "{message}");
    }

    #[test]
    fn load_asset_version_matches_load_asset() {
        let (vm, _dir) = seeded("loadversion", crate_seed());
        let value = eval_string(
            &vm,
            r#"
            local model = game:GetService("InsertService"):LoadAssetVersion(12345)
            assert(model.Name == "Crate")
            assert(#model:GetChildren() == 1)
            return "ok"
            "#,
        );
        assert_eq!(value, "ok");
    }

    #[test]
    fn load_package_asset_returns_a_list_of_objects() {
        let (vm, _dir) = seeded("package", crate_seed());
        let value = eval_string(
            &vm,
            r#"
            local objects = game:GetService("InsertService"):LoadPackageAsset(12345)
            assert(#objects == 1)
            assert(objects[1].Name == "Crate")
            return "ok"
            "#,
        );
        assert_eq!(value, "ok");
    }

    #[test]
    fn create_mesh_part_raises_not_supported_locally() {
        let (vm, _dir) = seeded("mesh", crate_seed());
        let message = eval_error(
            &vm,
            r#"game:GetService("InsertService"):CreateMeshPartAsync(42)"#,
        );
        assert!(message.contains("not supported locally"), "{message}");
    }

    #[test]
    fn a_deviation_warns_once_per_vm() {
        let (vm, _dir) = seeded("warn", crate_seed());
        eval_string(
            &vm,
            r#"
            local service = game:GetService("InsertService")
            service:LoadAsset(12345)
            service:LoadAsset(12345)
            return "ok"
            "#,
        );
        let warnings = vm.take_warnings();
        assert_eq!(
            warnings.iter().filter(|w| w.contains("assets.json")).count(),
            1,
            "{warnings:?}"
        );
    }
}
