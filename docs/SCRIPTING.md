# Scripting FrameMC with Rhai

Writing Java plugins for simple proxy tasks has always felt like overkill. Compiling a fat JAR, wrestling with classloader isolation, and debugging third-party memory leaks just to redirect `/hub` or check a maintenance whitelist burns engineering time.

FrameMC embeds [Rhai](https://rhai.rs/), an embedded scripting language designed specifically for Rust. Scripts compile directly to abstract syntax trees in memory and execute inside strict runtime sandboxes. You can edit a `.rhai` script, test it immediately, and run it with native performance and zero JVM metaspace overhead.

---

## Script Sandboxing & Safety Limits

Because scripts execute within the Tokio connection loop, an unconstrained script could easily stall worker threads or consume excessive host memory. FrameMC locks the Rhai engine down with strict hardware-style limits:

- **Opcode Fuel Limit**: Capped at **50,000 operations** per hook call (`set_max_operations(50_000)`). If a script enters an accidental `while true` loop without an exit branch, the fuel gauge hits zero and Rhai halts immediately with an evaluation error. The connection drops back to safe default routing; the proxy process never freezes.
- **Recursion Call Depth**: Hard-clamped at **32 call frames** (`set_max_call_levels(32)`). Unbounded recursion fails fast before blowing the Tokio worker thread stack.
- **String Allocation Ceiling**: Limited to **1,024 bytes** per string (`set_max_string_size(1024)`). You can format chat messages and parse arguments, but scripts cannot trigger out-of-memory crashes by doubling strings in a loop.
- **Filesystem & Shell Isolation**: The module resolver is disabled (`DummyModuleResolver`). Scripts cannot call `import` to read arbitrary files from disk or execute external host binaries.

---

## Where Scripts Live

FrameMC loads scripts from two locations:

1. **`plugins_dir` (default: `plugins/`)**: 
   Scanned on startup. Any file ending in `.rhai` is compiled and loaded into memory in alphabetical order (e.g. `01_auth.rhai`, `02_server_switcher.rhai`).
2. **`script_path` (default: `scripts/main.rhai`)**: 
   The primary entrypoint script, loaded after directory plugins.

You can organize your logic across independent modular scripts in `plugins/` or keep everything consolidated in `scripts/main.rhai`.

---

## Event Hooks Reference

Scripts define one or more of the following lifecycle functions:

### 1. `on_player_join(event)`

Fires after the client finishes authentication (Mojang session check or offline UUID generation), right before the proxy connects them to a backend server.

#### Event Payload (`event` map)
| Field | Type | Description |
| :--- | :--- | :--- |
| `event.player_name` | `String` | Player's Minecraft username (e.g. `"Steve"`). |
| `event.uuid` | `String` | Hyphenated UUID string (e.g. `"069a79f4-44e9-4726-a5be-fca90e38aaf5"`). |
| `event.ip` | `String` | Remote client IP address without port (e.g. `"192.168.1.100"`). |
| `event.protocol_version` | `i64` | Protocol version integer (e.g. `765` for 1.20.4, `776` for 1.21.4). |

#### Return Value
Must return a map with this structure:
```rhai
#{
    allow: true,                 // Set to false to reject the player
    disconnect_reason: "",       // Kick message shown if allow is false
    target_server: ""            // Destination backend override (empty for default)
}
```

#### Evaluation Order
When multiple plugins define `on_player_join`:
- Plugins execute in alphabetical order, followed by `script_path`.
- If **any** script returns `allow: false`, execution stops immediately and the player is disconnected with that script's `disconnect_reason`.
- If a script sets `target_server`, that server becomes the target backend for downstream scripts (subsequent scripts can still override it).

---

### 2. `on_player_command(event)`

Intercepts chat commands (any chat message beginning with `/`) before they are dispatched to the backend server.

#### Event Payload (`event` map)
| Field | Type | Description |
| :--- | :--- | :--- |
| `event.player_name` | `String` | Username of the player executing the command. |
| `event.command` | `String` | Full command string including leading slash (e.g. `"/server survival"`). |
| `event.current_server` | `String` | Name of the backend server the player is currently on. |

#### Return Value
Must return a map with this structure:
```rhai
#{
    cancel: true,                // true prevents the command from reaching the backend
    reroute_server: "lobby",     // Initiates a server transfer to this backend (empty for none)
    send_message: "§aConnecting" // Text message sent to the player's chat (empty for none)
}
```

#### Evaluation Rules
- If a plugin returns `cancel: true` or sets `reroute_server`, the command is considered handled: subsequent scripts are skipped and the command is blocked from reaching the downstream backend.
- If no script cancels the command (`cancel: false`), the command passes through to the backend unmodified.

---

### 3. `on_tab_complete(event)`

Fires when a client hits the `Tab` key to auto-complete a command.

#### Event Payload (`event` map)
| Field | Type | Description |
| :--- | :--- | :--- |
| `event.player_name` | `String` | Username of the player requesting completions. |
| `event.command` | `String` | Text typed so far (e.g. `"/server su"`). |
| `event.current_server` | `String` | Name of the backend server the player is currently on. |

#### Return Value
Must return an array of string suggestions:
```rhai
["survival", "skyblock"]
```

Suggestions from all plugins are aggregated and delivered back to the client.

---

## Built-in Functions Reference

FrameMC exposes a curated set of thread-safe helper functions directly in the Rhai global scope:

### Shared In-Memory Key-Value Store
Because scripts execute across multiple connections concurrently, FrameMC provides a thread-safe in-memory store backed by `RwLock<HashMap<String, Dynamic>>` for sharing state (cooldowns, maintenance flags, session counts):

- `kv_set(key: String, value: Any)`: Stores a value under `key`. Supports strings, booleans, numbers, arrays, and maps.
- `kv_get(key: String) -> Any`: Retrieves the value for `key`. Returns unit `()` if the key does not exist.
- `kv_has(key: String) -> bool`: Checks whether `key` is present in the store.
- `kv_remove(key: String) -> bool`: Deletes `key`. Returns `true` if it existed.
- `kv_keys(prefix: String) -> Array`: Returns a sorted array of all keys starting with `prefix` (pass `""` for all keys).
- `kv_clear()`: Clears all keys from the store.

### Time & Timestamps
- `timestamp_sec() -> i64`: Current Unix timestamp in seconds.
- `timestamp_ms() -> i64`: Current Unix timestamp in milliseconds. Ideal for rate limiting and command cooldown math.

### Server Queries
- `get_servers() -> Array`: Returns an array of strings representing all backend servers declared in `config.toml` (e.g. `["lobby", "paper", "steelmc"]`).
- `get_default_server() -> String`: Returns the configured fallback/default backend name.
- `server_exists(name: String) -> bool`: Case-insensitive check whether `name` is a configured backend.

### Console Logging
Messages are dispatched directly to FrameMC's structured tracing subscriber:
- `proxy_info(message: String)`: Logs at `INFO` level (`[Rhai] ...`).
- `proxy_warn(message: String)`: Logs at `WARN` level (`[Rhai] ...`).
- `proxy_error(message: String)`: Logs at `ERROR` level (`[Rhai] ...`).

---

## Practical Script Examples

### Example 1: Robust Server Switcher (`plugins/server_switcher.rhai`)

Handles `/server <target>`, `/hub`, `/lobby`, and dynamic tab completion:

```rhai
fn on_player_command(event) {
    let cmd = event.command;

    // Handle /server <target>
    if cmd.starts_with("/server ") {
        let target = cmd[8..cmd.len];
        target.trim();

        if !server_exists(target) {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cServer '" + target + "' does not exist. Use /server to list."
            };
        }

        if target == event.current_server {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§eYou are already connected to " + target + "."
            };
        }

        return #{
            cancel: true,
            reroute_server: target,
            send_message: "§aTransferring to " + target + "..."
        };
    }

    // List servers on plain /server
    if cmd == "/server" {
        let list = get_servers();
        let msg = "§6Configured servers: §f" + list.to_string();
        return #{
            cancel: true,
            reroute_server: "",
            send_message: msg
        };
    }

    // Quick shortcuts
    if cmd == "/hub" || cmd == "/lobby" {
        let dest = get_default_server();
        if event.current_server == dest {
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§eYou are already in the lobby."
            };
        }
        return #{
            cancel: true,
            reroute_server: dest,
            send_message: "§aReturning to lobby..."
        };
    }

    // Pass everything else through to the backend
    #{ cancel: false, reroute_server: "", send_message: "" }
}

fn on_tab_complete(event) {
    let cmd = event.command;
    if cmd.starts_with("/server ") {
        let prefix = if cmd.len > 8 { cmd[8..cmd.len] } else { "" };
        let servers = get_servers();
        let matches = [];
        for s in servers {
            if prefix == "" || s.starts_with(prefix) {
                matches.push(s);
            }
        }
        return matches;
    }
    []
}
```

---

### Example 2: Maintenance Mode with Admin Whitelist (`plugins/maintenance.rhai`)

Toggle maintenance with `/maintenance on` and `/maintenance off`, allowing only whitelisted admins through:

```rhai
fn is_admin(player_name) {
    // Whitelisted administrator usernames
    player_name == "AdminDave" || player_name == "LeadDev"
}

fn on_player_join(event) {
    let maintenance = kv_get("maintenance_enabled");
    if maintenance == true && !is_admin(event.player_name) {
        return #{
            allow: false,
            disconnect_reason: "§cNetwork Maintenance In Progress\n§7We are currently updating. Check Discord for status.",
            target_server: ""
        };
    }

    #{ allow: true, disconnect_reason: "", target_server: "" }
}

fn on_player_command(event) {
    let cmd = event.command;

    if cmd.starts_with("/maintenance ") && is_admin(event.player_name) {
        let arg = cmd[13..cmd.len];
        arg.trim();

        if arg == "on" {
            kv_set("maintenance_enabled", true);
            proxy_warn("Maintenance mode ENABLED by " + event.player_name);
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§a[FrameMC] Maintenance mode is now §cENABLED§a."
            };
        }

        if arg == "off" {
            kv_set("maintenance_enabled", false);
            proxy_info("Maintenance mode DISABLED by " + event.player_name);
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§a[FrameMC] Maintenance mode is now §aDISABLED§a."
            };
        }
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}
```

---

### Example 3: Command Cooldowns / Spam Protection (`plugins/rate_limit.rhai`)

Prevents players from spamming heavy proxy commands using timestamps and the in-memory key-value store:

```rhai
fn on_player_command(event) {
    let cmd = event.command;

    // Apply a 3-second cooldown to /server switches
    if cmd.starts_with("/server ") {
        let key = "cooldown:" + event.player_name;
        let now = timestamp_ms();
        let last = kv_get(key);

        if last != () && (now - last) < 3000 {
            let remaining = (3000 - (now - last)) / 1000 + 1;
            return #{
                cancel: true,
                reroute_server: "",
                send_message: "§cPlease wait " + remaining + "s before switching servers again."
            };
        }

        // Update cooldown timestamp
        kv_set(key, now);
    }

    #{ cancel: false, reroute_server: "", send_message: "" }
}
```

---

### Example 4: Version-Based Dynamic Routing (`plugins/version_routing.rhai`)

Direct players to specific backends based on their Minecraft protocol version:

```rhai
fn on_player_join(event) {
    let proto = event.protocol_version;

    // 765 = Minecraft 1.20.4
    // 776 = Minecraft 1.21.4
    if proto >= 776 && server_exists("modern_1_21") {
        return #{
            allow: true,
            disconnect_reason: "",
            target_server: "modern_1_21"
        };
    }

    if proto <= 765 && server_exists("legacy_1_20") {
        return #{
            allow: true,
            disconnect_reason: "",
            target_server: "legacy_1_20"
        };
    }

    // Default to standard lobby
    #{ allow: true, disconnect_reason: "", target_server: "lobby" }
}
```

---

## Practical Tips for Rhai Scripts

- **Keep hooks fast**: Scripts run directly in Tokio's connection handler. Avoid heavy loops or complex string manipulation. Do quick checks, inspect maps, and return.
- **Use section sign formatting**: Minecraft chat components support classic `§` color codes (`§a` green, `§c` red, `§e` yellow, `§7` gray, `§f` white, `§l` bold, `§r` reset).
- **Prefix key-value keys**: To prevent state collisions between multiple plugins, namespace your keys (e.g. `"party:" + id`, `"auth:" + uuid`).
- **Use structured logs**: Call `proxy_warn` or `proxy_error` when an unexpected edge case occurs so admins can spot issues directly in server terminal output.
