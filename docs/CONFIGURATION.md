# FrameMC Configuration Guide

FrameMC stores all its settings in a single `config.toml` file located in the current working directory. There are no XML schemas, no nested YAML indentation traps, and no external database connections to manage.

If no configuration file exists when the binary starts, FrameMC automatically writes a working starter configuration bound to `0.0.0.0:25565` with fallback routes to local backends.

---

## Complete `config.toml` Example

Below is a complete `config.toml` showing every available configuration directive with standard defaults:

```toml
# =============================================================================
# Network Binding
# =============================================================================

# IP interface to bind. "0.0.0.0" listens on all IPv4 interfaces.
# Use "127.0.0.1" if fronting with HAProxy or a local tunnel.
bind_address = "0.0.0.0"

# Listening TCP port (standard Minecraft port is 25565)
bind_port = 25565

# =============================================================================
# Server List Ping (Status)
# =============================================================================

# Message of the day shown in the multiplayer server list.
# Supports legacy section sign color codes (§a, §b, §c, etc.).
motd = "§aFrameMC §7Reverse Proxy §8| §fHigh Throughput"

# Maximum player count advertised to clients during status pings.
max_players = 1000

# Server list icon. Can be a relative path to a 64x64 PNG file,
# or a raw base64 data URI ("data:image/png;base64,...").
# If omitted, FrameMC checks for a file named "server-icon.png" in the
# working directory and loads it automatically.
favicon = "server-icon.png"

# =============================================================================
# Authentication & Identity
# =============================================================================

# Online mode controls how connecting players are authenticated:
# - true:  Authenticates with Mojang's session servers. Generates an RSA-1024
#          keypair per proxy process, verifies shared secrets, and enables
#          AES-128-CFB8 stream encryption across the client TCP connection.
# - false: Offline mode. Skips Mojang auth and encryption. Player UUIDs are
#          derived deterministically using standard offline MD5 hashing (UUID v3).
online_mode = true

# Optional: Override the Mojang session server URL.
# Useful for private authentication servers, mock testing, or staging proxies.
# If omitted or commented out, defaults to Mojang's official endpoint:
# https://sessionserver.mojang.com/session/minecraft/hasJoined
# session_server_url = "https://sessionserver.mojang.com/session/minecraft/hasJoined"

# =============================================================================
# Routing & Rhai Scripting
# =============================================================================

# Name of the fallback / default backend server. Newly connected clients are
# routed here unless an on_player_join script redirects them elsewhere.
default_server = "lobby"

# Directory scanned for drop-in Rhai plugins (*.rhai).
# Files in this directory are loaded alphabetically on startup.
plugins_dir = "plugins"

# Path to the primary Rhai script. Executed after directory plugins are loaded.
script_path = "scripts/main.rhai"

# =============================================================================
# Backend Server Definitions
# =============================================================================

# Paper / Purpur / Folia backend using modern Velocity HMAC-SHA256 forwarding
[servers.paper]
address = "127.0.0.1"
port = 25568
forwarding_mode = "velocity_modern"
forwarding_secret = "framemc_secret_velocity_2026"

# Spigot / CraftBukkit backend using legacy BungeeCord handshake host appending
[servers.spigot]
address = "127.0.0.1"
port = 25570
forwarding_mode = "legacy_bungee"

# SteelMC (high-performance native Rust Minecraft server)
[servers.steelmc]
address = "127.0.0.1"
port = 25567
forwarding_mode = "none"

# Default lobby / fallback hub
[servers.lobby]
address = "127.0.0.1"
port = 25566
forwarding_mode = "none"
```

---

## Configuration Keys Reference

### Top-Level Directives

| Key | Type | Default | Usage Notes |
| :--- | :--- | :--- | :--- |
| `bind_address` | String | `"0.0.0.0"` | Network interface to bind. Keep `"0.0.0.0"` to accept public traffic on all interfaces. Use `"127.0.0.1"` if you place FrameMC behind HAProxy or a local port forwarder. |
| `bind_port` | Integer | `25565` | TCP port for incoming client connections. On Linux, binding ports below 1024 requires root or granting `CAP_NET_BIND_SERVICE` to the binary. |
| `motd` | String | `"§aFrameMC..."` | Server list description text component. Supports classic `§` section formatting codes. |
| `max_players` | Integer | `1000` | Display count shown in the client multiplayer menu. This is purely visual and does not reject connections when exceeded. |
| `online_mode` | Boolean | `true` | When `true`, enforces official Mojang authentication and AES-128-CFB8 stream encryption. Set to `false` for offline or LAN test networks where UUIDs are derived deterministically via MD5 (UUID v3). |
| `favicon` | String (optional) | None | File path to a 64x64 PNG image or raw `data:image/png;base64,...` string. When omitted, FrameMC looks for `server-icon.png` in the current folder. |
| `session_server_url` | String (optional) | None | Custom session verification URL. Primarily used for private auth implementations and mock integration tests. |
| `default_server` | String | `"lobby"` | Backend where authenticated players land on initial join, and where players are redirected if their active backend crashes. |
| `plugins_dir` | String | `"plugins"` | Directory scanned for modular `.rhai` script plugins. Automatically created if missing. |
| `script_path` | String | `"scripts/main.rhai"` | Path to the main event script executed on joins, commands, and tab completions. |

