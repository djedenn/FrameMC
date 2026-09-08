use crate::script::engine::ScriptHost;

/// Event dispatched when an authenticated player joins the proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerJoinEvent {
    pub player_name: String,
    pub uuid: String,
    pub ip: String,
    pub protocol_version: i32,
}

impl PlayerJoinEvent {
    pub fn new(
        player_name: impl Into<String>,
        uuid: impl Into<String>,
        ip: impl Into<String>,
        protocol_version: i32,
    ) -> Self {
        Self {
            player_name: player_name.into(),
            uuid: uuid.into(),
            ip: ip.into(),
            protocol_version,
        }
    }
}

/// Result returned from the `on_player_join` Rhai hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinResult {
    pub allow: bool,
    pub disconnect_reason: String,
    pub target_server: String,
}

impl Default for JoinResult {
    fn default() -> Self {
        Self {
            allow: true,
            disconnect_reason: String::new(),
            target_server: String::new(),
        }
    }
}

/// Event dispatched when a player issues a chat command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerCommandEvent {
    pub player_name: String,
    pub command: String,
    pub current_server: String,
}

impl PlayerCommandEvent {
    pub fn new(player_name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            player_name: player_name.into(),
            command: command.into(),
            current_server: "lobby".to_string(),
        }
    }

    pub fn with_server(
        player_name: impl Into<String>,
        command: impl Into<String>,
        current_server: impl Into<String>,
    ) -> Self {
        Self {
            player_name: player_name.into(),
            command: command.into(),
            current_server: current_server.into(),
        }
    }
}

/// Result returned from the `on_player_command` Rhai hook.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandResult {
    pub cancel: bool,
    pub reroute_server: String,
    pub send_message: String,
}

/// Event dispatched when a player requests command tab completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerTabCompleteEvent {
    pub player_name: String,
    pub command: String,
    pub current_server: String,
}

impl PlayerTabCompleteEvent {
    pub fn new(
        player_name: impl Into<String>,
        command: impl Into<String>,
        current_server: impl Into<String>,
    ) -> Self {
        Self {
            player_name: player_name.into(),
            command: command.into(),
            current_server: current_server.into(),
        }
    }
}

impl From<PlayerTabCompleteEvent> for rhai::Map {
    fn from(event: PlayerTabCompleteEvent) -> Self {
        let mut map = rhai::Map::new();
        map.insert("player_name".into(), rhai::Dynamic::from(event.player_name));
        map.insert("command".into(), rhai::Dynamic::from(event.command));
        map.insert(
            "current_server".into(),
            rhai::Dynamic::from(event.current_server),
        );
        map
    }
}

impl From<PlayerJoinEvent> for rhai::Map {
    fn from(event: PlayerJoinEvent) -> Self {
        let mut map = rhai::Map::new();
        map.insert("player_name".into(), rhai::Dynamic::from(event.player_name));
        map.insert("uuid".into(), rhai::Dynamic::from(event.uuid));
        map.insert("ip".into(), rhai::Dynamic::from(event.ip));
        map.insert(
            "protocol_version".into(),
            rhai::Dynamic::from(event.protocol_version as i64),
        );
        map
    }
}

impl From<rhai::Map> for JoinResult {
    fn from(map: rhai::Map) -> Self {
        let allow = map
            .get("allow")
            .and_then(|v| v.as_bool().ok())
            .unwrap_or(true);
        let disconnect_reason = map
            .get("disconnect_reason")
            .map(|v| v.clone().into_string().unwrap_or_default())
            .unwrap_or_default();
        let target_server = map
            .get("target_server")
            .map(|v| v.clone().into_string().unwrap_or_default())
            .unwrap_or_default();

        Self {
            allow,
            disconnect_reason,
            target_server,
        }
    }
}

