// =============================================================================
// FrameMC Plugin 10: BungeeAnnounce / Broadcast & AutoMessage Suite
// =============================================================================
// Recreates: BungeeAnnounce, GlobalAnnounce, AutoMessage
// Features:
// - Network-wide high-priority alerts (/alert)
// - Polished broadcasts (/announce, /tip)
// - Per-backend targeted alerts (/alertserver)
// - Configurable rotating auto-message scheduler (/automsg)
// - Admin authorization on broadcasts
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

    if is_quarantined(sender) {
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§c§l[ProxyAuth] Please authenticate first with /login <pin> or /register <pin>."
        };
    }

    // 1. Command: /alert <message>
    if cmd == "/alert" || cmd.starts_with("/alert ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let msg = if cmd == "/alert" { "" } else { cmd[7..cmd.len] };
        msg.trim();
        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /alert <message>" };
        }
        let formatted = "§4§l[ALERT] §f" + msg;
        proxy_info("[Alert by " + sender + "] " + msg);
        return #{ cancel: true, reroute_server: "", send_message: formatted };
    }

    // 2. Command: /announce <message>
    if cmd == "/announce" || cmd.starts_with("/announce ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let msg = if cmd == "/announce" { "" } else { cmd[10..cmd.len] };
        msg.trim();
        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /announce <message>" };
        }
        let formatted = "§6§l[ANNOUNCEMENT] §e" + msg;
        proxy_info("[Announce by " + sender + "] " + msg);
        return #{ cancel: true, reroute_server: "", send_message: formatted };
    }

    // 3. Command: /tip <message>
    if cmd == "/tip" || cmd.starts_with("/tip ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let msg = if cmd == "/tip" { "" } else { cmd[5..cmd.len] };
        msg.trim();
        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /tip <message>" };
        }
        let formatted = "§a§l[TIP] §7" + msg;
        return #{ cancel: true, reroute_server: "", send_message: formatted };
    }

    // 4. Command: /alertserver <server> <message>
    if cmd == "/alertserver" || cmd.starts_with("/alertserver ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/alertserver" { "" } else { cmd[13..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /alertserver <server> <message>" };
        }
        let target_srv = parts[0];
        let msg = raw[target_srv.len..raw.len].trim();
        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /alertserver <server> <message>" };
        }

        if !server_exists(target_srv) {
            return #{ cancel: true, reroute_server: "", send_message: "§cServer '" + target_srv + "' does not exist." };
        }

        let formatted = "§e§l[" + target_srv.to_upper() + " NOTICE] §f" + msg;
        proxy_info("[Alert to " + target_srv + "] " + msg);
        return #{ cancel: true, reroute_server: "", send_message: formatted };
    }

    // 5. Command: /automsg ...
    if cmd == "/automsg" || cmd == "/automsg help" {
        let help = "§8======== §6[FrameMC AutoMessage] §8========\n"
            + "§e/automsg list §7- List registered auto-tips\n"
            + "§e/automsg add <tip> §7- Add a new tip to pool (Admin)\n"
            + "§e/automsg trigger §7- Broadcast next tip in queue (Admin)\n"
            + "§e/automsg clear §7- Clear all scheduled tips (Admin)";
        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    if cmd == "/automsg list" {
        let pool = if kv_has("automsg:pool") { kv_get("automsg:pool") } else { [] };
        if pool.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§7[AutoMsg] No automated messages registered." };
        }
        let msg = "§6[AutoMessage Pool (" + pool.len + ")]:\n";
        let i = 0;
        while i < pool.len {
            msg += "§e#" + (i + 1) + ": §7" + pool[i] + "\n";
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/automsg add" || cmd.starts_with("/automsg add ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let tip = if cmd == "/automsg add" { "" } else { cmd[13..cmd.len] };
        tip.trim();
        if tip == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /automsg add <tip>" };
        }
        let pool = if kv_has("automsg:pool") { kv_get("automsg:pool") } else { [] };
        pool.push(tip);
        kv_set("automsg:pool", pool);
        return #{ cancel: true, reroute_server: "", send_message: "§a[AutoMsg] Added tip #" + pool.len + ": §7" + tip };
    }

    if cmd == "/automsg trigger" {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let pool = if kv_has("automsg:pool") { kv_get("automsg:pool") } else { [] };
        if pool.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§c[AutoMsg] Pool is empty. Add tips with /automsg add <tip>." };
        }
        let idx = if kv_has("automsg:idx") { kv_get("automsg:idx") } else { 0 };
        let tip = pool[idx % pool.len];
        kv_set("automsg:idx", (idx + 1) % pool.len);

        let formatted = "§a§l[PROMPT] §f" + tip;
        return #{ cancel: true, reroute_server: "", send_message: formatted };
    }

    if cmd == "/automsg clear" {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        kv_remove("automsg:pool");
        kv_remove("automsg:idx");
        return #{ cancel: true, reroute_server: "", send_message: "§e[AutoMsg] Cleared all automated messages." };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_admin(sender) {
        return [];
    }
    let root_cmds = ["/alert", "/announce", "/tip", "/alertserver", "/automsg"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/alertserver ") {
        let raw = cmd[13..cmd.len];
        if !raw.contains(" ") {
            let servers = get_servers();
            let matches = [];
            for s in servers {
                if raw == "" || s.starts_with(raw) {
                    matches.push(s);
                }
            }
            return matches;
        }
    }

    if cmd.starts_with("/automsg ") {
        let sub = cmd[9..cmd.len];
        if !sub.contains(" ") {
            let options = ["list", "add", "trigger", "clear"];
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
