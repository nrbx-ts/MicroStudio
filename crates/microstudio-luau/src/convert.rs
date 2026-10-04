// all rust/luau value conversion goes through here

use std::collections::HashMap;

use mlua::{AnyUserData, Lua, MultiValue, Result as LuaResult, Table, Value};
use microstudio_datamodel::{AttributeValue, EventArg, InstanceId};
use microstudio_services::ScriptError;

use crate::bind::datatype::*;
use crate::bind::instance::LuaInstance;
use crate::vm::Ctx;

// same id must resolve to the same userdata; destroyed instances keep their handle
// ids are never reused
#[derive(Default)]
pub struct InstanceCache {
    map: HashMap<InstanceId, AnyUserData>,
}

impl InstanceCache {
    pub fn userdata(&mut self, lua: &Lua, ctx: &Ctx, id: InstanceId) -> LuaResult<AnyUserData> {
        if let Some(existing) = self.map.get(&id) {
            return Ok(existing.clone());
        }
        let ud = lua.create_userdata(LuaInstance {
            id,
            ctx: ctx.clone(),
        })?;
        self.map.insert(id, ud.clone());
        Ok(ud)
    }

    pub fn value(&mut self, lua: &Lua, ctx: &Ctx, id: InstanceId) -> LuaResult<Value> {
        Ok(Value::UserData(self.userdata(lua, ctx, id)?))
    }

    pub fn existing(&self, id: InstanceId) -> Option<AnyUserData> {
        self.map.get(&id).cloned()
    }

    pub fn forget(&mut self, id: InstanceId) {
        self.map.remove(&id);
    }

    pub fn insert(&mut self, id: InstanceId, ud: AnyUserData) {
        self.map.insert(id, ud);
    }
}

// datamodel/services avoid mlua, conversion lives here not in impl From
pub fn lua_err(error: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(error.to_string())
}

pub fn attribute_to_lua(
    lua: &Lua,
    cache: &mut InstanceCache,
    ctx: &Ctx,
    value: &AttributeValue,
) -> LuaResult<Value> {
    Ok(match value {
        AttributeValue::Bool(v) => Value::Boolean(*v),
        AttributeValue::Number(v) => Value::Number(*v),
        AttributeValue::String(v) => Value::String(lua.create_string(v)?),
        AttributeValue::Vector2(v) => Value::UserData(lua.create_userdata(LuaVector2(*v))?),
        AttributeValue::Vector3(v) => Value::UserData(lua.create_userdata(LuaVector3(*v))?),
        AttributeValue::CFrame(v) => Value::UserData(lua.create_userdata(LuaCFrame(*v))?),
        AttributeValue::Color3(v) => Value::UserData(lua.create_userdata(LuaColor3(*v))?),
        AttributeValue::UDim(v) => Value::UserData(lua.create_userdata(LuaUDim(*v))?),
        AttributeValue::UDim2(v) => Value::UserData(lua.create_userdata(LuaUDim2(*v))?),
        AttributeValue::Rect(v) => Value::UserData(lua.create_userdata(LuaRect(*v))?),
        AttributeValue::BrickColor(v) => Value::UserData(lua.create_userdata(LuaBrickColor(*v))?),
        AttributeValue::Ray(v) => Value::UserData(lua.create_userdata(LuaRay(*v))?),
        AttributeValue::Instance(id) => cache.value(lua, ctx, *id)?,
    })
}

pub fn lua_to_attribute(lua: &Lua, value: &Value) -> LuaResult<AttributeValue> {
    Ok(match value {
        Value::Boolean(v) => AttributeValue::Bool(*v),
        Value::Integer(v) => AttributeValue::Number(*v as f64),
        Value::Number(v) => AttributeValue::Number(*v),
        Value::String(v) => AttributeValue::String(v.to_str()?.to_string()),
        Value::UserData(ud) => datatype_from_userdata(lua, ud)?,
        other => {
            return Err(mlua::Error::runtime(format!(
                "{} cannot be stored as an attribute",
                other.type_name()
            )))
        }
    })
}

