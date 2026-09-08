// =============================================================================
// FrameMC Plugin 07: Maintenance Mode & Network Lockdown
// =============================================================================
// Recreates: Maintenance, CleanMOTD, BungeeMaintenance
// Features:
// - Network-wide maintenance toggle (/maintenance on, /maintenance off)
// - Whitelist management (/maintenance add, /maintenance remove, /maintenance list)
// - Dynamic kick screen with custom reason (/maintenance setmsg)
// - Protocol-level rejection during login phase
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
    let maint_on = if kv_has("maintenance:enabled") { kv_get("maintenance:enabled") } else { false };

    if maint_on {
        let is_whitelisted = kv_has("maint_wl:" + name) || is_admin(name);
        if !is_whitelisted {
            let reason = if kv_has("maintenance:reason") { kv_get("maintenance:reason") } else { "Scheduled network maintenance & upgrades." };
            let screen = "§c§l● FRAMEMC MAINTENANCE MODE ●\n\n"
                + "§eThe server network is currently offline for maintenance.\n"
                + "§7Reason: §f" + reason + "\n\n"
                + "§6Only whitelisted staff can join at this time.\n"
                + "§7Updates: §bhttps://discord.gg/yourserver";

            return #{
                allow: false,
                disconnect_reason: screen,
                target_server: ""
            };
        }
    }

    #{ allow: true, disconnect_reason: "", target_server: "" }
}

fn on_player_command(event) {
    let cmd = event.command;
    let sender = event.player_name;

    if is_quarantined(sender) {
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§c§l[ProxyAuth] Please authenticate first with /login <pin> or /register <pin>."
        };
    }

    if cmd == "/maintenance" || cmd == "/maintenance help" {
        let help = "§8======== §c[FrameMC Maintenance] §8========\n"
            + "§e/maintenance on §7- Enable maintenance mode (Admin)\n"
            + "§e/maintenance off §7- Disable maintenance mode (Admin)\n"
            + "§e/maintenance status §7- View status and whitelist\n"
            + "§e/maintenance add <player> §7- Whitelist a player (Admin)\n"
            + "§e/maintenance remove <player> §7- Remove from whitelist (Admin)\n"
            + "§e/maintenance list §7- List whitelisted players\n"
            + "§e/maintenance setmsg <reason> §7- Set maintenance message (Admin)";
        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    if cmd == "/maintenance on" {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        kv_set("maintenance:enabled", true);
        proxy_info("Maintenance mode was ENABLED by " + sender);
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[Maintenance] §cMaintenance mode has been §lACTIVATED§c. §7Non-whitelisted players can no longer join."
        };
    }

    if cmd == "/maintenance off" {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        kv_set("maintenance:enabled", false);
        proxy_info("Maintenance mode was DISABLED by " + sender);
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[Maintenance] §aMaintenance mode has been §lDEACTIVATED§a. §7All players may now connect."
        };
    }

    if cmd == "/maintenance status" {
        let is_on = if kv_has("maintenance:enabled") { kv_get("maintenance:enabled") } else { false };
        let reason = if kv_has("maintenance:reason") { kv_get("maintenance:reason") } else { "Scheduled network maintenance & upgrades." };
        let wl_keys = kv_keys("maint_wl:");
        let state_str = if is_on { "§c● ENABLED" } else { "§a● DISABLED" };

        let msg = "§6[Maintenance Status]\n"
            + "§7- Mode: " + state_str + "\n"
            + "§7- Reason: §f" + reason + "\n"
            + "§7- Whitelisted Players: §e" + wl_keys.len;

        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/maintenance add" || cmd.starts_with("/maintenance add ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target = if cmd == "/maintenance add" { "" } else { cmd[17..cmd.len] };
        target.trim();
        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /maintenance add <player>" };
        }
        kv_set("maint_wl:" + target, true);
        return #{ cancel: true, reroute_server: "", send_message: "§a[Maintenance] Added §e" + target + " §ato the maintenance whitelist." };
    }

    if cmd == "/maintenance remove" || cmd.starts_with("/maintenance remove ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target = if cmd == "/maintenance remove" { "" } else { cmd[20..cmd.len] };
        target.trim();
        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /maintenance remove <player>" };
        }
        kv_remove("maint_wl:" + target);
        return #{ cancel: true, reroute_server: "", send_message: "§e[Maintenance] Removed §f" + target + " §efrom the maintenance whitelist." };
    }

    if cmd == "/maintenance list" {
        let keys = kv_keys("maint_wl:");
        if keys.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§7[Maintenance] No players in maintenance whitelist." };
        }
        let list_str = "";
        let i = 0;
        while i < keys.len {
            if i > 0 { list_str += "§7, "; }
            list_str += "§e" + keys[i][9..keys[i].len];
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: "§6[Maintenance] Whitelisted (" + keys.len + "): " + list_str };
    }

    if cmd == "/maintenance setmsg" || cmd.starts_with("/maintenance setmsg ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let reason = if cmd == "/maintenance setmsg" { "" } else { cmd[20..cmd.len] };
        reason.trim();
        if reason == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /maintenance setmsg <reason>" };
        }
        kv_set("maintenance:reason", reason);
        return #{ cancel: true, reroute_server: "", send_message: "§a[Maintenance] Updated maintenance reason to: §f" + reason };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_admin(sender) {
        return [];
    }
    let root_cmds = ["/maintenance"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/maintenance ") {
        let sub = cmd[13..cmd.len];
        if !sub.contains(" ") {
            let options = ["on", "off", "status", "add", "remove", "list", "setmsg"];
            let matches = [];
            for o in options {
                if sub == "" || o.starts_with(sub) {
                    matches.push(o);
                }
            }
            return matches;
        }
    }

    if cmd.starts_with("/maintenance add ") || cmd.starts_with("/maintenance remove ") {
        let prefix_len = if cmd.starts_with("/maintenance add ") { 17 } else { 20 };
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

    []
}
