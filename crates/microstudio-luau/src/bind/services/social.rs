// BadgeService, MarketplaceService, UserService and GroupService, backed by json state files
//
// state files (plain json under the state directory, seedable by hand like miniflare):
//   badges.json      { "badges": { "<badgeId>": { "name", "description", "iconImageId", "enabled" } },
//                      "awards": { "<userId>": ["<badgeId>"] } }
//   marketplace.json { "products": { "<assetId>": { "name", "description", "priceInRobux", "assetTypeId" } },
//                      "gamePasses": { "<gamePassId>": { "name", "priceInRobux" } },
//                      "ownership": { "<userId>": { "assets": [], "gamePasses": [], "products": [] } } }
//   users.json       { "<userId>": { "Username", "DisplayName", "HasVerifiedBadge" } }
//   groups.json      { "groups": { "<groupId>": { "Name", "Description", "OwnerId", "MemberCount", "Created" } },
//                      "members": { "<groupId>": { "<userId>": { "Role", "Rank" } } },
//                      "allies": { "<groupId>": [2] },
//                      "enemies": { "<groupId>": [3] } }
//
// ownership.products is where a local developer product purchase is recorded; it is not read back

use mlua::{AnyUserData, Error as LuaError, Lua, Result as LuaResult, Table, Value};
use microstudio_datamodel::{AttributeValue, EventArg, InstanceId, PendingEvent};
use serde_json::{json, Value as Json};

use crate::bind::instance::{instance_of, LuaInstance};
use crate::bind::services::{methods, require_class};
use crate::convert::{lua_err, take_instance};
use crate::vm::Ctx;

const BADGES: &str = "badges.json";
const MARKETPLACE: &str = "marketplace.json";
const USERS: &str = "users.json";
const GROUPS: &str = "groups.json";

const PURCHASE_DEVIATION: &str = "MarketplaceService: a local purchase completes instantly \
    and is written to marketplace.json; a real server prompts the client and waits for the answer";

