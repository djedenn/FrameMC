// =============================================================================
// FrameMC Plugin 12: CommandSpy / Administrative Command Audit Engine
// =============================================================================
// Recreates: BungeeCommandSpy, CommandWatcher, SocialSpy
// Features:
// - Real-time command interception & audit logging
// - Spy mode toggle for administrators (/cmdspy, /commandspy toggle)
// - Configurable ignore list for private/sensitive commands (/cmdspy ignore)
// - Active spy listener registry
// - Staff role authorization
// =============================================================================

fn is_staff(player) {
    if player == "Notch" || player == "admin" || player == "Admin" || player == "Console" { return true; }
    kv_has("staff_member:" + player) || kv_has("admin:" + player)
}

fn is_quarantined(player) {
    kv_has("auth_session:" + player) && kv_get("auth_session:" + player) == false
}

fn is_ignored_command(cmd) {
    let defaults = ["/login", "/register", "/changepin", "/auth", "/msg", "/tell", "/w", "/r"];
    for d in defaults {
        if cmd == d || cmd.starts_with(d + " ") {
            return true;
        }
    }
    let keys = kv_keys("cmdspy_ignore:");
    let i = 0;
    while i < keys.len {
        let ign = keys[i][14..keys[i].len];
        if cmd == ign || cmd.starts_with(ign + " ") {
            return true;
        }
        i += 1;
    }
    false
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

    // 1. Management: /cmdspy or /commandspy
    let is_spy_cmd = cmd == "/cmdspy" || cmd.starts_with("/cmdspy ") || cmd == "/commandspy" || cmd.starts_with("/commandspy ");
    if is_spy_cmd {
        if !is_staff(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }

        let raw = if cmd == "/cmdspy" || cmd == "/commandspy" {
            ""
        } else if cmd.starts_with("/cmdspy ") {
            cmd[8..cmd.len]
        } else {
            cmd[12..cmd.len]
        };
        raw.trim();

        if raw == "" || raw == "toggle" {
            let key = "cmdspy_active:" + sender;
            let curr = if kv_has(key) { kv_get(key) } else { false };
            let next = !curr;
            kv_set(key, next);
            let state = if next { "§aENABLED" } else { "§cDISABLED" };
            return #{ cancel: true, reroute_server: "", send_message: "§6[CmdSpy] §7Command spy notifications are now " + state + "§7." };
        }

        if raw == "list" {
            let keys = kv_keys("cmdspy_active:");
            let list_str = "";
            let i = 0;
            while i < keys.len {
                if kv_get(keys[i]) == true {
                    if list_str.len > 0 { list_str += "§7, "; }
                    list_str += "§b" + keys[i][14..keys[i].len];
                }
                i += 1;
            }
            return #{ cancel: true, reroute_server: "", send_message: "§6[CmdSpy] Active listeners: " + (if list_str == "" { "§8(none)" } else { list_str }) };
        }

        if raw == "ignore add" || raw.starts_with("ignore add ") {
            let ign = if raw == "ignore add" { "" } else { raw[11..raw.len] };
            ign.trim();
            if ign == "" {
                return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /cmdspy ignore add <cmd>" };
            }
            if !ign.starts_with("/") { ign = "/" + ign; }
            kv_set("cmdspy_ignore:" + ign, true);
            return #{ cancel: true, reroute_server: "", send_message: "§a[CmdSpy] Added '§e" + ign + "§a' to ignore filter." };
        }

        if raw == "ignore remove" || raw.starts_with("ignore remove ") {
            let ign = if raw == "ignore remove" { "" } else { raw[14..raw.len] };
            ign.trim();
            if ign == "" {
                return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /cmdspy ignore remove <cmd>" };
            }
            if !ign.starts_with("/") { ign = "/" + ign; }
            kv_remove("cmdspy_ignore:" + ign);
            return #{ cancel: true, reroute_server: "", send_message: "§e[CmdSpy] Removed '§f" + ign + "§e' from ignore filter." };
        }

        if raw == "ignore" || raw == "ignore list" {
            let keys = kv_keys("cmdspy_ignore:");
            let list_str = "";
            let i = 0;
            while i < keys.len {
                if i > 0 { list_str += "§7, "; }
                list_str += "§e" + keys[i][14..keys[i].len];
                i += 1;
            }
            return #{ cancel: true, reroute_server: "", send_message: "§6[CmdSpy] Ignored custom commands (" + keys.len + "): " + list_str };
        }

        let help = "§6[CmdSpy Usage]:\n§7- §e/cmdspy toggle\n§7- §e/cmdspy list\n§7- §e/cmdspy ignore add <cmd>\n§7- §e/cmdspy ignore remove <cmd>";
        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    // 2. Audit: Log non-ignored command to proxy tracing
    if !is_ignored_command(cmd) {
        proxy_info("[CmdSpy] [" + current_server + "] " + sender + ": " + cmd);
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_staff(sender) {
        return [];
    }
    let root_cmds = ["/cmdspy", "/commandspy"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/cmdspy ignore ") || cmd.starts_with("/commandspy ignore ") {
        let prefix_len = if cmd.starts_with("/cmdspy ignore ") { 15 } else { 19 };
        let sub = cmd[prefix_len..cmd.len];
        if !sub.contains(" ") {
            let options = ["add", "remove", "list"];
            let matches = [];
            for o in options {
                if sub == "" || o.starts_with(sub) {
                    matches.push(o);
                }
            }
            return matches;
        }
    }

    if cmd.starts_with("/cmdspy ") || cmd.starts_with("/commandspy ") {
        let prefix_len = if cmd.starts_with("/cmdspy ") { 8 } else { 12 };
        let sub = cmd[prefix_len..cmd.len];
        if !sub.contains(" ") {
            let options = ["toggle", "list", "ignore"];
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
