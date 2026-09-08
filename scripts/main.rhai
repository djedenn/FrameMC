// =============================================================================
// FrameMC - Default Routing & Interception Script
// =============================================================================

/// Invoked whenever a player successfully completes authentication and is ready
/// to be routed to a backend server.
///
/// Event payload:
/// - event.player_name: string
/// - event.uuid: string
/// - event.ip: string
/// - event.protocol_version: int
///
/// Returns: #{ allow: bool, disconnect_reason: string, target_server: string }
fn on_player_join(event) {
    proxy_info("Player " + event.player_name + " joined from " + event.ip + " (protocol " + event.protocol_version + ")");

    #{
        allow: true,
        disconnect_reason: "",
        target_server: ""
    }
}

/// Invoked whenever a player issues a chat command (beginning with '/').
///
/// Event payload:
/// - event.player_name: string
/// - event.command: string
///
/// Returns: #{ cancel: bool, reroute_server: string, send_message: string }
fn on_player_command(event) {
    let cmd = event.command;
    proxy_info("Player " + event.player_name + " issued command: " + cmd);

    // Route /hub or /lobby to the lobby backend
    if cmd == "/hub" || cmd == "/lobby" {
        return #{
            cancel: true,
            reroute_server: "lobby",
            send_message: "§aConnecting to lobby..."
        };
    }

    // Route /steel to the steelmc backend
    if cmd == "/steel" {
        return #{
            cancel: true,
            reroute_server: "steelmc",
            send_message: "§aConnecting to SteelMC..."
        };
    }

    // Guard against unauthorized administrative commands
    if cmd == "/stop" || cmd == "/op" {
        return #{
            cancel: true,
            reroute_server: "",
            send_message: "§cYou do not have permission to execute this command."
        };
    }

    // Default: allow command to proceed to the current downstream backend
    #{
        cancel: false,
        reroute_server: "",
        send_message: ""
    }
}
