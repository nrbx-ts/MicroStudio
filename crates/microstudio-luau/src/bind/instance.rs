// receivers arrive as AnyUserData and are cloned out: mlua holds a RefCell borrow for &T calls
// lookup order matters: methods, events, name/class/parent, properties, children, services

use mlua::{
    AnyUserData, Error as LuaError, Function, Lua, MetaMethod, MultiValue, Result as LuaResult,
    Table, UserData, UserDataFields, UserDataMethods, Value,
};
use microstudio_datamodel::{
    api, classes, default_property_value as declared_default, property_type, AttributeValue,
    EventKind, InstanceId, PropertyType,
};

use crate::convert::{self, lua_to_attribute};
use crate::vm::{self, Ctx};

#[derive(Clone)]
pub struct LuaInstance {
    pub id: InstanceId,
    pub ctx: Ctx,
}

// clone out and release the borrow immediately
pub fn instance_of(ud: &AnyUserData) -> LuaResult<LuaInstance> {
    ud.borrow::<LuaInstance>().map(|instance| instance.clone())
}

impl LuaInstance {
    fn class(&self) -> String {
        self.ctx
            .state
            .borrow()
            .world
            .dm
            .class_of(self.id)
            .unwrap_or("<destroyed>")
            .to_string()
    }

    fn name(&self) -> String {
        self.ctx
            .state
            .borrow()
            .world
            .dm
            .name_of(self.id)
            .unwrap_or("")
            .to_string()
    }

    fn alive(&self) -> bool {
        self.ctx.state.borrow().world.dm.contains(self.id)
    }
}

impl UserData for LuaInstance {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_meta_field(MetaMethod::Type, "Instance");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // add_meta_function not add_meta_method: we get AnyUserData and control the borrow
        methods.add_meta_function(MetaMethod::Index, |lua, (ud, key): (AnyUserData, Value)| {
            instance_index(lua, &instance_of(&ud)?, &key)
        });
        methods.add_meta_function(
            MetaMethod::NewIndex,
            |lua, (ud, key, value): (AnyUserData, Value, Value)| {
                let this = instance_of(&ud)?;
                instance_newindex(lua, &this, &key, value)
            },
        );
        methods.add_meta_function(MetaMethod::ToString, |_, ud: AnyUserData| {
            Ok(instance_of(&ud).map(|i| i.name()).unwrap_or_default())
        });
        methods.add_meta_function(MetaMethod::Eq, |_, (a, b): (Value, Value)| {
            Ok(match (&a, &b) {
                (Value::UserData(x), Value::UserData(y)) => match (instance_of(x), instance_of(y)) {
                    (Ok(x), Ok(y)) => x.id == y.id,
                    _ => false,
                },
                _ => false,
            })
        });
    }
}

fn key_string(key: &Value) -> LuaResult<String> {
    match key {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        other => Err(LuaError::runtime(format!(
            "instance members must be indexed with a string, got {}",
            other.type_name()
        ))),
    }
}

fn event_kind(key: &str) -> Option<EventKind> {
    EventKind::all()
        .iter()
        .copied()
        .find(|kind| kind.name() == key)
}

