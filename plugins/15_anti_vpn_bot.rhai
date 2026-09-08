// =============================================================================
// FrameMC Plugin 15: AntiVPN / AntiBot & IP Defense System
// =============================================================================
// Recreates: AntiVPN, AntiBotDeluxe, ProxyIpSecurity
// Features:
// - Connection flood burst protection (sliding window join rate limiter)
// - Dynamic IP blacklist & whitelist (/antibot banip, /antibot whitelist)
// - Threat status inspection & join rate metrics (/antibot status)
// - Fast fail-closed handshake rejection
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
    let ip = event.ip;
    let name = event.player_name;
    let now = timestamp_sec();

    // 1. Check IP Blacklist
    let ip_key = "ipban:" + ip;
    if kv_has(ip_key) {
        let ban_info = kv_get(ip_key);
        proxy_warn("[AntiBot] Blocked connection from banned IP: " + ip + " (" + name + ")");
        return #{
            allow: false,
            disconnect_reason: "§c§lYOUR IP ADDRESS IS BANNED\n\n§7Reason: §f" + ban_info.reason + "\n§7IP: §e" + ip,
            target_server: ""
        };
    }

    // 2. Whitelist bypass
    if kv_has("ipwl:" + ip) {
        return #{ allow: true, disconnect_reason: "", target_server: "" };
    }

    // 3. Sliding Window Join Rate Limiting (Max 3 joins per 10 seconds per IP)
    let rate_key = "ip_rate:" + ip;
    let record = if kv_has(rate_key) { kv_get(rate_key) } else { #{ count: 0, first_seen: now } };

    if now - record.first_seen < 10 {
        if record.count >= 3 {
            proxy_warn("[AntiBot] Rate limit exceeded for IP " + ip + " (Player: " + name + ")");
            let blocked_cnt = if kv_has("antibot:blocked_count") { kv_get("antibot:blocked_count") } else { 0 };
            kv_set("antibot:blocked_count", blocked_cnt + 1);

            return #{
                allow: false,
                disconnect_reason: "§c[AntiBot] Connection rate limit exceeded. Please wait 10 seconds before reconnecting.",
                target_server: ""
            };
        }
        record.count += 1;
        kv_set(rate_key, record);
    } else {
        // Reset sliding window
        kv_set(rate_key, #{ count: 1, first_seen: now });
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

    if cmd == "/antibot" || cmd == "/antibot help" {
        let help = "§8======== §c[FrameMC AntiBot / IP Security] §8========\n"
            + "§e/antibot status §7- View blocked connections & metrics\n"
            + "§e/antibot banip <ip> [reason] §7- Ban an IP address (Admin)\n"
            + "§e/antibot unbanip <ip> §7- Unban an IP address (Admin)\n"
            + "§e/antibot whitelist <ip> §7- Whitelist IP from rate limits (Admin)\n"
            + "§e/antibot list §7- List banned IP addresses";
        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    if cmd == "/antibot status" {
        let blocked = if kv_has("antibot:blocked_count") { kv_get("antibot:blocked_count") } else { 0 };
        let banned_ips = kv_keys("ipban:").len;
        let wl_ips = kv_keys("ipwl:").len;

        let msg = "§6[AntiBot Engine Status]\n"
            + "§7- Status: §aARMED & ACTIVE\n"
            + "§7- Blocked Burst Connections: §c" + blocked + "\n"
            + "§7- Banned IP Addresses: §e" + banned_ips + "\n"
            + "§7- Whitelisted IP Addresses: §b" + wl_ips + "\n"
            + "§7- Threshold: §f3 joins / 10s per IP";

        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/antibot banip" || cmd.starts_with("/antibot banip ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let raw = if cmd == "/antibot banip" { "" } else { cmd[15..cmd.len] };
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 1 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /antibot banip <ip> [reason]" };
        }
        let target_ip = parts[0];
        let reason = if parts.len > 1 { raw[target_ip.len..raw.len].trim() } else { "Banned by staff." };

        kv_set("ipban:" + target_ip, #{
            ip: target_ip,
            reason: reason,
            banner: sender,
            created_at: timestamp_sec()
        });
        proxy_info("IP " + target_ip + " was banned by " + sender + " (" + reason + ")");

        return #{ cancel: true, reroute_server: "", send_message: "§a[AntiBot] IP §e" + target_ip + " §ahas been banned (§f" + reason + "§a)." };
    }

    if cmd == "/antibot unbanip" || cmd.starts_with("/antibot unbanip ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target_ip = if cmd == "/antibot unbanip" { "" } else { cmd[17..cmd.len] };
        target_ip.trim();
        if target_ip == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /antibot unbanip <ip>" };
        }
        if !kv_has("ipban:" + target_ip) {
            return #{ cancel: true, reroute_server: "", send_message: "§cIP '" + target_ip + "' is not banned." };
        }
        kv_remove("ipban:" + target_ip);
        return #{ cancel: true, reroute_server: "", send_message: "§a[AntiBot] IP §e" + target_ip + " §ahas been unbanned." };
    }

    if cmd == "/antibot whitelist" || cmd.starts_with("/antibot whitelist ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let target_ip = if cmd == "/antibot whitelist" { "" } else { cmd[19..cmd.len] };
        target_ip.trim();
        if target_ip == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /antibot whitelist <ip>" };
        }
        kv_set("ipwl:" + target_ip, true);
        return #{ cancel: true, reroute_server: "", send_message: "§a[AntiBot] IP §e" + target_ip + " §awhitelisted from rate limits." };
    }

    if cmd == "/antibot list" {
        let keys = kv_keys("ipban:");
        if keys.len == 0 {
            return #{ cancel: true, reroute_server: "", send_message: "§7[AntiBot] No IP bans active." };
        }
        let list_str = "";
        let i = 0;
        while i < keys.len {
            if i > 0 { list_str += "§7, "; }
            list_str += "§c" + keys[i][6..keys[i].len];
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: "§6[AntiBot] Banned IPs (" + keys.len + "): " + list_str };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) || !is_admin(sender) {
        return [];
    }
    let root_cmds = ["/antibot"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/antibot ") {
        let sub = cmd[9..cmd.len];
        if !sub.contains(" ") {
            let options = ["status", "banip", "unbanip", "whitelist", "list"];
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
