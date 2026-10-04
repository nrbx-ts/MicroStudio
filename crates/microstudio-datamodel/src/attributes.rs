use serde::{Deserialize, Serialize};

use crate::instance::InstanceId;
use microstudio_types::{BrickColor, CFrame, Color3, Rect, Ray, UDim, UDim2, Vector2, Vector3};

// intentionally missing: sequences, NumberRange, other rare attribute types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum AttributeValue {
    Bool(bool),
    Number(f64),
    String(String),
    Vector2(Vector2),
    Vector3(Vector3),
    CFrame(CFrame),
    Color3(Color3),
    UDim(UDim),
    UDim2(UDim2),
    Rect(Rect),
    BrickColor(BrickColor),
    Ray(Ray),
    Instance(InstanceId),
}

impl AttributeValue {
    // luau typeof() name
    pub fn type_name(&self) -> &'static str {
        match self {
            AttributeValue::Bool(_) => "boolean",
            AttributeValue::Number(_) => "number",
            AttributeValue::String(_) => "string",
            AttributeValue::Vector2(_) => "Vector2",
            AttributeValue::Vector3(_) => "Vector3",
            AttributeValue::CFrame(_) => "CFrame",
            AttributeValue::Color3(_) => "Color3",
            AttributeValue::UDim(_) => "UDim",
            AttributeValue::UDim2(_) => "UDim2",
            AttributeValue::Rect(_) => "Rect",
            AttributeValue::BrickColor(_) => "BrickColor",
            AttributeValue::Ray(_) => "Ray",
            AttributeValue::Instance(_) => "Instance",
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            AttributeValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            AttributeValue::Number(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            AttributeValue::String(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_instance(&self) -> Option<InstanceId> {
        match self {
            AttributeValue::Instance(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_vector3(&self) -> Option<Vector3> {
        match self {
            AttributeValue::Vector3(v) => Some(*v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_match_luau() {
        assert_eq!(AttributeValue::Bool(true).type_name(), "boolean");
        assert_eq!(AttributeValue::Number(1.0).type_name(), "number");
        assert_eq!(AttributeValue::String("x".into()).type_name(), "string");
        assert_eq!(AttributeValue::Vector3(Vector3::one()).type_name(), "Vector3");
    }

    #[test]
    fn round_trips_through_json() {
        let value = AttributeValue::Vector3(Vector3::new(1.0, 2.0, 3.0));
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<AttributeValue>(&json).unwrap(), value);
    }
}
