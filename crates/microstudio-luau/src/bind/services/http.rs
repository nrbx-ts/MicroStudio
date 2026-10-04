// HttpService: real requests, real json, real guids

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mlua::{AnyUserData, Error as LuaError, Lua, LuaSerdeExt, Result as LuaResult, Table, Value};
use microstudio_datamodel::AttributeValue;
use serde_json::Value as Json;

use crate::bind::instance::{instance_of, LuaInstance};
use crate::bind::services::{methods, require_class};
use crate::vm::Ctx;

const TIMEOUT: Duration = Duration::from_secs(30);
const SECRETS: &str = "secrets.json";

pub fn install(lua: &Lua, _ctx: &Ctx) -> LuaResult<()> {
    let methods = methods(lua)?;

    methods.set(
        "JSONEncode",
        lua.create_function(|lua, (_ud, value): (AnyUserData, Value)| {
            let json: Json = lua
                .from_value(value)
                .map_err(|error| LuaError::runtime(error.to_string()))?;
            serde_json::to_string(&json).map_err(|error| LuaError::runtime(error.to_string()))
        })?,
    )?;

    methods.set(
        "JSONDecode",
        lua.create_function(|lua, (_ud, text): (AnyUserData, String)| {
            let json: Json = serde_json::from_str(&text)
                .map_err(|error| LuaError::runtime(format!("Invalid JSON: {error}")))?;
            lua.to_value(&json)
                .map_err(|error| LuaError::runtime(error.to_string()))
        })?,
    )?;

    methods.set(
        "GenerateGUID",
        lua.create_function(|_, (_ud, wrap): (AnyUserData, Option<bool>)| {
            let guid = guid();
            Ok(if wrap.unwrap_or(true) {
                format!("{{{guid}}}")
            } else {
                guid
            })
        })?,
    )?;

    methods.set(
        "UrlEncode",
        lua.create_function(|_, (_ud, text): (AnyUserData, String)| Ok(url_encode(&text)))?,
    )?;

    methods.set(
        "UrlDecode",
        lua.create_function(|_, (_ud, text): (AnyUserData, String)| {
            Ok(url_decode(&text))
        })?,
    )?;

    methods.set(
        "GetHttpEnabled",
        lua.create_function(|_, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "HttpService")?;
            Ok(http_enabled(&this))
        })?,
    )?;

    methods.set(
        "SetHttpEnabled",
        lua.create_function(|_, (ud, enabled): (AnyUserData, bool)| {
            let this = instance_of(&ud)?;
            require_class(&this, "HttpService")?;
            this.ctx
                .state
                .borrow_mut()
                .world
                .dm
                .set_property(this.id, "HttpEnabled", AttributeValue::Bool(enabled))
                .map_err(crate::convert::lua_err)?;
            Ok(())
        })?,
    )?;

    methods.set(
        "GetUserAgent",
        lua.create_function(|_, ud: AnyUserData| {
            let this = instance_of(&ud)?;
            require_class(&this, "HttpService")?;
            Ok(user_agent())
        })?,
    )?;

    methods.set(
        "GetSecret",
        lua.create_function(|lua, (ud, name): (AnyUserData, Option<String>)| {
            let this = instance_of(&ud)?;
            require_class(&this, "HttpService")?;
            let name = name.unwrap_or_else(|| "default".to_string());
            let found = {
                let state = this.ctx.state.borrow();
                read_secret(&state.world.store, &name)
            };
            match found {
                Some(value) => Ok(Value::String(lua.create_string(value)?)),
                None => Err(LuaError::runtime(format!(
                    "HttpService:GetSecret: no secret named {name:?}. set {} or add it to {}",
                    env_key(&name),
                    SECRETS
                ))),
            }
        })?,
    )?;

    methods.set(
        "SetSecret",
        lua.create_function(|_, (ud, name, value): (AnyUserData, String, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "HttpService")?;
            let state = this.ctx.state.borrow();
            write_secret(&state.world.store, &name, &value).map_err(crate::convert::lua_err)?;
            Ok(())
        })?,
    )?;

    methods.set(
        "RequestAsync",
        lua.create_function(|lua, (ud, options): (AnyUserData, Table)| {
            let this = instance_of(&ud)?;
            require_class(&this, "HttpService")?;
            ensure_enabled(&this)?;
            let request = parse_request(lua, &options)?;
            let response = send(&request)?;
            response_table(lua, response)
        })?,
    )?;

    for (name, method) in [("GetAsync", "GET"), ("PostAsync", "POST")] {
        methods.set(
            name,
            lua.create_function(move |_, args: mlua::MultiValue| simple_request(name, method, args))?,
        )?;
    }

    Ok(())
}

