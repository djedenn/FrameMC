# FrameMC Configuration Guide

FrameMC uses a single TOML file (`config.toml`) for its network listener, authentication flags, script paths, and backend definitions. 

When you start the binary without a configuration file, it writes out a starter configuration bound to `0.0.0.0:25565` pointing to a local fallback backend.

---

## Complete `config.toml` Reference

Here is a fully populated `config.toml` demonstrating all available configuration directives:

```toml
# =============================================================================
# Network Binding
# =============================================================================

# IP interface to bind. "0.0.0.0" listens on all IPv4 interfaces.
# Use "127.0.0.1" if fronting with HAProxy or an external firewall.
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

## Detailed Key Breakdown

### Top-Level Settings

| Key | Type | Default | Details |
| :--- | :--- | :--- | :--- |
| `bind_address` | String | `"0.0.0.0"` | The local IP address to bind. To accept connections from any interface, keep `"0.0.0.0"`. Set to `"127.0.0.1"` if you run a local load balancer or tunnel. |
| `bind_port` | Integer | `25565` | Listening TCP port. Non-root users on Linux cannot bind ports below 1024 without `CAP_NET_BIND_SERVICE`. |
| `motd` | String | `"§aFrameMC..."` | Text component shown in the server browser. Section symbols (`§`) work for formatting. |
| `max_players` | Integer | `1000` | Display figure for max players in the ping packet. Does not act as a hard connection limit. |
| `online_mode` | Boolean | `true` | When `true`, enforces Mojang cryptographic handshakes and AES-128-CFB8 stream ciphers. |
| `favicon` | String (optional) | None | Path to a 64x64 PNG image or raw `data:image/png;base64,...` string. If omitted, FrameMC looks for `server-icon.png` in the proxy directory and encodes it on startup. |
| `session_server_url` | String (optional) | None | Custom Mojang session server check URL. |
| `default_server` | String | `"lobby"` | Target backend server where players land initially and where they are sent if an active backend crashes. |
| `plugins_dir` | String | `"plugins"` | Folder holding independent `.rhai` plugin files. Created automatically if it does not exist. |
| `script_path` | String | `"scripts/main.rhai"` | Path to the main event intercept script. |

### Backend Definitions (`[servers.<name>]`)

Each backend entry lives under a `[servers.<name>]` table header. The table key (`<name>`) becomes the identifier used in `/server <name>`, Rhai scripts, and log entries.

| Key | Type | Default | Details |
| :--- | :--- | :--- | :--- |
| `address` | String | *Required* | Hostname or IP address of the downstream Minecraft server. |
| `port` | Integer | *Required* | Port of the downstream Minecraft server. |
| `forwarding_mode` | String | `"none"` | Forwarding strategy. Accepted values: `"velocity_modern"` (or `"modern"` / `"velocity"`), `"legacy_bungee"` (or `"legacy"` / `"bungee"`), `"none"`. |
| `forwarding_secret` | String (optional) | None | Shared HMAC secret key. Required when using `velocity_modern`. Ignored for other modes. |

---

## Routing Rules & Failover Mechanics

### Initial Join Routing
When a player finishes authentication, FrameMC determines the initial backend destination through this order:

1. Dispatch `on_player_join` to all plugins in `plugins_dir`, followed by `script_path`.
2. If any script sets `allow: false`, the connection drops immediately with the specified `disconnect_reason`.
3. If a script specifies `target_server` (e.g. `target_server: "survival"`), the proxy routes the player there.
4. If no script overrides the target, the player goes to `default_server`.

### Graceful Mid-Game Server Transfers
When a player switches servers (via `/server <target>` or a script action):

1. **Independent TCP Handshake**: FrameMC opens a new TCP connection to the destination backend, passing the player's credentials and forwarding data without disconnecting the client.
2. **Configuration Phase Caching**: In Minecraft 1.20.2+, servers exchange registry codecs (`minecraft:dimension_type`, biomes, damage types) during a distinct Configuration phase. FrameMC intercepts these packets, updates its session cache, and replies to the backend's `FinishConfiguration` packet.
3. **World Respawn Synthesis**: FrameMC crafts a clientbound `Respawn` packet containing the target backend's dimension attributes, coordinates, and gamemode, delivering it to the client. The client transitions worlds smoothly without kicking back to the title screen.
4. **Socket Splicing**: Sockets switch to `tokio::io::copy_bidirectional`.

### Backend Disconnect Interception & Failover
If an active backend crashes, restarts, or closes its socket mid-game:

- Standard proxies let the backend's `Disconnect` packet pass through to the player, dumping them to the main menu.
- **FrameMC intercepts this packet**. It suppresses the kick screen, initiates a transfer sequence to the configured `default_server` (fallback lobby), sends a clientbound chat warning (`"§cLost connection to backend server. Reconnecting to lobby..."`), and keeps the player connected to the proxy.

---

## Compression Decoupling & Tuning

Minecraft packet compression is one of the most common failure points in multi-server networks.

### The Mismatched Threshold Problem
Suppose your lobby server runs with network compression disabled (`network-compression-threshold: -1`), while your survival server enables compression (`network-compression-threshold: 256`). 

In a naive proxy, when a player moves from lobby to survival, the survival backend expects compressed framing (a VarInt uncompressed size prefix preceding zlib deflated data), while the client socket is still configured for uncompressed raw packets. The client immediately throws a `DecoderException: Badly compressed packet` and crashes.

### How FrameMC Solves This
FrameMC decouples compression contexts:

- `client_compression_threshold`: The compression threshold negotiated between the Minecraft client and FrameMC during initial login.
- `backend_compression_threshold`: The compression threshold set by the current downstream backend via the `SetCompression (0x03)` login packet.

When routing packets:
1. If the client and backend thresholds match, packets pass through directly.
2. If they differ (or one is uncompressed while the other is compressed), FrameMC's framing layer strips or adds the zlib envelope dynamically.
3. Once the session transitions to the uninspected `Play` bridge, the thresholds on both sides remain aligned so raw socket splicing can take over.

### Decompression Bomb Protection
Malformed or malicious compressed packets can declare a huge uncompressed length to trigger memory exhaustion. FrameMC clamps all decompressed buffers and rejects packets exceeding 2 MiB (`DEFAULT_MAX_PACKET_SIZE = 2 * 1024 * 1024`) before allocating inflate buffers.

---

## Timeouts & Network Thresholds

FrameMC enforces deterministic timeouts across all phases of the connection lifecycle:

- **Initial Handshake & Login**: 10 seconds. If a connecting client fails to send a valid Handshake and LoginStart within 10 seconds, the socket is dropped to protect against slowloris exhaustion attacks.
- **Encryption Handshake**: 10 seconds. Awaits the client's `EncryptionResponse` packet.
- **Mojang Authentication**: 10 seconds. Outbound HTTP request to `sessionserver.mojang.com`. If Mojang's API hangs, the proxy times out and cleanly informs the player.
- **Server Switching Negotiation**: 5 seconds. If a target backend accepts a TCP connection but hangs during the Login or Configuration phase (for instance, when overloaded by heavy world chunks), FrameMC times out, cancels the switch, notifies the player in chat, and keeps them on their current server.
- **Status Ping / Server List Ping**: 10 seconds for the initial status request, followed by an immediate graceful socket shutdown after delivering the JSON response and favicon.

---

## Backend Server Setup Examples

### 1. Paper / Purpur / Folia (Modern Velocity Forwarding)

Velocity modern forwarding is the most secure option. It sends genuine player UUIDs, skins, textures, and IP addresses wrapped in an HMAC-SHA256 signature, preventing client IP spoofing without modifying the handshake host string.

#### `config.toml` (FrameMC):
```toml
[servers.paper]
address = "127.0.0.1"
port = 25568
forwarding_mode = "velocity_modern"
forwarding_secret = "make_this_a_64_char_random_hex_string"
```

#### `config/paper-global.yml` (Paper 1.20+):
```yaml
proxies:
  velocity:
    enabled: true
    online-mode: true
    secret: "make_this_a_64_char_random_hex_string"