fn instance_index(lua: &Lua, this: &LuaInstance, key: &Value) -> LuaResult<Value> {
    let key = key_string(key)?;

    let methods: Table = lua.named_registry_value("microstudio.instance_methods")?;
    let method: Value = methods.get(key.as_str())?;
    if !method.is_nil() {
        return Ok(method);
    }

    // prelude methods (the ones that must yield)
    let lua_methods: Table = lua.named_registry_value("microstudio.lua_instance_methods")?;
    let method: Value = lua_methods.get(key.as_str())?;
    if !method.is_nil() {
        return Ok(method);
    }

    let ctx = &this.ctx;
    let id = this.id;
    if !this.alive() {
        return Err(LuaError::runtime(format!(
            "attempt to index a destroyed Instance ({key})"
        )));
    }

    if let Some(kind) = event_kind(&key) {
        let signal = ctx
            .state
            .borrow_mut()
            .world
            .dm
            .signal(id, kind)
            .map_err(convert::lua_err)?;
        let table = crate::bind::signal::signal_table(lua, ctx, &signal)?;
        return Ok(Value::Table(table));
    }

    match key.as_str() {
        "Name" => return Ok(Value::String(lua.create_string(this.name())?)),
        "ClassName" => {
            let class = this.class();
            return Ok(Value::String(lua.create_string(class)?));
        }
        "Parent" => {
            let parent = ctx.state.borrow().world.dm.parent_of(id);
            return match parent {
                Some(parent) => vm::instance_value(lua, ctx, parent),
                None => Ok(Value::Nil),
            };
        }
        _ => {}
    }

    let class = this.class();
    if let Some(property_type) = property_type(&class, &key) {
        let stored = ctx.state.borrow().world.dm.get_property(id, &key);
        return match stored {
            Some(value) => {
                let mut state = ctx.state.borrow_mut();
                convert::attribute_to_lua(lua, &mut state.cache, ctx, &value)
            }
            None => Ok(property_default(&class, &key, property_type)),
        };
    }

    // children by name: workspace.Train is the common path
    let child = ctx.state.borrow().world.dm.find_first_child(id, &key);
    if let Some(child) = child {
        return vm::instance_value(lua, ctx, child);
    }

    if ctx.state.borrow().world.dm.class_of(id) == Some("DataModel")
        && classes::descriptor(&key)
            .map(|descriptor| descriptor.is_service())
            .unwrap_or(false)
    {
        let service = ctx
            .state
            .borrow_mut()
            .world
            .get_service(&key)
            .map_err(convert::lua_err)?;
        return vm::instance_value(lua, ctx, service);
    }

    // a member only the generated table declares: every Roblox service is callable
    if let Some(member) = api::member(&class, &key) {
        match member.kind {
            api::MemberKind::Event => {
                let signal = ctx
                    .state
                    .borrow_mut()
                    .world
                    .dm
                    .named_signal(id, member.name)
                    .map_err(convert::lua_err)?;
                let table = crate::bind::signal::signal_table(lua, ctx, &signal)?;
                return Ok(Value::Table(table));
            }
            api::MemberKind::Function | api::MemberKind::Callback => {
                return Ok(Value::Function(simulated_function(lua, class, member)?));
            }
            // properties resolve before this, through property_type
            api::MemberKind::Property => {}
        }
    }

    Err(LuaError::runtime(format!(
        "'{key}' is not a valid member of {class}"
    )))
}

// a stand-in for a member the runtime does not model: a fixed value, and one note
// so a game leaning on it is not quietly wrong
fn simulated_function(
    lua: &Lua,
    class: String,
    member: &'static api::Member,
) -> LuaResult<Function> {
    lua.create_function(move |lua, (receiver, _args): (AnyUserData, MultiValue)| {
        let this = instance_of(&receiver)?;
        note_simulated(lua, &this.ctx, &class, member.name)?;
        fake_value(lua, member.type_name)
    })
}

fn note_simulated(lua: &Lua, ctx: &Ctx, class: &str, member: &str) -> LuaResult<()> {
    let noted: Table = lua.named_registry_value("microstudio.simulated")?;
    let key = format!("{class}.{member}");
    if noted.get::<bool>(key.as_str())? {
        return Ok(());
    }
    noted.set(key.as_str(), true)?;
    ctx.state
        .borrow_mut()
        .scheduler
        .warn(format!("{key} is simulated and returns a fixed value"));
    Ok(())
}

// the declared return type decides: a zero value, an empty table, or nil
fn fake_value(lua: &Lua, type_name: &str) -> LuaResult<Value> {
    Ok(match type_name {
        "bool" => Value::Boolean(false),
        "int" | "int64" | "float" | "double" | "number" => Value::Number(0.0),
        "string" | "Content" | "ContentId" => Value::String(lua.create_string("")?),
        "Array" | "Dictionary" | "Map" | "Tuple" | "Objects" | "Variant" => {
            Value::Table(lua.create_table()?)
        }
        _ => Value::Nil,
    })
}