// GetAsync and PostAsync return the body, or raise: only RequestAsync reports failures
fn simple_request(name: &'static str, method: &'static str, args: mlua::MultiValue) -> LuaResult<String> {
    let mut args = args.into_iter();
    let receiver = match args.next() {
        Some(Value::UserData(ud)) => ud,
        _ => return Err(LuaError::runtime("this method must be called on a service")),
    };
    let this = instance_of(&receiver)?;
    require_class(&this, "HttpService")?;
    ensure_enabled(&this)?;

    let url = match args.next() {
        Some(Value::String(text)) => text.to_str()?.to_string(),
        _ => {
            return Err(LuaError::runtime(format!(
                "HttpService:{name} expects a URL string"
            )))
        }
    };

    // only PostAsync carries a body; GetAsync's nocache and headers have no local meaning
    let body = if method == "POST" {
        match args.next() {
            None | Some(Value::Nil) => None,
            Some(Value::String(text)) => Some(text.as_bytes().to_vec()),
            Some(other) => {
                return Err(LuaError::runtime(format!(
                    "HttpService:{name} expects a string body, got {}",
                    other.type_name()
                )))
            }
        }
    } else {
        None
    };

    let request = HttpRequest {
        url: validate_url(&url)?,
        method: method.to_string(),
        headers: Vec::new(),
        body,
    };
    let response = send(&request)?;
    if !(200..300).contains(&response.status) {
        return Err(LuaError::runtime(format!(
            "HttpError: {} {} ({})",
            method, request.url, response.status
        )));
    }
    Ok(response.body)
}

fn http_enabled(this: &LuaInstance) -> bool {
    let state = this.ctx.state.borrow();
    match state.world.dm.get_property(this.id, "HttpEnabled") {
        Some(AttributeValue::Bool(value)) => value,
        _ => true,
    }
}

fn ensure_enabled(this: &LuaInstance) -> LuaResult<()> {
    if http_enabled(this) {
        return Ok(());
    }
    Err(LuaError::runtime(
        "Http requests are not enabled. call HttpService:SetHttpEnabled(true) or set HttpEnabled",
    ))
}

fn user_agent() -> String {
    format!("Roblox/WinInet MicroStudio/{}", env!("CARGO_PKG_VERSION"))
}

struct HttpRequest {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

struct HttpResponse {
    status: u16,
    status_message: String,
    headers: Vec<(String, String)>,
    body: String,
}

// the dictionary RequestAsync hands back, success included rather than raised
fn response_table(lua: &Lua, response: HttpResponse) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set("Success", (200..300).contains(&response.status))?;
    table.set("StatusCode", response.status)?;
    table.set("StatusMessage", response.status_message)?;
    table.set("Body", response.body)?;

    let headers = lua.create_table()?;
    for (name, value) in response.headers {
        let name = name.to_ascii_lowercase();
        // a repeated header is joined, the way roblox reports them
        let merged = match headers.get::<Option<String>>(name.as_str())? {
            Some(existing) => format!("{existing}, {value}"),
            None => value,
        };
        headers.set(name, merged)?;
    }
    table.set("Headers", headers)?;
    Ok(table)
}

fn parse_request(lua: &Lua, options: &Table) -> LuaResult<HttpRequest> {
    let url: String = options.get("Url").map_err(|_| {
        LuaError::runtime("RequestAsync expects a options table with a Url field")
    })?;

    let method: Option<String> = options.get("Method")?;

    let mut headers = Vec::new();
    if let Some(table) = options.get::<Option<Table>>("Headers")? {
        for pair in table.pairs::<String, Value>() {
            let (name, value) = pair?;
            let value = match value {
                Value::String(text) => text.to_str()?.to_string(),
                other => {
                    return Err(LuaError::runtime(format!(
                        "header {name:?} must be a string, got {}",
                        other.type_name()
                    )))
                }
            };
            headers.push((name, value));
        }
    }

    // a table body is json, with the content type added unless the caller chose one
    let mut body = None;
    match options.get::<Option<Value>>("Body")? {
        None | Some(Value::Nil) => {}
        Some(Value::String(text)) => body = Some(text.as_bytes().to_vec()),
        Some(Value::Table(table)) => {
            let json: Json = lua
                .from_value(Value::Table(table))
                .map_err(|error| LuaError::runtime(error.to_string()))?;
            let text = serde_json::to_string(&json)
                .map_err(|error| LuaError::runtime(error.to_string()))?;
            if !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".to_string(), "application/json".to_string()));
            }
            body = Some(text.into_bytes());
        }
        Some(other) => {
            return Err(LuaError::runtime(format!(
                "RequestAsync expects a string or table Body, got {}",
                other.type_name()
            )))
        }
    }

    Ok(HttpRequest {
        url: validate_url(&url)?,
        // roblox documents GET as the default, body or not
        method: method.unwrap_or_else(|| "GET".to_string()),
        headers,
        body,
    })
}

