// =============================================================================
// FrameMC Plugin 13: BungeeFind / Cross-Server Player Locator
// =============================================================================
// Recreates: BungeeFind, WhereAmI, PlayerFinder
// Features:
// - Cross-server player locator (/find <player>, /whereis <player>)
// - Own connection diagnostic summary (/whereami)
// - Server roster inspection (/serverplayers <server>)
// - Automatic location tracking
// =============================================================================

fn is_quarantined(player) {
    kv_has("auth_session:" + player) && kv_get("auth_session:" + player) == false
}

fn on_player_join(event) {
    let name = event.player_name;
    kv_set("player_loc:" + name, get_default_server());
    kv_set("player_ip:" + name, event.ip);
    kv_set("player_proto:" + name, event.protocol_version);
    #{ allow: true, disconnect_reason: "", target_server: "" }
}

fn on_player_command(event) {
    let cmd = event.command;
    let sender = event.player_name;
    let current_server = event.current_server;

    if is_quarantined(sender) {
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§c§l[ProxyAuth] Please authenticate first with /login <pin> or /register <pin>."
        };
    }

    // Refresh sender's location
    kv_set("player_loc:" + sender, current_server);

    // 1. Command: /find <player> or /whereis <player>
    let is_find = cmd == "/find" || cmd == "/whereis" || cmd.starts_with("/find ") || cmd.starts_with("/whereis ");
    if is_find {
        let raw = if cmd == "/find" || cmd == "/whereis" {
            ""
        } else if cmd.starts_with("/find ") {
            cmd[6..cmd.len]
        } else {
            cmd[9..cmd.len]
        };
        raw.trim();

        if raw == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /find <player>" };
        }

        let loc_key = "player_loc:" + raw;
        if !kv_has(loc_key) && !kv_has("online:" + raw) {
            return #{ cancel: true, reroute_server: "", send_message: "§c[Find] Player '§f" + raw + "§c' is not currently online." };
        }

        let srv = if kv_has(loc_key) { kv_get(loc_key) } else { kv_get("online:" + raw) };
        let msg = "§6[Find] §f" + raw + " §7is currently online on server: §a" + srv;
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    // 2. Command: /whereami
    if cmd == "/whereami" {
        let proto = if kv_has("player_proto:" + sender) { kv_get("player_proto:" + sender) } else { 765 };
        let msg = "§8======== §6[FrameMC Connection Info] §8========\n"
            + "§7- Player Name: §f" + sender + "\n"
            + "§7- Connected Server: §a" + current_server + "\n"
            + "§7- Protocol Version: §e" + proto + "\n"
            + "§7- Proxy Architecture: §bNative Rust (Zero-GC)\n"
            + "§7- Socket State: §aBridged & Active";

        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    // 3. Command: /serverplayers <server>
    if cmd == "/serverplayers" || cmd.starts_with("/serverplayers ") {
        let target_srv = if cmd == "/serverplayers" { "" } else { cmd[15..cmd.len] };
        target_srv.trim();

        if target_srv == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /serverplayers <server>" };
        }

        if !server_exists(target_srv) {
            return #{ cancel: true, reroute_server: "", send_message: "§cServer '" + target_srv + "' does not exist." };
        }

        let keys = kv_keys("player_loc:");
        let found = [];
        let i = 0;
        while i < keys.len {
            if kv_get(keys[i]) == target_srv {
                found.push(keys[i][11..keys[i].len]);
            }
            i += 1;
        }

        let list_str = "";
        let j = 0;
        while j < found.len {
            if j > 0 { list_str += "§7, "; }
            list_str += "§b" + found[j];
            j += 1;
        }

        let msg = "§6[Server Players: " + target_srv + " (" + found.len + ")]:\n"
            + (if found.len == 0 { "§8(No players on this server)" } else { list_str });

        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) {
        return [];
    }
    let root_cmds = ["/find", "/whereis", "/whereami", "/serverplayers"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/find ") || cmd.starts_with("/whereis ") {
        let prefix_len = if cmd.starts_with("/find ") { 6 } else { 9 };
        let target = cmd[prefix_len..cmd.len];
        target.trim();
        let matches = [];
        let online_keys = kv_keys("online:");
        let i = 0;
        while i < online_keys.len {
            let p_name = online_keys[i][7..online_keys[i].len];
            if target == "" || p_name.starts_with(target) {
                matches.push(p_name);
            }
            i += 1;
        }
        return matches;
    }

    if cmd.starts_with("/serverplayers ") {
        let target_srv = cmd[15..cmd.len];
        target_srv.trim();
        let servers = get_servers();
        let matches = [];
        for s in servers {
            if target_srv == "" || s.starts_with(target_srv) {
                matches.push(s);
            }
        }
        return matches;
    }

    []
}
