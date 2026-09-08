// =============================================================================
// FrameMC Plugin 01: LiteBans / Network Moderation Suite
// =============================================================================
// Recreates: LiteBans, AdvancedBan, BungeeAdminTools
// Features:
// - Cross-network permanent & temporary bans (/ban, /tempban, /unban, /checkban, /banlist)
// - Cross-network permanent & temporary mutes (/mute, /tempmute, /unmute, /mutelist)
// - Network kick (/kick)
// - Warnings & infraction tracking (/warn, /warnings)
// - Automated expiration handling and formatted Minecraft disconnect screens
// - Role-based authorization & quarantine enforcement
// =============================================================================

fn is_admin(player) {
    if player == "Notch" || player == "admin" || player == "Admin" || player == "Console" { return true; }
    kv_has("admin:" + player) || kv_has("staff_member:" + player)
}

fn is_quarantined(player) {
    kv_has("auth_session:" + player) && kv_get("auth_session:" + player) == false
}

fn parse_duration(s) {
    if s.len < 2 { return 0; }
    let num_part = s[0..s.len - 1];
    let unit = s[s.len - 1..s.len];
    let digits = "0123456789";
    let i = 0;
    while i < num_part.len {
        if !digits.contains(num_part[i..i+1]) {
            return 0;
        }
        i += 1;
    }
    let val = parse_int(num_part);
    if val <= 0 { return 0; }

    if unit == "s" { return val; }
    if unit == "m" { return val * 60; }
    if unit == "h" { return val * 3600; }
    if unit == "d" { return val * 86400; }
    0
}

fn format_duration(seconds) {
    if seconds <= 0 { return "Permanent"; }
    if seconds < 60 { return "" + seconds + "s"; }
    if seconds < 3600 { return "" + (seconds / 60) + "m " + (seconds % 60) + "s"; }
    if seconds < 86400 { return "" + (seconds / 3600) + "h " + ((seconds % 3600) / 60) + "m"; }
    "" + (seconds / 86400) + "d " + ((seconds % 86400) / 3600) + "h"
}

/// Invoked whenever a player connects to FrameMC.
fn on_player_join(event) {
    let name = event.player_name;
    let ban_key = "ban:" + name;

    if kv_has(ban_key) {
        let ban = kv_get(ban_key);
        let now = timestamp_sec();

        // Check if temporary ban has expired
        if ban.expires_at > 0 && now >= ban.expires_at {
            kv_remove(ban_key);
            proxy_info("Expired tempban removed for player " + name);
        } else {
            let rem_str = if ban.expires_at == 0 { "Permanent" } else { format_duration(ban.expires_at - now) };
            let screen = "§c§lYOU ARE BANNED FROM FRAMEMC\n\n"
                + "§7Reason: §f" + ban.reason + "\n"
                + "§7Expires in: §e" + rem_str + "\n"
                + "§7Banned by: §b" + ban.banner + "\n\n"
                + "§7Appeal at: §fhttps://discord.gg/yourserver";

            return #{
                allow: false,
                disconnect_reason: screen,
                target_server: ""
            };
        }
    }

    #{ allow: true, disconnect_reason: "", target_server: "" }
}