// roblox refuses anything that is not plain http or https
fn validate_url(url: &str) -> LuaResult<String> {
    let lowered = url.trim().to_ascii_lowercase();
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        return Ok(url.trim().to_string());
    }
    Err(LuaError::runtime(format!(
        "Http requests can only be executed with HTTP or HTTPS URLs, got {url:?}"
    )))
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            // RequestAsync reports a failed request in its dictionary instead of raising
            .http_status_as_error(false)
            .build()
            .new_agent()
    })
}

// transport failures raise, http failures come back in the dictionary
fn send(request: &HttpRequest) -> LuaResult<HttpResponse> {
    let method = ureq::http::Method::from_bytes(request.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| LuaError::runtime(format!("{} is not a valid HTTP method", request.method)))?;

    let mut builder = ureq::http::Request::builder()
        .method(method)
        .uri(&request.url);
    for (name, value) in &request.headers {
        // header errors surface when the request is built
        builder = builder.header(name.as_str(), value.as_str());
    }

    let agent = agent();
    let response = match &request.body {
        Some(body) => agent.run(
            builder
                .body(body.clone())
                .map_err(|error| LuaError::runtime(error.to_string()))?,
        ),
        None => agent.run(
            builder
                .body(())
                .map_err(|error| LuaError::runtime(error.to_string()))?,
        ),
    }
    .map_err(|error| transport_error(error, &request.url))?;

    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| LuaError::runtime(format!("HttpError: NetFail {error}")))?;

    Ok(HttpResponse {
        status: status.as_u16(),
        status_message: status.canonical_reason().unwrap_or("").to_string(),
        headers,
        body,
    })
}

// roblox names the way a request failed, so keep the same vocabulary
fn transport_error(error: ureq::Error, url: &str) -> LuaError {
    use std::io::ErrorKind;

    let kind = match &error {
        ureq::Error::Timeout(_) => "Timeout",
        ureq::Error::HostNotFound => "DnsResolve",
        // tls failures arrive as io errors too, so a refused socket is the one worth naming
        ureq::Error::Io(io) => match io.kind() {
            ErrorKind::TimedOut | ErrorKind::WouldBlock => "Timeout",
            ErrorKind::ConnectionRefused | ErrorKind::NotConnected => "ConnectFail",
            _ => "NetFail",
        },
        ureq::Error::ConnectionFailed | ureq::Error::BadUri(_) => "ConnectFail",
        ureq::Error::Protocol(_) => "NetFail",
        _ => "NetFail",
    };
    LuaError::runtime(format!("HttpError: {kind} {error} ({url})"))
}

fn env_key(name: &str) -> String {
    format!(
        "MICROSTUDIO_SECRET_{}",
        name.to_ascii_uppercase().replace(['-', ' '], "_")
    )
}

fn read_secret(store: &microstudio_services::Store, name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(env_key(name)) {
        if !value.is_empty() {
            return Some(value);
        }
    }
    let file = store.read_json(SECRETS).ok()??;
    file.get(name)?.as_str().map(str::to_string)
}

fn write_secret(
    store: &microstudio_services::Store,
    name: &str,
    value: &str,
) -> Result<(), microstudio_services::StoreError> {
    if !store.is_persistent() {
        return Ok(());
    }
    store.update_json(SECRETS, |current| {
        let mut object = match current {
            Some(Json::Object(object)) => object,
            _ => serde_json::Map::new(),
        };
        object.insert(name.to_string(), Json::String(value.to_string()));
        Json::Object(object)
    })?;
    Ok(())
}

// A-Za-z0-9-_. and nothing else, the set a url parameter can hold unescaped
fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

