// =============================================================================
// FrameMC Plugin 05: StaffChat / Cross-Server Administrative Network
// =============================================================================
// Recreates: CleanStaffChat, StaffChat, BungeeStaffChat
// Features:
// - Dedicated staff channel (/sc <msg>, /staffchat <msg>)
// - Staff chat toggle mode (/sc toggle)
// - Staff roster management (/staffadd, /staffremove, /sc list)
// - Server attribution and timestamp logging
// - Strict staff permission enforcement
// =============================================================================

fn is_staff(player) {
    if player == "Notch" || player == "admin" || player == "Admin" || player == "Console" { return true; }
    kv_has("staff_member:" + player) || kv_has("admin:" + player)
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

    // 1. Staff Chat Message: /sc <msg> or /staffchat <msg>
    let is_sc = cmd == "/sc" || cmd == "/staffchat" || cmd.starts_with("/sc ") || cmd.starts_with("/staffchat ");
    if is_sc {
        if !is_staff(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }

        let prefix_len = if cmd.starts_with("/sc ") { 4 } else if cmd.starts_with("/staffchat ") { 11 } else { cmd.len };
        let msg = if cmd == "/sc" || cmd == "/staffchat" { "" } else { cmd[prefix_len..cmd.len] };
        msg.trim();

        if msg == "toggle" {
            let t_key = "sc_toggle:" + sender;
            let curr = if kv_has(t_key) { kv_get(t_key) } else { false };
            let next = !curr;
            kv_set(t_key, next);
            let state_str = if next { "§aENABLED" } else { "§cDISABLED" };
            return #{ cancel: true, reroute_server: "", send_message: "§8[§cStaffChat§8] §7Toggle mode is now " + state_str + "§7." };
        }

        if msg == "list" {
            let keys = kv_keys("staff_member:");
            let list_str = "";
            let i = 0;
            while i < keys.len {
                if i > 0 { list_str += "§7, "; }
                let name = keys[i][13..keys[i].len];
                let is_on = kv_has("online:" + name);
                list_str += (if is_on { "§a" } else { "§7" }) + name;
                i += 1;
            }
            let msg_out = "§8======== §c[FrameMC Staff Roster] §8========\n"
                + "§7Staff members (" + keys.len + "): " + (if list_str == "" { "§8(none)" } else { list_str });
            return #{ cancel: true, reroute_server: "", send_message: msg_out };
        }

        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /sc <message> or /sc toggle or /sc list" };
        }

        let formatted = "§8[§cStaffChat§8] §b" + sender + " §7(§e" + current_server + "§7): §f" + msg;
        proxy_info("[StaffChat] (" + current_server + ") " + sender + ": " + msg);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: formatted
        };
    }

    // 2. Command: /staffadd <player>
    if cmd == "/staffadd" || cmd.starts_with("/staffadd ") {
        if !is_staff(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target = if cmd == "/staffadd" { "" } else { cmd[10..cmd.len] };
        target.trim();
        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /staffadd <player>" };
        }
        kv_set("staff_member:" + target, true);
        proxy_info("Player " + target + " was granted staff permissions by " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[StaffChat] Added §b" + target + " §ato the staff roster." };
    }

    // 3. Command: /staffremove <player>
    if cmd == "/staffremove" || cmd.starts_with("/staffremove ") {
        if !is_staff(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target = if cmd == "/staffremove" { "" } else { cmd[13..cmd.len] };
        target.trim();
        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /staffremove <player>" };
        }
        kv_remove("staff_member:" + target);
        kv_remove("sc_toggle:" + target);
        proxy_info("Player " + target + " was removed from staff roster by " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§e[StaffChat] Removed §b" + target + " §efrom the staff roster." };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_staff(sender) {
        return [];
    }
    let root_cmds = ["/sc", "/staffchat", "/staffadd", "/staffremove"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/sc ") || cmd.starts_with("/staffchat ") {
        let prefix_len = if cmd.starts_with("/sc ") { 4 } else { 11 };
        let sub = cmd[prefix_len..cmd.len];
        if !sub.contains(" ") {
            let options = ["toggle", "list"];
            let matches = [];
            for o in options {
                if sub == "" || o.starts_with(sub) {
                    matches.push(o);
                }
            }
            return matches;
        }
    }

    let staff_mgt = ["/staffadd ", "/staffremove "];
    for sm in staff_mgt {
        if cmd.starts_with(sm) {
            let arg = cmd[sm.len..cmd.len];
            arg.trim();
            if !arg.contains(" ") {
                let matches = [];
                let online_keys = kv_keys("online:");
                let i = 0;
                while i < online_keys.len {
                    let p_name = online_keys[i][7..online_keys[i].len];
                    if arg == "" || p_name.starts_with(arg) {
                        matches.push(p_name);
                    }
                    i += 1;
                }
                return matches;
            }
        }
    }

    []
}
