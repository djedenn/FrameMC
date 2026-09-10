# Getting Started with FrameMC

Getting FrameMC running takes about two minutes. Because it compiles to a standalone native binary, there's no Java installation to worry about, no classpath arguments to pass, and no tuning JVM garbage collectors before you start.

## Table of Contents
- [System Requirements](#system-requirements)
- [Building from Source](#building-from-source)
- [First Run & Bootstrapping](#first-run--bootstrapping)
- [CLI Options & Flags](#cli-options--flags)
- [Connecting Your First Backend](#connecting-your-first-backend)
  - [Recommended Port Layout](#recommended-port-layout)
  - [Step-by-Step Paper Setup (Velocity Modern)](#step-by-step-paper-setup-velocity-modern-forwarding)
- [Production Deployment](#production-deployment)
  - [1. Systemd Service](#1-systemd-service-etcsystemdsystemframemcservice)
  - [2. macOS Deployment (launchd daemon)](#2-macos-deployment-launchd-daemon)
  - [3. File Descriptor Limits (ulimit)](#3-file-descriptor-limits-ulimit)
- [Troubleshooting Initial Setup](#troubleshooting-initial-setup)
- [Quick Diagnostic Commands](#quick-diagnostic-commands)
- [Next Steps](#next-steps)

---

## System Requirements

FrameMC doesn't depend on an external runtime or shared C libraries. It runs as a self-contained executable:

- **Operating System**: 64-bit Linux (glibc 2.17+ or musl), Windows 10/11/Server, or macOS 12+ (Apple Silicon or Intel).
- **Memory footprint**: ~15 MB RSS baseline. Unlike Java proxies that hold onto hundreds of megabytes for heap pools and GC metadata, FrameMC runs comfortably on a 512 MB VPS alongside other services.
- **CPU**: Any modern x86_64 or aarch64 core. If your x86_64 CPU supports SSE4.2 and AES-NI, encryption handshakes run with hardware acceleration.
- **Build Toolchain**: Rust 1.80 or newer (`rustc` and `cargo`). Check your installed version with `rustc --version`.

---

## Building from Source

Building locally takes roughly two minutes on a modern quad-core machine.

### 1. Grab the repository

```bash
git clone https://github.com/djedenn/FrameMC.git
cd FrameMC
```

### 2. Compile a release build

Always compile with `--release`. Unoptimized debug builds include heavy runtime assertions and skip link-time optimization, which severely hurts AES cipher throughput and VarInt parsing speeds.

```bash
cargo build --release
```

Once Cargo finishes compiling dependencies (Tokio, Rhai, RSA, Flate2), the compiled binary is located at:
- **Linux / macOS**: `target/release/framemc`
- **Windows**: `target\release\framemc.exe`

### 3. Verify the build locally

Before deploying, run the test suite to confirm your local platform passes all wire-level protocol checks:

```bash
cargo test --all-targets
```

All 152 tests should pass cleanly without ignored or failing cases.

---

## First Run & Bootstrapping

Starting FrameMC without existing config files takes one command:

```bash
# On Linux / macOS
./target/release/framemc

# On Windows
.\target\release\framemc.exe
```

If no `config.toml` exists in the current directory, FrameMC writes out a documented starter template and binds to `0.0.0.0:25565`.

Startup logs look like this:

```text
2026-09-08T14:20:00Z  INFO framemc: Starting FrameMC Minecraft Proxy using config: config.toml
2026-09-08T14:20:00Z  INFO framemc: Loaded 1 plugin(s) from 'plugins'
2026-09-08T14:20:00Z  INFO framemc: Loaded script at 'scripts/main.rhai'
2026-09-08T14:20:00Z  INFO framemc::network::listener: FrameMC listener bound to 0.0.0.0:25565 (online_mode: true)
```

At this point, the proxy is live and waiting for incoming Minecraft client connections.

---

## CLI Options & Flags

FrameMC keeps command-line flags deliberately minimal:

```text
Usage: framemc [OPTIONS]

Options:
  -c, --config <PATH>  Path to configuration file [default: config.toml]
  -V, --version        Display version information
  -h, --help           Display help message
```

### Running with a custom configuration file
If you manage multiple environments or staging instances on one machine:

```bash
./framemc -c /etc/framemc/staging.toml
```

### Adjusting log verbosity
FrameMC routes logs through `tracing-subscriber`. To change log levels or filter specific subsystems:

```bash
# Enable debug logs across the whole proxy
RUST_LOG=debug ./framemc

# Focus strictly on network handshakes and Rhai script execution
RUST_LOG=framemc::network=debug,framemc::script=trace ./framemc
```

On Windows PowerShell:
```powershell
$env:RUST_LOG="debug"; .\framemc.exe
```

---

## Connecting Your First Backend

A proxy needs at least one downstream Minecraft server to send players to. **Do not run your backend server on the same port as FrameMC.**

### Recommended Port Layout
| Service | Bind Host | Port | Forwarding Mode |
| :--- | :--- | :--- | :--- |
| **FrameMC (Proxy)** | `0.0.0.0` (Public) | `25565` | Incoming player gateway |
| **Lobby Backend (Paper)** | `127.0.0.1` (Local) | `25568` | `velocity_modern` |
| **Survival Backend (Paper)** | `127.0.0.1` (Local) | `25569` | `velocity_modern` |
| **Fallback Hub (SteelMC)** | `127.0.0.1` (Local) | `25566` | `none` |

### Step-by-Step Paper Setup (Velocity Modern Forwarding)

Here is how to connect a local Paper server running on port `25568`:

1. In your `config.toml` (auto-generated on first launch), update `default_server` and add the backend definition:

```toml
bind_address = "0.0.0.0"
bind_port = 25565
motd = "§aFrameMC §7High-Performance Minecraft Proxy"
max_players = 1000
online_mode = true
default_server = "paper"
script_path = "scripts/main.rhai"
plugins_dir = "plugins"

[servers.paper]
address = "127.0.0.1"
port = 25568
forwarding_mode = "velocity_modern"
forwarding_secret = "make_this_a_random_64_char_secret_key"
```

2. Open `config/paper-global.yml` in your Paper server folder:
```yaml
proxies:
  velocity:
    enabled: true
    online-mode: true
    secret: "make_this_a_random_64_char_secret_key"
```

3. Open `server.properties` on Paper:
```properties
server-port=25568
online-mode=false
```
> **Critical**: Set `online-mode=false` on backend servers. FrameMC handles Mojang authentication at the proxy edge. If Paper also tries to authenticate, connections will fail with encryption errors.

4. Start Paper first, then start FrameMC. Connect your Minecraft client to `localhost:25565`.

For Spigot, Fabric, SteelMC, and Vanilla configs, check the [Configuration Reference](CONFIGURATION.md).

---

## Production Deployment

When deploying FrameMC to a production Linux host (Ubuntu, Debian, AlmaLinux, Arch):

### 1. Systemd Service (`/etc/systemd/system/framemc.service`)

Create a dedicated system user and service file so the proxy restarts automatically on boot or failure:

```ini
[Unit]
Description=FrameMC Minecraft Reverse Proxy
After=network.target

[Service]
Type=simple
User=minecraft
Group=minecraft
WorkingDirectory=/opt/framemc
ExecStart=/opt/framemc/framemc -c /opt/framemc/config.toml
Restart=always
RestartSec=5s

# Raise open file descriptor limits for high connection counts
LimitNOFILE=65536

# Security sandbox flags
NoNewPrivileges=true
ProtectSystem=full
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

Enable and start the service:
```bash
sudo systemctl daemon-reload
sudo systemctl enable --now framemc
```

Check live logs:
```bash
journalctl -u framemc -f
```

> **Note on Graceful Shutdown**: FrameMC handles POSIX `SIGTERM` and `SIGINT` natively via Tokio's Unix signal subsystem. When `systemctl stop framemc`, `docker stop`, or Kubernetes container termination initiates, the proxy immediately stops accepting new connections, flushes active in-flight packets, and cleanly terminates downstream sessions without abrupt connection drops.

### 2. macOS Deployment (`launchd` daemon)

On macOS servers, FrameMC can be managed as a native `launchd` daemon:

Create `/Library/LaunchDaemons/com.framemc.proxy.plist`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.framemc.proxy</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/framemc</string>
        <string>-c</string>
        <string>/etc/framemc/config.toml</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardErrorPath</key>
    <string>/var/log/framemc.err.log</string>
    <key>StandardOutPath</key>
    <string>/var/log/framemc.out.log</string>
</dict>
</plist>
```

Load and start the service:
```bash
sudo launchctl load -w /Library/LaunchDaemons/com.framemc.proxy.plist
```
When `launchctl stop` or `launchctl unload` is executed, macOS sends `SIGTERM`, triggering clean proxy termination.

### 3. File Descriptor Limits (`ulimit`)
Each active client connection and backend connection consumes a TCP socket (file descriptor). Ensure your host limits allow scaling:

```bash
# Check current limit
ulimit -n

# Set temporary limit in current shell
ulimit -n 65536
```
In `systemd`, `LimitNOFILE=65536` takes care of this automatically. On macOS, configure `kern.maxfiles` via `/etc/sysctl.conf` or `launchd`.

---

## Troubleshooting Initial Setup

When a proxy doesn't connect on the first try, the issue is almost always a port conflict, a firewall block, or mismatched forwarding secrets.

### Port already bound (`os error 98` or `os error 10048`)
Another process is already listening on port 25565. This is usually an old server instance, an abandoned Velocity daemon, or a previous run of FrameMC that wasn't killed properly.
- **Linux**: Run `ss -tulpn | grep 25565` or `lsof -i :25565` to find the holding PID, then terminate it with `kill -9 <PID>`.
- **Windows**: Run `netstat -ano | findstr 25565` in PowerShell, then run `taskkill /PID <PID> /F`.
- If you're running FrameMC behind HAProxy or a local reverse proxy, change `bind_port = 25577` in `config.toml`.

### `Connection refused (os error 111` or `10061)`
FrameMC accepted the player's connection, but when it reached out to the backend socket (`127.0.0.1:25566`), nothing was listening.
- Make sure your Minecraft backend is fully booted before connecting. Paper and Spigot often take 15 to 30 seconds to generate spawn chunks before opening their network socket.
- Verify that the port configured in `config.toml` matches `server-port` in your backend's `server.properties`.

### Paper kicks with `Unable to verify player details` or `Invalid signature`
Paper received the `velocity:player_info` login plugin message, calculated the HMAC-SHA256 signature, and found a mismatch.
- The `forwarding_secret` in `config.toml` must match `proxies.velocity.secret` in Paper's `paper-global.yml` byte-for-byte.
- Watch out for accidental leading spaces, trailing newlines, or extra quotation marks copied from web guides.
- Double-check that `proxies.velocity.enabled` is set to `true`.

### Client hangs on "Encrypting..." then disconnects
When `online_mode = true`, FrameMC reaches out to Mojang's session servers (`sessionserver.mojang.com`) over outbound HTTPS to verify account ownership.
- If outbound port 443 is blocked by host firewall rules or DNS lookups fail, the connection stalls until our 10-second authentication timeout fires.
- Run `curl -I https://sessionserver.mojang.com` on the host to verify connectivity.
- If you are developing locally without an active internet connection, set `online_mode = false` in `config.toml`.

### Protocol Version Mismatch (`Outdated server` or `Outdated client`)
FrameMC supports Minecraft Java Edition protocols 764 through 776+ (versions 1.20.4 to 1.21.4+).
- If a player connects with an older version (e.g. 1.16.5 or 1.12.2), FrameMC terminates the handshake to avoid protocol framing desynchronization.
- For mixed legacy networks, place a protocol translation layer (like ViaVersion / ViaProxy) either upstream or on your backend servers.

### Linux: `Permission denied (os error 13)` on Privileged Ports (<1024)
If you configure `bind_port` below 1024 (e.g. port 80 or 443) and run FrameMC as a non-root system user, the Linux kernel rejects the bind syscall.
- Grant socket bind capabilities directly to the binary:
  ```bash
  sudo setcap 'cap_net_bind_service=+ep' /opt/framemc/framemc
  ```
- Or keep FrameMC on port 25565 and let an edge firewall/HAProxy handle port 80/443 redirection.

### Players kicked when switching backends mid-game
If world transfers fail between two running servers:
- Check backend response times: If the target server takes longer than 5 seconds to reply during the Configuration handshake (often caused by main-thread stall during heavy chunk generation), FrameMC cancels the transfer to keep the client connection from freezing.
- Verify compression consistency: While FrameMC handles decoupled compression translation automatically, verify that backend servers aren't rejecting custom plugin channels sent by downstream mods.

---

## Quick Diagnostic Commands

| Diagnostic Task | Linux Command | Windows PowerShell Command |
| :--- | :--- | :--- |
| **Check listening port** | `ss -tulpn \| grep 25565` | `netstat -ano \| findstr 25565` |
| **Find process by port** | `lsof -i :25565` | `Get-Process -Id (Get-NetTCPConnection -LocalPort 25565).OwningProcess` |
| **Kill stuck process** | `kill -9 <PID>` | `taskkill /PID <PID> /F` |
| **Test Mojang auth API** | `curl -I https://sessionserver.mojang.com` | `curl.exe -I https://sessionserver.mojang.com` |
| **Follow live logs** | `journalctl -u framemc -f` | `Get-Content framemc.log -Wait -Tail 50` |
| **Run with trace logs** | `RUST_LOG=trace ./framemc` | `$env:RUST_LOG="trace"; .\framemc.exe` |

---

## Next Steps

- Explore every configuration key in the [Configuration Reference](CONFIGURATION.md).
- Write custom commands, permissions, and maintenance logic in [Scripting with Rhai](SCRIPTING.md).
- Inspect low-level packet framing and state transitions in [Architecture Specification](ARCHITECTURE.md).
- Review wire-level test coverage in [Testing Reference](TESTING.md).