fn datatype_from_userdata(_lua: &Lua, ud: &AnyUserData) -> LuaResult<AttributeValue> {
    if let Ok(v) = ud.borrow::<LuaVector2>() {
        return Ok(AttributeValue::Vector2(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaVector3>() {
        return Ok(AttributeValue::Vector3(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaCFrame>() {
        return Ok(AttributeValue::CFrame(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaColor3>() {
        return Ok(AttributeValue::Color3(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaUDim>() {
        return Ok(AttributeValue::UDim(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaUDim2>() {
        return Ok(AttributeValue::UDim2(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaRect>() {
        return Ok(AttributeValue::Rect(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaBrickColor>() {
        return Ok(AttributeValue::BrickColor(v.0));
    }
    if let Ok(v) = ud.borrow::<LuaRay>() {
        return Ok(AttributeValue::Ray(v.0));
    }
    if let Ok(instance) = ud.borrow::<LuaInstance>() {
        return Ok(AttributeValue::Instance(instance.id));
    }
    Err(mlua::Error::runtime(
        "unsupported attribute type (expected a Roblox datatype or Instance)",
    ))
}

pub fn event_arg_to_lua(
    lua: &Lua,
    cache: &mut InstanceCache,
    ctx: &Ctx,
    arg: &EventArg,
) -> LuaResult<Value> {
    Ok(match arg {
        EventArg::Instance(id) => cache.value(lua, ctx, *id)?,
        EventArg::Parent(Some(id)) => cache.value(lua, ctx, *id)?,
        EventArg::Parent(None) => Value::Nil,
        EventArg::Attribute(_, value) => attribute_to_lua(lua, cache, ctx, value)?,
        EventArg::Number(v) => Value::Number(*v),
        EventArg::Text(v) => Value::String(lua.create_string(v)?),
    })
}

pub fn event_args_to_multi(
    lua: &Lua,
    cache: &mut InstanceCache,
    ctx: &Ctx,
    args: &[EventArg],
) -> LuaResult<MultiValue> {
    let mut out = MultiValue::new();
    for arg in args {
        out.push_back(event_arg_to_lua(lua, cache, ctx, arg)?);
    }
    Ok(out)
}

pub fn event_args_to_table(
    lua: &Lua,
    cache: &mut InstanceCache,
    ctx: &Ctx,
    args: &[EventArg],
) -> LuaResult<Table> {
    let table = lua.create_table()?;
    for (index, arg) in args.iter().enumerate() {
        table.set(index + 1, event_arg_to_lua(lua, cache, ctx, arg)?)?;
    }
    Ok(table)
}

pub fn take_instance(value: &Value) -> LuaResult<InstanceId> {
    match value {
        Value::UserData(ud) => ud
            .borrow::<LuaInstance>()
            .map(|i| i.id)
            .map_err(|_| mlua::Error::runtime("expected an Instance")),
        other => Err(mlua::Error::runtime(format!(
            "expected an Instance, got {}",
            other.type_name()
        ))),
    }
}

pub fn take_signal_id(value: &Value) -> LuaResult<microstudio_datamodel::SignalId> {
    match value {
        Value::Table(table) => table
            .raw_get::<Option<u64>>("__microstudio_signal")?
            .map(microstudio_datamodel::SignalId)
            .ok_or_else(|| mlua::Error::runtime("expected an RBXScriptSignal")),
        other => Err(mlua::Error::runtime(format!(
            "expected an RBXScriptSignal, got {}",
            other.type_name()
        ))),
    }
}

pub fn take_connection_handle(value: &Value) -> LuaResult<u64> {
    match value {
        Value::Table(table) => table
            .raw_get::<Option<u64>>("__microstudio_connection")?
            .ok_or_else(|| mlua::Error::runtime("expected an RBXScriptConnection")),
        other => Err(mlua::Error::runtime(format!(
            "expected an RBXScriptConnection, got {}",
            other.type_name()
        ))),
    }
}

pub fn format_error(error: &mlua::Error, fallback_location: &str) -> ScriptError {
    let rendered = match error {
        mlua::Error::RuntimeError(message) => message.clone(),
        other => other.to_string(),
    };

    let mut lines = rendered.lines();
    let first = lines.next().unwrap_or_default().to_string();
    let traceback: Vec<String> = lines.map(|l| l.trim().to_string()).collect();

    // luau errors look like chunk:line: message
    let (location, message) = match split_location(&first) {
        Some((loc, msg)) => (loc, msg),
        None => (fallback_location.to_string(), first),
    };

    ScriptError {
        location,
        message,
        traceback: if traceback.is_empty() {
            fallback_location.to_string()
        } else {
            traceback.join("\n")
        },
    }
}

fn split_location(line: &str) -> Option<(String, String)> {
    // chunk can contain a colon (C:\...); split on the first :<digits>:
    let chars: Vec<char> = line.chars().collect();

    for (index, ch) in chars.iter().enumerate() {
        if *ch != ':' {
            continue;
        }

        let digits: String = chars[index + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            continue;
        }

        let after = index + 1 + digits.len();
        if chars.get(after) != Some(&':') {
            continue;
        }

        let chunk: String = chars[..index].iter().collect();
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }

        let message: String = chars[after + 1..].iter().collect();
        return Some((format!("{chunk}:{digits}"), message.trim_start().to_string()));
    }

    None
}

pub fn tostring_for_log(lua: &Lua, value: &Value) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => crate::bind::datatype::num(*n),
        Value::String(s) => s.to_string_lossy().to_string(),
        // must use luau tostring (honours __tostring); coerce_string renders userdata
        _ => lua
            .globals()
            .get::<mlua::Function>("tostring")
            .and_then(|tostring| tostring.call::<mlua::LuaString>(value.clone()))
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|_| value.type_name().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_luau_runtime_errors() {
        let (location, message) =
            split_location("ServerScriptService.Server:42: attempt to index nil").unwrap();
        assert_eq!(location, "ServerScriptService.Server:42");
        assert_eq!(message, "attempt to index nil");
    }

    #[test]
    fn splits_locations_that_contain_a_drive_letter() {
        // windows path must not be mistaken for chunk:line
        let (location, message) =
            split_location(r"C:\project\out\game.server.luau:12: attempt to index nil").unwrap();
        assert_eq!(location, r"C:\project\out\game.server.luau:12");
        assert_eq!(message, "attempt to index nil");
    }

    #[test]
    fn splits_locations_without_a_message() {
        let (location, message) = split_location("script.luau:7:").unwrap();
        assert_eq!(location, "script.luau:7");
        assert_eq!(message, "");
    }

    #[test]
    fn leaves_plain_messages_alone() {
        assert!(split_location("just a message").is_none());
    }

    #[test]
    fn formats_the_documented_error_shape() {
        let error = mlua::Error::RuntimeError(
            "ServerScriptService.Server:42: attempt to index nil with 'Position'".to_string(),
        );
        let formatted = format_error(&error, "unknown");
        assert_eq!(formatted.location, "ServerScriptService.Server:42");
        assert_eq!(formatted.message, "attempt to index nil with 'Position'");
    }
}