pub fn install(lua: &Lua, _ctx: &Ctx) -> LuaResult<()> {
    // deviations warn once; the flags sit in the registry so they survive a world reset
    lua.set_named_registry_value("microstudio.social_warned", lua.create_table()?)?;
    let methods = methods(lua)?;

    methods.set(
        "UserHasBadgeAsync",
        lua.create_function(|_, (ud, user_id, badge_id): (AnyUserData, f64, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "BadgeService")?;
            let badges = read_store(&this, BADGES)?;
            Ok(user_has_badge(&badges, user_id as i64, badge_id as i64))
        })?,
    )?;

    methods.set(
        "UserHasBadge",
        lua.create_function(|_, (ud, user_id, badge_id): (AnyUserData, f64, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "BadgeService")?;
            let badges = read_store(&this, BADGES)?;
            Ok(user_has_badge(&badges, user_id as i64, badge_id as i64))
        })?,
    )?;

    methods.set(
        "AwardBadge",
        lua.create_function(|lua, (ud, user_id, badge_id): (AnyUserData, f64, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "BadgeService")?;
            let badges = read_store(&this, BADGES)?;
            if !badge_exists(&badges, badge_id as i64) {
                return Err(LuaError::runtime("The badge does not exist"));
            }
            if user_has_badge(&badges, user_id as i64, badge_id as i64) {
                return Ok(false);
            }
            record_award(&this, user_id as i64, badge_id as i64)?;
            fire_badge_awarded(lua, &this, user_id as i64, badge_id as i64)?;
            Ok(true)
        })?,
    )?;

    methods.set(
        "GetBadgeInfoAsync",
        lua.create_function(|lua, (ud, badge_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "BadgeService")?;
            let badges = read_store(&this, BADGES)?;
            let entry = badges
                .get("badges")
                .and_then(|all| all.get(&(badge_id as i64).to_string()))
                .ok_or_else(|| LuaError::runtime("The badge does not exist"))?;
            let table = lua.create_table()?;
            table.set("Name", field_text(entry, "name"))?;
            table.set("Description", field_text(entry, "description"))?;
            table.set("IconImageId", field_number(entry, "iconImageId"))?;
            table.set("IsEnabled", field_bool(entry, "enabled"))?;
            Ok(table)
        })?,
    )?;

    methods.set(
        "GetProductInfo",
        lua.create_function(
            |lua, (ud, asset_id, _info_type): (AnyUserData, f64, Value)| {
                let this = instance_of(&ud)?;
                require_class(&this, "MarketplaceService")?;
                let marketplace = read_store(&this, MARKETPLACE)?;
                let entry = product_entry(&marketplace, asset_id as i64)?;
                let table = lua.create_table()?;
                table.set("AssetId", asset_id)?;
                table.set("Name", field_text(entry, "name"))?;
                table.set("Description", field_text(entry, "description"))?;
                table.set("PriceInRobux", field_number(entry, "priceInRobux"))?;
                table.set("AssetTypeId", field_number(entry, "assetTypeId"))?;
                Ok(table)
            },
        )?,
    )?;

    methods.set(
        "PlayerOwnsAsset",
        lua.create_function(|_, (ud, player, asset_id): (AnyUserData, Value, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "MarketplaceService")?;
            let player = take_instance(&player)?;
            let user_id = player_user_id(&this, player)?;
            let marketplace = read_store(&this, MARKETPLACE)?;
            Ok(owns(&marketplace, user_id, "assets", asset_id as i64))
        })?,
    )?;

    methods.set(
        "UserOwnsGamePassAsync",
        lua.create_function(|_, (ud, user_id, game_pass_id): (AnyUserData, f64, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "MarketplaceService")?;
            let marketplace = read_store(&this, MARKETPLACE)?;
            Ok(owns(&marketplace, user_id as i64, "gamePasses", game_pass_id as i64))
        })?,
    )?;

    methods.set(
        "PlayerCanMakePurchases",
        lua.create_function(|_, (ud, _player): (AnyUserData, Value)| {
            let this = instance_of(&ud)?;
            require_class(&this, "MarketplaceService")?;
            Ok(true)
        })?,
    )?;

    methods.set(
        "PromptPurchase",
        lua.create_function(
            |lua, (ud, player, asset_id, _equip): (AnyUserData, Value, f64, Option<bool>)| {
                let this = instance_of(&ud)?;
                require_class(&this, "MarketplaceService")?;
                let player = take_instance(&player)?;
                let user_id = player_user_id(&this, player)?;
                let marketplace = read_store(&this, MARKETPLACE)?;
                product_entry(&marketplace, asset_id as i64)?;
                warn_purchase(lua, &this)?;
                record_ownership(&this, user_id, "assets", asset_id as i64)?;
                fire_prompt(
                    lua,
                    &this,
                    "PromptPurchaseFinished",
                    vec![
                        EventArg::Instance(player),
                        EventArg::Number(asset_id),
                        event_bool(true),
                    ],
                )
            },
        )?,
    )?;

    methods.set(
        "PromptGamePassPurchase",
        lua.create_function(
            |lua, (ud, player, game_pass_id): (AnyUserData, Value, f64)| {
                let this = instance_of(&ud)?;
                require_class(&this, "MarketplaceService")?;
                let player = take_instance(&player)?;
                let user_id = player_user_id(&this, player)?;
                let marketplace = read_store(&this, MARKETPLACE)?;
                game_pass_entry(&marketplace, game_pass_id as i64)?;
                warn_purchase(lua, &this)?;
                record_ownership(&this, user_id, "gamePasses", game_pass_id as i64)?;
                fire_prompt(
                    lua,
                    &this,
                    "PromptGamePassPurchaseFinished",
                    vec![
                        EventArg::Instance(player),
                        EventArg::Number(game_pass_id),
                        event_bool(true),
                    ],
                )
            },
        )?,
    )?;

    methods.set(
        "PromptProductPurchase",
        lua.create_function(|lua, (ud, player, product_id): (AnyUserData, Value, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "MarketplaceService")?;
            let player = take_instance(&player)?;
            let user_id = player_user_id(&this, player)?;
            let marketplace = read_store(&this, MARKETPLACE)?;
            product_entry(&marketplace, product_id as i64)?;
            warn_purchase(lua, &this)?;
            record_ownership(&this, user_id, "products", product_id as i64)?;
            fire_prompt(
                lua,
                &this,
                "PromptProductPurchaseFinished",
                vec![
                    EventArg::Number(user_id as f64),
                    EventArg::Number(product_id),
                    event_bool(true),
                ],
            )
        })?,
    )?;

    // ProcessReceipt is a callback property and GetDeveloperProductsAsync returns a Pages object;
    // both are left to the generated simulated fallback

    methods.set(
        "GetUserInfosByUserIdsAsync",
        lua.create_function(|lua, (ud, user_ids): (AnyUserData, Table)| {
            let this = instance_of(&ud)?;
            require_class(&this, "UserService")?;
            let users = read_store(&this, USERS)?;
            let count = user_ids.raw_len();
            let result = lua.create_table()?;
            for index in 1..=count {
                let user_id = user_ids.raw_get::<f64>(index)? as i64;
                let (username, display, verified) = user_entry(&users, user_id)
                    .ok_or_else(|| LuaError::runtime(format!("Unknown user: {user_id}")))?;
                let info = lua.create_table()?;
                info.set("Id", user_id as f64)?;
                info.set("Username", username)?;
                info.set("DisplayName", display)?;
                info.set("HasVerifiedBadge", verified)?;
                result.set(index, info)?;
            }
            Ok(result)
        })?,
    )?;

    // these read the same users.json; in real roblox they live on Players, not UserService
    methods.set(
        "GetNameFromUserIdAsync",
        lua.create_function(|_, (ud, user_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "Players")?;
            let users = read_store(&this, USERS)?;
            let (username, _, _) = user_entry(&users, user_id as i64)
                .ok_or_else(|| LuaError::runtime(format!("Unknown user: {user_id}")))?;
            Ok(username.to_string())
        })?,
    )?;

    methods.set(
        "GetUserIdFromNameAsync",
        lua.create_function(|_, (ud, name): (AnyUserData, String)| {
            let this = instance_of(&ud)?;
            require_class(&this, "Players")?;
            let users = read_store(&this, USERS)?;
            let found = users.as_object().and_then(|all| {
                all.iter().find_map(|(key, entry)| {
                    let username = entry.get("Username").and_then(Json::as_str)?;
                    if username == name || username.eq_ignore_ascii_case(&name) {
                        key.parse::<i64>().ok()
                    } else {
                        None
                    }
                })
            });
            found
                .map(|user_id| user_id as f64)
                .ok_or_else(|| LuaError::runtime(format!("Unknown user: {name}")))
        })?,
    )?;

    methods.set(
        "GetGroupInfoAsync",
        lua.create_function(|lua, (ud, group_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "GroupService")?;
            let groups = read_store(&this, GROUPS)?;
            let users = read_store(&this, USERS)?;
            group_value(lua, &groups, &users, group_id as i64)
        })?,
    )?;

    methods.set(
        "GetGroupsAsync",
        lua.create_function(|lua, (ud, user_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "GroupService")?;
            let groups = read_store(&this, GROUPS)?;
            let users = read_store(&this, USERS)?;
            let user_id = user_id as i64;

            let mut ids = Vec::new();
            if let Some(members) = groups.get("members").and_then(Json::as_object) {
                for (key, member_map) in members {
                    if member_map.get(&user_id.to_string()).is_some() {
                        if let Ok(group_id) = key.parse::<i64>() {
                            ids.push(group_id);
                        }
                    }
                }
            }
            // sorted so the array is stable whatever order the json object came back in
            ids.sort_unstable();

            let result = lua.create_table()?;
            for (index, group_id) in ids.into_iter().enumerate() {
                let table = group_value(lua, &groups, &users, group_id)?;
                if let Some(member) = groups
                    .get("members")
                    .and_then(|all| all.get(&group_id.to_string()))
                    .and_then(|all| all.get(&user_id.to_string()))
                {
                    table.set("Role", field_text(member, "Role"))?;
                    table.set("Rank", field_number(member, "Rank"))?;
                }
                result.set(index + 1, table)?;
            }
            Ok(result)
        })?,
    )?;

    methods.set(
        "GetAlliesAsync",
        lua.create_function(|lua, (ud, group_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "GroupService")?;
            relation_values(lua, &this, group_id as i64, "allies", "GetAlliesAsync")
        })?,
    )?;

    methods.set(
        "GetEnemiesAsync",
        lua.create_function(|lua, (ud, group_id): (AnyUserData, f64)| {
            let this = instance_of(&ud)?;
            require_class(&this, "GroupService")?;
            relation_values(lua, &this, group_id as i64, "enemies", "GetEnemiesAsync")
        })?,
    )?;

    Ok(())
}

