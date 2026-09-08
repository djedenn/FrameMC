// =============================================================================
// FrameMC Plugin 20: AutoReconnect & Smart Fallback Router
// =============================================================================
// Recreates: AutoReconnect, BungeeFallback, HubMagic
// Features:
// - Automatic session memory of player's last active server
// - Direct instant reconnect command (/reconnect)
// - Server inspection query (/lastserver)
// - Fallback hierarchy navigation (/fallback)
// - Auto-reconnect preference toggle on join (/autoreconnect, /autoreconnect toggle)
// =============================================================================

fn is_quarantined(player) {
    if kv_has("auth_session:" + player) {
        return !kv_get("auth_session:" + player);
    }
    false
}

fn on_player_join(event) {
    let name = event.player_name;
    let auto_on = if kv_has("autoreconn_enabled:" + name) { kv_get("autoreconn_enabled:" + name) } else { false };

    if auto_on && kv_has("last_server:" + name) {
        let last = kv_get("last_server:" + name);
        if server_exists(last) {
            proxy_info("AutoReconnect routing " + name + " back to last server: " + last);
            return #{
                allow: true,
                disconnect_reason: "",
                target_server: last
            };
        }
    }

    #{ allow: true, disconnect_reason: "", target_server: "" }
}

fn on_player_command(event) {
    let cmd = event.command;
    let sender = event.player_name;
    let current_server = event.current_server;

    if is_quarantined(sender) && (cmd == "/reconnect" || cmd == "/lastserver" || cmd == "/fallback" || cmd == "/autoreconnect" || cmd.starts_with("/autoreconnect ")) {
        return #{ cancel: true, reroute_server: "", send_message: "§c[ProxyAuth] You must authenticate before using server navigation commands." };
    }

    // Update last server memory whenever player executes a command
    kv_set("last_server:" + sender, current_server);

    // 1. Command: /reconnect
    if cmd == "/reconnect" {
        if !kv_has("last_server:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cNo previous server history found." };
        }
        let last = kv_get("last_server:" + sender);
        if current_server == last {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are already on your last server (" + last + ")!" };
        }
        if !server_exists(last) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYour previous server '" + last + "' is no longer available." };
        }

        return #{
            cancel: true,
            reroute_server: last,
            send_message: "§a[AutoReconnect] Reconnecting to previous server: §e" + last + "§a..."
        };
    }

    // 2. Command: /lastserver
    if cmd == "/lastserver" {
        let last = if kv_has("last_server:" + sender) { kv_get("last_server:" + sender) } else { current_server };
        return #{ cancel: true, reroute_server: "", send_message: "§6[AutoReconnect] Your last recorded server: §a" + last };
    }

    // 3. Command: /fallback
    if cmd == "/fallback" {
        let servers = get_servers();
        let def = get_default_server();
        let target = "";

        if def != "" && def != current_server && server_exists(def) {
            target = def;
        } else {
            for s in servers {
                if s != current_server && server_exists(s) {
                    target = s;
                    break;
                }
            }
        }

        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cNo alternative fallback server available." };
        }

        return #{
            cancel: true,
            reroute_server: target,
            send_message: "§e[Fallback] Safely transferring to fallback server: §a" + target + "§e..."
        };
    }

    // 4. Command: /autoreconnect
    if cmd == "/autoreconnect" {
        let key = "autoreconn_enabled:" + sender;
        let curr = if kv_has(key) { kv_get(key) } else { false };
        let state = if curr { "§aENABLED" } else { "§cDISABLED" };
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§6[AutoReconnect] Current status: " + state + "§6. Type §e/autoreconnect toggle §6to switch."
        };
    }

    if cmd == "/autoreconnect toggle" {
        let key = "autoreconn_enabled:" + sender;
        let curr = if kv_has(key) { kv_get(key) } else { false };
        let next = !curr;
        kv_set(key, next);
        let state = if next { "§aENABLED" } else { "§cDISABLED" };
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§6[AutoReconnect] Automatic reconnect on join is now " + state + "§6."
        };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;

    if is_quarantined(sender) {
        return [];
    }

    if cmd == "" || !cmd.contains(" ") {
        let list = ["/reconnect", "/lastserver", "/fallback", "/autoreconnect"];
        let matches = [];
        for item in list {
            if cmd == "" || item.starts_with(cmd) {
                matches.push(item);
            }
        }
        return matches;
    }

    if cmd.starts_with("/autoreconnect ") {
        let sub = cmd[15..cmd.len];
        if !sub.contains(" ") {
            if sub == "" || "toggle".starts_with(sub) {
                return ["toggle"];
            }
        }
    }

    []
}
