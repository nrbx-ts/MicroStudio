// newtypes because UserData impl needs a local type; __type is what typeof() reports

use mlua::{
    Error as LuaError, Lua, MetaMethod, MultiValue, Result as LuaResult, Table, UserData,
    UserDataFields, UserDataMethods, Value,
};
use microstudio_types::{BrickColor, CFrame, Color3, Ray, Rect, UDim, UDim2, Vector2, Vector3};

pub fn num(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

// roblox renders 3 decimals, trims trailing zeros (and -0)
pub fn component(value: f64) -> String {
    if !value.is_finite() {
        return num(value);
    }
    // -0 would survive otherwise
    let value = if value == 0.0 { 0.0 } else { value };
    let fixed = format!("{value:.3}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

pub fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(i) => Some(*i as f64),
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

pub fn type_error(expected: &str, value: &Value) -> LuaError {
    LuaError::runtime(format!(
        "invalid argument: expected {expected}, got {}",
        value.type_name()
    ))
}

fn numbers(args: &MultiValue) -> Vec<f64> {
    args.iter().filter_map(as_number).collect()
}

macro_rules! newtype {
    ($name:ident, $inner:ty, $type_name:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct $name(pub $inner);

        impl $name {
            #[inline]
            pub fn get(self) -> $inner {
                self.0
            }
        }

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                $name(value)
            }
        }

        // mlua derives IntoLua for UserData but FromLua must be hand-written
        impl mlua::FromLua for $name {
            fn from_lua(value: Value, _lua: &Lua) -> LuaResult<Self> {
                match value {
                    Value::UserData(ud) => Ok(*ud.borrow::<$name>()?),
                    other => Err(type_error($type_name, &other)),
                }
            }
        }
    };
}

newtype!(LuaVector2, Vector2, "Vector2");
newtype!(LuaVector3, Vector3, "Vector3");
newtype!(LuaColor3, Color3, "Color3");
newtype!(LuaUDim, UDim, "UDim");
newtype!(LuaUDim2, UDim2, "UDim2");
newtype!(LuaRect, Rect, "Rect");
newtype!(LuaBrickColor, BrickColor, "BrickColor");
newtype!(LuaRay, Ray, "Ray");
newtype!(LuaCFrame, CFrame, "CFrame");

// tweeninfo is a value type, so it is a plain struct behind a userdata
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingStyle {
    Linear,
    Sine,
    Back,
    Quad,
    Quart,
    Quint,
    Bounce,
    Elastic,
    Exponential,
    Cubic,
    Circ,
}

// declaration order is the enum's numeric order
const EASING_STYLES: [EasingStyle; 11] = [
    EasingStyle::Linear,
    EasingStyle::Sine,
    EasingStyle::Back,
    EasingStyle::Quad,
    EasingStyle::Quart,
    EasingStyle::Quint,
    EasingStyle::Bounce,
    EasingStyle::Elastic,
    EasingStyle::Exponential,
    EasingStyle::Cubic,
    EasingStyle::Circ,
];

impl EasingStyle {
    pub fn from_name(name: &str) -> Option<Self> {
        EASING_STYLES
            .iter()
            .copied()
            .find(|style| style.name() == name)
    }

    pub fn from_number(value: f64) -> Option<Self> {
        if value < 0.0 || value.fract() != 0.0 {
            return None;
        }
        EASING_STYLES.get(value as usize).copied()
    }

    pub const fn name(self) -> &'static str {
        match self {
            EasingStyle::Linear => "Linear",
            EasingStyle::Sine => "Sine",
            EasingStyle::Back => "Back",
            EasingStyle::Quad => "Quad",
            EasingStyle::Quart => "Quart",
            EasingStyle::Quint => "Quint",
            EasingStyle::Bounce => "Bounce",
            EasingStyle::Elastic => "Elastic",
            EasingStyle::Exponential => "Exponential",
            EasingStyle::Cubic => "Cubic",
            EasingStyle::Circ => "Circ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingDirection {
    In,
    Out,
    InOut,
}

impl EasingDirection {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "In" => EasingDirection::In,
            "Out" => EasingDirection::Out,
            "InOut" => EasingDirection::InOut,
            _ => return None,
        })
    }

    pub fn from_number(value: f64) -> Option<Self> {
        match value as i64 {
            0 => Some(EasingDirection::In),
            1 => Some(EasingDirection::Out),
            2 => Some(EasingDirection::InOut),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            EasingDirection::In => "In",
            EasingDirection::Out => "Out",
            EasingDirection::InOut => "InOut",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TweenInfo {
    pub time: f64,
    pub easing_style: EasingStyle,
    pub easing_direction: EasingDirection,
    pub repeat_count: i64,
    pub reverses: bool,
    pub delay_time: f64,
}

impl Default for TweenInfo {
    fn default() -> Self {
        // the defaults roblox documents for TweenInfo.new
        Self {
            time: 1.0,
            easing_style: EasingStyle::Quad,
            easing_direction: EasingDirection::Out,
            repeat_count: 0,
            reverses: false,
            delay_time: 0.0,
        }
    }
}

newtype!(LuaTweenInfo, TweenInfo, "TweenInfo");

impl UserData for LuaTweenInfo {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Time", |_, this| Ok(this.0.time));
        fields.add_field_method_get("EasingStyle", |lua, this| {
            enum_item(lua, "EasingStyle", this.0.easing_style.name())
        });
        fields.add_field_method_get("EasingDirection", |lua, this| {
            enum_item(lua, "EasingDirection", this.0.easing_direction.name())
        });
        fields.add_field_method_get("RepeatCount", |_, this| Ok(this.0.repeat_count));
        fields.add_field_method_get("Reverses", |_, this| Ok(this.0.reverses));
        fields.add_field_method_get("DelayTime", |_, this| Ok(this.0.delay_time));
        fields.add_meta_field(MetaMethod::Type, "TweenInfo");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => Ok(ud
                .borrow::<LuaTweenInfo>()
                .map(|other| this.0 == other.0)
                .unwrap_or(false)),
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!(
                "{}, Enum.EasingStyle.{}, Enum.EasingDirection.{}, {}, {}, {}",
                num(this.0.time),
                this.0.easing_style.name(),
                this.0.easing_direction.name(),
                this.0.repeat_count,
                this.0.reverses,
                num(this.0.delay_time)
            ))
        });
    }
}

