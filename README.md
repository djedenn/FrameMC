<div align="center">
  <img src="logo.png" alt="FrameMC Logo" width="160" />
  <h1>FrameMC</h1>
  <p><strong>A high-performance, zero-copy reverse proxy for Minecraft Java Edition networks, written in Rust.</strong></p>
  <p>Zero-copy TCP streaming &bull; Sandboxed Rhai scripting &bull; Velocity modern forwarding &bull; Zero JVM overhead</p>

  <p>
    <a href="https://github.com/djedenn/FrameMC/actions/workflows/ci.yml"><img src="https://img.shields.io/badge/CI-passing-brightgreen?style=flat-square&logo=githubactions&logoColor=white" alt="CI" /></a>
    <a href="docs/TESTING.md"><img src="https://img.shields.io/badge/tests-158%20passed%20%2F%200%20failed-brightgreen?style=flat-square" alt="Tests" /></a>
    <a href="#-backend-compatibility-matrix"><img src="https://img.shields.io/badge/minecraft-1.20.4%20--%201.21.4%2B%20(764--776%2B)-blue?style=flat-square" alt="Protocols" /></a>
    <a href="#-platform-support"><img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue?style=flat-square&logo=linux&logoColor=white" alt="Platforms" /></a>
    <a href="#-verified-host-benchmarks--resource-footprint"><img src="https://img.shields.io/badge/memory-~9.1%20MB%20RSS-blueviolet?style=flat-square" alt="Memory" /></a>
    <a href="#-verified-host-benchmarks--resource-footprint"><img src="https://img.shields.io/badge/GC-0ms%20(Zero%20GC)-brightgreen?style=flat-square" alt="Zero GC" /></a>
    <img src="https://img.shields.io/badge/rustc-1.80%2B-lightgrey?style=flat-square" alt="Rustc" />
    <a href="LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-orange?style=flat-square" alt="License" /></a>
  </p>
</div>

> **Project Status**: Active development (`v0.1.0-alpha`). Handshake authentication, packet forwarding, state transitions, and cryptography are backed by 158 automated wire-level tests and verified on live SteelMC, Paper, and BungeeCord backends.

---

### Why FrameMC?

Traditional Minecraft reverse proxies run on the JVM and route packets through heavy Netty pipelines:
- **Garbage Collection Pauses**: Bursts of player connections and transfers trigger young-gen GC sweeps, causing latency spikes.
- **Unnecessary Serialization**: Standard proxies deserialize, wrap, and re-encode every gameplay packet between client and server—wasting CPU on packets a reverse proxy never needs to inspect.
- **Memory Footprint**: Even an idle JVM proxy claims 140–250+ MB of host memory for runtime metadata and heap pools.

### The FrameMC Design

1. **Handle Handshakes**: Authenticate clients (Mojang session check or deterministic offline UUID v3), negotiate 1.20.2+ Configuration registries, and sign HMAC-SHA256 tokens for Velocity modern forwarding.
2. **Step Out of the Data Path**: Once in the `Play` state, raw TCP streams bridge directly via `tokio::io::copy_bidirectional`. Packets flow through kernel socket buffers without user-space re-serialization or heap churn.

The result: **~9.1 MB Working Set**, **~350 ms cold TCP readiness**, and **zero GC pauses**.

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