impl From<PlayerCommandEvent> for rhai::Map {
    fn from(event: PlayerCommandEvent) -> Self {
        let mut map = rhai::Map::new();
        map.insert("player_name".into(), rhai::Dynamic::from(event.player_name));
        map.insert("command".into(), rhai::Dynamic::from(event.command));
        map.insert(
            "current_server".into(),
            rhai::Dynamic::from(event.current_server),
        );
        map
    }
}

impl From<rhai::Map> for CommandResult {
    fn from(map: rhai::Map) -> Self {
        let cancel = map
            .get("cancel")
            .and_then(|v| v.as_bool().ok())
            .unwrap_or(false);
        let reroute_server = map
            .get("reroute_server")
            .map(|v| v.clone().into_string().unwrap_or_default())
            .unwrap_or_default();
        let send_message = map
            .get("send_message")
            .map(|v| v.clone().into_string().unwrap_or_default())
            .unwrap_or_default();

        Self {
            cancel,
            reroute_server,
            send_message,
        }
    }
}

impl ScriptHost {
    /// Evaluates the `on_player_join` hook for a connecting player across all loaded plugins.
    pub async fn eval_join(&self, event: PlayerJoinEvent) -> JoinResult {
        let mut final_result = JoinResult::default();

        let plugins = self.plugins.read().await;
        for plugin in plugins.iter() {
            if plugin
                .ast
                .iter_functions()
                .any(|f| f.name == "on_player_join")
            {
                let mut scope = rhai::Scope::new();
                let event_map: rhai::Map = event.clone().into();
                match self.engine.call_fn::<rhai::Map>(
                    &mut scope,
                    &plugin.ast,
                    "on_player_join",
                    (event_map,),
                ) {
                    Ok(map) => {
                        let res: JoinResult = map.into();
                        if !res.allow {
                            return res;
                        }
                        if !res.target_server.is_empty() {
                            final_result.target_server = res.target_server;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Error executing 'on_player_join' in plugin '{}': {e}",
                            plugin.name
                        );
                    }
                }
            }
        }

        let ast = self.ast.read().await;
        if ast.iter_functions().any(|f| f.name == "on_player_join") {
            let mut scope = rhai::Scope::new();
            let event_map: rhai::Map = event.into();
            match self
                .engine
                .call_fn::<rhai::Map>(&mut scope, &ast, "on_player_join", (event_map,))
            {
                Ok(map) => {
                    let res: JoinResult = map.into();
                    if !res.allow {
                        return res;
                    }
                    if !res.target_server.is_empty() {
                        final_result.target_server = res.target_server;
                    }
                }
                Err(e) => {
                    tracing::error!("Error executing 'on_player_join' Rhai hook: {e}");
                }
            }
        }

        final_result
    }

    /// Evaluates the `on_player_command` hook for an incoming command across all loaded plugins.
    pub async fn eval_command(&self, event: PlayerCommandEvent) -> CommandResult {
        let plugins = self.plugins.read().await;
        for plugin in plugins.iter() {
            if plugin
                .ast
                .iter_functions()
                .any(|f| f.name == "on_player_command")
            {
                let mut scope = rhai::Scope::new();
                let event_map: rhai::Map = event.clone().into();
                match self.engine.call_fn::<rhai::Map>(
                    &mut scope,
                    &plugin.ast,
                    "on_player_command",
                    (event_map,),
                ) {
                    Ok(map) => {
                        let res: CommandResult = map.into();
                        if res.cancel || !res.reroute_server.is_empty() {
                            return res;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Error executing 'on_player_command' in plugin '{}': {e}",
                            plugin.name
                        );
                    }
                }
            }
        }

        let ast = self.ast.read().await;
        if ast.iter_functions().any(|f| f.name == "on_player_command") {
            let mut scope = rhai::Scope::new();
            let event_map: rhai::Map = event.into();
            match self.engine.call_fn::<rhai::Map>(
                &mut scope,
                &ast,
                "on_player_command",
                (event_map,),
            ) {
                Ok(map) => {
                    let res: CommandResult = map.into();
                    if res.cancel || !res.reroute_server.is_empty() {
                        return res;
                    }
                }
                Err(e) => {
                    tracing::error!("Error executing 'on_player_command' Rhai hook: {e}");
                }
            }
        }

        CommandResult::default()
    }