fn tween_info_new(_lua: &Lua, args: MultiValue) -> LuaResult<LuaTweenInfo> {
    let mut info = TweenInfo::default();
    let mut args = args.into_iter();

    if let Some(value) = args.next().filter(|value| !value.is_nil()) {
        info.time = number_or("time", &value)?;
    }
    if let Some(value) = args.next().filter(|value| !value.is_nil()) {
        info.easing_style = easing_style_or(&value)?;
    }
    if let Some(value) = args.next().filter(|value| !value.is_nil()) {
        info.easing_direction = easing_direction_or(&value)?;
    }
    if let Some(value) = args.next().filter(|value| !value.is_nil()) {
        info.repeat_count = match as_number(&value) {
            Some(number) => number as i64,
            None => return Err(type_error("number", &value)),
        };
    }
    if let Some(value) = args.next().filter(|value| !value.is_nil()) {
        info.reverses = match value {
            Value::Boolean(value) => value,
            other => return Err(type_error("boolean", &other)),
        };
    }
    if let Some(value) = args.next().filter(|value| !value.is_nil()) {
        info.delay_time = number_or("delayTime", &value)?;
    }
    Ok(LuaTweenInfo(info))
}

fn number_or(name: &str, value: &Value) -> LuaResult<f64> {
    as_number(value).ok_or_else(|| {
        LuaError::runtime(format!("TweenInfo.new expects a number for {name}"))
    })
}

fn easing_style_or(value: &Value) -> LuaResult<EasingStyle> {
    let found = match value {
        Value::Integer(number) => EasingStyle::from_number(*number as f64),
        Value::Number(number) => EasingStyle::from_number(*number),
        other => EasingStyle::from_name(&enum_name(other)?),
    };
    found.ok_or_else(|| LuaError::runtime("TweenInfo.new expects an Enum.EasingStyle"))
}

fn easing_direction_or(value: &Value) -> LuaResult<EasingDirection> {
    let found = match value {
        Value::Integer(number) => EasingDirection::from_number(*number as f64),
        Value::Number(number) => EasingDirection::from_number(*number),
        other => EasingDirection::from_name(&enum_name(other)?),
    };
    found.ok_or_else(|| LuaError::runtime("TweenInfo.new expects an Enum.EasingDirection"))
}

