// =============================================================================
// FrameMC - Server Switcher Plugin
// =============================================================================
// Provides /server command to list and transfer players between backend servers.

/// Invoked whenever a player issues a chat command (starting with '/').
///
/// Event payload:
/// - event.player_name: string
/// - event.command: string (e.g. "/server" or "/server steelmc")
/// - event.current_server: string (e.g. "lobby")
///
/// Returns: #{ cancel: bool, reroute_server: string, send_message: string }
fn on_player_command(event) {
    let cmd = event.command;
    let current = event.current_server;

    // 1. Handle /server (no arguments) -> List all servers and show current
    if cmd == "/server" {
        let servers = get_servers();
        let list_str = "";
        let i = 0;
        while i < servers.len {
            if i > 0 {
                list_str += "§7, ";
            }
            if servers[i] == current {
                list_str += "§a" + servers[i] + "§6*";
            } else {
                list_str += "§b" + servers[i];
            }
            i += 1;
        }

        let msg = "§6[FrameMC] §eYou are currently on: §a" + current + "\n§6[FrameMC] §eAvailable servers: " + list_str + "\n§7Use §f/server <name> §7to connect.";

        return #{
            cancel: true,
            reroute_server: "",
            send_message: msg
        };
    }

    // 2. Handle /server <target> -> Transfer to target server
    if cmd.starts_with("/server ") {
        let target = cmd[8..cmd.len];
        target.trim();

        if target == "" {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cUsage: /server <name>"
            };
        }

        if target == current {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cYou are already connected to " + target + "!"
            };
        }

        if !server_exists(target) {
            let servers = get_servers();
            let avail = "";
            let i = 0;
            while i < servers.len {
                if i > 0 {
                    avail += ", ";
                }
                avail += servers[i];
                i += 1;
            }
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cServer '" + target + "' does not exist. Available: " + avail
            };
        }

        proxy_info("Player " + event.player_name + " requested transfer to " + target);

        return #{
            cancel: true,
            reroute_server: target,
            send_message: "§aConnecting to " + target + "..."
        };
    }

    // 3. Convenience shortcuts: /hub, /lobby, /steel, plus dynamic /<server>
    if cmd == "/hub" || cmd == "/lobby" {
        let target = if server_exists("lobby") { "lobby" } else if server_exists("steelmc2") { "steelmc2" } else { "steelmc" };
        if current == target {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cYou are already connected to " + target + "!"
            };
        }
        return #{
            cancel: true,
            reroute_server: target,
            send_message: "§aConnecting to " + target + "..."
        };
    }

    if cmd == "/steel" {
        if current == "steelmc" {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cYou are already connected to SteelMC!"
            };
        }
        return #{
            cancel: true,
            reroute_server: "steelmc",
            send_message: "§aConnecting to SteelMC..."
        };
    }

    // Direct server command: e.g. /paper, /paper2, /steelmc, /steelmc2
    if cmd.starts_with("/") {
        let sname = cmd[1..cmd.len];
        if server_exists(sname) {
            if current == sname {
                return #{
                    cancel: true,
                    reroute_server: "",
                    send_message: "§cYou are already connected to " + sname + "!"
                };
            }
            return #{
                cancel: true,
                reroute_server: sname,
                send_message: "§aConnecting to " + sname + "..."
            };
        }
    }

    // Allow other commands to proceed to the backend server untouched
    #{
        cancel: false,
        reroute_server: "",
        send_message: ""
    }
}

/// Invoked when a player requests tab-completion for a command.
///
/// Event payload:
/// - event.player_name: string
/// - event.command: string (e.g. "/server " or "/serv")
/// - event.current_server: string (e.g. "steelmc")
///
/// Returns an array of string suggestions.
fn on_tab_complete(event) {
    let cmd = event.command;
    let servers = get_servers();

    if cmd.starts_with("/server ") {
        let arg = "";
        if cmd.len > 8 {
            arg = cmd[8..cmd.len];
        }
        let matches = [];
        for s in servers {
            if arg == "" || s.starts_with(arg) {
                matches.push(s);
            }
        }
        return matches;
    }

    if cmd == "/server" {
        return ["/server"];
    }

    if cmd.starts_with("/") {
        let prefix = cmd[1..cmd.len];
        let matches = [];
        let shortcuts = ["server", "hub", "lobby", "steel"];
        for s in servers {
            if !shortcuts.contains(s) {
                shortcuts.push(s);
            }
        }
        for sc in shortcuts {
            if prefix == "" || sc.starts_with(prefix) {
                matches.push("/" + sc);
            }
        }
        return matches;
    }

    []
}