// object-valued properties start nil; classes pre-populate the rest
fn property_default(class: &str, name: &str, property_type: PropertyType) -> Value {
    if let Some(AttributeValue::Bool(true)) = declared_default(class, name) {
        return Value::Boolean(true);
    }
    match property_type {
        PropertyType::Bool => Value::Boolean(false),
        PropertyType::Number => Value::Number(0.0),
        _ => Value::Nil,
    }
}

fn instance_newindex(lua: &Lua, this: &LuaInstance, key: &Value, value: Value) -> LuaResult<()> {
    let key = key_string(key)?;
    let ctx = &this.ctx;
    let id = this.id;

    if !this.alive() {
        return Err(LuaError::runtime(format!(
            "attempt to set '{key}' on a destroyed Instance"
        )));
    }

    match key.as_str() {
        "Name" => {
            let Value::String(name) = value else {
                return Err(LuaError::runtime("Name must be a string"));
            };
            let name = name.to_str()?.to_string();
            ctx.state
                .borrow_mut()
                .world
                .dm
                .set_name(id, &name)
                .map_err(convert::lua_err)?;
            Ok(())
        }
        "Parent" => {
            let parent = match value {
                Value::Nil => None,
                other => Some(convert::take_instance(&other)?),
            };
            let events = ctx
                .state
                .borrow_mut()
                .world
                .dm
                .set_parent(id, parent)
                .map_err(convert::lua_err)?;
            vm::dispatch(lua, ctx, events)?;
            Ok(())
        }
        _ => {
            let class = this.class();
            if property_type(&class, &key).is_none() {
                return Err(LuaError::runtime(format!(
                    "'{key}' is not a valid member of {class}"
                )));
            }
            let converted = lua_to_attribute(lua, &value)?;
            ctx.state
                .borrow_mut()
                .world
                .dm
                .set_property(id, &key, converted)
                .map_err(convert::lua_err)?;
            Ok(())
        }
    }
}