// the prelude materialises enum items on demand, so ask it for one by name
pub(crate) fn enum_item(lua: &Lua, enum_type: &str, item: &str) -> LuaResult<Value> {
    let globals = lua.globals();
    let enum_table: Table = globals.get("Enum")?;
    let type_table: Table = enum_table.get(enum_type)?;
    type_table.get(item)
}

pub fn take_vector3(value: &Value) -> LuaResult<Vector3> {
    match value {
        Value::UserData(ud) => ud
            .borrow::<LuaVector3>()
            .map(|v| v.0)
            .map_err(|_| type_error("Vector3", value)),
        _ => Err(type_error("Vector3", value)),
    }
}

macro_rules! vector_type {
    ($lua_name:ident, $type_name:literal, ($($field:ident => $lua_field:literal),+)) => {
        impl UserData for $lua_name {
            fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
                $(
                    fields.add_field_method_get($lua_field, |_, this| Ok(this.0.$field));
                )+
                fields.add_field_method_get("Magnitude", |_, this| Ok(this.0.magnitude()));
                fields.add_field_method_get("Unit", |_, this| Ok($lua_name(this.0.unit())));
                fields.add_meta_field(MetaMethod::Type, $type_name);
            }

            fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
                methods.add_method("Dot", |_, this, other: $lua_name| Ok(this.0.dot(&other.0)));
                methods.add_method("Lerp", |_, this, (other, alpha): ($lua_name, f64)| {
                    Ok($lua_name(this.0.lerp(&other.0, alpha)))
                });
                methods.add_method("FuzzyEq", |_, this, other: $lua_name| {
                    Ok(this.0.fuzzy_eq(&other.0))
                });
                methods.add_method("Abs", |_, this, ()| Ok($lua_name(this.0.abs())));
                methods.add_method("Min", |_, this, other: $lua_name| {
                    Ok($lua_name(this.0.min(other.0)))
                });
                methods.add_method("Max", |_, this, other: $lua_name| {
                    Ok($lua_name(this.0.max(other.0)))
                });

                methods.add_meta_method(MetaMethod::Add, |_, this, other: $lua_name| {
                    Ok($lua_name(this.0 + other.0))
                });
                methods.add_meta_method(MetaMethod::Sub, |_, this, other: $lua_name| {
                    Ok($lua_name(this.0 - other.0))
                });
                methods.add_meta_method(MetaMethod::Unm, |_, this, ()| Ok($lua_name(-this.0)));
                methods.add_meta_method(MetaMethod::Mul, |_, this, other: Value| match other {
                    Value::Integer(i) => Ok($lua_name(this.0 * i as f64)),
                    Value::Number(n) => Ok($lua_name(this.0 * n)),
                    Value::UserData(ref ud) => match ud.borrow::<$lua_name>() {
                        Ok(v) => Ok($lua_name(this.0.mul_componentwise(v.0))),
                        Err(_) => Err(type_error($type_name, &other)),
                    },
                    _ => Err(type_error("number or Vector", &other)),
                });
                methods.add_meta_method(MetaMethod::Div, |_, this, other: Value| match other {
                    Value::Integer(i) => Ok($lua_name(this.0 / i as f64)),
                    Value::Number(n) => Ok($lua_name(this.0 / n)),
                    _ => Err(type_error("number", &other)),
                });
                methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
                    Value::UserData(ref ud) => {
                        Ok(ud.borrow::<$lua_name>().map(|o| this.0 == o.0).unwrap_or(false))
                    }
                    _ => Ok(false),
                });
                methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
                    Ok([$(component(this.0.$field)),+].join(", "))
                });
            }
        }
    };
}

vector_type!(LuaVector2, "Vector2", (x => "X", y => "Y"));
vector_type!(LuaVector3, "Vector3", (x => "X", y => "Y", z => "Z"));

impl LuaVector2 {
    // vector2 promotes to vector3 with z = 0, like roblox
    pub fn to_vector3(self) -> Vector3 {
        Vector3::new(self.0.x, self.0.y, 0.0)
    }
}

