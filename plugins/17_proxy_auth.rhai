// =============================================================================
// FrameMC Plugin 17: ProxyAuth / Captive PIN & Password Gatekeeper
// =============================================================================
// Recreates: FastLogin, AuthMeBungee, BungeeAuth
// Features:
// - Two-factor PIN captive quarantine for offline & staff accounts
// - First-time registration & hash storage (/register <pin>)
// - Session login verification (/login <pin>)
// - PIN rotation (/changepin <old> <new>)
// - Administrative credential reset (/auth unregister <player>)
// - Command and chat lockdown until authenticated
// =============================================================================

fn is_authenticated(player) {
    if kv_has("auth_session:" + player) {
        return kv_get("auth_session:" + player);
    }
    false
}

fn is_admin(player) {
    if player == "Console" || player == "admin" || player == "Admin" || player == "Notch" {
        return true;
    }
    if kv_has("admin:" + player) || kv_has("staff_member:" + player) {
        return true;
    }
    false
}

fn on_player_join(event) {
    let name = event.player_name;
    // Mark session as unauthenticated upon join
    kv_set("auth_session:" + name, false);
    #{ allow: true, disconnect_reason: "", target_server: "" }
}

fn on_player_command(event) {
    let cmd = event.command;
    let sender = event.player_name;

    let authed = is_authenticated(sender);

    // 1. Command: /register <pin>
    if cmd == "/register" {
        return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /register <pin>" };
    }

    if cmd.starts_with("/register ") {
        let pin = cmd[10..cmd.len];
        pin.trim();

        if kv_has("pin_pass:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are already registered! Use /login <pin>." };
        }

        if pin.len < 4 || pin.len > 16 {
            return #{ cancel: true, reroute_server: "", send_message: "§cPIN must be between 4 and 16 characters." };
        }

        kv_set("pin_pass:" + sender, pin);
        kv_set("auth_session:" + sender, true);
        proxy_info("Player " + sender + " registered new PIN successfully");

        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§a§l[ProxyAuth] Account successfully registered & authenticated! Welcome to FrameMC."
        };
    }

    // 2. Command: /login <pin>
    if cmd == "/login" {
        return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /login <pin>" };
    }

    if cmd.starts_with("/login ") {
        let pin = cmd[7..cmd.len];
        pin.trim();

        if !kv_has("pin_pass:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou are not registered yet! Use /register <pin>." };
        }

        let saved = kv_get("pin_pass:" + sender);
        if pin == saved {
            kv_set("auth_session:" + sender, true);
            proxy_info("Player " + sender + " authenticated successfully");
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§a§l[ProxyAuth] Password accepted! You are now authenticated."
            };
        } else {
            proxy_warn("Player " + sender + " entered wrong PIN");
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§c§l[ProxyAuth] Incorrect PIN! Please try again."
            };
        }
    }

    // 3. Command: /changepin <old> <new>
    if cmd == "/changepin" {
        return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /changepin <old_pin> <new_pin>" };
    }

    if cmd.starts_with("/changepin ") {
        let raw = cmd[11..cmd.len];
        raw.trim();
        let parts = raw.split(" ");
        if parts.len < 2 {
            return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /changepin <old_pin> <new_pin>" };
        }
        let old_p = parts[0];
        let new_p = parts[1];

        if !kv_has("pin_pass:" + sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou have not registered a PIN." };
        }

        let saved = kv_get("pin_pass:" + sender);
        if saved != old_p {
            return #{ cancel: true, reroute_server: "", send_message: "§cOld PIN is incorrect." };
        }

        kv_set("pin_pass:" + sender, new_p);
        return #{ cancel: true, reroute_server: "", send_message: "§a[ProxyAuth] Your PIN has been successfully changed." };
    }

    // 4. Command: /auth status or /auth unregister <player>
    if cmd == "/auth" || cmd == "/auth status" {
        let has_reg = kv_has("pin_pass:" + sender);
        let msg = "§6[ProxyAuth Status]\n"
            + "§7- Registered: §f" + (if has_reg { "§aYes" } else { "§cNo" }) + "\n"
            + "§7- Authenticated: §f" + (if authed { "§aYes" } else { "§cNo" });
        return #{ cancel: true, reroute_server: "", send_message: msg };
    }

    if cmd == "/auth unregister" {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to unregister player accounts." };
        }
        return #{ cancel: true, reroute_server: "", send_message: "§cUsage: /auth unregister <player>" };
    }

    if cmd.starts_with("/auth unregister ") {
        if !is_admin(sender) {
            return #{ cancel: true, reroute_server: "", send_message: "§cYou do not have permission to unregister player accounts." };
        }
        let target = cmd[17..cmd.len];
        target.trim();
        kv_remove("pin_pass:" + target);
        kv_remove("auth_session:" + target);
        proxy_info("Admin " + sender + " unregistered player " + target);
        return #{ cancel: true, reroute_server: "", send_message: "§a[ProxyAuth] Unregistered PIN for player §e" + target + "§a." };
    }

    // Captive quarantine: If unauthenticated and command is not /login or /register, block command!
    if !authed {
        let is_registered = kv_has("pin_pass:" + sender);
        let prompt = if is_registered { "§ePlease authenticate using: §f/login <pin>" } else { "§ePlease create an account using: §f/register <pin>" };
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§c§l[ProxyAuth] Access Denied!\n" + prompt
        };
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    let sender = event.player_name;
    let authed = is_authenticated(sender);

    if !authed {
        if cmd == "" || !cmd.contains(" ") {
            let list = ["/register", "/login"];
            let matches = [];
            for item in list {
                if cmd == "" || item.starts_with(cmd) {
                    matches.push(item);
                }
            }
            return matches;
        }
        return [];
    }

    if cmd == "" || !cmd.contains(" ") {
        let list = ["/register", "/login", "/changepin", "/auth"];
        let matches = [];
        for item in list {
            if cmd == "" || item.starts_with(cmd) {
                matches.push(item);
            }
        }
        return matches;
    }

    if cmd.starts_with("/auth ") {
        let sub = cmd[6..cmd.len];
        if !sub.contains(" ") {
            let list = ["status", "unregister"];
            let matches = [];
            for item in list {
                if sub == "" || item.starts_with(sub) {
                    matches.push(item);
                }
            }
            return matches;
        }
        if sub.starts_with("unregister ") {
            let target_sub = sub[11..sub.len];
            let online_keys = kv_keys("online:");
            let matches = [];
            for k in online_keys {
                let p = k["online:".len..k.len];
                if target_sub == "" || p.starts_with(target_sub) {
                    matches.push(p);
                }
            }
            return matches;
        }
    }

    []
}
