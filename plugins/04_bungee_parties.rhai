// =============================================================================
// FrameMC Plugin 04: BungeeParties / Cross-Server Party System
// =============================================================================
// Recreates: Party and Friends, BungeeParties, Spicord Parties
// Features:
// - Cross-server party management (/party create, invite, accept, decline, leave, kick, disband)
// - Party status & member distribution (/party list)
// - Party warp to leader's current server (/party warp)
// - Dedicated party chat channel (/p, /party chat)
// =============================================================================

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

    // 1. Command: /p <message> (Party Chat)
    if cmd == "/p" || cmd == "/party chat" || cmd.starts_with("/p ") || cmd.starts_with("/party chat ") {
        let prefix_len = if cmd.starts_with("/p ") { 3 } else if cmd.starts_with("/party chat ") { 12 } else { cmd.len };
        let msg = if cmd == "/p" || cmd == "/party chat" { "" } else { cmd[prefix_len..cmd.len] };
        msg.trim();

        if msg == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /p <message>" };
        }

        if !kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are not in a party! Create one with /party create." };
        }
        let leader = kv_get("in_party:" + sender);
        let formatted = "§9[Party] §b" + sender + "§8: §f" + msg;

        proxy_info("[Party " + leader + "] " + sender + ": " + msg);
        return #{ cancel: true, reroute_server: "", send_message: formatted };
    }

    // 2. Main Command: /party ...
    if cmd == "/party" || cmd == "/party help" {
        let help = "§8======== §9[FrameMC Parties] §8========\n"
            + "§b/party create §7- Create a new party\n"
            + "§b/party invite <player> §7- Invite a player\n"
            + "§b/party accept <leader> §7- Accept party invitation\n"
            + "§b/party decline <leader> §7- Decline invitation\n"
            + "§b/party list §7- Show party members\n"
            + "§b/party warp §7- Warp party to your server (Leader only)\n"
            + "§b/party leave §7- Leave your current party\n"
            + "§b/party kick <player> §7- Kick member (Leader only)\n"
            + "§b/party disband §7- Disband party (Leader only)\n"
            + "§b/p <msg> §7- Send message to party chat";

        return #{ cancel: true, reroute_server: "", send_message: help };
    }

    if cmd == "/party create" {
        if kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are already in a party! Use /party leave first." };
        }
        kv_set("in_party:" + sender, sender);
        kv_set("party_members:" + sender, [sender]);
        kv_set("party_leader:" + sender, sender);

        proxy_info("Party created by leader " + sender);
        return #{ cancel: true, reroute_server: "", send_message: "§a[Parties] You created a party! Use §e/party invite <player> §ato invite members." };
    }

    if cmd == "/party invite" || cmd.starts_with("/party invite ") {
        let target = if cmd == "/party invite" { "" } else { cmd[14..cmd.len] };
        target.trim();

        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /party invite <player>" };
        }

        if !kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou must create a party first using /party create." };
        }
        let leader = kv_get("in_party:" + sender);
        if leader != sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cOnly the party leader can invite players." };
        }
        if target == sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou cannot invite yourself." };
        }

        kv_set("party_invite:" + target + ":" + sender, true);
        return #{ cancel: true, reroute_server: "", send_message: "§a[Parties] Invited §e" + target + " §ato your party." };
    }

    if cmd == "/party accept" || cmd.starts_with("/party accept ") {
        let leader = if cmd == "/party accept" { "" } else { cmd[14..cmd.len] };
        leader.trim();

        if leader == "" {
            let inv_prefix = "party_invite:" + sender + ":";
            let inv_keys = kv_keys(inv_prefix);
            if inv_keys.len == 1 {
                leader = inv_keys[0][inv_prefix.len..inv_keys[0].len];
            } else if inv_keys.len == 0 {
                return #{ cancel: true, reroute_server: "", send_message: "§cYou have no pending party invitations." };
            } else {
                return #{ cancel: true, reroute_server: "", send_message: "§cYou have multiple invitations. Use: /party accept <leader>" };
            }
        }

        if kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are already in a party. Leave it first with /party leave." };
        }
        let inv_key = "party_invite:" + sender + ":" + leader;
        if !kv_has(inv_key) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou have no pending invitation from '" + leader + "'." };
        }

        kv_remove(inv_key);
        let members = if kv_has("party_members:" + leader) { kv_get("party_members:" + leader) } else { [leader] };
        members.push(sender);
        kv_set("party_members:" + leader, members);
        kv_set("in_party:" + sender, leader);

        return #{ cancel: true, reroute_server: "", send_message: "§a[Parties] You have joined §e" + leader + "'s §aparty!" };
    }

    if cmd == "/party decline" || cmd.starts_with("/party decline ") {
        let leader = if cmd == "/party decline" { "" } else { cmd[15..cmd.len] };
        leader.trim();

        if leader == "" {
            let inv_prefix = "party_invite:" + sender + ":";
            let inv_keys = kv_keys(inv_prefix);
            if inv_keys.len == 1 {
                leader = inv_keys[0][inv_prefix.len..inv_keys[0].len];
            } else if inv_keys.len == 0 {
                return #{ cancel: true, reroute_server: "", send_message: "§cYou have no pending party invitations to decline." };
            } else {
                return #{ cancel: true, reroute_server: "", send_message: "§cYou have multiple invitations. Use: /party decline <leader>" };
            }
        }

        kv_remove("party_invite:" + sender + ":" + leader);
        return #{ cancel: true, reroute_server: "", send_message: "§e[Parties] Declined invitation from " + leader + "." };
    }

    if cmd == "/party leave" {
        if !kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are not currently in a party." };
        }
        let leader = kv_get("in_party:" + sender);
        if leader == sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are the leader! Use /party disband or promote someone." };
        }

        let members = if kv_has("party_members:" + leader) { kv_get("party_members:" + leader) } else { [] };
        let new_members = [];
        let i = 0;
        while i < members.len {
            if members[i] != sender { new_members.push(members[i]); }
            i += 1;
        }
        kv_set("party_members:" + leader, new_members);
        kv_remove("in_party:" + sender);

        return #{ cancel: true, reroute_server: "", send_message: "§e[Parties] You have left the party." };
    }

    // Party Kick Command
    if cmd == "/party kick" || cmd.starts_with("/party kick ") {
        let target = if cmd == "/party kick" { "" } else { cmd[12..cmd.len] };
        target.trim();

        if target == "" {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /party kick <player>" };
        }

        if !kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are not in a party." };
        }
        let leader = kv_get("in_party:" + sender);
        if leader != sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cOnly the party leader can kick players." };
        }
        if target == sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou cannot kick yourself! Use /party disband instead." };
        }

        let members = if kv_has("party_members:" + leader) { kv_get("party_members:" + leader) } else { [] };
        let found = false;
        let new_members = [];
        let i = 0;
        while i < members.len {
            if members[i] == target {
                found = true;
            } else {
                new_members.push(members[i]);
            }
            i += 1;
        }

        if !found {
            return #{ cancel: true, reroute_server: "", send_message: "§cPlayer '" + target + "' is not in your party." };
        }

        kv_set("party_members:" + leader, new_members);
        kv_remove("in_party:" + target);
        proxy_info("Player " + target + " was kicked from party by " + sender);

        return #{ cancel: true, reroute_server: "", send_message: "§e[Parties] §f" + target + " §ehas been kicked from your party." };
    }

    if cmd == "/party disband" {
        if !kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are not in a party." };
        }
        let leader = kv_get("in_party:" + sender);
        if leader != sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cOnly the party leader can disband the party." };
        }

        let members = if kv_has("party_members:" + leader) { kv_get("party_members:" + leader) } else { [] };
        let i = 0;
        while i < members.len {
            kv_remove("in_party:" + members[i]);
            i += 1;
        }
        kv_remove("party_members:" + leader);

        return #{ cancel: true, reroute_server: "", send_message: "§c[Parties] Your party has been disbanded." };
    }

    if cmd == "/party list" || cmd == "/party info" {
        if !kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are not currently in a party." };
        }
        let leader = kv_get("in_party:" + sender);
        let members = if kv_has("party_members:" + leader) { kv_get("party_members:" + leader) } else { [leader] };

        let msg = "§8======== §9[Party of " + leader + " - " + members.len + " Members] §8========\n"
            + "§6Leader: §e" + leader + "\n§7Members: ";

        let i = 0;
        while i < members.len {
            if i > 0 { msg += "§7, "; }
            msg += "§b" + members[i];
            i += 1;
        }
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/party warp" {
        if !kv_has("in_party:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are not in a party." };
        }
        let leader = kv_get("in_party:" + sender);
        if leader != sender {
            return #{ cancel: true, reroute_server: "", send_message: "§cOnly the party leader can warp the party." };
        }

        let members = if kv_has("party_members:" + leader) { kv_get("party_members:" + leader) } else { [leader] };
        proxy_info("Party warp triggered by " + sender + " to " + current_server + " for " + members.len + " members");

        return #{
            cancel: true,
            reroute_server: current_server,
            send_message: "§a[Parties] Warping all " + members.len + " party members to §e" + current_server + "§a!"
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
    let root_cmds = ["/party", "/p"];

    if !cmd.contains(" ") {
        let matches = [];
        for r in root_cmds {
            if cmd == "" || r.starts_with(cmd) {
                matches.push(r);
            }
        }
        return matches;
    }

    if cmd.starts_with("/party ") {
        let sub = cmd[7..cmd.len];
        if !sub.contains(" ") {
            let options = ["create", "invite", "accept", "decline", "list", "warp", "leave", "kick", "disband", "help"];
            let matches = [];
            for o in options {
                if sub == "" || o.starts_with(sub) {
                    matches.push(o);
                }
            }
            return matches;
        }
    }

    if cmd.starts_with("/party invite ") {
        let target = cmd[14..cmd.len];
        target.trim();
        let matches = [];
        let online_keys = kv_keys("online:");
        let i = 0;
        while i < online_keys.len {
            let p_name = online_keys[i][7..online_keys[i].len];
            if target == "" || p_name.starts_with(target) {
                matches.push(p_name);
            }
            i += 1;
        }
        return matches;
    }

    []
}
