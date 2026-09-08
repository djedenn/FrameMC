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
    async fn test_all_20_popular_bungee_plugins_load_and_run() {
        let host = ScriptHost::new();
        host.set_servers(
            vec![
                "lobby".to_string(),
                "paper".to_string(),
                "steelmc".to_string(),
            ],
            "lobby".to_string(),
        );

        // 1. Load all plugins from plugins directory
        let mut entries = tokio::fs::read_dir("plugins").await.unwrap();
        let mut loaded = 0;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("rhai") {
                let p_str = p.to_str().unwrap();
                match host.load_plugin_file(p_str).await {
                    Ok(()) => loaded += 1,
                    Err(e) => eprintln!("FAILED to load plugin {}: {:?}", p_str, e),
                }
            }
        }
        assert!(
            loaded >= 21,
            "Expected at least 21 plugins loaded, got: {}",
            loaded
        );

        // 2. Test Plugin 01: LiteBans (/ban, /unban, /mute, join hook)
        let ban_cmd = PlayerCommandEvent::with_server("Admin", "/ban BadActor Cheating", "lobby");
        let ban_res = host.eval_command(ban_cmd).await;
        assert!(ban_res.cancel);
        assert!(ban_res.send_message.contains("LiteBans"));

        // Join as BadActor should be denied
        let bad_join = PlayerJoinEvent::new(
            "BadActor",
            "069a79f4-44e3-4726-a9be-254cc4d37b01",
            "127.0.0.1",
            765,
        );
        let join_res = host.eval_join(bad_join).await;
        assert!(!join_res.allow, "Banned player must not be allowed to join");
        assert!(join_res.disconnect_reason.contains("BANNED"));

        // Unban BadActor
        let unban_cmd = PlayerCommandEvent::with_server("Admin", "/unban BadActor", "lobby");
        let unban_res = host.eval_command(unban_cmd).await;
        assert!(unban_res.cancel);
        assert!(unban_res.send_message.contains("unbanned"));

        // Join as BadActor should now succeed
        let good_join = PlayerJoinEvent::new(
            "BadActor",
            "069a79f4-44e3-4726-a9be-254cc4d37b01",
            "127.0.0.1",
            765,
        );
        let join_res2 = host.eval_join(good_join).await;
        assert!(join_res2.allow, "Unbanned player must be allowed to join");

        // 3. Test Plugin 07: Maintenance Mode
        let maint_on = PlayerCommandEvent::with_server("Admin", "/maintenance on", "lobby");
        let maint_res = host.eval_command(maint_on).await;
        assert!(maint_res.cancel);
        assert!(maint_res.send_message.contains("ACTIVATED"));

        // Non-whitelisted join should be denied
        let regular_join = PlayerJoinEvent::new(
            "RegularPlayer",
            "11111111-2222-3333-4444-555555555555",
            "127.0.0.1",
            765,
        );
        let m_join_res = host.eval_join(regular_join).await;
        assert!(
            !m_join_res.allow,
            "Non-whitelisted player must be rejected during maintenance"
        );
        assert!(m_join_res.disconnect_reason.contains("MAINTENANCE"));

        // Add to whitelist
        let wl_cmd =
            PlayerCommandEvent::with_server("Admin", "/maintenance add RegularPlayer", "lobby");
        let wl_res = host.eval_command(wl_cmd).await;
        assert!(wl_res.cancel);

        // Join should now succeed
        let wl_join = PlayerJoinEvent::new(
            "RegularPlayer",
            "11111111-2222-3333-4444-555555555555",
            "127.0.0.1",
            765,
        );
        let wl_join_res = host.eval_join(wl_join).await;
        assert!(
            wl_join_res.allow,
            "Whitelisted player must be allowed during maintenance"
        );

        // Disable maintenance
        let maint_off = PlayerCommandEvent::with_server("Admin", "/maintenance off", "lobby");
        let _ = host.eval_command(maint_off).await;

        // 4. Test Plugin 09: BungeeGuard (handshake validation)
        let invalid_join = PlayerJoinEvent::new(
            "Bad Name!",
            "22222222-3333-4444-5555-666666666666",
            "127.0.0.1",
            765,
        );
        let inv_res = host.eval_join(invalid_join).await;
        assert!(
            !inv_res.allow,
            "Username with spaces and exclamation must be rejected by BungeeGuard"
        );
        assert!(
            inv_res.disconnect_reason.contains("Security")
                || inv_res.disconnect_reason.contains("username")
        );

        // 5. Test Plugin 17: ProxyAuth (/register, /login)
        let reg_cmd = PlayerCommandEvent::with_server("NewUser", "/register 12345", "lobby");
        let reg_res = host.eval_command(reg_cmd).await;
        assert!(reg_res.cancel);
        assert!(reg_res.send_message.contains("ProxyAuth"));

        let log_cmd = PlayerCommandEvent::with_server("NewUser", "/login 12345", "lobby");
        let log_res = host.eval_command(log_cmd).await;
        assert!(log_res.cancel);
        assert!(log_res.send_message.contains("accepted"));

        let admin_reg = PlayerCommandEvent::with_server("Admin", "/register admin123", "lobby");
        let _ = host.eval_command(admin_reg).await;

        // 6. Test Plugin 04: BungeeParties (/party create, /party list)
        let p_create = PlayerCommandEvent::with_server("Leader", "/party create", "lobby");
        let p_res = host.eval_command(p_create).await;
        assert!(p_res.cancel);
        assert!(p_res.send_message.contains("Parties"));

        let p_list = PlayerCommandEvent::with_server("Leader", "/party list", "lobby");
        let plist_res = host.eval_command(p_list).await;
        assert!(plist_res.cancel);
        assert!(plist_res.send_message.contains("Leader"));

        // 7. Test Plugin 19: NetworkStats (/proxyinfo) with authenticated player
        let pinfo_cmd = PlayerCommandEvent::with_server("NewUser", "/proxyinfo", "lobby");
        let pinfo_res = host.eval_command(pinfo_cmd).await;
        assert!(pinfo_res.cancel);
        assert!(pinfo_res.send_message.contains("FrameMC"));

        // 8. Test Plugin 02: TabList Plus (/glist, /tablist)
        let glist_cmd = PlayerCommandEvent::with_server("NewUser", "/glist", "lobby");
        let glist_res = host.eval_command(glist_cmd).await;
        assert!(glist_res.cancel);
        assert!(glist_res.send_message.contains("Network"));

        let tablist_cmd = PlayerCommandEvent::with_server("NewUser", "/tablist", "lobby");
        let tablist_res = host.eval_command(tablist_cmd).await;
        assert!(tablist_res.cancel);
        assert!(tablist_res.send_message.contains("TabListPlus"));

        // 9. Test Plugin 03: ProxyChat (/g, /channeltoggle)
        let gchat_cmd =
            PlayerCommandEvent::with_server("NewUser", "/g Hello from test suite!", "lobby");
        let gchat_res = host.eval_command(gchat_cmd).await;
        assert!(gchat_res.cancel);
        assert!(gchat_res.send_message.contains("Global"));

        let ch_cmd = PlayerCommandEvent::with_server("NewUser", "/channeltoggle", "lobby");
        let ch_res = host.eval_command(ch_cmd).await;
        assert!(ch_res.cancel);
        assert!(ch_res.send_message.contains("GLOBAL"));

        // 10. Test Plugin 05: StaffChat (/sc)
        let sc_cmd =
            PlayerCommandEvent::with_server("Admin", "/sc Attention staff members", "lobby");
        let sc_res = host.eval_command(sc_cmd).await;
        assert!(sc_res.cancel);
        assert!(sc_res.send_message.contains("StaffChat"));

        // 11. Test Plugin 06: ServerPortals (/portal link, /portal survival)
        let link_cmd =
            PlayerCommandEvent::with_server("Admin", "/portal link survival paper", "lobby");
        let link_res = host.eval_command(link_cmd).await;
        assert!(link_res.cancel);
        assert!(link_res.send_message.contains("survival"));

        let portal_enter = PlayerCommandEvent::with_server("NewUser", "/portal survival", "lobby");
        let enter_res = host.eval_command(portal_enter).await;
        assert!(enter_res.cancel);
        assert_eq!(enter_res.reroute_server, "paper");

        // 12. Test Plugin 08: LobbyBalancer (/balancer status)
        let bal_cmd = PlayerCommandEvent::with_server("NewUser", "/balancer status", "lobby");
        let bal_res = host.eval_command(bal_cmd).await;
        assert!(bal_res.cancel);
        assert!(bal_res.send_message.contains("LobbyBalancer"));

        // 13. Test Plugin 10: BungeeAnnounce (/alert, /announce, /tip)
        let alert_cmd =
            PlayerCommandEvent::with_server("Admin", "/alert Network reboot in 5m", "lobby");
        let alert_res = host.eval_command(alert_cmd).await;
        assert!(alert_res.cancel);
        assert!(alert_res.send_message.contains("ALERT"));

        let ann_cmd =
            PlayerCommandEvent::with_server("Admin", "/announce Welcome to our server!", "lobby");
        let ann_res = host.eval_command(ann_cmd).await;
        assert!(ann_res.cancel);
        assert!(ann_res.send_message.contains("ANNOUNCEMENT"));

        let tip_cmd =
            PlayerCommandEvent::with_server("Admin", "/tip Use /server to switch worlds", "lobby");
        let tip_res = host.eval_command(tip_cmd).await;
        assert!(tip_res.cancel);
        assert!(tip_res.send_message.contains("TIP"));

        // 14. Test Plugin 11: SkinsRestorer (/skin Notch, /skin info)
        let skin_cmd = PlayerCommandEvent::with_server("NewUser", "/skin Notch", "lobby");
        let skin_res = host.eval_command(skin_cmd).await;
        assert!(skin_res.cancel);
        assert!(skin_res.send_message.contains("SkinsRestorer"));

        let skin_info = PlayerCommandEvent::with_server("NewUser", "/skin info", "lobby");
        let sinfo_res = host.eval_command(skin_info).await;
        assert!(sinfo_res.cancel);
        assert!(sinfo_res.send_message.contains("Notch"));

        // 15. Test Plugin 12: CommandSpy (/cmdspy toggle, /cmdspy list)
        let spy_cmd = PlayerCommandEvent::with_server("Admin", "/cmdspy toggle", "lobby");
        let spy_res = host.eval_command(spy_cmd).await;
        assert!(spy_res.cancel);
        assert!(spy_res.send_message.contains("CmdSpy"));

        // 16. Test Plugin 13: BungeeFind (/whereami, /find NewUser)
        let where_cmd = PlayerCommandEvent::with_server("NewUser", "/whereami", "lobby");
        let where_res = host.eval_command(where_cmd).await;
        assert!(where_res.cancel);
        assert!(where_res.send_message.contains("Connection Info"));

        let find_cmd = PlayerCommandEvent::with_server("Admin", "/find NewUser", "lobby");
        let find_res = host.eval_command(find_cmd).await;
        assert!(find_res.cancel);
        assert!(find_res.send_message.contains("NewUser"));

        // 17. Test Plugin 14: ServerSend (/sendall paper)
        let send_cmd = PlayerCommandEvent::with_server("Admin", "/sendall paper", "lobby");
        let send_res = host.eval_command(send_cmd).await;
        assert!(send_res.cancel);
        assert_eq!(send_res.reroute_server, "paper");

        // 18. Test Plugin 15: AntiBot (/antibot status, /antibot banip)
        let abot_cmd = PlayerCommandEvent::with_server("Admin", "/antibot status", "lobby");
        let abot_res = host.eval_command(abot_cmd).await;
        assert!(abot_res.cancel);
        assert!(abot_res.send_message.contains("AntiBot"));

        let banip_cmd = PlayerCommandEvent::with_server(
            "Admin",
            "/antibot banip 10.0.0.99 Malicious flood",
            "lobby",
        );
        let banip_res = host.eval_command(banip_cmd).await;
        assert!(banip_res.cancel);
        assert!(banip_res.send_message.contains("10.0.0.99"));

        // 19. Test Plugin 16: DynamicMOTD (/motd get, /motd set)
        let motd_set = PlayerCommandEvent::with_server(
            "Admin",
            "/motd set §aFrameMC Test §7| §eHigh Perf",
            "lobby",
        );
        let mset_res = host.eval_command(motd_set).await;
        assert!(mset_res.cancel);
        assert!(mset_res.send_message.contains("DynamicMOTD"));

        let motd_get = PlayerCommandEvent::with_server("Admin", "/motd get", "lobby");
        let mget_res = host.eval_command(motd_get).await;
        assert!(mget_res.cancel);
        assert!(mget_res.send_message.contains("FrameMC Test"));

        // 20. Test Plugin 18: NetworkReport (/report, /reports list)
        let rep_cmd = PlayerCommandEvent::with_server(
            "NewUser",
            "/report HackerSpeed Flying around lobby",
            "lobby",
        );
        let rep_res = host.eval_command(rep_cmd).await;
        assert!(rep_res.cancel);
        assert!(rep_res.send_message.contains("Report"));

        let replist_cmd = PlayerCommandEvent::with_server("Admin", "/reports list", "lobby");
        let rlist_res = host.eval_command(replist_cmd).await;
        assert!(rlist_res.cancel);
        assert!(rlist_res.send_message.contains("HackerSpeed"));

        // 21. Test Plugin 20: AutoReconnect (/lastserver, /reconnect, /fallback)
        let last_cmd = PlayerCommandEvent::with_server("NewUser", "/lastserver", "lobby");
        let last_res = host.eval_command(last_cmd).await;
        assert!(last_res.cancel);
        assert!(last_res.send_message.contains("lobby"));

        let fb_cmd = PlayerCommandEvent::with_server("NewUser", "/fallback", "lobby");
        let fb_res = host.eval_command(fb_cmd).await;
        assert!(fb_res.cancel);
        assert!(!fb_res.reroute_server.is_empty());

        // 22. Test Tab Completion across plugins
        let tc_ban = PlayerTabCompleteEvent::new("Admin", "/ban", "lobby");
        let ban_sugg = host.eval_tab_complete(tc_ban).await;
        assert!(ban_sugg.iter().any(|s| s.contains("/ban")));

        let tc_p = PlayerTabCompleteEvent::new("Admin", "/party", "lobby");
        let p_sugg = host.eval_tab_complete(tc_p).await;
        assert!(p_sugg.iter().any(|s| s.contains("/party")));

        let tc_maint = PlayerTabCompleteEvent::new("Admin", "/maintenance", "lobby");
        let maint_sugg = host.eval_tab_complete(tc_maint).await;
        assert!(maint_sugg.iter().any(|s| s.contains("/maintenance")));

        let tc_gl = PlayerTabCompleteEvent::new("Admin", "/glist", "lobby");
        let gl_sugg = host.eval_tab_complete(tc_gl).await;
        assert!(gl_sugg.iter().any(|s| s.contains("/glist")));

        let tc_motd = PlayerTabCompleteEvent::new("Admin", "/motd", "lobby");
        let motd_sugg = host.eval_tab_complete(tc_motd).await;
        assert!(motd_sugg.iter().any(|s| s.contains("/motd")));
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

        let mut entries = tokio::fs::read_dir("plugins").await.unwrap();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("rhai") {
                let _ = host.load_plugin_file(p.to_str().unwrap()).await;
            }
        }

        // 1. Captive Quarantine Enforcement
        // Player joins -> marked unauthenticated
        let unauth_join = PlayerJoinEvent::new(
            "Quarantined",
            "33333333-4444-5555-6666-777777777777",
            "127.0.0.1",
            765,
        );
        let _ = host.eval_join(unauth_join).await;

        let blocked_cmds = vec![
            "/portal survival",
            "/g Hello everyone",
            "/whereami",
            "/report Hacker speedhack",
            "/reconnect",
            "/tablist",
            "/party create",
            "/proxyinfo",
        ];
        for cmd_str in blocked_cmds {
            let cmd = PlayerCommandEvent::with_server("Quarantined", cmd_str, "lobby");
            let res = host.eval_command(cmd).await;
            assert!(
                res.cancel,
                "Command '{}' should be blocked for quarantined player",
                cmd_str
            );
            assert!(
                res.send_message.to_lowercase().contains("auth")
                    || res.send_message.to_lowercase().contains("login")
                    || res.send_message.to_lowercase().contains("register"),
                "Command '{}' message should instruct player to authenticate: got '{}'",
                cmd_str,
                res.send_message
            );
        }

        // Quarantine tab completion: should not suggest 20 plugin commands like /ban, /party, /maintenance, etc.
        assert!(host
            .eval_tab_complete(PlayerTabCompleteEvent::new("Quarantined", "/ban", "lobby"))
            .await
            .is_empty());
        assert!(host
            .eval_tab_complete(PlayerTabCompleteEvent::new(
                "Quarantined",
                "/party",
                "lobby"
            ))
            .await
            .is_empty());
        assert!(host
            .eval_tab_complete(PlayerTabCompleteEvent::new(
                "Quarantined",
                "/maintenance",
                "lobby"
            ))
            .await
            .is_empty());
        assert!(host
            .eval_tab_complete(PlayerTabCompleteEvent::new(
                "Quarantined",
                "/report",
                "lobby"
            ))
            .await
            .is_empty());
        assert!(host
            .eval_tab_complete(PlayerTabCompleteEvent::new(
                "Quarantined",
                "/tablist",
                "lobby"
            ))
            .await
            .is_empty());
        // But should suggest /register and /login
        let tc_reg = host
            .eval_tab_complete(PlayerTabCompleteEvent::new("Quarantined", "/reg", "lobby"))
            .await;
        assert_eq!(tc_reg, vec!["/register"]);
        let tc_log = host
            .eval_tab_complete(PlayerTabCompleteEvent::new("Quarantined", "/log", "lobby"))
            .await;
        assert_eq!(tc_log, vec!["/login"]);

        // Authenticate player for subsequent regular-user tests
        let reg_cmd = PlayerCommandEvent::with_server("RegularUser", "/register pass123", "lobby");
        let _ = host.eval_command(reg_cmd).await;
        let log_cmd = PlayerCommandEvent::with_server("RegularUser", "/login pass123", "lobby");
        let _ = host.eval_command(log_cmd).await;

        // 2. Admin / Staff Authorization Guards
        let admin_only_cmds = vec![
            "/ban TargetCheater Cheating",
            "/kick TargetCheater Go away",
            "/mute TargetCheater Spamming",
            "/maintenance on",
            "/alert Network broadcast message",
            "/reports list",
            "/portal link survival paper",
            "/antibot banip 1.2.3.4 Botnet attack",
            "/sc Secret staff chat",
            "/cmdspy toggle",
            "/auth unregister Admin",
            "/security minprotocol 760",
            "/motd set New Motd Message",
            "/chatfilter add badword",
        ];
        for cmd_str in admin_only_cmds {
            let cmd = PlayerCommandEvent::with_server("RegularUser", cmd_str, "lobby");
            let res = host.eval_command(cmd).await;
            assert!(
                res.cancel,
                "Admin command '{}' should be cancelled for RegularUser",
                cmd_str
            );
            assert!(
                res.send_message.to_lowercase().contains("permission")
                    || res.send_message.to_lowercase().contains("staff")
                    || res.send_message.to_lowercase().contains("admin"),
                "Admin command '{}' should return permission error: got '{}'",
                cmd_str,
                res.send_message
            );
        }

        // 3. Brigadier Tab Completion Format (no duplicate root prefix!)
        let tc_party = PlayerTabCompleteEvent::new("Admin", "/party ", "lobby");
        let party_sugg = host.eval_tab_complete(tc_party).await;
        assert!(!party_sugg.is_empty());
        for s in &party_sugg {
            assert!(
                !s.starts_with("/party "),
                "Tab complete suggestion must not repeat root command: got '{}'",
                s
            );
        }
        assert!(party_sugg.contains(&"create".to_string()));
        assert!(party_sugg.contains(&"kick".to_string()));

        let tc_maint = PlayerTabCompleteEvent::new("Admin", "/maintenance ", "lobby");
        let maint_sugg = host.eval_tab_complete(tc_maint).await;
        assert!(!maint_sugg.is_empty());
        for s in &maint_sugg {
            assert!(
                !s.starts_with("/maintenance "),
                "Tab complete suggestion must not repeat root command: got '{}'",
                s
            );
        }
        assert!(maint_sugg.contains(&"on".to_string()));
        assert!(maint_sugg.contains(&"off".to_string()));

        let tc_tab = PlayerTabCompleteEvent::new("Admin", "/tablist ", "lobby");
        let tab_sugg = host.eval_tab_complete(tc_tab).await;
        assert!(!tab_sugg.is_empty());
        for s in &tab_sugg {
            assert!(
                !s.starts_with("/tablist "),
                "Tab complete suggestion must not repeat root command: got '{}'",
                s
            );
        }
        assert!(tab_sugg.contains(&"setheader".to_string()));

        let tc_rep = PlayerTabCompleteEvent::new("Admin", "/reports ", "lobby");
        let rep_sugg = host.eval_tab_complete(tc_rep).await;
        assert!(!rep_sugg.is_empty());
        for s in &rep_sugg {
            assert!(
                !s.starts_with("/reports "),
                "Tab complete suggestion must not repeat root command: got '{}'",
                s
            );
        }
        assert!(rep_sugg.contains(&"list".to_string()));

        // 4. Robustness against malformed numbers (no Rhai runtime crashes)
        let malformed_cmds = vec![
            "/tempban BadGuy abc Spam",
            "/motd countdown Event notanumber",
            "/security minprotocol notanumber",
        ];
        for cmd_str in malformed_cmds {
            let cmd = PlayerCommandEvent::with_server("Admin", cmd_str, "lobby");
            let res = host.eval_command(cmd).await;
            assert!(res.cancel);
            assert!(!res.send_message.is_empty());
        }

        // 5. No-Argument Command Handling (graceful usage, not passed to backend)
        let no_arg_cmds = vec![
            "/ban",
            "/kick",
            "/mute",
            "/portal link",
            "/reports view",
            "/reports claim",
            "/reports close",
            "/changepin",
            "/auth unregister",
            "/report",
            "/autoreconnect",
        ];
        for cmd_str in no_arg_cmds {
            let cmd = PlayerCommandEvent::with_server("Admin", cmd_str, "lobby");
            let res = host.eval_command(cmd).await;
            assert!(
                res.cancel,
                "No-arg command '{}' should be handled (cancel=true)",
                cmd_str
            );
            assert!(
                !res.send_message.is_empty(),
                "No-arg command '{}' should provide guidance",
                cmd_str
            );
        }

        // 6. Party Kick lifecycle
        let p_create = PlayerCommandEvent::with_server("PartyLeader", "/party create", "lobby");
        let _ = host.eval_command(p_create).await;

        let p_invite =
            PlayerCommandEvent::with_server("PartyLeader", "/party invite PartyMember", "lobby");
        let _ = host.eval_command(p_invite).await;

        let p_accept = PlayerCommandEvent::with_server("PartyMember", "/party accept", "lobby");
        let _ = host.eval_command(p_accept).await;

        let p_list1 = PlayerCommandEvent::with_server("PartyLeader", "/party list", "lobby");
        let list1_res = host.eval_command(p_list1).await;
        assert!(
            list1_res.send_message.contains("PartyMember"),
            "PartyMember must be in party list"
        );

        // Kick PartyMember
        let p_kick =
            PlayerCommandEvent::with_server("PartyLeader", "/party kick PartyMember", "lobby");
        let kick_res = host.eval_command(p_kick).await;
        assert!(kick_res.cancel);
        assert!(kick_res.send_message.contains("kicked"));

        let p_list2 = PlayerCommandEvent::with_server("PartyLeader", "/party list", "lobby");
        let list2_res = host.eval_command(p_list2).await;
        assert!(
            !list2_res.send_message.contains("PartyMember"),
            "PartyMember must be removed after kick"
        );
    }
}
