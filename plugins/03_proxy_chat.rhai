// =============================================================================
// FrameMC Plugin 03: ProxyChat / Global Network Messaging
// =============================================================================
// Recreates: BungeeChat, ProxyChat, GlobalChat
// Features:
// - Global cross-server chat channel (/g, /global)
// - Network-wide private messaging (/msg, /tell, /w, /whisper)
// - Quick reply system (/r, /reply)
// - Channel toggling between local and proxy-wide chat (/channeltoggle)
// - Administrative screen clear (/clearchat)
// - Dynamic bad words / spam filter (/chatfilter)
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
    // Set default chat channel to local
    if !kv_has("chat_channel:" + name) {
        kv_set("chat_channel:" + name, "local");
    }
    #{ allow: true, disconnect_reason: "", target_server: "" }
}

fn contains_filtered_word(message) {
    let keys = kv_keys("filter_word:");
    let lower = message.to_lower();
    let i = 0;
    while i < keys.len {
        let word = keys[i][12..keys[i].len];
        if word != "" && lower.contains(word.to_lower()) {
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

    // 1. Command: /g <message> or /global <message>
    if cmd == "/g" || cmd == "/global" || cmd.starts_with("/g ") || cmd.starts_with("/global ") {
        let prefix_len = if cmd.starts_with("/g ") { 3 } else if cmd.starts_with("/global ") { 8 } else { cmd.len };
        let msg = if cmd == "/g" || cmd == "/global" { "" } else { cmd[prefix_len..cmd.len] };
        msg.trim();

        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /g <message>" };
        }

        if contains_filtered_word(msg) {
            return #{ cancel: true, reroute_server: "", send_message: "§c[ProxyChat] Your message was blocked by the network profanity filter." };
        }

        let formatted = "§8[§bGlobal§8] §7[§e" + current_server + "§7] §f" + sender + "§8: §f" + msg;
        proxy_info("[GlobalChat] [" + current_server + "] " + sender + ": " + msg);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: formatted
        };
    }

    // 2. Command: /msg, /tell, /w, /whisper <player> <message>
    let is_msg = cmd == "/msg" || cmd.starts_with("/msg ")
        || cmd == "/tell" || cmd.starts_with("/tell ")
        || cmd == "/w" || cmd.starts_with("/w ")
        || cmd == "/whisper" || cmd.starts_with("/whisper ");
    if is_msg {
        let raw = "";
        if cmd.starts_with("/msg ") { raw = cmd[5..cmd.len]; }
        else if cmd.starts_with("/tell ") { raw = cmd[6..cmd.len]; }
        else if cmd.starts_with("/w ") { raw = cmd[3..cmd.len]; }
        else if cmd.starts_with("/whisper ") { raw = cmd[9..cmd.len]; }
        raw.trim();

        let parts = raw.split(" ");
        if parts.len < 2 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /msg <player> <message>" };
        }
        let target = parts[0];
        let msg = raw[target.len..raw.len].trim();
        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /msg <player> <message>" };
        }

        if !kv_has("online:" + target) {
            return #{ cancel: true, reroute_server: "", send_message: "§cPlayer '" + target + "' is not online on the network." };
        }

        if contains_filtered_word(msg) {
            return #{ cancel: true, reroute_server: "", send_message: "§c[ProxyChat] Your message was blocked by the filter." };
        }

        // Store reply targets
        kv_set("reply:" + target, sender);
        kv_set("reply:" + sender, target);

        let sent_msg = "§d[To " + target + "] §f" + msg;
        proxy_info("[PM] " + sender + " -> " + target + ": " + msg);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: sent_msg
        };
    }

    // 3. Command: /r <message> or /reply <message>
    if cmd == "/r" || cmd == "/reply" || cmd.starts_with("/r ") || cmd.starts_with("/reply ") {
        let prefix_len = if cmd.starts_with("/r ") { 3 } else if cmd.starts_with("/reply ") { 7 } else { cmd.len };
        let msg = if cmd == "/r" || cmd == "/reply" { "" } else { cmd[prefix_len..cmd.len] };
        msg.trim();

        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /r <message>" };
        }

        let rep_key = "reply:" + sender;
        if !kv_has(rep_key) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou have no one to reply to." };
        }
        let target = kv_get(rep_key);
        if !kv_has("online:" + target) {
            return #{ cancel: true, reroute_server: "", send_message: "§cPlayer '" + target + "' is no longer online." };
        }

        if contains_filtered_word(msg) {
            return #{ cancel: true, reroute_server: "", send_message: "§c[ProxyChat] Your message was blocked by the filter." };
        }

        kv_set("reply:" + target, sender);
        let sent_msg = "§d[To " + target + "] §f" + msg;
        proxy_info("[PM Reply] " + sender + " -> " + target + ": " + msg);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: sent_msg
        };
    }

    // 4. Command: /channeltoggle
    if cmd == "/channeltoggle" {
        let ch_key = "chat_channel:" + sender;
        let curr = if kv_has(ch_key) { kv_get(ch_key) } else { "local" };
        let next = if curr == "local" { "global" } else { "local" };
        kv_set(ch_key, next);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[ProxyChat] Switched default chat channel to: §e" + next.to_upper()
        };
    }

    // 5. Command: /clearchat
    if cmd == "/clearchat" {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to execute this command." };
        }
        let blanks = "\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n";
        return #{
            cancel: true,
            reroute_server: "",
            send_message: blanks + "§a[ProxyChat] The chat was cleared by §b" + sender + "§a."
        };
    }

    // 6. Command: /chatfilter add <word> / remove <word> / list
    if cmd == "/chatfilter" || cmd.starts_with("/chatfilter ") {
        let sub = if cmd == "/chatfilter" { "" } else { cmd[12..cmd.len] };
        sub.trim();
        let parts = sub.split(" ");
        if parts.len < 1 || parts[0] == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /chatfilter <add|remove|list> [word]" };
        }
        let action = parts[0];

        if action == "list" {
            let keys = kv_keys("filter_word:");
            let list_str = "";
            let i = 0;
            while i < keys.len {
                if i > 0 { list_str += "§7, "; }
                list_str += "§e" + keys[i][12..keys[i].len];
                i += 1;
            }
            return #{ cancel: true, reroute_server: "", send_message: "§6[ProxyChat] Filtered words (" + keys.len + "): " + list_str };
        }

        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to modify the chat filter." };
        }

        if action == "add" && parts.len > 1 {
            let word = parts[1];
            word.trim();
            if word != "" {
                kv_set("filter_word:" + word, true);
                return #{ cancel: true, reroute_server: "", send_message: "§a[ProxyChat] Added '" + word + "' to chat filter." };
            }
        }

        if action == "remove" && parts.len > 1 {
            let word = parts[1];
            word.trim();
            if word != "" {
                kv_remove("filter_word:" + word);
                return #{ cancel: true, reroute_server: "", send_message: "§a[ProxyChat] Removed '" + word + "' from chat filter." };
            }
        }

        return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /chatfilter <add|remove|list> [word]" };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) {
        return [];
    }
    let root_cmds = ["/g", "/global", "/msg", "/tell", "/w", "/whisper", "/r", "/reply", "/channeltoggle", "/clearchat", "/chatfilter"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    let msg_cmds = ["/msg ", "/tell ", "/w ", "/whisper "];
    for mc in msg_cmds {
        if cmd.starts_with(mc) {
            let arg = cmd[mc.len..cmd.len];
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

    if cmd.starts_with("/chatfilter ") {
        let sub = cmd[12..cmd.len];
        sub.trim();
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

    []
}
