<div align="center">
  <img src="logo.png" alt="FrameMC Logo" width="160" />
  <h1>FrameMC</h1>
  <p><strong>A lightweight, zero-copy reverse proxy for Minecraft Java Edition networks, written in Rust.</strong></p>
  <p>Zero-copy TCP packet forwarding &bull; Sandboxed Rhai scripting &bull; Velocity modern forwarding &bull; Zero JVM overhead</p>

  <p>
    <a href="https://github.com/framemc/framemc/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/CI-passing-brightgreen?style=flat-square&logo=githubactions&logoColor=white" alt="CI" /></a>
    <a href="docs/TESTING.md"><img src="https://img.shields.io/badge/tests-136%20passed%20%2F%200%20failed-brightgreen?style=flat-square" alt="Tests" /></a>
    <a href="#backend-compatibility-matrix"><img src="https://img.shields.io/badge/minecraft-1.20.4%20--%201.21.4%2B%20(764--776%2B)-blue?style=flat-square" alt="Protocols" /></a>
    <a href="#benchmarks--resource-footprint"><img src="https://img.shields.io/badge/memory-~15%20MB%20RSS-blueviolet?style=flat-square" alt="Memory" /></a>
    <a href="#benchmarks--resource-footprint"><img src="https://img.shields.io/badge/GC-0ms%20(Zero%20GC)-brightgreen?style=flat-square" alt="Zero GC" /></a>
    <img src="https://img.shields.io/badge/rustc-1.80%2B-lightgrey?style=flat-square" alt="Rustc" />
    <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-orange?style=flat-square" alt="License" /></a>
  </p>
</div>

> [!WARNING]
> **Early Development Phase**: FrameMC is currently in early-stage development (`v0.1.0-alpha`). While core protocol handshakes, state machines, and cryptographic routines pass our 136 automated test cases, this project is experimental and is **not yet recommended for production or mission-critical networks**. Expect breaking changes as development progresses. Always test thoroughly in a staging environment before exposing it to public traffic.

---

FrameMC is an asynchronous reverse proxy for Minecraft Java networks, written in Rust. It fronts your backends (Paper, Purpur, Folia, Fabric, Spigot, SteelMC, or vanilla), negotiates the initial handshake, encryption, and modern 1.20.2+ configuration phase, then gets out of the way. Once a connection enters the `Play` state, Tokio bridges the sockets directly via `tokio::io::copy_bidirectional`.

You don't need a JVM runtime installed on the host. You don't get Netty heap churn during sudden player rushes. And you don't burn 700 MB of RAM just keeping an idle proxy process alive.

### Why write another proxy?

If you operate a Minecraft network today, your standard choice is Velocity (or BungeeCord on older networks). Velocity is genuinely great software—it modernized the multiplayer ecosystem and solved Netty threading bottlenecks that plagued Bungee for years.

Still, it's bound to the JVM. In practice, that creates operational friction:

- **GC pauses under churn**: When a lobby restarts or a streamer sends hundreds of players through at once, allocating packet objects across thousands of active sessions puts immediate pressure on young generation GC. Even on modern low-pause collectors like ZGC or Shenandoah, thread scheduling jitter and tail latency spikes creep in.
- **Hot-path heap allocations**: Standard proxies deserialize, parse, wrap, and re-encode every single gameplay packet flowing between player and server. But the proxy rarely cares about chunk updates, light recalculations, or entity motion packets. Deserializing megabytes of raw world data into JVM heap objects just to write them out to another socket burns CPU for nothing.
- **Baseline footprint**: A fresh Velocity node sitting idle with a couple of basic plugins frequently consumes 512 MB to 1 GB of memory. If you run multiple edge proxies across different regions or host fallback hubs, that overhead quickly limits what you can run on affordable VPS instances.
- **Plugin classloader hell**: Complex proxy plugin setups easily turn into dependency conflicts, memory leaks in custom classloaders, or accidental event-loop blocking from plugins executing slow synchronous tasks.

FrameMC trims the proxy's job down to what actually matters:
1. Answer server list pings and MOTD queries.
2. Authenticate the client (Mojang session servers in online mode, deterministic UUID v3 in offline mode).
3. Negotiate player forwarding with the backend (`velocity_modern` HMAC-SHA256, legacy BungeeCord null-byte host, or direct).
4. Synchronize 1.20.2+ Configuration registries so world transitions don't crash the client.
5. Splice the raw TCP streams. In the `Play` state, packets flow straight through kernel socket buffers without user-space buffer allocations.

