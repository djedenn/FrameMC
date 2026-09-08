// =============================================================================
// FrameMC Plugin 16: DynamicMOTD / Server List Customizer
// =============================================================================
// Recreates: ServerListPlus, DynamicMOTD, CleanMOTD
// Features:
// - Runtime dynamic MOTD management (/motd set, /motd get)
// - Rotating randomized MOTD pool (/motd random)
// - Live countdown event timers (/motd countdown)
// - Custom Minecraft color code formatting (§a, §b, §c, etc.)
// - Admin role authorization
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

    if cmd == "/motd" || cmd == "/motd help" {
        let help = "§8======== §6[FrameMC DynamicMOTD] §8========\n"
            + "§e/motd get §7- View current dynamic MOTD\n"
            + "§e/motd set <line1> | <line2> §7- Set custom MOTD lines (Admin)\n"
            + "§e/motd random add <text> §7- Add to randomized MOTD pool (Admin)\n"
            + "§e/motd random list §7- List randomized MOTD pool\n"
            + "§e/motd countdown <event> <epoch> §7- Set countdown MOTD (Admin)\n"
            + "§e/motd reset §7- Reset MOTD to proxy config default (Admin)";
        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    if cmd == "/motd get" {
        let motd = if kv_has("custom_motd:text") { kv_get("custom_motd:text") } else { "§aFrameMC §7High-Performance Minecraft Proxy" };
        let msg = "§6[DynamicMOTD] Active Ping Message:\n§f" + motd;
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/motd set" || cmd.starts_with("/motd set ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let text = if cmd == "/motd set" { "" } else { cmd[10..cmd.len] };
        text.trim();
        if text == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /motd set <line1> | <line2>" };
        }
        let parts = text.split("|");
        let formatted = "";
        let i = 0;
        while i < parts.len {
            if i > 0 { formatted += "\n"; }
            let p = parts[i];
            p.trim();
            formatted += p;
            i += 1;
        }
        kv_set("custom_motd:text", formatted);
        proxy_info("Dynamic MOTD updated by " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[DynamicMOTD] MOTD successfully updated to:\n§f" + formatted };
    }

    if cmd == "/motd reset" {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        kv_remove("custom_motd:text");
        kv_remove("custom_motd:countdown");
        return #{ cancel: true, reroute_server: "", send_message: "§a[DynamicMOTD] MOTD reset to config.toml default." };
    }

    if cmd == "/motd random add" || cmd.starts_with("/motd random add ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let entry = if cmd == "/motd random add" { "" } else { cmd[17..cmd.len] };
        entry.trim();
        if entry == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /motd random add <text>" };
        }
        let pool = if kv_has("motd_pool:entries") { kv_get("motd_pool:entries") } else { [] };
        pool.push(entry);
        kv_set("motd_pool:entries", pool);
        return #{ cancel: true, reroute_server: "", send_message: "§a[DynamicMOTD] Added entry #" + pool.len + " to random pool." };
    }

    if cmd == "/motd random list" {
        let pool = if kv_has("motd_pool:entries") { kv_get("motd_pool:entries") } else { [] };
        if pool.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§7[DynamicMOTD] No randomized MOTDs configured." };
        }
        let msg = "§6[Random MOTD Pool (" + pool.len + ")]:\n";
        let i = 0;
        while i < pool.len {
            msg += "§e#" + (i + 1) + ": §f" + pool[i] + "\n";
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/motd countdown" || cmd.starts_with("/motd countdown ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/motd countdown" { "" } else { cmd[16..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /motd countdown <event_name> <target_timestamp_sec>" };
        }
        let event_name = parts[0];
        let epoch_str = parts[1];
        let digits = "0123456789";
        let valid = epoch_str.len > 0;
        let i = 0;
        while i < epoch_str.len {
            if !digits.contains(epoch_str[i..i+1]) {
                valid = false;
                break;
            }
            i += 1;
        }

        if !valid {
            return #{ cancel: true, reroute_server: "", send_message: "§cInvalid epoch timestamp integer." };
        }

        let target_epoch = parse_int(epoch_str);
        if target_epoch <= 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§cInvalid epoch timestamp." };
        }

        kv_set("custom_motd:countdown", #{
            name: event_name,
            target_epoch: target_epoch
        });

        return #{ cancel: true, reroute_server: "", send_message: "§a[DynamicMOTD] Countdown configured for event '§b" + event_name + "§a'!" };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_admin(sender) {
        return [];
    }
    let root_cmds = ["/motd"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/motd random ") {
        let sub = cmd[13..cmd.len];
        if !sub.contains(" ") {
            let options = ["add", "list"];
            let matches = [];
            for o in options {
                if sub == "" || o.starts_with(sub) {
                    matches.push(o);
                }
            }
            return matches;
        }
    }

    if cmd.starts_with("/motd ") {
        let sub = cmd[6..cmd.len];
        if !sub.contains(" ") {
            let options = ["get", "set", "reset", "random", "countdown"];
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