### Backend Definitions (`[servers.<name>]`)

Each downstream server is configured under a `[servers.<name>]` table header. The table key (`<name>`) becomes the identifier used in `/server <name>`, Rhai routing hooks, and proxy logs.

| Key | Type | Default | Usage Notes |
| :--- | :--- | :--- | :--- |
| `address` | String | *Required* | Hostname or IP address of the downstream Minecraft server. |
| `port` | Integer | *Required* | TCP port of the downstream Minecraft server. |
| `forwarding_mode` | String | `"none"` | Forwarding strategy. Accepted values: `"velocity_modern"` (or `"modern"` / `"velocity"`), `"legacy_bungee"` (or `"legacy"` / `"bungee"`), `"none"`. |
| `forwarding_secret` | String (optional) | None | Shared HMAC secret token. Required for `velocity_modern`; ignored for other forwarding modes. |

---

## Routing Mechanics & Failover

### Join Routing Precedence
When a player completes login authentication, FrameMC chooses their initial destination through this sequence:

1. FrameMC executes `on_player_join` across all plugins in `plugins_dir` (alphabetical order), followed by `script_path`.
2. If any script returns `allow: false`, the handshake aborts immediately and the player sees the returned `disconnect_reason`.
3. If a script sets `target_server` (e.g. `target_server: "survival"`), that backend becomes the destination.
4. If no script specifies an override, the player routes to `default_server`.

### Mid-Game World Transfers
When a player runs `/server <target>` or a script initiates a transfer:

1. **Independent TCP Handshake**: FrameMC opens a new connection to the target backend and completes the Handshake and Login phases in the background without disturbing the active client socket.
2. **Configuration Phase Caching**: In Minecraft 1.20.2+, servers exchange registry codecs (`minecraft:dimension_type`, biomes, damage types) during a dedicated Configuration phase. FrameMC intercepts these packets, updates the session cache, and sends `FinishConfiguration` back to the backend.
3. **World Respawn Synthesis**: FrameMC crafts a clientbound `Respawn` packet containing the target backend's dimension attributes, world coordinates, and gamemode, delivering it to the client. The client renders the new world immediately without triggering a disconnect screen.
4. **Socket Splicing**: Sockets transition directly to `tokio::io::copy_bidirectional`.

### Backend Crash Interception
If an active backend crashes, restarts, or terminates its TCP stream mid-game:

- Most proxies pass the backend's `Disconnect` packet through to the player, dumping them to the title screen.
- **FrameMC intercepts this packet**. It suppresses the disconnect screen, initiates an immediate transfer sequence to `default_server` (fallback lobby), sends a clientbound chat notice (`"§cLost connection to backend server. Reconnecting to lobby..."`), and keeps the player connected to the proxy.

---

## Compression Decoupling & Tuning

Mismatched packet compression is one of the most common failure modes in Minecraft networks.

### The Problem with Mismatched Thresholds
Suppose your lobby server runs with compression disabled (`network-compression-threshold: -1`), but your survival server enables compression (`network-compression-threshold: 256`).

In a naive proxy, when a player moves from lobby to survival, the survival backend sends deflated packets preceded by a VarInt uncompressed size. But the client socket is still configured for raw, uncompressed packets. The client's protocol parser immediately crashes with `DecoderException: Badly compressed packet`.

### FrameMC's Independent Compression Tracking
FrameMC decouples compression contexts:

- `client_compression_threshold`: The threshold negotiated between the Minecraft client and FrameMC during initial login.
- `backend_compression_threshold`: The threshold declared by the active downstream backend via the `SetCompression (0x03)` login packet.

During transitions:
- If client and backend thresholds match, packets pass through directly.
- If they differ, FrameMC's framing layer strips or adds the zlib envelope dynamically.
- Once both sides align, the session transitions to the uninspected `Play` bridge where raw kernel socket splicing takes over.