impl UserData for LuaColor3 {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("R", |_, this| Ok(this.0.r));
        fields.add_field_method_get("G", |_, this| Ok(this.0.g));
        fields.add_field_method_get("B", |_, this| Ok(this.0.b));
        fields.add_meta_field(MetaMethod::Type, "Color3");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Lerp", |_, this, (other, alpha): (LuaColor3, f64)| {
            Ok(LuaColor3(this.0.lerp(&other.0, alpha)))
        });
        methods.add_method("ToHSV", |lua, this, ()| {
            let hsv = this.0.to_hsv();
            let table = lua.create_table()?;
            table.set("H", hsv.h)?;
            table.set("S", hsv.s)?;
            table.set("V", hsv.v)?;
            Ok(table)
        });
        methods.add_method("ToHex", |_, this, ()| Ok(this.0.to_hex()));
        methods.add_meta_method(MetaMethod::Add, |_, this, other: LuaColor3| {
            Ok(LuaColor3(this.0 + other.0))
        });
        methods.add_meta_method(MetaMethod::Sub, |_, this, other: LuaColor3| {
            Ok(LuaColor3(this.0 - other.0))
        });
        methods.add_meta_method(MetaMethod::Mul, |_, this, other: Value| match other {
            Value::Integer(i) => Ok(LuaColor3(this.0 * i as f64)),
            Value::Number(n) => Ok(LuaColor3(this.0 * n)),
            Value::UserData(ref ud) => match ud.borrow::<LuaColor3>() {
                Ok(c) => Ok(LuaColor3(this.0 * c.0)),
                Err(_) => Err(type_error("Color3", &other)),
            },
            _ => Err(type_error("number or Color3", &other)),
        });
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => Ok(ud.borrow::<LuaColor3>().map(|o| this.0 == o.0).unwrap_or(false)),
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("{}, {}, {}", component(this.0.r), component(this.0.g), component(this.0.b)))
        });
    }
}

impl UserData for LuaUDim {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Scale", |_, this| Ok(this.0.scale));
        fields.add_field_method_get("Offset", |_, this| Ok(this.0.offset));
        fields.add_meta_field(MetaMethod::Type, "UDim");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Add, |_, this, other: LuaUDim| {
            Ok(LuaUDim(this.0 + other.0))
        });
        methods.add_meta_method(MetaMethod::Sub, |_, this, other: LuaUDim| {
            Ok(LuaUDim(this.0 - other.0))
        });
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => Ok(ud.borrow::<LuaUDim>().map(|o| this.0 == o.0).unwrap_or(false)),
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("{{{}, {}}}", component(this.0.scale), this.0.offset))
        });
    }
}

impl UserData for LuaUDim2 {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("X", |_, this| Ok(LuaUDim(this.0.x)));
        fields.add_field_method_get("Y", |_, this| Ok(LuaUDim(this.0.y)));
        fields.add_field_method_get("Width", |_, this| Ok(LuaUDim(this.0.x)));
        fields.add_field_method_get("Height", |_, this| Ok(LuaUDim(this.0.y)));
        fields.add_meta_field(MetaMethod::Type, "UDim2");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Lerp", |_, this, (other, alpha): (LuaUDim2, f64)| {
            Ok(LuaUDim2(this.0.lerp(&other.0, alpha)))
        });
        methods.add_meta_method(MetaMethod::Add, |_, this, other: LuaUDim2| {
            Ok(LuaUDim2(this.0 + other.0))
        });
        methods.add_meta_method(MetaMethod::Sub, |_, this, other: LuaUDim2| {
            Ok(LuaUDim2(this.0 - other.0))
        });
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => Ok(ud.borrow::<LuaUDim2>().map(|o| this.0 == o.0).unwrap_or(false)),
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!(
                "{{{}, {}}}, {{{}, {}}}",
                component(this.0.x.scale),
                this.0.x.offset,
                component(this.0.y.scale),
                this.0.y.offset
            ))
        });
    }
}

impl UserData for LuaRect {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Min", |_, this| Ok(LuaVector2(this.0.min)));
        fields.add_field_method_get("Max", |_, this| Ok(LuaVector2(this.0.max)));
        fields.add_field_method_get("Width", |_, this| Ok(this.0.width()));
        fields.add_field_method_get("Height", |_, this| Ok(this.0.height()));
        fields.add_meta_field(MetaMethod::Type, "Rect");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => Ok(ud.borrow::<LuaRect>().map(|o| this.0 == o.0).unwrap_or(false)),
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!(
                "{}, {}, {}, {}",
                component(this.0.min.x),
                component(this.0.min.y),
                component(this.0.max.x),
                component(this.0.max.y)
            ))
        });
    }
}