pub fn install(lua: &Lua, _ctx: &Ctx) -> LuaResult<()> {
    let methods = lua.create_table()?;
    // one note per simulated member, not per call
    lua.set_named_registry_value("microstudio.simulated", lua.create_table()?)?;

    methods.set(
        "GetChildren",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            let children = this.ctx.state.borrow().world.dm.get_children(this.id);
            instance_list(lua, &this.ctx, children)
        })?,
    )?;

    methods.set(
        "GetDescendants",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            let descendants = this.ctx.state.borrow().world.dm.get_descendants(this.id);
            instance_list(lua, &this.ctx, descendants)
        })?,
    )?;

    methods.set(
        "FindFirstChild",
        lua.create_function(
            |lua, (ud, name, recursive): (AnyUserData, String, Option<bool>)| {
                let this = instance_of(&ud)?;
                let found = {
                    let state = this.ctx.state.borrow();
                    let dm = &state.world.dm;
                    if recursive.unwrap_or(false) {
                        dm.find_first_descendant(this.id, &name)
                    } else {
                        dm.find_first_child(this.id, &name)
                    }
                };
                optional_instance(lua, &this.ctx, found)
            },
        )?,
    )?;

    methods.set(
        "FindFirstChildOfClass",
        lua.create_function(|lua, (ud, class): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            let found = this
                .ctx
                .state
                .borrow()
                .world
                .dm
                .find_first_child_of_class(this.id, &class);
            optional_instance(lua, &this.ctx, found)
        })?,
    )?;

    methods.set(
        "FindFirstChildWhichIsA",
        lua.create_function(
            |lua, (ud, class, recursive): (AnyUserData, String, Option<bool>)| {
                let this = instance_of(&ud)?;
                let found = {
                    let state = this.ctx.state.borrow();
                    let dm = &state.world.dm;
                    if recursive.unwrap_or(false) {
                        dm.get_descendants(this.id)
                            .into_iter()
                            .find(|id| dm.is_a(*id, &class))
                    } else {
                        dm.find_first_child_which_is_a(this.id, &class)
                    }
                };
                optional_instance(lua, &this.ctx, found)
            },
        )?,
    )?;

    methods.set(
        "FindFirstDescendant",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            let found = this
                .ctx
                .state
                .borrow()
                .world
                .dm
                .find_first_descendant(this.id, &name);
            optional_instance(lua, &this.ctx, found)
        })?,
    )?;

    methods.set(
        "IsA",
        lua.create_function(|_, (ud, class): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            let result = this.ctx.state.borrow().world.dm.is_a(this.id, &class);
            Ok(result)
        })?,
    )?;

    methods.set(
        "GetFullName",
        lua.create_function(|_, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            let result = this.ctx.state.borrow().world.dm.path(this.id, true);
            Ok(result)
        })?,
    )?;

    methods.set(
        "GetAttribute",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            let value = this
                .ctx
                .state
                .borrow()
                .world
                .dm
                .get_attribute(this.id, &name);
            match value {
                Some(value) => {
                    let mut state = this.ctx.state.borrow_mut();
                    convert::attribute_to_lua(lua, &mut state.cache, &this.ctx, &value)
                }
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    methods.set(
        "SetAttribute",
        lua.create_function(|lua, (ud, name, value): (AnyUserData, String, Value)| {
            let this = instance_of(&ud)?;
            let converted = match value {
                Value::Nil => None,
                other => Some(lua_to_attribute(lua, &other)?),
            };
            let events = this
                .ctx
                .state
                .borrow_mut()
                .world
                .dm
                .set_attribute(this.id, &name, converted)
                .map_err(convert::lua_err)?;
            vm::dispatch(lua, &this.ctx, events)?;
            Ok(())
        })?,
    )?;

    methods.set(
        "GetAttributes",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            let attributes = this.ctx.state.borrow().world.dm.get_attributes(this.id);
            let table = lua.create_table()?;
            for (name, value) in attributes {
                let value = {
                    let mut state = this.ctx.state.borrow_mut();
                    convert::attribute_to_lua(lua, &mut state.cache, &this.ctx, &value)?
                };
                table.set(name, value)?;
            }
            Ok(table)
        })?,
    )?;

    methods.set(
        "Destroy",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            let events = this
                .ctx
                .state
                .borrow_mut()
                .world
                .dm
                .destroy(this.id)
                .map_err(convert::lua_err)?;
            vm::dispatch(lua, &this.ctx, events)?;
            Ok(())
        })?,
    )?;

    methods.set(
        "ClearAllChildren",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            let children = this.ctx.state.borrow().world.dm.get_children(this.id);
            for child in children {
                let events = this
                    .ctx
                    .state
                    .borrow_mut()
                    .world
                    .dm
                    .destroy(child)
                    .map_err(convert::lua_err)?;
                vm::dispatch(lua, &this.ctx, events)?;
            }
            Ok(())
        })?,
    )?;

    methods.set(
        "Clone",
        lua.create_function(|lua, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            let copy = this
                .ctx
                .state
                .borrow_mut()
                .world
                .dm
                .clone_instance(this.id, true)
                .map_err(convert::lua_err)?;
            match copy {
                Some(copy) => vm::instance_value(lua, &this.ctx, copy),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    methods.set(
        "GetService",
        lua.create_function(|lua, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            let is_data_model = this.ctx.state.borrow().world.dm.class_of(this.id) == Some("DataModel");
            if !is_data_model {
                return Err(LuaError::runtime(
                    "GetService can only be called on the DataModel",
                ));
            }
            let service = this
                .ctx
                .state
                .borrow_mut()
                .world
                .get_service(&name)
                .map_err(convert::lua_err)?;
            vm::instance_value(lua, &this.ctx, service)
        })?,
    )?;

    methods.set(
        "AddTag",
        lua.create_function(
            |lua, (ud, first, second): (AnyUserData, Value, Option<String>)| {
                let this = instance_of(&ud)?;
                let (target, tag) = tag_target(&this, first, second)?;
                let events = this
                    .ctx
                    .state
                    .borrow_mut()
                    .world
                    .add_tag(target, &tag)
                    .map_err(convert::lua_err)?;
                vm::dispatch(lua, &this.ctx, events)?;
                Ok(())
            },
        )?,
    )?;

    methods.set(
        "RemoveTag",
        lua.create_function(
            |lua, (ud, first, second): (AnyUserData, Value, Option<String>)| {
                let this = instance_of(&ud)?;
                let (target, tag) = tag_target(&this, first, second)?;
                let events = this
                    .ctx
                    .state
                    .borrow_mut()
                    .world
                    .remove_tag(target, &tag)
                    .map_err(convert::lua_err)?;
                vm::dispatch(lua, &this.ctx, events)?;
                Ok(())
            },
        )?,
    )?;

    methods.set(
        "HasTag",
        lua.create_function(
            |_, (ud, first, second): (AnyUserData, Value, Option<String>)| {
                let this = instance_of(&ud)?;
                let (target, tag) = tag_target(&this, first, second)?;
                let result = this
                    .ctx
                    .state
                    .borrow()
                    .world
                    .dm
                    .get(target)
                    .map(|data| data.tags.contains(&tag))
                    .unwrap_or(false);
                Ok(result)
            },
        )?,
    )?;

    methods.set(
        "GetTags",
        lua.create_function(|lua, (ud, target): (AnyUserData, Option<Value>)| {
            let this = instance_of(&ud)?;
            let id = match target {
                Some(Value::UserData(ref other)) => instance_of(other)?.id,
                Some(ref other) => {
                    return Err(LuaError::runtime(format!(
                        "invalid argument: expected Instance, got {}",
                        other.type_name()
                    )))
                }
                None => this.id,
            };
            let tags = this
                .ctx
                .state
                .borrow()
                .world
                .dm
                .get(id)
                .map(|data| data.tags.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let table = lua.create_table()?;
            for (index, tag) in tags.into_iter().enumerate() {
                table.set(index + 1, tag)?;
            }
            Ok(table)
        })?,
    )?;

    lua.set_named_registry_value("microstudio.instance_methods", methods)?;
    Ok(())
}

// two arities: CollectionService:AddTag(inst, tag) and inst:AddTag(tag)
fn tag_target(
    this: &LuaInstance,
    first: Value,
    second: Option<String>,
) -> LuaResult<(InstanceId, String)> {
    match second {
        Some(tag) => match first {
            Value::UserData(ref target) => Ok((instance_of(target)?.id, tag)),
            ref other => Err(LuaError::runtime(format!(
                "invalid argument: expected Instance, got {}",
                other.type_name()
            ))),
        },
        None => match first {
            Value::String(ref tag) => Ok((this.id, tag.to_str()?.to_string())),
            ref other => Err(LuaError::runtime(format!(
                "invalid argument: expected a tag string, got {}",
                other.type_name()
            ))),
        },
    }
}

fn instance_list(lua: &Lua, ctx: &Ctx, ids: Vec<InstanceId>) -> LuaResult<Value> {
    let table = lua.create_table()?;
    for (index, id) in ids.into_iter().enumerate() {
        table.set(index + 1, vm::instance_value(lua, ctx, id)?)?;
    }
    Ok(Value::Table(table))
}

fn optional_instance(lua: &Lua, ctx: &Ctx, id: Option<InstanceId>) -> LuaResult<Value> {
    match id {
        Some(id) => vm::instance_value(lua, ctx, id),
        None => Ok(Value::Nil),
    }
}