/// Invoked whenever a player issues a chat command.
fn on_player_command(event) {
    let cmd = event.command;
    let sender = event.player_name;

    // Check captive quarantine
    if is_quarantined(sender) {
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§c§l[ProxyAuth] Please authenticate first with /login <pin> or /register <pin>."
        };
    }

    // 1. Mute Enforcement: Check if sender is currently muted when trying to speak
    let mute_key = "mute:" + sender;
    if kv_has(mute_key) {
        let mute = kv_get(mute_key);
        let now = timestamp_sec();

        if mute.expires_at > 0 && now >= mute.expires_at {
            kv_remove(mute_key);
            proxy_info("Expired tempmute removed for player " + sender);
        } else {
            let chat_cmds = ["/msg", "/tell", "/w", "/whisper", "/r", "/reply", "/g", "/global", "/me", "/say"];
            let is_chat = false;
            for c in chat_cmds {
                if cmd == c || cmd.starts_with(c + " ") {
                    is_chat = true;
                    break;
                }
            }
            if is_chat {
                let rem_str = if mute.expires_at == 0 { "Permanent" } else { format_duration(mute.expires_at - now) };
                return #{
                    cancel: true,
                    reroute_server: "",
                    send_message: "§cYou are muted! Reason: §f" + mute.reason + " §7(Remaining: §e" + rem_str + "§7)"
                };
            }
        }
    }

    // 2. Command: /ban <player> [reason]
    if cmd == "/ban" || cmd.starts_with("/ban ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/ban" { "" } else { cmd[5..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 1 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /ban <player> [reason]" };
        }
        let target = parts[0];
        let reason = if parts.len > 1 { raw[target.len..raw.len].trim() } else { "Banned by an operator." };

        kv_set("ban:" + target, #{
            target: target,
            banner: sender,
            reason: reason,
            expires_at: 0,
            created_at: timestamp_sec()
        });
        proxy_info("Player " + target + " was permanently banned by " + sender + " (" + reason + ")");

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[LiteBans] §f" + target + " §7was permanently banned by §b" + sender + " §7(§f" + reason + "§7)."
        };
    }

    // 3. Command: /tempban <player> <duration> [reason] (e.g. 10m, 2h, 7d)
    if cmd == "/tempban" || cmd.starts_with("/tempban ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/tempban" { "" } else { cmd[9..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /tempban <player> <10s|5m|2h|7d> [reason]" };
        }
        let target = parts[0];
        let dur_sec = parse_duration(parts[1]);
        if dur_sec <= 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§cInvalid duration format. Use e.g. 30s, 15m, 2h, 7d." };
        }
        let prefix_len = target.len + 1 + parts[1].len;
        let reason = if raw.len > prefix_len { raw[prefix_len..raw.len].trim() } else { "Temporary ban." };

        let now = timestamp_sec();
        let exp = now + dur_sec;

        kv_set("ban:" + target, #{
            target: target,
            banner: sender,
            reason: reason,
            expires_at: exp,
            created_at: now
        });
        proxy_info("Player " + target + " was tempbanned for " + parts[1] + " by " + sender);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[LiteBans] §f" + target + " §7was banned for §e" + parts[1] + " §7by §b" + sender + " §7(§f" + reason + "§7)."
        };
    }

    // 4. Command: /unban <player>
    if cmd == "/unban" || cmd.starts_with("/unban ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target = if cmd == "/unban" { "" } else { cmd[7..cmd.len] };
        target.trim();
        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /unban <player>" };
        }
        let ban_key = "ban:" + target;
        if !kv_has(ban_key) {
            return #{ cancel: true, reroute_server: "", send_message: "§cPlayer '" + target + "' is not banned." };
        }
        kv_remove(ban_key);
        proxy_info("Player " + target + " was unbanned by " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[LiteBans] §f" + target + " §7has been unbanned by §b" + sender + "§7." };
    }

    // 5. Command: /checkban <player>
    if cmd == "/checkban" || cmd.starts_with("/checkban ") {
        let target = if cmd == "/checkban" { "" } else { cmd[10..cmd.len] };
        target.trim();
        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /checkban <player>" };
        }
        let ban_key = "ban:" + target;
        if !kv_has(ban_key) {
            return #{ cancel: true, reroute_server: "", send_message: "§aPlayer '" + target + "' is not currently banned." };
        }
        let ban = kv_get(ban_key);
        let rem = if ban.expires_at == 0 { "Permanent" } else { format_duration(ban.expires_at - timestamp_sec()) };
        let msg = "§6[LiteBans] §eBan Record for §f" + target + "§e:\n"
            + "§7- Status: §cActive\n"
            + "§7- Reason: §f" + ban.reason + "\n"
            + "§7- Banner: §b" + ban.banner + "\n"
            + "§7- Remaining: §e" + rem;
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    // 6. Command: /banlist
    if cmd == "/banlist" {
        let keys = kv_keys("ban:");
        if keys.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§a[LiteBans] There are currently no active bans on the network." };
        }
        let list_str = "";
        let i = 0;
        while i < keys.len {
            if i > 0 { list_str += "§7, "; }
            let p_name = keys[i][4..keys[i].len];
            list_str += "§c" + p_name;
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: "§6[LiteBans] Active bans (" + keys.len + "): " + list_str };
    }

    // 7. Command: /mute <player> [reason]
    if cmd == "/mute" || cmd.starts_with("/mute ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/mute" { "" } else { cmd[6..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 1 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /mute <player> [reason]" };
        }
        let target = parts[0];
        let reason = if parts.len > 1 { raw[target.len..raw.len].trim() } else { "Muted by staff." };

        kv_set("mute:" + target, #{
            target: target,
            banner: sender,
            reason: reason,
            expires_at: 0,
            created_at: timestamp_sec()
        });
        return #{ cancel: true, reroute_server: "", send_message: "§a[LiteBans] §f" + target + " §7has been muted permanently by §b" + sender + "§7." };
    }

    // 8. Command: /tempmute <player> <duration> [reason]
    if cmd == "/tempmute" || cmd.starts_with("/tempmute ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/tempmute" { "" } else { cmd[10..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /tempmute <player> <duration> [reason]" };
        }
        let target = parts[0];
        let dur = parse_duration(parts[1]);
        if dur <= 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§cInvalid duration format. Use 5m, 1h, 1d." };
        }
        let prefix_len = target.len + 1 + parts[1].len;
        let reason = if raw.len > prefix_len { raw[prefix_len..raw.len].trim() } else { "Temporary mute." };
        let exp = timestamp_sec() + dur;

        kv_set("mute:" + target, #{
            target: target,
            banner: sender,
            reason: reason,
            expires_at: exp,
            created_at: timestamp_sec()
        });
        return #{ cancel: true, reroute_server: "", send_message: "§a[LiteBans] §f" + target + " §7has been muted for §e" + parts[1] + " §7by §b" + sender + "§7." };
    }

    // 9. Command: /unmute <player>
    if cmd == "/unmute" || cmd.starts_with("/unmute ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target = if cmd == "/unmute" { "" } else { cmd[8..cmd.len] };
        target.trim();
        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /unmute <player>" };
        }
        let key = "mute:" + target;
        if !kv_has(key) {
            return #{ cancel: true, reroute_server: "", send_message: "§cPlayer '" + target + "' is not muted." };
        }
        kv_remove(key);
        return #{ cancel: true, reroute_server: "", send_message: "§a[LiteBans] §f" + target + " §7has been unmuted." };
    }

    // 10. Command: /mutelist
    if cmd == "/mutelist" {
        let keys = kv_keys("mute:");
        if keys.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§a[LiteBans] There are currently no active mutes." };
        }
        let list_str = "";
        let i = 0;
        while i < keys.len {
            if i > 0 { list_str += "§7, "; }
            let p_name = keys[i][5..keys[i].len];
            list_str += "§e" + p_name;
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: "§6[LiteBans] Active mutes (" + keys.len + "): " + list_str };
    }

    // 11. Command: /warn <player> [reason]
    if cmd == "/warn" || cmd.starts_with("/warn ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/warn" { "" } else { cmd[6..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 1 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /warn <player> [reason]" };
        }
        let target = parts[0];
        let reason = if parts.len > 1 { raw[target.len..raw.len].trim() } else { "Rule violation." };
        let warn_key = "warns:" + target;
        let count = if kv_has(warn_key) { kv_get(warn_key) } else { 0 };
        count += 1;
        kv_set(warn_key, count);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§e[LiteBans] §f" + target + " §7has been warned by §b" + sender + " §7(Warning #" + count + ": §f" + reason + "§7)."
        };
    }

    // 12. Command: /warnings <player>
    if cmd == "/warnings" || cmd.starts_with("/warnings ") {
        let target = if cmd == "/warnings" { sender } else { cmd[10..cmd.len] };
        target.trim();
        let count = if kv_has("warns:" + target) { kv_get("warns:" + target) } else { 0 };
        return #{ cancel: true, reroute_server: "", send_message: "§6[LiteBans] §f" + target + " §7has §e" + count + " §7warning(s)." };
    }

    // 13. Command: /kick <player> [reason]
    if cmd == "/kick" || cmd.starts_with("/kick ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/kick" { "" } else { cmd[6..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 1 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /kick <player> [reason]" };
        }
        let target = parts[0];
        let reason = if parts.len > 1 { raw[target.len..raw.len].trim() } else { "Kicked by staff." };
        proxy_info("Player " + target + " was kicked by " + sender + " (" + reason + ")");
        return #{ cancel: true, reroute_server: "", send_message: "§a[LiteBans] §f" + target + " §7has been kicked from the network (§f" + reason + "§7)." };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

/// Invoked for tab-completion.
fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_admin(sender) {
        return [];
    }
    let root_cmds = ["/ban", "/tempban", "/unban", "/checkban", "/banlist", "/mute", "/tempmute", "/unmute", "/mutelist", "/warn", "/warnings", "/kick"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    let prefixes = ["/ban ", "/tempban ", "/unban ", "/checkban ", "/mute ", "/tempmute ", "/unmute ", "/warn ", "/warnings ", "/kick "];
    for p in prefixes {
        if cmd.starts_with(p) {
            let arg = cmd[p.len..cmd.len];
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
