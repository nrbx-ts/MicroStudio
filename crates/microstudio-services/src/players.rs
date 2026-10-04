use std::collections::HashMap;

use microstudio_datamodel::{
    AttributeValue, DataModel, DataModelError, EventArg, EventKind, InstanceId, PendingEvent,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerOptions {
    // placeholder Model only: no physics, no Humanoid simulation
    pub with_character: bool,
    pub user_id: Option<i64>,
}

#[derive(Debug, Default)]
pub struct PlayersState {
    next_user_id: i64,
    players: Vec<InstanceId>,
    local_player: Option<InstanceId>,
}

impl PlayersState {
    pub fn players(&self) -> &[InstanceId] {
        &self.players
    }

    pub fn local_player(&self) -> Option<InstanceId> {
        self.local_player
    }

    pub fn contains(&self, player: InstanceId) -> bool {
        self.players.contains(&player)
    }
}

pub fn add_mock_player(
    dm: &mut DataModel,
    state: &mut PlayersState,
    name: &str,
    options: PlayerOptions,
) -> Result<(InstanceId, Vec<PendingEvent>), DataModelError> {
    let players_service = dm.get_service("Players")?;
    let workspace = dm.get_service("Workspace")?;

    let user_id = match options.user_id {
        Some(id) => id,
        None => {
            state.next_user_id += 1;
            state.next_user_id
        }
    };

    let player = dm.create_internal("Player")?;
    dm.set_name(player, name)?;
    dm.set_property(player, "UserId", AttributeValue::Number(user_id as f64))?;
    dm.set_property(
        player,
        "DisplayName",
        AttributeValue::String(name.to_string()),
    )?;
    dm.set_property(player, "AccountAge", AttributeValue::Number(0.0))?;

    let mut events = dm.set_parent(player, Some(players_service))?;

    if options.with_character {
        let (character, mut character_events) = spawn_character(dm, name, workspace)?;
        dm.set_property(player, "Character", AttributeValue::Instance(character))?;
        events.append(&mut character_events);

        let signal = dm.signal(player, EventKind::CharacterAdded)?;
        events.push(PendingEvent {
            signal,
            args: vec![EventArg::Instance(character)],
        });
    }

    state.players.push(player);
    if state.local_player.is_none() {
        state.local_player = Some(player);
    }

    let signal = dm.signal(players_service, EventKind::PlayerAdded)?;
    events.push(PendingEvent {
        signal,
        args: vec![EventArg::Instance(player)],
    });

    Ok((player, events))
}

fn spawn_character(
    dm: &mut DataModel,
    name: &str,
    workspace: InstanceId,
) -> Result<(InstanceId, Vec<PendingEvent>), DataModelError> {
    let character = dm.create_instance("Model")?;
    dm.set_name(character, name)?;

    let humanoid = dm.create_instance("Humanoid")?;
    dm.set_name(humanoid, "Humanoid")?;
    dm.set_property(humanoid, "Health", AttributeValue::Number(100.0))?;
    dm.set_property(humanoid, "MaxHealth", AttributeValue::Number(100.0))?;
    dm.set_property(humanoid, "WalkSpeed", AttributeValue::Number(16.0))?;
    dm.set_property(humanoid, "JumpPower", AttributeValue::Number(50.0))?;

    let mut events = dm.set_parent(humanoid, Some(character))?;
    events.append(&mut dm.set_parent(character, Some(workspace))?);

    Ok((character, events))
}

pub fn get_players(state: &PlayersState) -> Vec<InstanceId> {
    state.players.clone()
}

pub fn get_player_by_user_id(dm: &DataModel, state: &PlayersState, user_id: i64) -> Option<InstanceId> {
    state.players.iter().copied().find(|p| {
        dm.get_property(*p, "UserId")
            .and_then(|v| v.as_number())
            .map(|v| v as i64 == user_id)
            .unwrap_or(false)
    })
}

pub fn get_player_from_character(
    dm: &DataModel,
    state: &PlayersState,
    character: InstanceId,
) -> Option<InstanceId> {
    state.players.iter().copied().find(|p| {
        dm.get_property(*p, "Character")
            .and_then(|v| v.as_instance())
            == Some(character)
    })
}

pub fn remove_mock_player(
    dm: &mut DataModel,
    state: &mut PlayersState,
    player: InstanceId,
) -> Result<Vec<PendingEvent>, DataModelError> {
    let players_service = dm.get_service("Players")?;
    let mut events = Vec::new();

    if state.players.contains(&player) {
        let signal = dm.signal(players_service, EventKind::PlayerRemoving)?;
        events.push(PendingEvent {
            signal,
            args: vec![EventArg::Instance(player)],
        });
    }

    if let Some(character) = dm.get_property(player, "Character").and_then(|v| v.as_instance()) {
        dm.destroy(character)?;
    }

    events.extend(dm.destroy(player)?);
    state.players.retain(|p| *p != player);
    if state.local_player == Some(player) {
        state.local_player = state.players.first().copied();
    }
    Ok(events)
}

impl crate::world::World {
    pub fn add_mock_player(
        &mut self,
        name: &str,
        options: PlayerOptions,
    ) -> Result<(InstanceId, Vec<PendingEvent>), DataModelError> {
        add_mock_player(&mut self.dm, &mut self.players, name, options)
    }

    pub fn remove_mock_player(
        &mut self,
        player: InstanceId,
    ) -> Result<Vec<PendingEvent>, DataModelError> {
        remove_mock_player(&mut self.dm, &mut self.players, player)
    }

    pub fn player_names(&self) -> Vec<String> {
        self.players
            .players()
            .iter()
            .filter_map(|p| self.dm.name_of(*p).map(str::to_string))
            .collect()
    }

    pub fn player_by_name(&self, name: &str) -> Option<InstanceId> {
        self.players
            .players()
            .iter()
            .copied()
            .find(|p| self.dm.name_of(*p) == Some(name))
    }

    pub fn player_table(&self) -> HashMap<String, InstanceId> {
        self.players
            .players()
            .iter()
            .filter_map(|p| self.dm.name_of(*p).map(|n| (n.to_string(), *p)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> crate::world::World {
        let mut world = crate::world::World::new();
        world.bootstrap().unwrap();
        world
    }

    fn run(_world: &mut crate::world::World, events: Vec<PendingEvent>) {
        for event in events {
            event.signal.fire(&event.args);
        }
    }

    #[test]
    fn adding_a_mock_player_parents_it_and_fires_player_added() {
        let mut world = world();
        let players = world.get_service("Players").unwrap();
        let signal = world.dm.signal(players, EventKind::PlayerAdded).unwrap();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen2 = seen.clone();
        signal.connect(move |args| seen2.borrow_mut().push(args[0].clone()));

        let (player, events) = world.add_mock_player("Eddie", PlayerOptions::default()).unwrap();
        run(&mut world, events);

        assert_eq!(world.dm.name_of(player), Some("Eddie"));
        assert_eq!(world.dm.parent_of(player), Some(players));
        assert_eq!(
            world.dm.get_property(player, "UserId").and_then(|v| v.as_number()),
            Some(1.0)
        );
        assert_eq!(world.players.local_player(), Some(player));
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(world.player_names(), vec!["Eddie"]);
    }

    #[test]
    fn user_ids_increment_and_can_be_overridden() {
        let mut world = world();
        let (a, _) = world.add_mock_player("A", PlayerOptions::default()).unwrap();
        let (b, _) = world
            .add_mock_player(
                "B",
                PlayerOptions {
                    with_character: false,
                    user_id: Some(999),
                },
            )
            .unwrap();
        assert_eq!(world.dm.get_property(a, "UserId").and_then(|v| v.as_number()), Some(1.0));
        assert_eq!(world.dm.get_property(b, "UserId").and_then(|v| v.as_number()), Some(999.0));
    }

    #[test]
    fn character_spawns_in_workspace_with_a_humanoid() {
        let mut world = world();
        let workspace = world.get_service("Workspace").unwrap();
        let (player, events) = world
            .add_mock_player(
                "Eddie",
                PlayerOptions {
                    with_character: true,
                    user_id: None,
                },
            )
            .unwrap();
        run(&mut world, events);

        let character = world
            .dm
            .get_property(player, "Character")
            .and_then(|v| v.as_instance())
            .expect("character should be assigned");
        assert_eq!(world.dm.parent_of(character), Some(workspace));
        assert_eq!(world.dm.name_of(character), Some("Eddie"));
        assert!(world.dm.find_first_child_of_class(character, "Humanoid").is_some());
        assert!(world.dm.is_a(character, "Model"));
    }

    #[test]
    fn removing_a_player_fires_removing_and_destroys_the_instance() {
        let mut world = world();
        let players = world.get_service("Players").unwrap();
        let signal = world.dm.signal(players, EventKind::PlayerRemoving).unwrap();
        let hits = std::rc::Rc::new(std::cell::Cell::new(0));
        let hits2 = hits.clone();
        signal.connect(move |_| hits2.set(hits2.get() + 1));

        let (player, _) = world.add_mock_player("Eddie", PlayerOptions::default()).unwrap();
        let events = world.remove_mock_player(player).unwrap();
        run(&mut world, events);

        assert_eq!(hits.get(), 1);
        assert!(!world.dm.contains(player));
        assert!(world.player_names().is_empty());
        assert_eq!(world.players.local_player(), None);
    }

    #[test]
    fn lookup_helpers_find_players() {
        let mut world = world();
        let (player, events) = world
            .add_mock_player(
                "Eddie",
                PlayerOptions {
                    with_character: true,
                    user_id: None,
                },
            )
            .unwrap();
        run(&mut world, events);
        let character = world
            .dm
            .get_property(player, "Character")
            .and_then(|v| v.as_instance())
            .unwrap();

        assert_eq!(get_player_by_user_id(&world.dm, &world.players, 1), Some(player));
        assert_eq!(
            get_player_from_character(&world.dm, &world.players, character),
            Some(player)
        );
        assert_eq!(world.player_by_name("Eddie"), Some(player));
        assert_eq!(world.player_by_name("Nobody"), None);
    }
}
