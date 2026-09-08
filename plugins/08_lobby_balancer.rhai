// =============================================================================
// FrameMC Plugin 08: LobbyBalancer / Dynamic Hub Load Balancer
// =============================================================================
// Recreates: LobbyBalancer, DynamicLobby, HubMagic
// Features:
// - Dynamic load balancing across multiple lobby instances (/hub, /lobby)
// - Configurable balancing algorithms (round-robin, least-loaded)
// - Initial connection distribution during on_player_join
// - Real-time lobby health tracking
// =============================================================================

fn is_quarantined(player) {
    kv_has("auth_session:" + player) && kv_get("auth_session:" + player) == false
}

fn get_available_lobbies() {
    let servers = get_servers();
    let def = get_default_server();
    let lobbies = [];
    let i = 0;
    while i < servers.len {
        let s = servers[i];
        if s.starts_with("lobby") || s.starts_with("hub") || s == def {
            if !lobbies.contains(s) {
                lobbies.push(s);
            }
        }
        i += 1;
    }
    if lobbies.len == 0 {
        lobbies.push(def);
    }
    lobbies
}

fn select_balanced_lobby(current) {
    let lobbies = get_available_lobbies();
    if lobbies.len == 1 { return lobbies[0]; }

    let strategy = if kv_has("balancer:strategy") { kv_get("balancer:strategy") } else { "roundrobin" };

    if strategy == "roundrobin" {
        let idx = if kv_has("balancer:rr_idx") { kv_get("balancer:rr_idx") } else { 0 };
        let candidate = lobbies[idx % lobbies.len];
        kv_set("balancer:rr_idx", (idx + 1) % lobbies.len);

        if candidate == current && lobbies.len > 1 {
            candidate = lobbies[(idx + 1) % lobbies.len];
            kv_set("balancer:rr_idx", (idx + 2) % lobbies.len);
        }
        return candidate;
    }

    if strategy == "least" {
        // Find lobby with lowest registered player count
        let best_lobby = lobbies[0];
        let min_count = 999999;
        let online_keys = kv_keys("online:");

        let l_idx = 0;
        while l_idx < lobbies.len {
            let l_name = lobbies[l_idx];
            let count = 0;
            let k_idx = 0;
            while k_idx < online_keys.len {
                if kv_get(online_keys[k_idx]) == l_name {
                    count += 1;
                }
                k_idx += 1;
            }
            if count < min_count {
                min_count = count;
                best_lobby = l_name;
            }
            l_idx += 1;
        }
        return best_lobby;
    }

    // Default: first lobby
    lobbies[0]
}

fn on_player_join(event) {
    let target = select_balanced_lobby("");
    proxy_info("LobbyBalancer routing new join " + event.player_name + " to " + target);
    #{
        allow: true,
        disconnect_reason: "",
        target_server: target
    }
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

    // 1. /hub and /lobby load balancing
    if cmd == "/hub" || cmd == "/lobby" {
        let target = select_balanced_lobby(current_server);
        if current_server == target {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cYou are already on balanced lobby: §e" + target + "§c!"
            };
        }

        return #{
            cancel: true,
            reroute_server: target,
            send_message: "§a[LobbyBalancer] Routing you to optimal lobby: §e" + target + "§a..."
        };
    }

    // 2. /balancer command
    if cmd == "/balancer" || cmd == "/balancer status" {
        let strategy = if kv_has("balancer:strategy") { kv_get("balancer:strategy") } else { "roundrobin" };
        let lobbies = get_available_lobbies();
        let lobby_str = "";
        let i = 0;
        while i < lobbies.len {
            if i > 0 { lobby_str += "§7, "; }
            lobby_str += "§a" + lobbies[i];
            i += 1;
        }

        let msg = "§8======== §6[LobbyBalancer Status] §8========\n"
            + "§7- Active Strategy: §e" + strategy + "\n"
            + "§7- Monitored Lobbies (" + lobbies.len + "): " + lobby_str + "\n"
            + "§7Use §f/balancer strategy <roundrobin|least> §7to switch.";
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/balancer strategy" || cmd.starts_with("/balancer strategy ") {
        let strat = if cmd == "/balancer strategy" { "" } else { cmd[19..cmd.len] };
        strat.trim();
        if strat != "roundrobin" && strat != "least" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /balancer strategy <roundrobin|least>" };
        }
        kv_set("balancer:strategy", strat);
        proxy_info("LobbyBalancer strategy changed to " + strat + " by " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[LobbyBalancer] Strategy changed to: §e" + strat };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) {
        return [];
    }
    let root_cmds = ["/hub", "/lobby", "/balancer"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd == "/balancer " {
        return ["status", "strategy"];
    }

    if cmd.starts_with("/balancer strategy ") {
        let arg = cmd[19..cmd.len];
        arg.trim();
        let options = ["roundrobin", "least"];
        let matches = [];
        for o in options {
            if arg == "" || o.starts_with(arg) {
                matches.push(o);
            }
        }
        return matches;
    }

    if cmd.starts_with("/balancer ") {
        let sub = cmd[10..cmd.len];
        if !sub.contains(" ") {
            let options = ["status", "strategy"];
            let matches = [];
            for o in options {
                if sub == "" || o.starts_with(sub) {
                    matches.push(o);
                }
            }
            return matches;
        }
    }

    []
}