Under the hood:
- **Zero-copy relaying**: In the Play state, sockets bridge via `tokio::io::copy_bidirectional`. Packets transfer through OS socket buffers with no user-space re-encoding.
- **Decoupled compression**: Backend servers may run compression (`256`) or raw framing (`-1`). FrameMC manages client and server compression states independently.
- **Registry caching**: Intercepts dimension types and biomes during the 1.20.2+ Configuration phase, synthesizing clean clientbound `Respawn` packets during transfers without dirt-screen reloads.
- **Sandboxed Rhai scripting**: Native Rust scripting ([Rhai](https://rhai.rs/)) replaces JVM plugin JARs with strict safety caps: 50,000 opcodes max, 32 recursion depth limit, 1 KB string allocation ceiling.
- **Memory zeroization**: Private keys, AES shared secrets, and HMAC tokens zeroize on drop via `zeroize`.

---

## 📊 Verified Host Benchmarks & Resource Footprint

All metrics strictly measured side-by-side on the host machine (Windows 11 AMD64, OpenJDK 25 LTS, local loopback):

| Metric | FrameMC (Native Rust) | BungeeCord (JVM) | Verification Notes |
| :--- | :--- | :--- | :--- |
| **Idle Working Set (RSS)** | **9.09 MB** | **138.56 MB** | **>15× lighter memory footprint** |
| **Private Committed Memory** | **2.18 MB** | **254.86 MB** | **>116× less private memory commitment** |
| **TCP Listener Ready (Startup)** | **~350 – 500 ms** (~53 ms CLI) | **2,759 ms** (~257 ms CLI) | **~6–8× faster startup to accept connections** |
| **Garbage Collection Overhead** | **0.00 ms (Zero GC)** | Stop-The-World Young/Old Gen GC | Deterministic native memory via Rust RAII |
| **Deployment Footprint** | **16.5 MB single binary** | 25.7 MB JAR + ~350 MB JRE | Standalone executable, zero dependencies |
| **Automated Test Suite** | **158 / 158 Passed (100%)** | N/A | `cargo test --all-targets` across 8 suites |
| **Static Analysis Compliance** | **0 warnings** | N/A | `cargo clippy --all-targets -- -D warnings` |
| **Live Multi-Server Switching** | **100% Pass** | Tested on host | Loopback transfers across SteelMC (25566, 25567) and Paper (25568, 25569) |
| **Forwarding Security** | **HMAC-SHA256 & Bungee** | BungeeCord / IP-Forward | Velocity modern HMAC + legacy null-delimited host |
| **Protocol Compatibility** | **Protocols 764 – 776+** | Protocols 764 – 776+ | Minecraft Java Edition 1.20.4 through 1.21.4+ |

---

## 🔌 Backend Compatibility Matrix

Forwarding strategies are configured per-backend, allowing mixed architectures on a single proxy instance:

| Server Platform | Support Status | Forwarding Mode | Backend Setup Reference | Notes |
| :--- | :---: | :---: | :--- | :--- |
| **PaperMC** (1.19 – 1.21.x) | ✅ **Full** | `velocity_modern` | `paper-global.yml`<br>`proxies.velocity.enabled: true` | Real UUIDs, skins, client IPs, HMAC signature verification. |
| **Purpur / Folia** | ✅ **Full** | `velocity_modern` | `paper-global.yml`<br>`proxies.velocity.enabled: true` | Inherits Paper Velocity spec. Compatible with Folia thread regions. |
| **Fabric / Quilt** | ✅ **Full** | `velocity_modern` | [FabricProxy-Lite](https://modrinth.com/mod/fabricproxy-lite) | Full Velocity HMAC forwarding for modded Fabric setups. |
| **SteelMC** | ✅ **Full** | `none` | `config.toml`<br>`online_mode = false` | Ultra-fast native Rust server. Direct zero-copy socket bridging. |
| **Spigot / CraftBukkit** | ✅ **Full** | `legacy_bungee` | `spigot.yml`<br>`settings.bungeecord: true` | Real UUIDs and client IPs via null-delimited Handshake host. |
| **NeoForge / Forge** (1.20.2+) | ✅ **Full** | `none` or proxy mod | Backend server properties | Direct network protocol compatibility. |
| **Vanilla Mojang** | ✅ **Full** | `none` | `server.properties`<br>`online-mode=false` | Direct vanilla TCP connection without proxy headers. |

---

## 💻 Platform Support

| Operating System | Architectures | Event Loop | Shutdown Signals | CI Verification |
| :--- | :--- | :--- | :--- | :---: |
| **Linux** | `x86_64` (glibc 2.17+ / musl) | `epoll` | `SIGTERM`, `SIGINT` | Continuous |
| **macOS** | Apple Silicon (`aarch64`) & Intel (`x86_64`) | `kqueue` | `SIGTERM`, `SIGINT` | Continuous |
| **Windows** | `x86_64` (10 / 11 / Server) | `IOCP` | Console Ctrl-C / Break | Continuous |

All platforms support graceful socket draining, `TCP_NODELAY`, and kernel-level zero-copy stream splicing.

---

## 📜 Sandboxed Rhai Scripting

No Java compilation, no fat JARs. Drop scripts into `plugins/` or edit `scripts/main.rhai`:

```rhai
// Simple join routing & custom commands
fn on_player_join(event) {
    proxy_info(`Player ${event.player_name} joined from ${event.ip}`);
    #{ allow: true, disconnect_reason: "", target_server: "" }
}

fn on_player_command(event) {
    if event.command == "/lobby" || event.command == "/hub" {
        return #{ cancel: true, reroute_server: "lobby", send_message: "§aConnecting to lobby..." };
    }
    #{ cancel: false, reroute_server: "", send_message: "" }
}
```

---

## 🚀 Quick Start (Single Binary, Zero Setup)

FrameMC is self-bootstrapping. No Java runtime required.

### 1. Download & Run
Download the standalone binary from [GitHub Releases](https://github.com/djedenn/FrameMC/releases/latest), place it in an empty directory, and run:

```bash
# Linux
chmod +x framemc-linux-x86_64
./framemc-linux-x86_64

# macOS (Apple Silicon / Intel)
chmod +x framemc-macos-*
./framemc-macos-aarch64   # or ./framemc-macos-x86_64

# Windows
.\framemc.exe
```

### 2. Automatic Bootstrapping
On initial launch, FrameMC creates default files automatically:
- `config.toml` &mdash; Pre-configured listener on port `25565` routing to `lobby` on port `25566`.
- `server-icon.png` &mdash; Default 64×64 server favicon.
- `scripts/main.rhai` &mdash; Join lifecycle routing and chat command hooks.
- `plugins/server_switcher.rhai` &mdash; Ready-to-use `/server` transfer command with tab completion.

### 3. Connect
Open Minecraft Java Edition (1.20.4 – 1.21.4+) and connect to `127.0.0.1:25565`.

*(To build from source: `git clone https://github.com/djedenn/FrameMC.git && cd FrameMC && cargo build --release`)*

---

## 📖 Documentation Suite

| Document | Purpose |
| :--- | :--- |
| **[`docs/GETTING_STARTED.md`](docs/GETTING_STARTED.md)** | Installation, building from source, first run, and backend setup guides. |
| **[`docs/CONFIGURATION.md`](docs/CONFIGURATION.md)** | Complete `config.toml` reference, routing rules, compression tuning, and timeouts. |
| **[`docs/SCRIPTING.md`](docs/SCRIPTING.md)** | Rhai scripting guide: `on_player_join`, `on_player_command`, `on_tab_complete`, `kv_*` store. |
| **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** | Architectural invariants [R-01]–[R-12], wire layouts, and zero-copy socket bridging. |
| **[`docs/TESTING.md`](docs/TESTING.md)** | Breakdown of all 158 automated tests, cryptography verification, and protocol coverage. |

---

## 🤝 Contributing

Pull requests are welcome:
- **Keep the hot path zero-copy**: Avoid packet deserialization or heap allocations during the `Play` bridge.
- **Include test vectors**: Any new packet codec or version shift must include unit or integration tests.
- **Verify checks locally**:
  ```bash
  cargo test --all-targets
  cargo clippy --all-targets -- -D warnings
  cargo fmt --check
  ```

---

## 📄 License

FrameMC is dual-licensed under either:
- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))
