// =============================================================================
// FrameMC Plugin 06: ServerPortals / Gateway Warps & Gateways
// =============================================================================
// Recreates: BungeePortals, BungeeSuitePortals, ServerPortals
// Features:
// - Virtual portal triggers & links (/portal <name>, /portal list, /portal link, /portal unlink)
// - Game mode shortcuts (/survival, /creative, /skyblock, /bedwars, /pvp, /prison)
// - Auto-discovery and safe transfer fallback
// - Admin authorization for portal linking
// =============================================================================

fn is_admin(player) {
    if player == "Notch" || player == "admin" || player == "Admin" || player == "Console" { return true; }
    kv_has("admin:" + player) || kv_has("staff_member:" + player)
}

fn is_quarantined(player) {
    kv_has("auth_session:" + player) && kv_get("auth_session:" + player) == false
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

    // 1. Command: /portal ...
    if cmd == "/portal" || cmd == "/portal help" {
        let help = "§8======== §6[FrameMC Portals] §8========\n"
            + "§e/portal <name> §7- Enter a virtual portal to server\n"
            + "§e/portal list §7- List active portal links\n"
            + "§e/portal link <name> <server> §7- Link portal name to backend (Admin)\n"
            + "§e/portal unlink <name> §7- Remove a portal link (Admin)\n"
            + "§e/portal info <name> §7- Show portal details";
        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    if cmd == "/portal list" {
        let keys = kv_keys("portal:");
        if keys.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§7[Portals] No custom portals configured. Use §e/portal link <name> <server>§7." };
        }
        let msg = "§6[Portals] Configured Portals (" + keys.len + "):\n";
        let i = 0;
        while i < keys.len {
            let p_name = keys[i][7..keys[i].len];
            let dest = kv_get(keys[i]);
            msg += "§7- §b" + p_name + " §7-> §a" + dest + "\n";
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/portal link" || cmd.starts_with("/portal link ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/portal link" { "" } else { cmd[13..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 || parts[0] == "" || parts[1] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /portal link <name> <server>" };
        }
        let p_name = parts[0];
        let target_srv = parts[1];

        if !server_exists(target_srv) {
            return #{ cancel: true, reroute_server: "", send_message: "§cTarget server '" + target_srv + "' does not exist." };
        }

        kv_set("portal:" + p_name, target_srv);
        proxy_info("Portal '" + p_name + "' linked to server '" + target_srv + "' by " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[Portals] Portal '§b" + p_name + "§a' successfully linked to server '§e" + target_srv + "§a'." };
    }

    if cmd == "/portal unlink" || cmd.starts_with("/portal unlink ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let p_name = if cmd == "/portal unlink" { "" } else { cmd[15..cmd.len] };
        p_name.trim();
        if p_name == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /portal unlink <name>" };
        }
        if !kv_has("portal:" + p_name) {
            return #{ cancel: true, reroute_server: "", send_message: "§cPortal '" + p_name + "' does not exist." };
        }
        kv_remove("portal:" + p_name);
        return #{ cancel: true, reroute_server: "", send_message: "§e[Portals] Portal '§b" + p_name + "§e' unlinked." };
    }

    if cmd == "/portal info" || cmd.starts_with("/portal info ") {
        let p_name = if cmd == "/portal info" { "" } else { cmd[13..cmd.len] };
        p_name.trim();
        if p_name == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /portal info <name>" };
        }
        if !kv_has("portal:" + p_name) {
            return #{ cancel: true, reroute_server: "", send_message: "§cPortal '" + p_name + "' does not exist." };
        }
        let dest = kv_get("portal:" + p_name);
        return #{ cancel: true, reroute_server: "", send_message: "§6[Portals] Portal '§b" + p_name + "§6' leads to backend: §a" + dest };
    }

    // Portal entry: /portal <name>
    if cmd.starts_with("/portal ") {
        let p_name = cmd[8..cmd.len];
        p_name.trim();
        if kv_has("portal:" + p_name) {
            let dest = kv_get("portal:" + p_name);
            if current_server == dest {
                return #{ cancel: true, reroute_server: "", send_message: "§cYou are already on server '" + dest + "'!" };
            }
            return #{
                cancel: true,
                reroute_server: dest,
                send_message: "§a[Portals] Warping through portal '§b" + p_name + "§a' to §e" + dest + "§a..."
            };
        } else if server_exists(p_name) {
            if current_server == p_name {
                return #{ cancel: true, reroute_server: "", send_message: "§cYou are already on server '" + p_name + "'!" };
            }
            return #{
                cancel: true,
                reroute_server: p_name,
                send_message: "§a[Portals] Connecting to server §e" + p_name + "§a..."
            };
        } else {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cPortal '" + p_name + "' does not exist. Use /portal list to see available portals."
            };
        }
    }

    // 2. Game Mode Shortcuts: /survival, /creative, /skyblock, /bedwars, /pvp, /prison
    let shortcuts = ["survival", "creative", "skyblock", "bedwars", "pvp", "prison"];
    for sc in shortcuts {
        if cmd == "/" + sc {
            let target = "";
            if kv_has("portal:" + sc) {
                target = kv_get("portal:" + sc);
            } else if server_exists(sc) {
                target = sc;
            }

            if target != "" {
                if current_server == target {
                    return #{ cancel: true, reroute_server: "", send_message: "§cYou are already connected to " + target + "!" };
                }
                return #{
                    cancel: true,
                    reroute_server: target,
                    send_message: "§a[Portals] Teleporting to §e" + target + "§a..."
                };
            }
        }
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) {
        return [];
    }
    let root_cmds = ["/portal", "/survival", "/creative", "/skyblock", "/bedwars", "/pvp", "/prison"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/portal link ") {
        let raw = cmd[13..cmd.len];
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
    }

    if cmd.starts_with("/portal unlink ") || cmd.starts_with("/portal info ") {
        let p_arg = if cmd.starts_with("/portal unlink ") { cmd[15..cmd.len] } else { cmd[13..cmd.len] };
        p_arg.trim();
        let keys = kv_keys("portal:");
        let matches = [];
        let i = 0;
        while i < keys.len {
            let name = keys[i][7..keys[i].len];
            if p_arg == "" || name.starts_with(p_arg) {
                matches.push(name);
            }
            i += 1;
        }
        return matches;
    }

    if cmd.starts_with("/portal ") {
        let sub = cmd[8..cmd.len];
        if !sub.contains(" ") {
            let options = ["list", "link", "unlink", "info"];
            let keys = kv_keys("portal:");
            let i = 0;
            while i < keys.len {
                options.push(keys[i][7..keys[i].len]);
                i += 1;
            }
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