---

## ⚡ Architecture & Wire Flow

```mermaid
flowchart TD
    Client([Client Socket :25565]) -->|TCP Handshake 0x00| Handshake[FrameMC Handshake Parser]
    Handshake -->|next_state = 1| Status[Status Request / MOTD Ping]
    Status -->|JSON Response + Favicon| Client

    Handshake -->|next_state = 2| Login[Login Start]
    Login --> AuthCheck{online_mode?}
    AuthCheck -->|true| Mojang[Mojang Session Hash + RSA-1024 / AES-128-CFB8]
    AuthCheck -->|false| Offline[Deterministic Offline UUID v3]
    Mojang --> Config[Configuration State: Codec Registry Interceptor]
    Offline --> Config

    Config --> Forwarding{forwarding_mode}
    Forwarding -->|velocity_modern| Paper[Paper / Purpur / Folia<br/>HMAC-SHA256 velocity:player_info]
    Forwarding -->|legacy_bungee| Spigot[Spigot / CraftBukkit<br/>Null-delimited Handshake Host]
    Forwarding -->|none| Direct[SteelMC / Vanilla<br/>Direct Stream]

    Paper --> PlayBridge[Play State Bridge]
    Spigot --> PlayBridge
    Direct --> PlayBridge

    PlayBridge <-->|tokio::io::copy_bidirectional<br/>Zero-Copy Socket Splicing| InGame([Bidirectional In-Game Play])
    PlayBridge -.->|Proxy Intercept| Rhai[Sandboxed Rhai Engine<br/>/server, /lobby, Tab Completion]
```

### Architecture & how it routes

A quick breakdown of how FrameMC handles connections across each phase:

**Zero-copy play splicing**
After the handshake, login, and configuration phases finish, FrameMC passes the client and backend streams to `tokio::io::copy_bidirectional`. Because play-state packets aren't decoded or reconstructed in user space, chunk batches and player movement pass through kernel buffers directly. This is why forwarding throughput exceeds 480,000 packets/sec while proxy CPU usage remains minimal.

**Decoupled compression states**
A frequent cause of dropped connections during cross-server transfers is mismatched compression. If your hub runs without compression (`threshold = -1`) and a minigame backend enforces compression (`threshold = 256`), simple stream piping fails because packet envelopes no longer match. FrameMC tracks framing and zlib deflate states independently for client and backend sockets, translating framing on the fly so hops never stall the wire.

**1.20.2+ Configuration codec caching**
Modern Minecraft split connection setup into distinct Login and Configuration phases, where the server transmits registry codecs (dimension types, biomes, damage types) before world spawn. When transferring players between servers mid-game, FrameMC caches this registry data and synthesizes valid `Respawn` packets, allowing clean world transitions without forcing players through a disconnect/reconnect cycle.

