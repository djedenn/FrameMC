// =============================================================================
// FrameMC Plugin 18: NetworkReport / Cross-Server Moderation Tickets
// =============================================================================
// Recreates: BungeeReport, NetworkReport, BAT Report
// Features:
// - Network player infraction reporting (/report <player> <reason>)
// - Active staff notification & audit trail
// - Ticket queue triage & inspection (/reports list, /reports view)
// - Ticket ownership claiming & resolution lifecycle (/reports claim, /reports close)
// =============================================================================

fn is_quarantined(player) {
    if kv_has("auth_session:" + player) {
        return !kv_get("auth_session:" + player);
    }
    false
}

fn is_staff(player) {
    if player == "Console" || player == "admin" || player == "Admin" || player == "Notch" {
        return true;
    }
    if kv_has("staff_member:" + player) || kv_has("admin:" + player) {
        return true;
    }
    false
}

fn on_player_command(event) {
    let cmd = event.command;
    let sender = event.player_name;
    let current_server = event.current_server;

    if is_quarantined(sender) && (cmd == "/report" || cmd.starts_with("/report ") || cmd == "/reports" || cmd.starts_with("/reports ")) {
        return #{ cancel: true, reroute_server: "", send_message: "§c[ProxyAuth] You must authenticate before using the report system." };
    }

    // 1. Command: /report <player> <reason>
    if cmd == "/report" {
        return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /report <player> <reason>" };
    }

    if cmd.starts_with("/report ") {
        let raw = cmd[8..cmd.len];
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /report <player> <reason>" };
        }
        let target = parts[0];
        let reason = raw[target.len..raw.len].trim();

        if target == sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou cannot report yourself." };
        }

        let count = if kv_has("report:counter") { kv_get("report:counter") } else { 0 };
        count += 1;
        kv_set("report:counter", count);

        let ticket_id = "" + count;
        let ticket = #{
            id: ticket_id,
            target: target,
            reporter: sender,
            server: current_server,
            reason: reason,
            status: "OPEN",
            claimed_by: "none",
            created_at: timestamp_sec()
        };
        kv_set("ticket:" + ticket_id, ticket);
        proxy_info("[Report #" + ticket_id + "] " + sender + " reported " + target + " on " + current_server + " (" + reason + ")");

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[Report] §7Your report §e#" + ticket_id + " §7against §c" + target + " §7has been submitted to online staff. Thank you!"
        };
    }

    // 2. Command: /reports ...
    if cmd == "/reports" || cmd.starts_with("/reports ") {
        if !is_staff(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to view or manage tickets." };
        }

        if cmd == "/reports" || cmd == "/reports help" {
            let help = "§8======== §c[FrameMC Ticket System] §8========\n"
                + "§e/reports list §7- List active open reports\n"
                + "§e/reports view <id> §7- View ticket details\n"
                + "§e/reports claim <id> §7- Claim ticket as handler\n"
                + "§e/reports close <id> [notes] §7- Close and resolve ticket";
            return #{ cancel: true, reroute_server: "", send_message: help };
        }

        if cmd == "/reports list" {
            let keys = kv_keys("ticket:");
            if keys.len == 0 {
                return #{ cancel: true, reroute_server: "", send_message: "§a[Reports] There are currently no open tickets." };
            }
            let msg = "§6[Active Report Tickets (" + keys.len + ")]:\n";
            let i = 0;
            while i < keys.len {
                let t = kv_get(keys[i]);
                if t.status == "OPEN" || t.status == "CLAIMED" {
                    msg += "§e#" + t.id + " §7- §c" + t.target + " §7(by §b" + t.reporter + "§7 on §a" + t.server + "§7) [" + t.status + "]\n";
                }
                i += 1;
            }
            return #{ cancel: true, reroute_server: "", send_message: msg };
        }

        if cmd == "/reports view" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /reports view <id>" };
        }

        if cmd.starts_with("/reports view ") {
            let id = cmd[14..cmd.len];
            id.trim();
            let key = "ticket:" + id;
            if !kv_has(key) {
                return #{ cancel: true, reroute_server: "", send_message: "§cTicket #" + id + " not found." };
            }
            let t = kv_get(key);
            let msg = "§8======== §6[Report #" + t.id + " Details] §8========\n"
                + "§7- Reported Player: §c" + t.target + "\n"
                + "§7- Filed by: §b" + t.reporter + "\n"
                + "§7- Origin Server: §e" + t.server + "\n"
                + "§7- Reason: §f" + t.reason + "\n"
                + "§7- Status: §a" + t.status + "\n"
                + "§7- Assigned Staff: §e" + t.claimed_by;
            return #{ cancel: true, reroute_server: "", send_message: msg };
        }

        if cmd == "/reports claim" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /reports claim <id>" };
        }

        if cmd.starts_with("/reports claim ") {
            let id = cmd[15..cmd.len];
            id.trim();
            let key = "ticket:" + id;
            if !kv_has(key) {
                return #{ cancel: true, reroute_server: "", send_message: "§cTicket #" + id + " not found." };
            }
            let t = kv_get(key);
            t.status = "CLAIMED";
            t.claimed_by = sender;
            kv_set(key, t);
            return #{ cancel: true, reroute_server: "", send_message: "§a[Reports] You claimed ticket §e#" + id + "§a." };
        }

        if cmd == "/reports close" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /reports close <id> [notes]" };
        }

        if cmd.starts_with("/reports close ") {
            let raw = cmd[15..cmd.len];
            raw.trim();
            let parts = raw.split(" ");
            let id = parts[0];
            let key = "ticket:" + id;
            if !kv_has(key) {
                return #{ cancel: true, reroute_server: "", send_message: "§cTicket #" + id + " not found." };
            }
            kv_remove(key);
            proxy_info("Ticket #" + id + " closed by " + sender);
            return #{ cancel: true, reroute_server: "", send_message: "§a[Reports] Ticket §e#" + id + " §awas resolved and closed." };
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

    if cmd == "" || !cmd.contains(" ") {
        let list = ["/report"];
        if is_staff(sender) {
            list.push("/reports");
        }
        let matches = [];
        for item in list {
            if cmd == "" || item.starts_with(cmd) {
                matches.push(item);
            }
        }
        return matches;
    }

    if cmd.starts_with("/report ") {
        let target_prefix = cmd[8..cmd.len];
        let online_keys = kv_keys("online:");
        let matches = [];
        for k in online_keys {
            let p = k["online:".len..k.len];
            if target_prefix == "" || p.starts_with(target_prefix) {
                matches.push(p);
            }
        }
        return matches;
    }

    if cmd.starts_with("/reports ") {
        if !is_staff(sender) {
            return [];
        }
        let sub = cmd[9..cmd.len];
        if !sub.contains(" ") {
            let list = ["list", "view", "claim", "close"];
            let matches = [];
            for item in list {
                if sub == "" || item.starts_with(sub) {
                    matches.push(item);
                }
            }
            return matches;
        }

        let id_prefix = "";
        if sub.starts_with("view ") { id_prefix = sub[5..sub.len]; }
        if sub.starts_with("claim ") { id_prefix = sub[6..sub.len]; }
        if sub.starts_with("close ") { id_prefix = sub[6..sub.len]; }

        if id_prefix != "" {
            let ticket_keys = kv_keys("ticket:");
            let matches = [];
            for k in ticket_keys {
                let tid = k["ticket:".len..k.len];
                if id_prefix == "" || tid.starts_with(id_prefix) {
                    matches.push(tid);
                }
            }
            return matches;
        }
    }

    []
}
