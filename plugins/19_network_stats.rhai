// =============================================================================
// FrameMC Plugin 19: NetworkStats / Performance Telemetry & Directory
// =============================================================================
// Recreates: BungeePlayerCounter, NetworkStats, ProxyStatus
// Features:
// - FrameMC engine telemetry & diagnostics (/proxyinfo)
// - Real-time network throughput and player distribution metrics (/stats)
// - Backend routing directory inspection (/servers)
// - Proxy uptime tracking (/uptime)
// =============================================================================

fn is_quarantined(player) {
    if kv_has("auth_session:" + player) {
        return !kv_get("auth_session:" + player);
    }
    false
}

fn format_time_diff(seconds) {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;

    let out = "";
    if days > 0 { out += "" + days + "d "; }
    if hours > 0 || days > 0 { out += "" + hours + "h "; }
    out += "" + mins + "m " + secs + "s";
    out
}

fn on_player_command(event) {
    let cmd = event.command;
    let sender = event.player_name;

    if is_quarantined(sender) && (cmd == "/proxyinfo" || cmd == "/stats" || cmd == "/servers" || cmd == "/uptime") {
        return #{ cancel: true, reroute_server: "", send_message: "§c[ProxyAuth] You must authenticate before accessing proxy diagnostics." };
    }

    // Track proxy start time if not set
    if !kv_has("proxy:start_time") {
        kv_set("proxy:start_time", timestamp_sec());
    }

    // 1. Command: /proxyinfo
    if cmd == "/proxyinfo" {
        let servers = get_servers();
        let start_time = kv_get("proxy:start_time");
        let uptime = format_time_diff(timestamp_sec() - start_time);
        let online = kv_keys("online:").len;

        let msg = "§8======== §6[FrameMC Proxy Diagnostics] §8========\n"
            + "§7- Software: §aFrameMC (Native Rust v0.1.0)\n"
            + "§7- Engine: §bTokio Asynchronous Event Loop (Zero GC)\n"
            + "§7- Script Engine: §dSandboxed Rhai 1.19\n"
            + "§7- Active Connections: §e" + online + " players\n"
            + "§7- Registered Backends: §f" + servers.len + " servers\n"
            + "§7- Proxy Uptime: §b" + uptime + "\n"
            + "§7- Forwarding: §aVelocity HMAC + Legacy Bungee";

        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    // 2. Command: /stats
    if cmd == "/stats" {
        let servers = get_servers();
        let online_keys = kv_keys("online:");
        let total = online_keys.len;

        let msg = "§6[Network Distribution Stats]\n"
            + "§7Total Network Players: §a" + total + "\n";

        let i = 0;
        while i < servers.len {
            let s_name = servers[i];
            let count = 0;
            let j = 0;
            while j < online_keys.len {
                if kv_get(online_keys[j]) == s_name { count += 1; }
                j += 1;
            }
            let pct = if total > 0 { (count * 100) / total } else { 0 };
            msg += "§7- §e" + s_name + "§7: §f" + count + " §8(" + pct + "%)\n";
            i += 1;
        }

        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    // 3. Command: /servers
    if cmd == "/servers" {
        let servers = get_servers();
        let def = get_default_server();
        let msg = "§8======== §e[FrameMC Backend Directory] §8========\n"
            + "§7Default Server: §a" + def + "\n§7Available Servers:\n";

        let i = 0;
        while i < servers.len {
            let s = servers[i];
            let is_def = (s == def);
            msg += "§7  [" + (i + 1) + "] §b" + s + (if is_def { " §6(Default)" } else { "" }) + "\n";
            i += 1;
        }
        msg += "§7Switch with §f/server <name>§7.";

        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    // 4. Command: /uptime
    if cmd == "/uptime" {
        let start_time = kv_get("proxy:start_time");
        let uptime = format_time_diff(timestamp_sec() - start_time);
        return #{ cancel: true, reroute_server: "", send_message: "§6[FrameMC] Proxy Uptime: §a" + uptime };
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
        let list = ["/proxyinfo", "/stats", "/servers", "/uptime"];
        let matches = [];
        for item in list {
            if cmd == "" || item.starts_with(cmd) {
                matches.push(item);
            }
        }
        return matches;
    }
    []
}