static GUID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn guid() -> String {
    let mut rng = Rng::new();
    let mut bytes = [0u8; 16];
    for chunk in bytes.chunks_mut(8) {
        let value = rng.next().to_le_bytes();
        chunk.copy_from_slice(&value[..chunk.len()]);
    }
    // version 4, variant 1
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        let counter = GUID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = u64::from(std::process::id());
        // xorshift dies at zero, so make sure the seed is never zero
        Self((nanos ^ counter.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (pid << 17)) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        self.0 = state;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{Vm, VmOptions};

    fn vm() -> Vm {
        Vm::new(VmOptions::default()).expect("vm boots")
    }

    fn eval_string(vm: &Vm, code: &str) -> String {
        match vm.eval(code).unwrap() {
            Value::String(text) => text.to_str().unwrap().to_string(),
            other => panic!(
                "expected a string, got {} (recorded errors: {:?})",
                other.type_name(),
                vm.ctx().state.borrow().world.errors
            ),
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
    fn json_round_trips_through_a_lua_table() {
        let vm = vm();
        let value = vm
            .eval(
                r#"
                local service = game:GetService("HttpService")
                local text = service:JSONEncode({name = "board", scores = {1, 2, 3}})
                local back = service:JSONDecode(text)
                return {text, back.name, #back.scores}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        let text: String = table.get(1).unwrap();
        assert!(text.contains("\"name\":\"board\""), "{text}");
        assert_eq!(table.get::<String>(2).unwrap(), "board");
        assert_eq!(table.get::<i64>(3).unwrap(), 3);
    }

    #[test]
    fn json_keeps_a_flat_array_an_array() {
        let vm = vm();
        let text = eval_string(
            &vm,
            r#"return game:GetService("HttpService"):JSONEncode({10, 20})"#,
        );
        assert_eq!(text, "[10,20]");
    }

    #[test]
    fn decoding_bad_json_raises() {
        let vm = vm();
        let message = eval_error(
            &vm,
            r#"game:GetService("HttpService"):JSONDecode("{ nope")"#,
        );
        assert!(message.contains("Invalid JSON"), "{message}");
    }

    #[test]
    fn url_encoding_round_trips() {
        assert_eq!(url_encode("a b/c?d=1&e"), "a%20b%2Fc%3Fd%3D1%26e");
        assert_eq!(url_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%zz"), "%zz");
    }

    #[test]
    fn guids_are_unique_and_braced() {
        let first = guid();
        let second = guid();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(first.chars().nth(14), Some('4'));

        let vm = vm();
        let value = eval_string(
            &vm,
            r#"return game:GetService("HttpService"):GenerateGUID()"#,
        );
        assert!(value.starts_with('{') && value.ends_with('}'), "{value}");
        assert_eq!(value.len(), 38);

        let bare = eval_string(
            &vm,
            r#"return game:GetService("HttpService"):GenerateGUID(false)"#,
        );
        assert_eq!(bare.len(), 36);
    }

    #[test]
    fn http_enabled_starts_true_and_can_be_turned_off() {
        let vm = vm();
        let value: Value = vm
            .eval(
                r#"
                local service = game:GetService("HttpService")
                local before = service.HttpEnabled
                service:SetHttpEnabled(false)
                local after = service.HttpEnabled
                local ok, message = pcall(function() return service:RequestAsync({Url = "http://127.0.0.1:9/"}) end)
                service:SetHttpEnabled(true)
                return {before, after, ok, tostring(message)}
                "#,
            )
            .unwrap();
        let Value::Table(table) = value else {
            panic!("expected a table")
        };
        assert!(table.get::<bool>(1).unwrap());
        assert!(!table.get::<bool>(2).unwrap());
        assert!(!table.get::<bool>(3).unwrap());
        assert!(
            table.get::<String>(4).unwrap().contains("not enabled"),
            "{:?}",
            table.get::<String>(4)
        );
    }

    #[test]
    fn only_http_urls_are_accepted() {
        let vm = vm();
        let message = eval_error(
            &vm,
            r#"game:GetService("HttpService"):RequestAsync({Url = "ftp://example.com"})"#,
        );
        assert!(message.contains("HTTP or HTTPS"), "{message}");
    }

    #[test]
    fn a_connection_that_cannot_be_made_raises_connect_fail() {
        let vm = vm();
        let message = eval_error(
            &vm,
            r#"game:GetService("HttpService"):RequestAsync({Url = "http://127.0.0.1:9/", Method = "GET"})"#,
        );
        assert!(message.contains("HttpError:"), "{message}");
    }

    #[test]
    fn secrets_read_from_the_environment_first() {
        let store = microstudio_services::Store::ephemeral();
        std::env::set_var(env_key("token"), "from-env");
        assert_eq!(read_secret(&store, "token").as_deref(), Some("from-env"));
        std::env::remove_var(env_key("token"));
        assert_eq!(read_secret(&store, "token"), None);
    }

    #[test]
    fn secrets_persist_beside_the_project() {
        let dir = std::env::temp_dir().join("microstudio-http-secrets");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = microstudio_services::Store::at(&dir);

        write_secret(&store, "api", "s3cret").unwrap();
        assert_eq!(read_secret(&store, "api").as_deref(), Some("s3cret"));
        write_secret(&store, "other", "second").unwrap();
        assert_eq!(read_secret(&store, "api").as_deref(), Some("s3cret"));
        assert!(dir.join(SECRETS).exists());
    }
}
