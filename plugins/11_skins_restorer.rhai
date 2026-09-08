// =============================================================================
// FrameMC Plugin 11: SkinsRestorer / Proxy Skin Management
// =============================================================================
// Recreates: SkinsRestorer (BungeeCord / Velocity edition)
// Features:
// - Custom skin selection by username (/skin <name>)
// - Texture URL application (/skin url <url>)
// - Skin reset & clear (/skin clear)
// - Skin cache refresh & status inspection (/skin update, /skin info)
// - Cross-server skin profile persistence
// =============================================================================

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

    if cmd == "/skin" || cmd == "/skin help" {
        let help = "§8======== §a[FrameMC SkinsRestorer] §8========\n"
            + "§e/skin <name> §7- Apply skin of another player\n"
            + "§e/skin url <url> §7- Apply skin via texture URL\n"
            + "§e/skin clear §7- Reset to your default Minecraft skin\n"
            + "§e/skin update §7- Refresh your active skin cache\n"
            + "§e/skin info [player] §7- Check current skin configuration";
        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    if cmd == "/skin url" || cmd.starts_with("/skin url ") {
        let url = if cmd == "/skin url" { "" } else { cmd[10..cmd.len] };
        url.trim();
        if url == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /skin url <https://...>" };
        }
        kv_set("skin:" + sender, #{
            type: "url",
            value: url,
            updated_at: timestamp_sec()
        });
        proxy_info("Skin updated to URL for " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[SkinsRestorer] Custom skin URL applied! Reconnect to view changes." };
    }

    if cmd == "/skin clear" {
        kv_remove("skin:" + sender);
        proxy_info("Skin reset to default for " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[SkinsRestorer] Your skin has been reset to default." };
    }

    if cmd == "/skin update" {
        let skin = if kv_has("skin:" + sender) { kv_get("skin:" + sender) } else { () };
        if skin == () {
            return #{ cancel: true, reroute_server: "", send_message: "§e[SkinsRestorer] You are currently using your default skin." };
        }
        return #{ cancel: true, reroute_server: "", send_message: "§a[SkinsRestorer] Skin cache successfully refreshed!" };
    }

    if cmd == "/skin info" || cmd.starts_with("/skin info ") {
        let target = sender;
        if cmd.starts_with("/skin info ") {
            let t = cmd[11..cmd.len];
            t.trim();
            if t != "" { target = t; }
        }
        let skin = if kv_has("skin:" + target) { kv_get("skin:" + target) } else { () };
        if skin == () {
            return #{ cancel: true, reroute_server: "", send_message: "§7[SkinsRestorer] Player '§f" + target + "§7' is using standard default skin." };
        }
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§6[SkinsRestorer] Skin Profile for §f" + target + "§6:\n§7- Type: §e" + skin.type + "\n§7- Value: §b" + skin.value
        };
    }

    // Default /skin <name>
    if cmd.starts_with("/skin ") {
        let target_skin = cmd[6..cmd.len];
        target_skin.trim();
        if target_skin == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /skin <name>" };
        }

        kv_set("skin:" + sender, #{
            type: "player",
            value: target_skin,
            updated_at: timestamp_sec()
        });
        proxy_info("Player " + sender + " changed skin to: " + target_skin);

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a[SkinsRestorer] Your skin has been set to §b" + target_skin + "§a! Relog or switch servers to view."
        };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    if is_quarantined(sender) {
        return [];
    }
    let root_cmds = ["/skin"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/skin ") {
        let sub = cmd[6..cmd.len];
        if !sub.contains(" ") {
            let options = ["clear", "update", "info", "url", "Notch", "Dinnerbone", "Technoblade", "Jeb_", "Steve", "Alex"];
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
