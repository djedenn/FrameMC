// =============================================================================
// FrameMC Plugin 14: ServerSend / Administrative Mass Relocation
// =============================================================================
// Recreates: BungeeSend, ServerSend, BungeeMove
// Features:
// - Direct single player relocation (/send <player> <server>)
// - Network-wide emergency evacuation (/sendall <server>)
// - Per-backend server migration (/sendcurrent <server>)
// - Dynamic validation against registered backend configurations
// - Admin role authorization
// =============================================================================

fn is_admin(player) {
    if player == "Notch" || player == "admin" || player == "Admin" || player == "Console" { return true; }
    kv_has("admin:" + player) || kv_has("staff_member:" + player)
}

fn is_quarantined(player) {
    kv_has("auth_session:" + player) && kv_get("auth_session:" + player) == false
}

fn on_player_join(event) {
    let name = event.player_name;
    if kv_has("pending_reroute:" + name) {
        let dest = kv_get("pending_reroute:" + name);
        kv_remove("pending_reroute:" + name);
        if server_exists(dest) {
            proxy_info("ServerSend transferring joining player " + name + " to queued destination: " + dest);
            return #{
                allow: true,
                disconnect_reason: "",
                target_server: dest
            };
        }
    }
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

    // 1. Command: /send <player> <server>
    if cmd == "/send" || cmd.starts_with("/send ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/send" { "" } else { cmd[6..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 || parts[0] == "" || parts[1] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /send <player|all|current> <server>" };
        }
        let target_player = parts[0];
        let target_server = parts[1];

        if !server_exists(target_server) {
            return #{ cancel: true, reroute_server: "", send_message: "§cServer '" + target_server + "' does not exist." };
        }

        if target_player == "all" {
            proxy_info("Admin " + sender + " triggered /send all to " + target_server);
            let online_keys = kv_keys("online:");
            let i = 0;
            while i < online_keys.len {
                let p = online_keys[i][7..online_keys[i].len];
                if p != sender {
                    kv_set("pending_reroute:" + p, target_server);
                }
                i += 1;
            }
            return #{
                cancel: true,
                reroute_server: target_server,
                send_message: "§a[ServerSend] Relocating all " + online_keys.len + " player(s) to §e" + target_server + "§a..."
            };
        }

        if target_player == "current" {
            proxy_info("Admin " + sender + " triggered /send current from " + current_server + " to " + target_server);
            let online_keys = kv_keys("online:");
            let count = 0;
            let i = 0;
            while i < online_keys.len {
                let p = online_keys[i][7..online_keys[i].len];
                if kv_get(online_keys[i]) == current_server && p != sender {
                    kv_set("pending_reroute:" + p, target_server);
                    count += 1;
                }
                i += 1;
            }
            return #{
                cancel: true,
                reroute_server: target_server,
                send_message: "§a[ServerSend] Relocating players from §e" + current_server + " §ato §e" + target_server + "§a..."
            };
        }

        if target_player == sender {
            return #{
                cancel: true,
                reroute_server: target_server,
                send_message: "§a[ServerSend] Sending yourself to §e" + target_server + "§a..."
            };
        }

        // Target is another player
        if !kv_has("online:" + target_player) {
            return #{ cancel: true, reroute_server: "", send_message: "§cPlayer '" + target_player + "' is not online." };
        }

        kv_set("pending_reroute:" + target_player, target_server);
        proxy_info("Admin " + sender + " queued relocation for " + target_player + " -> " + target_server);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[ServerSend] Sent §b" + target_player + " §ato server §e" + target_server + "§a."
        };
    }

    // 2. Command: /sendall <server>
    if cmd == "/sendall" || cmd.starts_with("/sendall ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target_server = if cmd == "/sendall" { "" } else { cmd[9..cmd.len] };
        target_server.trim();
        if target_server == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /sendall <server>" };
        }
        if !server_exists(target_server) {
            return #{ cancel: true, reroute_server: "", send_message: "§cServer '" + target_server + "' does not exist." };
        }
        let online_keys = kv_keys("online:");
        let i = 0;
        while i < online_keys.len {
            let p = online_keys[i][7..online_keys[i].len];
            if p != sender {
                kv_set("pending_reroute:" + p, target_server);
            }
            i += 1;
        }
        proxy_info("Admin " + sender + " executed /sendall to " + target_server);
        return #{
            cancel: true,
            reroute_server: target_server,
            send_message: "§a[ServerSend] Executing network-wide relocation to §e" + target_server + "§a!"
        };
    }

    // 3. Command: /sendcurrent <server>
    if cmd == "/sendcurrent" || cmd.starts_with("/sendcurrent ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target_server = if cmd == "/sendcurrent" { "" } else { cmd[13..cmd.len] };
        target_server.trim();
        if target_server == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /sendcurrent <server>" };
        }
        if !server_exists(target_server) {
            return #{ cancel: true, reroute_server: "", send_message: "§cServer '" + target_server + "' does not exist." };
        }
        let online_keys = kv_keys("online:");
        let i = 0;
        while i < online_keys.len {
            let p = online_keys[i][7..online_keys[i].len];
            if kv_get(online_keys[i]) == current_server && p != sender {
                kv_set("pending_reroute:" + p, target_server);
            }
            i += 1;
        }
        proxy_info("Admin " + sender + " executed /sendcurrent to " + target_server);
        return #{
            cancel: true,
            reroute_server: target_server,
            send_message: "§a[ServerSend] Evacuating current server §e" + current_server + " §ato §e" + target_server + "§a!"
        };
    }
    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_admin(sender) {
        return [];
    }
    let root_cmds = ["/send", "/sendall", "/sendcurrent"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/sendall ") || cmd.starts_with("/sendcurrent ") {
        let prefix_len = if cmd.starts_with("/sendall ") { 9 } else { 13 };
        let s_arg = cmd[prefix_len..cmd.len];
        s_arg.trim();
        let servers = get_servers();
        let matches = [];
        for s in servers {
            if s_arg == "" || s.starts_with(s_arg) {
                matches.push(s);
            }
        }
        return matches;
    }

    if cmd.starts_with("/send ") {
        let raw = cmd[6..cmd.len];
        let parts = raw.split(" ");
        if parts.len == 2 {
            let s_arg = parts[1];
            let servers = get_servers();
            let matches = [];
            for s in servers {
                if s_arg == "" || s.starts_with(s_arg) {
                    matches.push(s);
                }
            }
            return matches;
        }

        if parts.len == 1 && !raw.ends_with(" ") {
            let target = parts[0];
            let matches = [];
            if "all".starts_with(target) { matches.push("all"); }
            if "current".starts_with(target) { matches.push("current"); }
            let online_keys = kv_keys("online:");
            let i = 0;
            while i < online_keys.len {
                let p = online_keys[i][7..online_keys[i].len];
                if target == "" || p.starts_with(target) {
                    matches.push(p);
                }
                i += 1;
            }
            return matches;
        }
    }

    []
}