    /// Evaluates the `on_tab_complete` hook for a command completion request across all loaded plugins and scripts.
    pub async fn eval_tab_complete(&self, event: PlayerTabCompleteEvent) -> Vec<String> {
        let mut results = Vec::new();

        let plugins = self.plugins.read().await;
        for plugin in plugins.iter() {
            if plugin
                .ast
                .iter_functions()
                .any(|f| f.name == "on_tab_complete")
            {
                let mut scope = rhai::Scope::new();
                let event_map: rhai::Map = event.clone().into();
                match self.engine.call_fn::<rhai::Array>(
                    &mut scope,
                    &plugin.ast,
                    "on_tab_complete",
                    (event_map,),
                ) {
                    Ok(arr) => {
                        for item in arr {
                            if let Ok(s) = item.into_string() {
                                if !results.contains(&s) {
                                    results.push(s);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "Error executing 'on_tab_complete' in plugin '{}': {e}",
                            plugin.name
                        );
                    }
                }
            }
        }

        let ast = self.ast.read().await;
        if ast.iter_functions().any(|f| f.name == "on_tab_complete") {
            let mut scope = rhai::Scope::new();
            let event_map: rhai::Map = event.into();
            match self.engine.call_fn::<rhai::Array>(
                &mut scope,
                &ast,
                "on_tab_complete",
                (event_map,),
            ) {
                Ok(arr) => {
                    for item in arr {
                        if let Ok(s) = item.into_string() {
                            if !results.contains(&s) {
                                results.push(s);
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Error executing 'on_tab_complete' Rhai hook: {e}");
                }
            }
        }

        results
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[tokio::test]
    async fn test_main_rhai_hub_and_lobby_reroute() {
        let host = ScriptHost::new();
        host.reload("scripts/main.rhai")
            .await
            .expect("Failed to load scripts/main.rhai");

        // Test /hub
        let hub_event = PlayerCommandEvent::new("Steve", "/hub");
        let hub_res = host.eval_command(hub_event).await;
        assert!(hub_res.cancel);
        assert_eq!(hub_res.reroute_server, "lobby");
        assert!(hub_res.send_message.contains("lobby"));

        // Test /lobby
        let lobby_event = PlayerCommandEvent::new("Alex", "/lobby");
        let lobby_res = host.eval_command(lobby_event).await;
        assert!(lobby_res.cancel);
        assert_eq!(lobby_res.reroute_server, "lobby");
        assert!(lobby_res.send_message.contains("lobby"));
    }

    #[tokio::test]
    async fn test_main_rhai_steel_reroute() {
        let host = ScriptHost::new();
        host.reload("scripts/main.rhai")
            .await
            .expect("Failed to load scripts/main.rhai");

        let steel_event = PlayerCommandEvent::new("Steve", "/steel");
        let steel_res = host.eval_command(steel_event).await;
        assert!(steel_res.cancel);
        assert_eq!(steel_res.reroute_server, "steelmc");
        assert!(steel_res.send_message.contains("SteelMC"));
    }

    #[tokio::test]
    async fn test_main_rhai_forbidden_commands() {
        let host = ScriptHost::new();
        host.reload("scripts/main.rhai")
            .await
            .expect("Failed to load scripts/main.rhai");

        // Test /stop
        let stop_event = PlayerCommandEvent::new("Steve", "/stop");
        let stop_res = host.eval_command(stop_event).await;
        assert!(stop_res.cancel);
        assert_eq!(stop_res.reroute_server, "");
        assert!(stop_res.send_message.contains("permission"));

        // Test /op
        let op_event = PlayerCommandEvent::new("Alex", "/op");
        let op_res = host.eval_command(op_event).await;
        assert!(op_res.cancel);
        assert_eq!(op_res.reroute_server, "");
        assert!(op_res.send_message.contains("permission"));
    }

    #[tokio::test]
    async fn test_main_rhai_passthrough_command() {
        let host = ScriptHost::new();
        host.reload("scripts/main.rhai")
            .await
            .expect("Failed to load scripts/main.rhai");

        let msg_event = PlayerCommandEvent::new("Steve", "/msg Notch Hello!");
        let msg_res = host.eval_command(msg_event).await;
        assert!(!msg_res.cancel);
        assert_eq!(msg_res.reroute_server, "");
        assert_eq!(msg_res.send_message, "");
    }

    #[tokio::test]
    async fn test_main_rhai_join_event() {
        let host = ScriptHost::new();
        host.reload("scripts/main.rhai")
            .await
            .expect("Failed to load scripts/main.rhai");

        let join_event = PlayerJoinEvent::new(
            "Steve",
            "069a79f4-44e3-4726-a9be-254cc4d37b01",
            "127.0.0.1",
            765,
        );
        let join_res = host.eval_join(join_event).await;
        assert!(join_res.allow);
        assert_eq!(join_res.target_server, "");
        assert_eq!(join_res.disconnect_reason, "");
    }

    #[tokio::test]
    async fn test_default_fallback_when_hooks_missing() {
        let host = ScriptHost::new(); // Empty AST

        let join_event = PlayerJoinEvent::new("Steve", "uuid", "127.0.0.1", 765);
        let join_res = host.eval_join(join_event).await;
        assert!(join_res.allow);
        assert_eq!(join_res.target_server, "");

        let cmd_event = PlayerCommandEvent::new("Steve", "/help");
        let cmd_res = host.eval_command(cmd_event).await;
        assert!(!cmd_res.cancel);
        assert_eq!(cmd_res.reroute_server, "");
        assert_eq!(cmd_res.send_message, "");
    }

    #[tokio::test]
    async fn test_server_switcher_plugin_commands() {
        let host = ScriptHost::new();
        host.set_servers(
            vec!["lobby".to_string(), "steelmc".to_string()],
            "lobby".to_string(),
        );
        host.load_plugin_file("plugins/server_switcher.rhai")
            .await
            .expect("Failed to load server_switcher.rhai plugin");

        // 1. Test /server (list servers)
        let list_cmd = PlayerCommandEvent::with_server("Steve", "/server", "lobby");
        let list_res = host.eval_command(list_cmd).await;
        assert!(list_res.cancel);
        assert_eq!(list_res.reroute_server, "");
        assert!(list_res.send_message.contains("Available servers"));
        assert!(list_res.send_message.contains("lobby"));
        assert!(list_res.send_message.contains("steelmc"));

        // 2. Test /server steelmc (valid switch from lobby)
        let switch_cmd = PlayerCommandEvent::with_server("Steve", "/server steelmc", "lobby");
        let switch_res = host.eval_command(switch_cmd).await;
        assert!(switch_res.cancel);
        assert_eq!(switch_res.reroute_server, "steelmc");
        assert!(switch_res.send_message.contains("Connecting to steelmc"));

        // 3. Test /server lobby when already on lobby (should notify already connected)
        let same_cmd = PlayerCommandEvent::with_server("Steve", "/server lobby", "lobby");
        let same_res = host.eval_command(same_cmd).await;
        assert!(same_res.cancel);
        assert_eq!(same_res.reroute_server, "");
        assert!(same_res.send_message.contains("already connected"));

        // 4. Test /server nonexistent (invalid server)
        let invalid_cmd = PlayerCommandEvent::with_server("Steve", "/server skyblock", "lobby");
        let invalid_res = host.eval_command(invalid_cmd).await;
        assert!(invalid_res.cancel);
        assert_eq!(invalid_res.reroute_server, "");
        assert!(invalid_res.send_message.contains("does not exist"));

        // 5. Test /hub alias
        let hub_cmd = PlayerCommandEvent::with_server("Steve", "/hub", "steelmc");
        let hub_res = host.eval_command(hub_cmd).await;
        assert!(hub_res.cancel);
        assert_eq!(hub_res.reroute_server, "lobby");
        assert!(hub_res.send_message.contains("Connecting to lobby"));
    }

    #[tokio::test]
    async fn test_server_switcher_tab_complete() {
        let host = ScriptHost::new();
        host.set_servers(
            vec!["lobby".to_string(), "steelmc".to_string()],
            "lobby".to_string(),
        );
        host.load_plugin_file("plugins/server_switcher.rhai")
            .await
            .expect("Failed to load server_switcher.rhai plugin");

        // 1. /server (all servers)
        let event = PlayerTabCompleteEvent::new("Steve", "/server ", "lobby");
        let suggestions = host.eval_tab_complete(event).await;
        assert_eq!(suggestions, vec!["lobby", "steelmc"]);

        // 2. /server st (filter by prefix)
        let event = PlayerTabCompleteEvent::new("Steve", "/server st", "lobby");
        let suggestions = host.eval_tab_complete(event).await;
        assert_eq!(suggestions, vec!["steelmc"]);

        // 3. /server lo (filter by prefix)
        let event = PlayerTabCompleteEvent::new("Steve", "/server lo", "lobby");
        let suggestions = host.eval_tab_complete(event).await;
        assert_eq!(suggestions, vec!["lobby"]);

        // 4. /s (suggest commands)
        let event = PlayerTabCompleteEvent::new("Steve", "/s", "lobby");
        let suggestions = host.eval_tab_complete(event).await;
        assert!(suggestions.contains(&"/server".to_string()));
        assert!(suggestions.contains(&"/steel".to_string()));

        // 5. / (all proxy shortcuts)
        let event = PlayerTabCompleteEvent::new("Steve", "/", "lobby");
        let suggestions = host.eval_tab_complete(event).await;
        assert!(suggestions.contains(&"/server".to_string()));
        assert!(suggestions.contains(&"/hub".to_string()));
        assert!(suggestions.contains(&"/lobby".to_string()));

        // 6. Unknown/backend command (should return empty to allow backend passthrough)
        let event = PlayerTabCompleteEvent::new("Steve", "/give ", "lobby");
        let suggestions = host.eval_tab_complete(event).await;
        assert!(suggestions.is_empty());
    }

    #[tokio::test]
    async fn test_server_switcher_plugin_loading_and_runtime() {
        let host = ScriptHost::new();
        host.set_servers(
            vec![
                "lobby".to_string(),
                "paper".to_string(),
                "steelmc".to_string(),
            ],
            "lobby".to_string(),
        );

        // 1. Load plugins from plugins directory
        let count = host
            .load_plugins_dir("plugins")
            .await
            .expect("Failed to load plugins directory");
        assert_eq!(
            count, 1,
            "Expected exactly 1 plugin loaded from plugins/, got: {}",
            count
        );

        // Verify loaded plugin metadata
        let plugins = host.plugins.read().await;
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "server_switcher");
        drop(plugins);

        // 2. Test server switcher execution through directory loading
        // /server (list available servers)
        let list_cmd = PlayerCommandEvent::with_server("Steve", "/server", "lobby");
        let list_res = host.eval_command(list_cmd).await;
        assert!(list_res.cancel);
        assert_eq!(list_res.reroute_server, "");
        assert!(list_res.send_message.contains("Available servers"));
        assert!(list_res.send_message.contains("lobby"));
        assert!(list_res.send_message.contains("paper"));
        assert!(list_res.send_message.contains("steelmc"));

        // /server paper (switch from lobby to paper)
        let switch_cmd = PlayerCommandEvent::with_server("Steve", "/server paper", "lobby");
        let switch_res = host.eval_command(switch_cmd).await;
        assert!(switch_res.cancel);
        assert_eq!(switch_res.reroute_server, "paper");
        assert!(switch_res.send_message.contains("Connecting to paper"));

        // /server lobby when already on lobby
        let same_cmd = PlayerCommandEvent::with_server("Steve", "/server lobby", "lobby");
        let same_res = host.eval_command(same_cmd).await;
        assert!(same_res.cancel);
        assert_eq!(same_res.reroute_server, "");
        assert!(same_res.send_message.contains("already connected"));

        // /server invalid target
        let invalid_cmd = PlayerCommandEvent::with_server("Steve", "/server nonexistent", "lobby");
        let invalid_res = host.eval_command(invalid_cmd).await;
        assert!(invalid_res.cancel);
        assert_eq!(invalid_res.reroute_server, "");
        assert!(invalid_res.send_message.contains("does not exist"));

        // Shortcut /hub -> routes to lobby
        let hub_cmd = PlayerCommandEvent::with_server("Steve", "/hub", "paper");
        let hub_res = host.eval_command(hub_cmd).await;
        assert!(hub_res.cancel);
        assert_eq!(hub_res.reroute_server, "lobby");
        assert!(hub_res.send_message.contains("Connecting to lobby"));

        // Shortcut /steel -> routes to steelmc
        let steel_cmd = PlayerCommandEvent::with_server("Steve", "/steel", "lobby");
        let steel_res = host.eval_command(steel_cmd).await;
        assert!(steel_res.cancel);
        assert_eq!(steel_res.reroute_server, "steelmc");
        assert!(steel_res.send_message.contains("Connecting to SteelMC"));

        // Direct /paper command
        let paper_cmd = PlayerCommandEvent::with_server("Steve", "/paper", "lobby");
        let paper_res = host.eval_command(paper_cmd).await;
        assert!(paper_res.cancel);
        assert_eq!(paper_res.reroute_server, "paper");
        assert!(paper_res.send_message.contains("Connecting to paper"));

        // 3. Dynamic tab-completion via the loaded plugin
        let tc_server = PlayerTabCompleteEvent::new("Steve", "/server ", "lobby");
        let s_sugg = host.eval_tab_complete(tc_server).await;
        assert!(s_sugg.contains(&"lobby".to_string()));
        assert!(s_sugg.contains(&"paper".to_string()));
        assert!(s_sugg.contains(&"steelmc".to_string()));

        let tc_prefix = PlayerTabCompleteEvent::new("Steve", "/server st", "lobby");
        let st_sugg = host.eval_tab_complete(tc_prefix).await;
        assert_eq!(st_sugg, vec!["steelmc"]);

        let tc_slash = PlayerTabCompleteEvent::new("Steve", "/", "lobby");
        let slash_sugg = host.eval_tab_complete(tc_slash).await;
        assert!(slash_sugg.contains(&"/server".to_string()));
        assert!(slash_sugg.contains(&"/hub".to_string()));
        assert!(slash_sugg.contains(&"/lobby".to_string()));
    }

    #[tokio::test]
    async fn test_plugin_security_and_edge_cases() {
        let host = ScriptHost::new();
        host.set_servers(
            vec![
                "lobby".to_string(),
                "paper".to_string(),
                "steelmc".to_string(),
            ],
            "lobby".to_string(),
        );

        // Load the server switcher plugin from plugins directory
        host.load_plugin_file("plugins/server_switcher.rhai")
            .await
            .expect("Failed to load server_switcher.rhai");

        // 1. Edge cases: Whitespace and empty targets
        let empty_arg = PlayerCommandEvent::with_server("Steve", "/server   ", "lobby");
        let empty_res = host.eval_command(empty_arg).await;
        assert!(empty_res.cancel);
        assert_eq!(empty_res.reroute_server, "");
        assert!(empty_res.send_message.contains("Usage: /server"));

        // 2. Unregistered command passthrough (e.g. gameplay or backend commands)
        let unhandled_cmd = PlayerCommandEvent::with_server("Steve", "/help", "lobby");
        let unhandled_res = host.eval_command(unhandled_cmd).await;
        assert!(!unhandled_res.cancel);
        assert_eq!(unhandled_res.reroute_server, "");
        assert_eq!(unhandled_res.send_message, "");

        let chat_cmd = PlayerCommandEvent::with_server("Steve", "/msg Friend hey", "lobby");
        let chat_res = host.eval_command(chat_cmd).await;
        assert!(!chat_res.cancel);
        assert_eq!(chat_res.reroute_server, "");

        // 3. Tab completion edge cases
        let tc_unknown = PlayerTabCompleteEvent::new("Steve", "/unknowncmd ", "lobby");
        let tc_res = host.eval_tab_complete(tc_unknown).await;
        assert!(
            tc_res.is_empty(),
            "Unknown command tab completion should be empty to allow backend passthrough"
        );

        let tc_root = PlayerTabCompleteEvent::new("Steve", "/", "lobby");
        let root_sugg = host.eval_tab_complete(tc_root).await;
        assert!(root_sugg.contains(&"/server".to_string()));
        assert!(root_sugg.contains(&"/lobby".to_string()));

        // 4. Non-map return types and script error isolation
        let bad_script_host = ScriptHost::new();
        let bad_script = r#"
            fn on_player_command(event) {
                return 42;
            }
            fn on_player_join(event) {
                return "invalid_string_return";
            }
            fn on_tab_complete(event) {
                return "not_an_array";
            }
        "#;
        bad_script_host
            .load_script(bad_script)
            .await
            .expect("Failed to load test bad_script");

        let cmd = PlayerCommandEvent::with_server("Steve", "/test", "lobby");
        let res = bad_script_host.eval_command(cmd).await;
        assert!(
            !res.cancel,
            "Invalid non-map return type should default to uncancelled command"
        );
        assert_eq!(res.reroute_server, "");
        assert_eq!(res.send_message, "");

        let join = PlayerJoinEvent::new(
            "Steve",
            "00000000-0000-0000-0000-000000000000",
            "127.0.0.1",
            765,
        );
        let join_res = bad_script_host.eval_join(join).await;
        assert!(
            join_res.allow,
            "Invalid non-map return type should default to allow join"
        );

        let tc_bad = PlayerTabCompleteEvent::new("Steve", "/test ", "lobby");
        let tc_bad_res = bad_script_host.eval_tab_complete(tc_bad).await;
        assert!(
            tc_bad_res.is_empty(),
            "Invalid non-array return type should default to empty suggestions"
        );

        // 5. Script runtime exception isolation
        let err_script_host = ScriptHost::new();
        let err_script = r#"
            fn on_player_command(event) {
                throw "runtime script failure";
            }
            fn on_player_join(event) {
                throw "runtime script failure";
            }
            fn on_tab_complete(event) {
                throw "runtime script failure";
            }
        "#;
        err_script_host
            .load_script(err_script)
            .await
            .expect("Failed to load test err_script");

        let err_cmd = PlayerCommandEvent::with_server("Steve", "/crash", "lobby");
        let err_res = err_script_host.eval_command(err_cmd).await;
        assert!(
            !err_res.cancel,
            "Script throwing error should gracefully fallback without crashing proxy"
        );

        let err_join = PlayerJoinEvent::new(
            "Steve",
            "00000000-0000-0000-0000-000000000000",
            "127.0.0.1",
            765,
        );
        let err_join_res = err_script_host.eval_join(err_join).await;
        assert!(
            err_join_res.allow,
            "Script throwing error should gracefully allow join"
        );

        let tc_err = PlayerTabCompleteEvent::new("Steve", "/crash ", "lobby");
        let tc_err_res = err_script_host.eval_tab_complete(tc_err).await;
        assert!(
            tc_err_res.is_empty(),
            "Script throwing error should gracefully return empty completions"
        );
    }
}
