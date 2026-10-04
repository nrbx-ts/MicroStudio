// flags describe a local dev runtime, not a player client

#[derive(Debug, Clone, Copy)]
pub struct RunServiceState {
    // IsStudio: default true so studio-dev code paths opt in
    pub is_studio: bool,
    pub is_server: bool,
    // always false in V1: no client
    pub is_client: bool,
    pub is_running: bool,
    pub is_edit: bool,
}

impl Default for RunServiceState {
    fn default() -> Self {
        Self {
            is_studio: true,
            is_server: true,
            is_client: false,
            is_running: true,
            is_edit: false,
        }
    }
}

impl RunServiceState {
    pub fn dev() -> Self {
        Self::default()
    }

    // separate from dev so tests can diverge
    pub fn test() -> Self {
        Self::default()
    }

    pub fn headless() -> Self {
        Self {
            is_studio: false,
            is_running: false,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_describe_a_server_side_dev_session() {
        let state = RunServiceState::default();
        assert!(state.is_studio);
        assert!(state.is_server);
        assert!(!state.is_client);
        assert!(state.is_running);
        assert!(!state.is_edit);
    }

    #[test]
    fn headless_turns_studio_semantics_off() {
        let state = RunServiceState::headless();
        assert!(!state.is_studio);
        assert!(!state.is_running);
        assert!(state.is_server);
    }
}