fn read_store(this: &LuaInstance, name: &str) -> LuaResult<Json> {
    let store = this.ctx.state.borrow().world.store.clone();
    Ok(store.read_json(name).map_err(lua_err)?.unwrap_or(Json::Null))
}

fn edit_store(this: &LuaInstance, name: &str, edit: impl FnOnce(Json) -> Json) -> LuaResult<()> {
    let store = this.ctx.state.borrow().world.store.clone();
    let _ = store
        .update_json(name, |current| edit(current.unwrap_or(Json::Null)))
        .map_err(lua_err)?;
    Ok(())
}

fn object_at<'a>(parent: &'a mut Json, key: &str) -> &'a mut Json {
    if parent.get(key).map(|value| !value.is_object()).unwrap_or(true) {
        parent[key] = json!({});
    }
    &mut parent[key]
}

fn array_at<'a>(parent: &'a mut Json, key: &str) -> &'a mut Vec<Json> {
    if parent.get(key).map(|value| !value.is_array()).unwrap_or(true) {
        parent[key] = json!([]);
    }
    parent[key].as_array_mut().expect("array_at just made an array")
}

fn id_of(value: &Json) -> Option<i64> {
    match value {
        Json::Number(number) => number.as_i64(),
        Json::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn field_text(value: &Json, key: &str) -> String {
    value.get(key).and_then(Json::as_str).unwrap_or("").to_string()
}

fn field_number(value: &Json, key: &str) -> f64 {
    value.get(key).and_then(Json::as_f64).unwrap_or(0.0)
}

fn field_bool(value: &Json, key: &str) -> bool {
    value.get(key).and_then(Json::as_bool).unwrap_or(true)
}

fn user_entry(users: &Json, user_id: i64) -> Option<(&str, &str, bool)> {
    let entry = users.get(&user_id.to_string())?;
    let username = entry.get("Username").and_then(Json::as_str).unwrap_or("");
    let display = entry
        .get("DisplayName")
        .and_then(Json::as_str)
        .unwrap_or(username);
    let verified = entry
        .get("HasVerifiedBadge")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    Some((username, display, verified))
}

fn badge_exists(badges: &Json, badge_id: i64) -> bool {
    badges
        .get("badges")
        .and_then(|all| all.get(&badge_id.to_string()))
        .is_some()
}

fn user_has_badge(badges: &Json, user_id: i64, badge_id: i64) -> bool {
    badges
        .get("awards")
        .and_then(|awards| awards.get(&user_id.to_string()))
        .and_then(Json::as_array)
        .map(|list| list.iter().any(|item| id_of(item) == Some(badge_id)))
        .unwrap_or(false)
}

fn record_award(this: &LuaInstance, user_id: i64, badge_id: i64) -> LuaResult<()> {
    edit_store(this, BADGES, |mut root| {
        {
            let awards = object_at(&mut root, "awards");
            let list = array_at(awards, &user_id.to_string());
            if !list.iter().any(|item| id_of(item) == Some(badge_id)) {
                list.push(json!(badge_id.to_string()));
            }
        }
        root
    })
}

fn record_ownership(this: &LuaInstance, user_id: i64, collection: &str, item_id: i64) -> LuaResult<()> {
    edit_store(this, MARKETPLACE, |mut root| {
        {
            let ownership = object_at(&mut root, "ownership");
            let owned = object_at(ownership, &user_id.to_string());
            let list = array_at(owned, collection);
            if !list.iter().any(|item| id_of(item) == Some(item_id)) {
                list.push(json!(item_id));
            }
        }
        root
    })
}

fn owns(marketplace: &Json, user_id: i64, collection: &str, item_id: i64) -> bool {
    marketplace
        .get("ownership")
        .and_then(|ownership| ownership.get(&user_id.to_string()))
        .and_then(|owned| owned.get(collection))
        .and_then(Json::as_array)
        .map(|list| list.iter().any(|item| id_of(item) == Some(item_id)))
        .unwrap_or(false)
}

fn product_entry(marketplace: &Json, asset_id: i64) -> LuaResult<&Json> {
    marketplace
        .get("products")
        .and_then(|products| products.get(&asset_id.to_string()))
        .ok_or_else(|| {
            LuaError::runtime(format!(
                "MarketplaceService: no product with id {asset_id}; add it to {MARKETPLACE}"
            ))
        })
}

fn game_pass_entry(marketplace: &Json, game_pass_id: i64) -> LuaResult<&Json> {
    marketplace
        .get("gamePasses")
        .and_then(|passes| passes.get(&game_pass_id.to_string()))
        .ok_or_else(|| {
            LuaError::runtime(format!(
                "MarketplaceService: no game pass with id {game_pass_id}; add it to {MARKETPLACE}"
            ))
        })
}

fn group_entry(groups: &Json, group_id: i64) -> LuaResult<&Json> {
    groups
        .get("groups")
        .and_then(|all| all.get(&group_id.to_string()))
        .ok_or_else(|| LuaError::runtime(format!("Unknown group: {group_id}")))
}

fn group_value(lua: &Lua, groups: &Json, users: &Json, group_id: i64) -> LuaResult<Table> {
    let entry = group_entry(groups, group_id)?;
    let table = lua.create_table()?;
    table.set("Id", group_id as f64)?;
    table.set("Name", field_text(entry, "Name"))?;
    table.set("Description", field_text(entry, "Description"))?;
    table.set("MemberCount", field_number(entry, "MemberCount"))?;
    table.set("Created", field_number(entry, "Created"))?;

    let owner_id = entry.get("OwnerId").and_then(Json::as_i64).unwrap_or(0);
    let owner = lua.create_table()?;
    owner.set("Id", owner_id as f64)?;
    match user_entry(users, owner_id) {
        Some((username, display, _)) => {
            owner.set("Username", username)?;
            owner.set("DisplayName", display)?;
        }
        None => {
            owner.set("Username", "")?;
            owner.set("DisplayName", "")?;
        }
    }
    table.set("Owner", owner)?;
    Ok(table)
}

fn relation_values(
    lua: &Lua,
    this: &LuaInstance,
    group_id: i64,
    relation: &str,
    method: &str,
) -> LuaResult<Table> {
    let groups = read_store(this, GROUPS)?;
    let users = read_store(this, USERS)?;
    // an unknown group is an error even when it has no relations
    group_entry(&groups, group_id)?;
    // deviation: a real server returns a Pages object; this engine hands back a plain array
    warn_once(
        lua,
        &this.ctx,
        &format!("group.{relation}"),
        &format!("GroupService:{method} returns a plain array; a real server returns a Pages object"),
    )?;

    let result = lua.create_table()?;
    let ids = groups
        .get(relation)
        .and_then(|rel| rel.get(&group_id.to_string()))
        .and_then(Json::as_array);
    let mut index = 1;
    if let Some(ids) = ids {
        for id in ids {
            let Some(related) = id_of(id) else { continue };
            // a dangling id names a group that was never seeded
            if groups
                .get("groups")
                .and_then(|all| all.get(&related.to_string()))
                .is_none()
            {
                continue;
            }
            result.set(index, group_value(lua, &groups, &users, related)?)?;
            index += 1;
        }
    }
    Ok(result)
}

fn player_user_id(this: &LuaInstance, player: InstanceId) -> LuaResult<i64> {
    let state = this.ctx.state.borrow();
    if state.world.dm.class_of(player).unwrap_or("<destroyed>") != "Player" {
        return Err(LuaError::runtime("expected a Player"));
    }
    match state.world.dm.get_property(player, "UserId") {
        Some(AttributeValue::Number(user_id)) => Ok(user_id as i64),
        _ => Err(LuaError::runtime("player has no UserId")),
    }
}

// EventArg has no bool variant; the attribute form converts straight back to a lua boolean
fn event_bool(value: bool) -> EventArg {
    EventArg::Attribute("value".to_string(), AttributeValue::Bool(value))
}

fn fire_badge_awarded(lua: &Lua, this: &LuaInstance, user_id: i64, badge_id: i64) -> LuaResult<()> {
    let signal = {
        let mut state = this.ctx.state.borrow_mut();
        state
            .world
            .dm
            .named_signal(this.id, "BadgeAwarded")
            .map_err(lua_err)?
    };
    // OnBadgeAwarded is the deprecated name for the same event
    let alias = {
        let mut state = this.ctx.state.borrow_mut();
        state
            .world
            .dm
            .named_signal(this.id, "OnBadgeAwarded")
            .map_err(lua_err)?
    };
    let args = vec![
        EventArg::Number(user_id as f64),
        EventArg::Number(badge_id as f64),
    ];
    crate::vm::dispatch(
        lua,
        &this.ctx,
        vec![
            PendingEvent {
                signal,
                args: args.clone(),
            },
            PendingEvent {
                signal: alias,
                args,
            },
        ],
    )?;
    Ok(())
}

fn fire_prompt(
    lua: &Lua,
    this: &LuaInstance,
    name: &'static str,
    args: Vec<EventArg>,
) -> LuaResult<()> {
    let signal = {
        let mut state = this.ctx.state.borrow_mut();
        state.world.dm.named_signal(this.id, name).map_err(lua_err)?
    };
    crate::vm::dispatch(lua, &this.ctx, vec![PendingEvent { signal, args }])?;
    Ok(())
}

fn warn_purchase(lua: &Lua, this: &LuaInstance) -> LuaResult<()> {
    // deviation: a real server prompts the client and waits; here the purchase is instant
    warn_once(lua, &this.ctx, "marketplace.purchase", PURCHASE_DEVIATION)
}

fn warn_once(lua: &Lua, ctx: &Ctx, key: &str, message: &str) -> LuaResult<()> {
    let warned: Table = lua.named_registry_value("microstudio.social_warned")?;
    if warned.get::<bool>(key)? {
        return Ok(());
    }
    warned.set(key, true)?;
    ctx.state.borrow_mut().scheduler.warn(message.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::{Vm, VmOptions};

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

    fn eval_bool(vm: &Vm, code: &str) -> bool {
        match vm.eval(code).unwrap() {
            Value::Boolean(value) => value,
            other => panic!(
                "expected a boolean, got {} (recorded errors: {:?})",
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

    fn seed_dir(case: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("microstudio-social-{case}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = microstudio_services::Store::at(&dir);
        store
            .write_json(
                BADGES,
                &json!({
                    "badges": {
                        "101": { "name": "Welcome", "description": "first badge", "iconImageId": 7, "enabled": true }
                    },
                    "awards": {}
                }),
            )
            .unwrap();
        store
            .write_json(
                MARKETPLACE,
                &json!({
                    "products": {
                        "100": { "name": "Sword", "description": "sharp", "priceInRobux": 50, "assetTypeId": 8 }
                    },
                    "gamePasses": { "3": { "name": "VIP", "priceInRobux": 99 } },
                    "ownership": {}
                }),
            )
            .unwrap();
        store
            .write_json(
                USERS,
                &json!({
                    "1": { "Username": "Builderman", "DisplayName": "Builder", "HasVerifiedBadge": true },
                    "10": { "Username": "Guest", "DisplayName": "Guest", "HasVerifiedBadge": false }
                }),
            )
            .unwrap();
        store
            .write_json(
                GROUPS,
                &json!({
                    "groups": {
                        "5": { "Name": "Builders", "Description": "we build", "OwnerId": 1, "MemberCount": 12, "Created": 123 },
                        "7": { "Name": "Allies", "Description": "", "OwnerId": 1, "MemberCount": 2 },
                        "9": { "Name": "Rivals", "Description": "", "OwnerId": 10, "MemberCount": 3 }
                    },
                    "members": { "5": { "10": { "Role": "Member", "Rank": 1 } } },
                    "allies": { "5": [7] },
                    "enemies": { "5": [9] }
                }),
            )
            .unwrap();
        dir
    }

    fn vm_at(dir: &std::path::Path) -> Vm {
        Vm::new(VmOptions {
            store: microstudio_services::Store::at(dir),
            ..Default::default()
        })
        .expect("vm boots")
    }

    fn seeded_vm(case: &str) -> Vm {
        vm_at(&seed_dir(case))
    }

    #[test]
    fn a_badge_award_persists_across_vms() {
        let dir = seed_dir("badge-award");
        let first = vm_at(&dir);
        assert!(eval_bool(
            &first,
            r#"return game:GetService("BadgeService"):AwardBadge(1, 101)"#
        ));

        let second = vm_at(&dir);
        assert!(eval_bool(
            &second,
            r#"return game:GetService("BadgeService"):UserHasBadgeAsync(1, 101)"#
        ));
        assert!(eval_bool(
            &second,
            r#"return game:GetService("BadgeService"):UserHasBadge(1, 101)"#
        ));
    }

    #[test]
    fn awarding_a_badge_twice_returns_false() {
        let vm = seeded_vm("badge-twice");
        let result = eval_string(
            &vm,
            r#"
            local service = game:GetService("BadgeService")
            local first = service:AwardBadge(1, 101)
            local second = service:AwardBadge(1, 101)
            return tostring(first) .. "," .. tostring(second)
            "#,
        );
        assert_eq!(result, "true,false");
    }

    #[test]
    fn awarding_a_missing_badge_errors() {
        let vm = seeded_vm("badge-missing");
        let message = eval_error(&vm, r#"game:GetService("BadgeService"):AwardBadge(1, 999)"#);
        assert!(message.contains("does not exist"), "{message}");
    }

    #[test]
    fn an_award_fires_the_event_and_its_deprecated_alias() {
        let vm = seeded_vm("badge-event");
        let result = eval_string(
            &vm,
            r#"
            local service = game:GetService("BadgeService")
            local seen = {}
            service.BadgeAwarded:Connect(function(userId, badgeId) seen.current = userId .. ":" .. badgeId end)
            service.OnBadgeAwarded:Connect(function(userId, badgeId) seen.alias = userId .. ":" .. badgeId end)
            service:AwardBadge(4, 101)
            return (seen.current or "none") .. "/" .. (seen.alias or "none")
            "#,
        );
        assert_eq!(result, "4:101/4:101");
    }

    #[test]
    fn badge_info_uses_roblox_field_names() {
        let vm = seeded_vm("badge-info");
        let result = eval_string(
            &vm,
            r#"
            local info = game:GetService("BadgeService"):GetBadgeInfoAsync(101)
            assert(info.Name == "Welcome", "Name")
            assert(info.Description == "first badge", "Description")
            assert(info.IconImageId == 7, "IconImageId")
            assert(info.IsEnabled == true, "IsEnabled")
            return "ok"
            "#,
        );
        assert_eq!(result, "ok");
    }

    #[test]
    fn a_purchase_prompt_fires_and_records_ownership() {
        let vm = seeded_vm("purchase");
        let result = eval_string(
            &vm,
            r#"
            local market = game:GetService("MarketplaceService")
            local player = game:GetService("Players"):CreateMockPlayer("Buyer")
            local finished = {}
            market.PromptPurchaseFinished:Connect(function(p, assetId, purchased)
                finished.player = p
                finished.assetId = assetId
                finished.purchased = purchased
            end)
            market:PromptPurchase(player, 100)
            assert(finished.player == player, "player")
            assert(finished.assetId == 100, "assetId")
            assert(finished.purchased == true, "purchased")
            assert(market:PlayerOwnsAsset(player, 100), "owned")

            local pass = {}
            market.PromptGamePassPurchaseFinished:Connect(function(p, gamePassId, purchased)
                pass.gamePassId = gamePassId
                pass.purchased = purchased
            end)
            market:PromptGamePassPurchase(player, 3)
            assert(pass.gamePassId == 3 and pass.purchased == true, "game pass event")
            assert(market:UserOwnsGamePassAsync(player.UserId, 3), "owns game pass")
            return "ok"
            "#,
        );
        assert_eq!(result, "ok");
    }

    #[test]
    fn an_unknown_product_errors_and_names_the_id() {
        let vm = seeded_vm("product-missing");
        let message = eval_error(&vm, r#"game:GetService("MarketplaceService"):GetProductInfo(999, 0)"#);
        assert!(message.contains("999"), "{message}");
    }

    #[test]
    fn an_unknown_user_id_raises() {
        let vm = seeded_vm("user-missing");
        let message = eval_error(
            &vm,
            r#"game:GetService("UserService"):GetUserInfosByUserIdsAsync({1, 42})"#,
        );
        assert!(message.contains("Unknown user: 42"), "{message}");
    }

    #[test]
    fn user_infos_follow_input_order_and_shape() {
        let vm = seeded_vm("user-infos");
        let result = eval_string(
            &vm,
            r#"
            local infos = game:GetService("UserService"):GetUserInfosByUserIdsAsync({10, 1})
            assert(#infos == 2, "count")
            assert(infos[1].Id == 10, "order")
            assert(infos[1].Username == "Guest" and infos[1].DisplayName == "Guest", "guest")
            assert(infos[1].HasVerifiedBadge == false, "guest badge")
            assert(infos[2].Id == 1 and infos[2].Username == "Builderman", "builder")
            assert(infos[2].DisplayName == "Builder" and infos[2].HasVerifiedBadge == true, "builder fields")

            local players = game:GetService("Players")
            assert(players:GetNameFromUserIdAsync(1) == "Builderman", "name")
            assert(players:GetUserIdFromNameAsync("Guest") == 10, "id")
            return "ok"
            "#,
        );
        assert_eq!(result, "ok");
    }

    #[test]
    fn group_info_includes_an_owner_from_users() {
        let vm = seeded_vm("group-info");
        let result = eval_string(
            &vm,
            r#"
            local info = game:GetService("GroupService"):GetGroupInfoAsync(5)
            assert(info.Id == 5, "id")
            assert(info.Name == "Builders", "name")
            assert(info.Description == "we build", "description")
            assert(info.MemberCount == 12, "members")
            assert(info.Created == 123, "created")
            assert(info.Owner.Id == 1, "owner id")
            assert(info.Owner.Username == "Builderman", "owner username")
            assert(info.Owner.DisplayName == "Builder", "owner display")
            return "ok"
            "#,
        );
        assert_eq!(result, "ok");

        let message = eval_error(&vm, r#"game:GetService("GroupService"):GetGroupInfoAsync(999)"#);
        assert!(message.contains("Unknown group: 999"), "{message}");
    }

    #[test]
    fn get_groups_carries_role_and_rank() {
        let vm = seeded_vm("group-list");
        let result = eval_string(
            &vm,
            r#"
            local list = game:GetService("GroupService"):GetGroupsAsync(10)
            assert(#list == 1, "count")
            assert(list[1].Id == 5, "id")
            assert(list[1].Name == "Builders", "name")
            assert(list[1].Role == "Member", "role")
            assert(list[1].Rank == 1, "rank")
            return "ok"
            "#,
        );
        assert_eq!(result, "ok");
    }

    #[test]
    fn allies_and_enemies_return_plain_arrays() {
        let vm = seeded_vm("group-relations");
        let result = eval_string(
            &vm,
            r#"
            local service = game:GetService("GroupService")
            local allies = service:GetAlliesAsync(5)
            assert(#allies == 1 and allies[1].Id == 7, "allies")
            local enemies = service:GetEnemiesAsync(5)
            assert(#enemies == 1 and enemies[1].Id == 9, "enemies")
            service:GetAlliesAsync(5)
            return "ok"
            "#,
        );
        assert_eq!(result, "ok");

        let warnings = vm.take_warnings();
        let allies = warnings
            .iter()
            .filter(|warning| warning.contains("GetAlliesAsync"))
            .count();
        assert_eq!(allies, 1, "{warnings:?}");
    }
}