### Decompression Bomb Protection
Malformed or hostile compressed packets can declare a massive uncompressed length in their VarInt header to cause heap exhaustion. FrameMC clamps all inflate allocations and drops any packet claiming an uncompressed size greater than 2 MiB (`DEFAULT_MAX_PACKET_SIZE = 2 * 1024 * 1024`) before allocating memory.

---

## Connection Timeouts

FrameMC enforces deterministic timeouts across the connection lifecycle to prevent slowloris connection-slot exhaustion:

- **Initial Handshake & Login**: 10 seconds. If a client connects but fails to send valid Handshake and LoginStart packets within 10 seconds, the socket is dropped.
- **Encryption Handshake**: 10 seconds. Awaits the client's `EncryptionResponse` packet.
- **Mojang Authentication**: 10 seconds. Outbound HTTPS request to `sessionserver.mojang.com`. If Mojang's API stalls, the proxy times out and informs the player cleanly.
- **Server Switching Negotiation**: 5 seconds. If a target backend accepts a TCP connection but stalls during the Login or Configuration phase (for example, when frozen by heavy world generation), FrameMC cancels the transfer, informs the player in chat, and keeps them on their current server.
- **Status Ping**: 10 seconds for the initial status query, followed by an immediate graceful socket shutdown after delivering the JSON response and favicon.

---

## Backend Configuration Examples

### 1. Paper / Purpur / Folia (Modern Velocity Forwarding)

Velocity modern forwarding is the recommended forwarding strategy. It transfers real player UUIDs, skins, textures, and client IP addresses wrapped in an HMAC-SHA256 signature, preventing IP spoofing without modifying the handshake host string.

#### FrameMC (`config.toml`):
```toml
[servers.paper]
address = "127.0.0.1"
port = 25568
forwarding_mode = "velocity_modern"
forwarding_secret = "make_this_a_64_char_random_hex_string"
```

#### Paper (`config/paper-global.yml`):
```yaml
proxies:
  velocity:
    enabled: true
    online-mode: true
    secret: "make_this_a_64_char_random_hex_string"
```

#### Paper (`server.properties`):
```properties
online-mode=false
server-port=25568
```

> **Note**: The `forwarding_secret` must match character-for-character between both files.

---

### 2. Spigot / CraftBukkit (Legacy BungeeCord Forwarding)

For legacy Spigot servers that do not support plugin-message player forwarding:

#### FrameMC (`config.toml`):
```toml
[servers.spigot]
address = "127.0.0.1"
port = 25570
forwarding_mode = "legacy_bungee"
```

#### Spigot (`spigot.yml`):
```yaml
settings:
  bungeecord: true
```

#### Spigot (`server.properties`):
```properties
online-mode=false
server-port=25570
```

FrameMC rewrites the initial Handshake host field to:
```text
<original_host>\0<client_ip>\0<client_uuid>
```
Spigot parses this null-delimited string to assign the player their authentic UUID and remote IP address.

---

### 3. Fabric / Quilt (Modded Backends)

Fabric servers do not include proxy forwarding in vanilla code. Install the lightweight [FabricProxy-Lite](https://modrinth.com/mod/fabricproxy-lite) mod to add Velocity modern forwarding support.

#### FrameMC (`config.toml`):
```toml
[servers.fabric]
address = "127.0.0.1"
port = 25571
forwarding_mode = "velocity_modern"
forwarding_secret = "your_shared_secret_here"
```

#### Fabric Server (`config/FabricProxy-Lite.toml`):
```toml
[general]
secret = "your_shared_secret_here"
```

#### Fabric Server (`server.properties`):
```properties
online-mode=false
server-port=25571
```

---

### 4. SteelMC (Native Rust Backend)

Pairing FrameMC with SteelMC creates an end-to-end Rust Minecraft infrastructure with zero JVM garbage collection on either side:

#### FrameMC (`config.toml`):
```toml
[servers.steelmc]
address = "127.0.0.1"
port = 25567
forwarding_mode = "none"
```

#### SteelMC (`config.toml`):
```toml
online_mode = false
bind_port = 25567
```

Connections flow straight through without proxy header overhead, yielding maximum packet throughput.

---

### 5. Vanilla Mojang Dedicated Server

To route connections to an unmodded Mojang server jar:

#### FrameMC (`config.toml`):
```toml
[servers.vanilla]
address = "127.0.0.1"
port = 25566
forwarding_mode = "none"
```

#### Vanilla (`server.properties`):
```properties
online-mode=false
server-port=25566
```

Because vanilla Mojang servers do not parse proxy forwarding headers, players will appear with offline UUIDs and the proxy's IP address on the vanilla server console. For production networks where skins and IP tracking matter, use Paper with `velocity_modern`.