```

#### `server.properties` (Paper):
```properties
online-mode=false
server-port=25568
```

> **Important**: The `forwarding_secret` must match character-for-character. If they differ, Paper logs `Unable to verify player details` and terminates the handshake.

---

### 2. Spigot / CraftBukkit (Legacy BungeeCord Forwarding)

For legacy Spigot servers that do not support modern plugin-message forwarding:

#### `config.toml` (FrameMC):
```toml
[servers.spigot]
address = "127.0.0.1"
port = 25570
forwarding_mode = "legacy_bungee"
```

#### `spigot.yml`:
```yaml
settings:
  bungeecord: true
```

#### `server.properties` (Spigot):
```properties
online-mode=false
server-port=25570
```

FrameMC rewrites the initial Handshake packet's host field to:
```text
<original_host>\0<client_ip>\0<client_uuid>
```
Spigot parses this string to assign the player their genuine UUID and IP address.

---

### 3. Fabric / Quilt (Modded)

Fabric servers running vanilla-like setups do not have built-in proxy forwarding out of the box. Install the lightweight [FabricProxy-Lite](https://modrinth.com/mod/fabricproxy-lite) mod to add Velocity forwarding support.

#### `config.toml` (FrameMC):
```toml
[servers.fabric]
address = "127.0.0.1"
port = 25571
forwarding_mode = "velocity_modern"
forwarding_secret = "your_shared_secret_here"
```

#### `config/FabricProxy-Lite.toml` (Fabric Server):
```toml
[general]
secret = "your_shared_secret_here"
```

#### `server.properties` (Fabric):
```properties
online-mode=false
server-port=25571
```

---

### 4. SteelMC (Native Rust Backend)

If you're pairing FrameMC with SteelMC for an end-to-end Rust Minecraft infrastructure:

#### `config.toml` (FrameMC):
```toml
[servers.steelmc]
address = "127.0.0.1"
port = 25567
forwarding_mode = "none"
```

#### `config.toml` (SteelMC):
```toml
online_mode = false
bind_port = 25567
```

Connections flow straight through without proxy header overhead, yielding maximum packet throughput and zero JVM garbage collection on either end.

---

### 5. Vanilla Mojang Dedicated Server

To route connections to an unmodded Mojang server binary:

#### `config.toml` (FrameMC):
```toml
[servers.vanilla]
address = "127.0.0.1"
port = 25566
forwarding_mode = "none"
```

#### `server.properties` (Vanilla):
```properties
online-mode=false
server-port=25566
```

Because vanilla Mojang servers do not support forwarding protocols, players will appear with their offline UUIDs and the proxy's IP address on the vanilla server console. For production networks where player skins and IP logging matter, use Paper with `velocity_modern`.