impl UserData for LuaBrickColor {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Number", |_, this| Ok(this.0.number()));
        fields.add_field_method_get("Name", |_, this| Ok(this.0.name()));
        fields.add_field_method_get("Color", |_, this| Ok(LuaColor3(this.0.color())));
        fields.add_field_method_get("r", |_, this| Ok(this.0.r()));
        fields.add_field_method_get("g", |_, this| Ok(this.0.g()));
        fields.add_field_method_get("b", |_, this| Ok(this.0.b()));
        fields.add_meta_field(MetaMethod::Type, "BrickColor");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => {
                Ok(ud.borrow::<LuaBrickColor>().map(|o| this.0 == o.0).unwrap_or(false))
            }
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            // roblox prints just the palette name
            Ok(this.0.name().to_string())
        });
    }
}

impl UserData for LuaRay {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Origin", |_, this| Ok(LuaVector3(this.0.origin)));
        fields.add_field_method_get("Direction", |_, this| Ok(LuaVector3(this.0.direction)));
        fields.add_field_method_get("Unit", |_, this| Ok(LuaVector3(this.0.unit_direction())));
        fields.add_meta_field(MetaMethod::Type, "Ray");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("ClosestPoint", |_, this, point: LuaVector3| {
            Ok(LuaVector3(this.0.closest_point(point.0)))
        });
        methods.add_method("Distance", |_, this, point: LuaVector3| {
            Ok(this.0.distance(point.0))
        });
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => Ok(ud.borrow::<LuaRay>().map(|o| this.0 == o.0).unwrap_or(false)),
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            let o = this.0.origin;
            let d = this.0.direction;
            Ok(format!(
                "{{{}, {}, {}}}, {{{}, {}, {}}}",
                num(o.x),
                num(o.y),
                num(o.z),
                num(d.x),
                num(d.y),
                num(d.z)
            ))
        });
    }
}

impl UserData for LuaCFrame {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("Position", |_, this| Ok(LuaVector3(this.0.position())));
        fields.add_field_method_get("X", |_, this| Ok(this.0.x()));
        fields.add_field_method_get("Y", |_, this| Ok(this.0.y()));
        fields.add_field_method_get("Z", |_, this| Ok(this.0.z()));
        fields.add_field_method_get("LookVector", |_, this| Ok(LuaVector3(this.0.look_vector())));
        fields.add_field_method_get("RightVector", |_, this| Ok(LuaVector3(this.0.right_vector())));
        fields.add_field_method_get("UpVector", |_, this| Ok(LuaVector3(this.0.up_vector())));
        fields.add_meta_field(MetaMethod::Type, "CFrame");
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("Inverse", |_, this, ()| Ok(LuaCFrame(this.0.inverse())));
        methods.add_method("ToWorldSpace", |_, this, other: LuaCFrame| {
            Ok(LuaCFrame(this.0.to_world_space(other.0)))
        });
        methods.add_method("ToObjectSpace", |_, this, other: LuaCFrame| {
            Ok(LuaCFrame(this.0.to_object_space(other.0)))
        });
        methods.add_method("PointToWorldSpace", |_, this, point: LuaVector3| {
            Ok(LuaVector3(this.0.point_to_world_space(point.0)))
        });
        methods.add_method("PointToObjectSpace", |_, this, point: LuaVector3| {
            Ok(LuaVector3(this.0.point_to_object_space(point.0)))
        });
        methods.add_method("VectorToWorldSpace", |_, this, v: LuaVector3| {
            Ok(LuaVector3(this.0.vector_to_world_space(v.0)))
        });
        methods.add_method("VectorToObjectSpace", |_, this, v: LuaVector3| {
            Ok(LuaVector3(this.0.vector_to_object_space(v.0)))
        });
        methods.add_method("Lerp", |_, this, (other, alpha): (LuaCFrame, f64)| {
            Ok(LuaCFrame(this.0.lerp(other.0, alpha)))
        });
        methods.add_method("FuzzyEq", |_, this, other: LuaCFrame| Ok(this.0.fuzzy_eq(&other.0)));
        methods.add_method("ToEulerAnglesXYZ", |_, this, ()| {
            let e = this.0.to_euler_angles_xyz();
            Ok(LuaVector3(Vector3::new(e.x, e.y, e.z)))
        });
        methods.add_method("ToOrientation", |_, this, ()| {
            let e = this.0.to_euler_angles_xyz();
            Ok(LuaVector3(Vector3::new(e.x, e.y, e.z)))
        });
        methods.add_method("GetComponents", |lua, this, ()| {
            let p = this.0.position();
            let r = this.0.rotation_components();
            lua.create_sequence_from([
                p.x, p.y, p.z, r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[8],
            ])
        });