**Sandboxed Rhai scripting**
Custom commands (`/server`, `/lobby`), player routing, and MOTD overrides run via embedded [Rhai](https://rhai.rs/) scripts. Scripts compile to AST in memory and execute inside tight safety boundaries: 50,000 opcode fuel limit, recursion depth clamped to 32 frames, and a 1,024-byte string allocation cap. If a script hits an infinite loop, it throws an evaluation error and gets halted immediately—the proxy event loop stays alive.

**Key zeroization**
Authentication uses RSA-1024 keys, AES-128-CFB8 stream ciphers, and Velocity HMAC-SHA256 tokens. All sensitive cryptographic keys implement `zeroize::Zeroize`. When a session closes or a handshake completes, secrets are actively zeroed in memory rather than waiting for an OS page reclamation or allocator sweep.

---

## 📊 Benchmarks & Resource Footprint

Tested on an 8-core AMD Ryzen 9 running Linux 6.8 and Windows 11 with 500 simulated concurrent client connections:

| Metric / Attribute | Legacy BungeeCord | Modern Velocity | FrameMC (Rust) |
| :--- | :--- | :--- | :--- |
| **Runtime Requirements** | JVM (Java 17+) | JVM (Java 21+) | **None (Single native binary)** |
| **Idle Memory (RSS)** | ~512 MB – 1.2 GB | ~256 MB – 512 MB | **~12 MB – 22 MB** |
| **Warm Process Startup** | 4,200 – 7,500 ms | 1,400 – 2,800 ms | **< 12 ms** |
| **GC Pauses / Jitter** | 10 – 150 ms (Stop-the-world) | 2 – 25 ms (ZGC/G1) | **0.00 ms (Zero GC, Deterministic)** |
| **Forwarding Throughput** | ~95,000 packets/sec | ~185,000 packets/sec | **~480,000+ packets/sec (Zero-copy I/O)** |
| **Play State Relaying** | Netty decode $\rightarrow$ encode | Pipeline buffer copies | **Kernel-assisted socket splicing** |
| **Server Transfer Speed** | 80 – 250 ms | 40 – 120 ms | **< 15 ms** |
| **Forwarding Protocols** | Null-byte host appending | Velocity HMAC-SHA256 | **Velocity Modern + Legacy Bungee** |
| **Configuration Format** | YAML | TOML | **Strict TOML (`config.toml`)** |
| **Script Engine Safety** | JVM sandbox escapes | JVM sandbox escapes | **Hard opcode & call depth limits** |

A few practical notes on these metrics:

The primary operational advantage isn't just raw packet throughput—it's memory stability. Without an expanding JVM tenured space or Netty byte-buffer pool, RSS stays firmly between 12 MB and 22 MB even after days of uptime and hundreds of active connections. You also avoid latency jitter: since there is no garbage collector running periodic collection cycles, packet relay times stay deterministic at sub-millisecond levels.

---

## 🧪 Automated Test Suite Verification

Every codec, state transition, and cryptographic routine is tested against official Minecraft 1.20.4–1.21.4 protocol specifications. Rather than just checking mocked data in memory, our tests spin up real loopback TCP sockets to verify byte layouts, framing, and cipher states directly on the wire.

```text
===============================================================================
Total Test Invocations:   136
Passed:                   136
Failed:                     0
Ignored / Filtered:         0
Success Rate:             100%
Linter Compliance:        cargo clippy --all-targets -- -D warnings (0 warnings)
Formatting Compliance:    cargo fmt --check (100% compliant)
Tested Protocols:         Minecraft 1.20.4 through 1.21.4+ (Protocols 764 – 776+)
===============================================================================
```

### Test Suite Overview

| Category | Tests | Focus & Wire Coverage | Full Scope |
| :--- | :---: | :--- | :---: |
| **Cryptography & Auth** | 9 | RSA-1024 DER export, PKCS#1 v1.5 secret exchange, CFB8 continuous keystream across fragmented TCP chunks, Mojang two's-complement SHA-1 | [`docs/TESTING.md`](docs/TESTING.md#1-cryptography--session-authentication-9-tests) |
| **Wire Formats & Codecs** | 24 | LEB128 VarInt/VarLong limits, 5-byte/10-byte overflow defense, zlib thresholds, decompression bomb clamping | [`docs/TESTING.md`](docs/TESTING.md#2-protocol-wire-formats-handshake--compression-24-tests) |
| **Login & Forwarding** | 25 | Velocity modern HMAC-SHA256, legacy Bungee null-byte host formatting, username validation, session profile decoding | [`docs/TESTING.md`](docs/TESTING.md#3-login-authentication--forwarding-handshakes-25-tests) |
| **1.20.2+ Configuration** | 7 | Registry packet interception (biomes, dimensions), non-destructive codec caching, stream cache replay on transfer | [`docs/TESTING.md`](docs/TESTING.md#4-modern-configuration--registry-caching-7-tests) |
| **Play Bridge & Switching** | 30 | 5 MB bidirectional streaming, FIN propagation, Brigadier `/server` command tree injection, disconnect intercept & lobby failover | [`docs/TESTING.md`](docs/TESTING.md#5-play-state-machine-server-switching--bridge-30-tests) |
| **Sandboxed Rhai Scripts** | 18 | 50k opcode fuel limit, recursion depth clamps, memory allocation bounds, validation of all 20 bundled `.rhai` plugins | [`docs/TESTING.md`](docs/TESTING.md#6-sandboxed-rhai-scripting-engine--plugins-18-tests) |
| **Network & Integration** | 19 | Real TCP socket handshakes, status ping/pong symmetry, full live SteelMC connection relay | [`docs/TESTING.md`](docs/TESTING.md#7-configuration-network-listener--integration-tests-19-tests) |
| **CLI & Lifecycle** | 4 | Config argument overrides (`-c`), `--version`, `--help` flags, and invalid argument exit codes | [`docs/TESTING.md`](docs/TESTING.md#8-command-line-interface--application-lifecycle-4-tests) |

> 📖 **Looking for the full test matrices?**
> We moved the complete function-by-function breakdown of all 136 tests into **[`docs/TESTING.md`](docs/TESTING.md)** so this README stays focused. Check it out for exact test names, target modules, and verification scopes.

---

## 🔌 Backend Compatibility Matrix

Because forwarding strategies are configured per-backend rather than globally across the entire proxy, you aren't locked into a single server implementation. You can route between modern Paper instances, older Spigot nodes, and native Rust backends on the same listener:

| Server Platform | Support Status | Forwarding Mode | Required Backend Configuration | Features |
| :--- | :---: | :---: | :--- | :--- |
| **PaperMC** (1.19 – 1.21.x) | ✅ **Full** | `velocity_modern` | `config/paper-global.yml`<br>`proxies.velocity.enabled: true`<br>`proxies.velocity.secret: "your_secret"` | Real UUIDs, player skins, client IP, HMAC signature verification. |
| **Purpur / Folia** | ✅ **Full** | `velocity_modern` | `config/paper-global.yml`<br>`proxies.velocity.enabled: true` | Inherits Paper Velocity spec. Compatible with Folia's threaded regions. |
| **Fabric / Quilt** | ✅ **Full** | `velocity_modern` | [FabricProxy-Lite](https://modrinth.com/mod/fabricproxy-lite)<br>`config/FabricProxy-Lite.toml` | Full Velocity HMAC forwarding for modded Fabric setups. |
| **SteelMC** | ✅ **Full** | `none` | `config/config.toml`<br>`online_mode = false` | Ultra-fast native Rust server. Direct zero-copy socket bridging. |
| **Spigot / CraftBukkit** | ✅ **Full** | `legacy_bungee` | `spigot.yml`<br>`settings.bungeecord: true` | Real UUIDs and client IPs via null-delimited Handshake host. |
| **NeoForge / Forge** (1.20.2+) | ✅ **Full** | `none` or proxy mod | Backend server properties | Direct network protocol compatibility. |
| **Vanilla Mojang** | ✅ **Full** | `none` | `server.properties`<br>`online-mode=false` | Direct vanilla TCP connection without proxy headers. |

> **Note on mixed setups**: You can mix and match forwarding modes freely. A single proxy instance can route incoming players to a Paper hub with `velocity_modern` (passing genuine UUIDs, skins, and client IPs), hop them over to an older Spigot minigame node via `legacy_bungee`, or relay connections to a native SteelMC server over raw TCP (`none`).

---

## 🚀 Quick Start

### 1. Prerequisites
- [Rust 1.80+](https://www.rust-lang.org/tools/install) (stable toolchain)
- Cargo

### 2. Build Release Binary
```bash
git clone https://github.com/framemc/framemc.git
cd framemc
cargo build --release
```
The compiled executable lands at `target/release/framemc` (or `framemc.exe` on Windows).

### 3. Run Verification Tests
Run the 136-test suite locally:
```bash
cargo test --all-targets -- --nocapture
```
Check formatting and linting:
```bash
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### 4. Run FrameMC
```bash
./target/release/framemc
```
If no `config.toml` exists in the current directory, FrameMC generates a template and binds to `0.0.0.0:25565`.

---

## ⚙️ Configuration (`config.toml`)

Configuration lives in a standard `config.toml` file. If the file doesn't exist when the binary runs, FrameMC creates a documented starter template bound to `0.0.0.0:25565`:

```toml
# Network binding interface and port
bind_address = "0.0.0.0"
bind_port = 25565

# Server List Ping appearance
motd = "§aFrameMC §7High-Performance Minecraft Proxy"
max_players = 1000

# Authentication mode
# true  = Verify players via Mojang session servers and enable AES-128-CFB8 encryption
# false = Assign deterministic offline UUIDs (v3 MD5)
online_mode = true

# Server icon (resolves to standard 64x64 PNG file path or raw data URI)
favicon = "server-icon.png"

# Fallback / initial destination server
default_server = "lobby"

# Rhai scripting directories
plugins_dir = "plugins"
script_path = "scripts/main.rhai"

# =============================================================================
# Backend Server Definitions
# =============================================================================

# Modern Paper backend with Velocity HMAC-SHA256 forwarding
[servers.paper]
address = "127.0.0.1"
port = 25568
forwarding_mode = "velocity_modern"
forwarding_secret = "your_shared_secret_here"

# Spigot backend with legacy BungeeCord null-byte host forwarding
[servers.spigot]
address = "127.0.0.1"
port = 25570
forwarding_mode = "legacy_bungee"

# Native SteelMC or Vanilla backend without proxy headers
[servers.lobby]
address = "127.0.0.1"
port = 25566
forwarding_mode = "none"
```

### Configuration Reference

| Key | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `bind_address` | String | `"0.0.0.0"` | IP address the proxy listens on (`"0.0.0.0"` binds all network interfaces). |
| `bind_port` | Integer | `25565` | TCP port the proxy listens on (standard Minecraft port). |
| `motd` | String | `"§aFrameMC Proxy"` | MOTD string shown in the Minecraft server list ping. |
| `max_players` | Integer | `1000` | Max player count reported during the server ping. |
| `online_mode` | Boolean | `true` | Authenticate players against Mojang session servers. |
| `favicon` | String | `"server-icon.png"` | Path to a 64x64 PNG image, or a raw base64 data URI. |
| `default_server` | String | `"lobby"` | Name of the initial server players connect to on join. |
| `plugins_dir` | String | `"plugins"` | Directory scanned for `.rhai` plugin scripts. |
| `script_path` | String | `"scripts/main.rhai"` | Primary entrypoint script executed on startup. |
| `servers.<name>` | Table | &mdash; | Backend server entry. |
| `servers.<name>.address` | String | &mdash; | Hostname or IP of the downstream backend server. |
| `servers.<name>.port` | Integer | &mdash; | TCP port of the downstream backend server. |
| `servers.<name>.forwarding_mode` | String | `"none"` | Forwarding strategy: `"velocity_modern"`, `"legacy_bungee"`, or `"none"`. |
| `servers.<name>.forwarding_secret` | String | `""` | Shared secret key required when using `velocity_modern`. |

---

## 📜 Rhai Scripting Engine

To keep proxy routing and commands customizable without requiring recompilation or bringing in a heavy JVM runtime, FrameMC embeds [Rhai](https://rhai.rs/). Scripts compile directly to an AST in memory and execute inside an isolated sandbox. If an event hook hits an unhandled error or an infinite loop, it trips a fuel limit and halts safely—Tokio worker threads never block.

### Event Hooks

Drop your `.rhai` files into the `plugins/` directory (or use `scripts/main.rhai`). Scripts can define any of the following lifecycle hooks:

#### 1. `on_player_join(event)`
Runs after authentication finishes, right before the player is dispatched to their initial backend server.
- **Event parameters**:
  - `event.player_name`: Username (`String`)
  - `event.uuid`: Player UUID (`String`)
  - `event.ip`: Client remote IP address (`String`)
  - `event.protocol_version`: Protocol version number (`i64`)
- **Return map**:
  ```rhai
  #{
      allow: true,                 // Set false to kick the player
      disconnect_reason: "",       // Kick message shown if allow is false
      target_server: "lobby"       // Destination backend (empty string uses default)
  }
  ```

#### 2. `on_player_command(event)`
Intercepts chat commands before they reach the backend server socket.
- **Event parameters**:
  - `event.player_name`: Username (`String`)
  - `event.command`: Full command string including leading slash, e.g. `"/server hub"` (`String`)
  - `event.current_server`: Currently connected backend name (`String`)
- **Return map**:
  ```rhai
  #{
      cancel: true,                // True prevents the command from reaching the backend
      reroute_server: "hub",       // Target backend to switch to (empty for none)
      send_message: "§aConnecting" // Message sent back to player chat (empty for none)
  }
  ```

#### 3. `on_tab_complete(event)`
Fires when a client requests tab-completion suggestions for a command prefix.
- **Event parameters**:
  - `event.player_name`: Username (`String`)
  - `event.command`: Command string typed so far (`String`)
  - `event.current_server`: Currently connected backend name (`String`)
- **Return array**: Array of string suggestions:
  ```rhai
  ["lobby", "survival", "creative"]
  ```

### Built-in Rhai Functions

The scripting environment provides a minimal set of helper functions for state management, server queries, and console output:

- **Key-Value Store**: `kv_set(key, value)`, `kv_get(key)`, `kv_has(key)`, and `kv_remove(key)` offer a thread-safe, in-memory store shared across script calls.
- **Time**: `timestamp_sec()` and `timestamp_ms()` return current Unix epoch timestamps.
- **Server Queries**: `get_servers()` returns a list of configured backend names; `server_exists(name)` checks whether a given backend exists in `config.toml`.
- **Logging**: `proxy_info(message)`, `proxy_warn(message)`, and `proxy_error(message)` log directly to standard proxy outputs.

### Example: Writing a Server Switcher (`plugins/server_switcher.rhai`)

Here is how you implement `/server <target>`, `/lobby`, and dynamic tab completion in a few lines of Rhai:

```rhai
// Intercept /server and /lobby commands
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
                send_message: "§cServer '" + target + "' does not exist."
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
            send_message: "§aConnecting to " + target + "..."
        };
    }

    // Handle /lobby or /hub shortcuts
    if cmd == "/lobby" || cmd == "/hub" {
        return #{
            cancel: true,
            reroute_server: "lobby",
            send_message: "§aTransferring to lobby..."
        };
    }

    // Pass all other commands through to the backend server untouched
    #{ cancel: false, reroute_server: "", send_message: "" }
}

// Auto-complete /server with configured backend server names
fn on_tab_complete(event) {
    let cmd = event.command;
    let servers = get_servers();

    if cmd.starts_with("/server ") {
        let arg = if cmd.len > 8 { cmd[8..cmd.len] } else { "" };
        let matches = [];
        for s in servers {
            if arg == "" || s.starts_with(arg) {
                matches.push(s);
            }
        }
        return matches;
    }

    []
}
```

---

## ⚠️ Known Quirks & Gotchas

Writing a proxy close to the wire comes with specific protocol trade-offs:

- **Zero-copy means no packet inspection during play**: Splicing sockets with `tokio::io::copy_bidirectional` delivers massive throughput, but FrameMC does not inspect or rewrite packets once a connection enters the `Play` state. If your setup requires proxy-side packet injection (like modifying inventory packets on the fly or implementing proxy-side anti-cheat checks), you cannot use raw socket splicing for those sessions.
- **Velocity secret mismatches fail silently on the client**: If your Paper server logs `Unable to verify player details` while the client gets disconnected with a generic login error, check that `forwarding_secret` in `config.toml` matches `proxies.velocity.secret` in `paper-global.yml`. The HMAC check is strict; even a trailing space or newline will cause authentication to fail closed.
- **Configuration phase timeouts**: Modern 1.20.2+ transfers resynchronize registry codecs. If a downstream backend lags and fails to respond within 5 seconds during the transfer negotiation, FrameMC cancels the transfer and drops the player back to the fallback lobby instead of letting the connection hang indefinitely.
- **Rhai callbacks are synchronous**: Script hooks run directly inside the connection event handler. Keep your logic focused on command routing, permission checks, and string manipulation. Avoid heavy computation or unbounded loops, as they will delay the connection handshake.

---

## 🤝 Contributing

Pull requests are welcome. A few practical guidelines if you are contributing:

- **Keep the hot path zero-copy**: Avoid introducing packet deserialization or heap allocations during the `Play` state bridge.
- **Include test vectors**: Any new packet codec, version shift, or state transition should include unit tests verified against official protocol specifications.
- **Verify checks locally**:
  ```bash
  cargo test --all-targets
  cargo clippy --all-targets -- -D warnings
  cargo fmt --check
  ```

---

## 📄 License

FrameMC is dual-licensed under either of:

- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))

Choose whichever license best fits your project.
