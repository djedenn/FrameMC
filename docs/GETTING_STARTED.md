# Getting Started with FrameMC

Getting FrameMC running takes about two minutes. Because it compiles to a standalone native binary, there's no Java installation to worry about, no classpath arguments to pass, and no tuning JVM garbage collectors before you start.

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

All 136 tests should pass cleanly without ignored or failing cases.

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

By default, FrameMC routes incoming connections to a backend named `lobby` at `127.0.0.1:25566`.

Here is how to connect a local Paper server running on port `25568`:

1. Open `config.toml` in your editor.
2. Configure Paper as your default server with modern Velocity forwarding enabled:

```toml
default_server = "paper"

[servers.paper]
address = "127.0.0.1"
port = 25568
forwarding_mode = "velocity_modern"
forwarding_secret = "replace_with_a_secure_random_token"
```

3. Open `config/paper-global.yml` in your Paper server directory:

```yaml
proxies:
  velocity:
    enabled: true
    online-mode: true
    secret: "replace_with_a_secure_random_token"
```

4. Set `online-mode=false` in Paper's `server.properties`. Because FrameMC authenticates players with Mojang, backend servers must not re-authenticate incoming connections.
5. Restart Paper, start FrameMC, and join via your client at `localhost:25565`.

For setup instructions covering Spigot, Fabric, SteelMC, and Vanilla, check the [Configuration Reference](CONFIGURATION.md).

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

### Players kicked when switching backends mid-game
If world transfers fail between two running servers:
- Check backend response times: If the target server takes longer than 5 seconds to reply during the Configuration handshake (often caused by main-thread stall during heavy chunk generation), FrameMC cancels the transfer to keep the client connection from freezing.
- Verify compression consistency: While FrameMC handles decoupled compression translation automatically, verify that backend servers aren't rejecting custom plugin channels sent by downstream mods.

---

## Next Steps

- Explore every configuration key in the [Configuration Reference](CONFIGURATION.md).
- Write custom commands, permissions, and maintenance logic in [Scripting with Rhai](SCRIPTING.md).
- Inspect low-level packet framing and state transitions in [Architecture Specification](ARCHITECTURE.md).