        methods.add_meta_method(MetaMethod::Mul, |lua, this, other: Value| {
            if let Value::UserData(ref ud) = other {
                if let Ok(cf) = ud.borrow::<LuaCFrame>() {
                    return Ok(Value::UserData(
                        lua.create_userdata(LuaCFrame(this.0 * cf.0))?,
                    ));
                }
                if let Ok(v) = ud.borrow::<LuaVector3>() {
                    return Ok(Value::UserData(
                        lua.create_userdata(LuaVector3(this.0 * v.0))?,
                    ));
                }
            }
            Err(type_error("CFrame or Vector3", &other))
        });
        methods.add_meta_method(MetaMethod::Add, |_, this, other: LuaVector3| {
            Ok(LuaCFrame(this.0.with_position(this.0.position() + other.0)))
        });
        methods.add_meta_method(MetaMethod::Sub, |_, this, other: LuaVector3| {
            Ok(LuaCFrame(this.0.with_position(this.0.position() - other.0)))
        });
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: Value| match other {
            Value::UserData(ref ud) => {
                Ok(ud.borrow::<LuaCFrame>().map(|o| this.0 == o.0).unwrap_or(false))
            }
            _ => Ok(false),
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            // roblox renders 12 components: position then row-major rotation matrix
            let p = this.0.position();
            let r = this.0.rotation_components();
            let mut parts = vec![component(p.x), component(p.y), component(p.z)];
            parts.extend(r.iter().map(|value| component(*value)));
            Ok(parts.join(", "))
        });
    }
}
fn clamp_byte(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

pub(crate) fn enum_name(value: &Value) -> LuaResult<String> {
    match value {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Table(table) => table.get::<String>("Name"),
        other => Err(type_error("EnumItem or string", other)),
    }
}

pub fn install(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();

    let vector3 = lua.create_table()?;
    vector3.set(
        "new",
        lua.create_function(|_, args: MultiValue| {
            let values = numbers(&args);
            let (x, y, z) = match values.as_slice() {
                [] => (0.0, 0.0, 0.0),
                [x] => (*x, *x, *x),
                [x, y] => (*x, *y, 0.0),
                [x, y, z, ..] => (*x, *y, *z),
            };
            Ok(LuaVector3(Vector3::new(x, y, z)))
        })?,
    )?;
    vector3.set("zero", LuaVector3(Vector3::zero()))?;
    vector3.set("one", LuaVector3(Vector3::one()))?;
    vector3.set("xAxis", LuaVector3(Vector3::X_AXIS))?;
    vector3.set("yAxis", LuaVector3(Vector3::Y_AXIS))?;
    vector3.set("zAxis", LuaVector3(Vector3::Z_AXIS))?;
    vector3.set(
        "FromNormalId",
        lua.create_function(|_, value: Value| {
            Ok(LuaVector3(match enum_name(&value)?.as_str() {
                "Top" => Vector3::Y_AXIS,
                "Bottom" => -Vector3::Y_AXIS,
                "Left" => -Vector3::X_AXIS,
                "Right" => Vector3::X_AXIS,
                "Front" => Vector3::new(0.0, 0.0, -1.0),
                "Back" => Vector3::Z_AXIS,
                other => return Err(LuaError::runtime(format!("unknown NormalId '{other}'"))),
            }))
        })?,
    )?;
    vector3.set(
        "FromAxis",
        lua.create_function(|_, value: Value| {
            Ok(LuaVector3(match enum_name(&value)?.as_str() {
                "X" => Vector3::X_AXIS,
                "Y" => Vector3::Y_AXIS,
                "Z" => Vector3::Z_AXIS,
                other => return Err(LuaError::runtime(format!("unknown Axis '{other}'"))),
            }))
        })?,
    )?;
    globals.set("Vector3", vector3)?;

    let vector2 = lua.create_table()?;
    vector2.set(
        "new",
        lua.create_function(|_, args: MultiValue| {
            let values = numbers(&args);
            let (x, y) = match values.as_slice() {
                [] => (0.0, 0.0),
                [x] => (*x, *x),
                [x, y, ..] => (*x, *y),
            };
            Ok(LuaVector2(Vector2::new(x, y)))
        })?,
    )?;
    vector2.set("zero", LuaVector2(Vector2::zero()))?;
    vector2.set("one", LuaVector2(Vector2::one()))?;
    globals.set("Vector2", vector2)?;

    let color3 = lua.create_table()?;
    color3.set(
        "new",
        lua.create_function(|_, args: MultiValue| {
            let values = numbers(&args);
            let (r, g, b) = match values.as_slice() {
                [] => (0.0, 0.0, 0.0),
                [r] => (*r, *r, *r),
                [r, g] => (*r, *g, 0.0),
                [r, g, b, ..] => (*r, *g, *b),
            };
            Ok(LuaColor3(Color3::new(r, g, b)))
        })?,
    )?;
    color3.set(
        "fromRGB",
        lua.create_function(|_, (r, g, b): (f64, f64, f64)| {
            Ok(LuaColor3(Color3::from_rgb(
                clamp_byte(r),
                clamp_byte(g),
                clamp_byte(b),
            )))
        })?,
    )?;
    color3.set(
        "fromHSV",
        lua.create_function(|_, (h, s, v): (f64, f64, f64)| {
            Ok(LuaColor3(Color3::from_hsv(h, s, v)))
        })?,
    )?;
    color3.set(
        "fromHex",
        lua.create_function(|_, hex: String| {
            Color3::from_hex(&hex)
                .map(LuaColor3)
                .ok_or_else(|| LuaError::runtime(format!("invalid hex colour '{hex}'")))
        })?,
    )?;
    globals.set("Color3", color3)?;

    let udim = lua.create_table()?;
    udim.set(
        "new",
        lua.create_function(|_, (scale, offset): (f64, i32)| Ok(LuaUDim(UDim::new(scale, offset))))?,
    )?;
    globals.set("UDim", udim)?;

    let udim2 = lua.create_table()?;
    udim2.set(
        "new",
        lua.create_function(|_, (xs, xo, ys, yo): (f64, i32, f64, i32)| {
            Ok(LuaUDim2(UDim2::new(xs, xo, ys, yo)))
        })?,
    )?;
    udim2.set(
        "fromScale",
        lua.create_function(|_, (xs, ys): (f64, f64)| Ok(LuaUDim2(UDim2::from_scale(xs, ys))))?,
    )?;
    udim2.set(
        "fromOffset",
        lua.create_function(|_, (xo, yo): (i32, i32)| Ok(LuaUDim2(UDim2::from_offset(xo, yo))))?,
    )?;
    globals.set("UDim2", udim2)?;

    let rect = lua.create_table()?;
    rect.set(
        "new",
        lua.create_function(|_, args: MultiValue| {
            let values = numbers(&args);
            match values.as_slice() {
                [min_x, min_y, max_x, max_y, ..] => Ok(LuaRect(Rect::from_min_max(
                    *min_x, *min_y, *max_x, *max_y,
                ))),
                _ => Err(LuaError::runtime(
                    "Rect.new expects four numbers: minX, minY, maxX, maxY",
                )),
            }
        })?,
    )?;
    globals.set("Rect", rect)?;

    let brick = lua.create_table()?;
    brick.set(
        "new",
        lua.create_function(|_, value: Value| match value {
            Value::Integer(i) => Ok(LuaBrickColor(BrickColor::from_number(i.max(0) as u16))),
            Value::Number(n) => Ok(LuaBrickColor(BrickColor::from_number(n.max(0.0) as u16))),
            Value::String(s) => {
                let name = s.to_str()?.to_string();
                BrickColor::from_name(&name)
                    .map(LuaBrickColor)
                    .ok_or_else(|| LuaError::runtime(format!("unknown BrickColor '{name}'")))
            }
            other => Err(type_error("number or string", &other)),
        })?,
    )?;
    brick.set(
        "palette",
        lua.create_function(|_, number: u16| Ok(LuaBrickColor(BrickColor::from_number(number))))?,
    )?;
    globals.set("BrickColor", brick)?;

    let ray = lua.create_table()?;
    ray.set(
        "new",
        lua.create_function(|_, (origin, direction): (Value, Value)| {
            Ok(LuaRay(Ray::new(take_vector3(&origin)?, take_vector3(&direction)?)))
        })?,
    )?;
    globals.set("Ray", ray)?;

    let tween_info = lua.create_table()?;
    tween_info.set("new", lua.create_function(tween_info_new)?)?;
    globals.set("TweenInfo", tween_info)?;

    let cframe = lua.create_table()?;
    cframe.set(
        "new",
        lua.create_function(|_, args: MultiValue| {
            let values: Vec<Value> = args.into_iter().collect();
            match values.len() {
                0 => Ok(LuaCFrame(CFrame::identity())),
                1 => Ok(LuaCFrame(CFrame::new(take_vector3(&values[0])?))),
                2 => Ok(LuaCFrame(CFrame::look_at(
                    take_vector3(&values[0])?,
                    take_vector3(&values[1])?,
                    Vector3::Y_AXIS,
                ))),
                3 => {
                    let n: Vec<f64> = values.iter().filter_map(as_number).collect();
                    match n.as_slice() {
                        [x, y, z] => Ok(LuaCFrame(CFrame::new(Vector3::new(*x, *y, *z)))),
                        _ => Err(LuaError::runtime("CFrame.new expects three numbers")),
                    }
                }
                _ => Err(LuaError::runtime(
                    "CFrame.new accepts (), (Vector3), (Vector3, Vector3) or (x, y, z)",
                )),
            }
        })?,
    )?;
    cframe.set(
        "lookAt",
        lua.create_function(|_, (position, target, up): (Value, Value, Option<Value>)| {
            let up = match up {
                Some(ref v) => take_vector3(v)?,
                None => Vector3::Y_AXIS,
            };
            Ok(LuaCFrame(CFrame::look_at(
                take_vector3(&position)?,
                take_vector3(&target)?,
                up,
            )))
        })?,
    )?;
    cframe.set(
        "Angles",
        lua.create_function(|_, (x, y, z): (f64, f64, f64)| {
            Ok(LuaCFrame(CFrame::from_euler_angles(x, y, z)))
        })?,
    )?;
    cframe.set(
        "fromEulerAnglesXYZ",
        lua.create_function(|_, (x, y, z): (f64, f64, f64)| {
            Ok(LuaCFrame(CFrame::from_euler_angles(x, y, z)))
        })?,
    )?;
    cframe.set(
        "fromAxisAngle",
        lua.create_function(|_, (axis, angle): (Value, f64)| {
            Ok(LuaCFrame(CFrame::from_axis_angle(take_vector3(&axis)?, angle)))
        })?,
    )?;
    cframe.set(
        "fromMatrix",
        lua.create_function(|_, (position, right, up): (Value, Value, Value)| {
            Ok(LuaCFrame(CFrame::from_basis(
                take_vector3(&position)?,
                take_vector3(&right)?,
                take_vector3(&up)?,
            )))
        })?,
    )?;
    cframe.set("identity", LuaCFrame(CFrame::identity()))?;
    globals.set("CFrame", cframe)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting_matches_luau() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-3.0), "-3");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(0.25), "0.25");
    }

    #[test]
    fn datatype_components_use_roblox_precision() {
        // 3 decimals, trailing zeros trimmed, no -0
        assert_eq!(component(1.0), "1");
        assert_eq!(component(0.0), "0");
        assert_eq!(component(-0.0), "0");
        assert_eq!(component(-1.0), "-1");
        assert_eq!(component(1.0 / 3.0), "0.333");
        assert_eq!(component(128.0 / 255.0), "0.502");
        // rotated cframe cosine error must not leak into output
        assert_eq!(component(6.123233995736766e-17), "0");
        assert_eq!(component(0.5), "0.5");
    }

    #[test]
    fn byte_clamping() {
        assert_eq!(clamp_byte(-5.0), 0);
        assert_eq!(clamp_byte(300.0), 255);
        assert_eq!(clamp_byte(127.6), 128);
    }
}