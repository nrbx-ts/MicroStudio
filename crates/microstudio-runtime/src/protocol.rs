// newline-delimited json-rpc: one json object per line, both directions

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub id: u64,
    pub ok: bool,
    pub error: RpcError,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RpcError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traceback: Option<String>,
}

// unsolicited, emitted while a command runs (print, warn, errors)
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EvalParams {
    pub code: String,
    // statement (default), expression, or auto
    pub mode: Option<String>,
    // chunk name used in error locations, e.g. script.luau
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AdvanceTimeParams {
    pub seconds: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddMockPlayerParams {
    pub name: String,
    #[serde(default)]
    pub with_character: bool,
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PathParams {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TreeParams {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: usize,
}

fn default_depth() -> usize {
    4
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SetClockParams {
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunScriptParams {
    // path of one script to run; absent runs every server script
    #[serde(default)]
    pub path: Option<String>,
    // raw source, executed as a server script
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

// document order: parents before children
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceEntry {
    // dotted path below game, e.g. ServerScriptService.TS.server
    pub path: String,
    // class to create: Script, LocalScript, ModuleScript, Folder, ...
    #[serde(default = "default_class")]
    pub class_name: String,
    #[serde(default)]
    pub source: Option<String>,
    // rojo $properties, still in typed form: {"Bool": true}
    #[serde(default)]
    pub properties: std::collections::BTreeMap<String, Value>,
}

fn default_class() -> String {
    "Folder".to_string()
}

// whole compiled project from the rojo reader in @microstudio/roblox-ts
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoadTreeParams {
    #[serde(default)]
    pub entries: Vec<PlaceEntry>,
}
