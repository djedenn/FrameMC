# Getting Started with FrameMC

So you want to run FrameMC. This guide walks you through building from source, spinning up your first instance, and wiring it to your backends without tearing your hair out over JVM flags or network sockets.

---

## System Requirements

FrameMC compiles down to a single self-contained native binary. You do not need Java, a JVM runtime, or any external shared libraries installed on the host.

- **Operating System**:
  - Linux (kernel 4.x+, glibc 2.17+ or musl)
  - Windows 10 / 11 / Server 2016+ (x86_64)
  - macOS 12+ (Apple Silicon or Intel)
- **Memory**: ~15 MB RSS baseline. Unlike JVM-based proxies that hold onto hundreds of megabytes for heap pools and garbage collector structures, FrameMC will comfortably run on a 512 MB VPS alongside other services.
- **CPU**: Any modern x86_64 (SSE4.2 recommended for fast crypto/hashing) or aarch64 core.
- **Build Toolchain**: Rust 1.80 or newer (`rustc` and `cargo`). Check your version with `rustc --version`.

---

## Building from Source

Building takes about two minutes on a standard developer machine.

### 1. Grab the repository

```bash
git clone https://github.com/djedenn/FrameMC.git
cd FrameMC
```

### 2. Compile a release build

Always use `--release`. Unoptimized debug builds include heavy runtime assertion checks and skip link-time optimization, which severely tanks AES cipher and packet framing throughput.

```bash
cargo build --release
```

Once Cargo finishes compiling dependencies (Tokio, Rhai, RSA, Flate2), you'll find the finished binary at:
- **Linux / macOS**: `target/release/framemc`
- **Windows**: `target\release\framemc.exe`

### 3. Verify the build locally

Before deploying, run the test suite to confirm your local platform and toolchain pass all wire-level verification tests:

```bash
cargo test --all-targets
```

All 136 tests should pass cleanly without ignored or failing cases.

---

## First Run & Bootstrapping

Running FrameMC for the first time is straightforward:

```bash
# On Linux / macOS
./target/release/framemc

# On Windows
.\target\release\framemc.exe
```

If no `config.toml` exists in the working directory, FrameMC automatically writes a clean, documented template and binds to `0.0.0.0:25565`.

You'll see log output similar to this:

```text
2026-09-08T14:20:00Z  INFO framemc: Starting FrameMC Minecraft Proxy using config: config.toml
2026-09-08T14:20:00Z  INFO framemc: Loaded 21 plugin(s) from 'plugins'
2026-09-08T14:20:00Z  INFO framemc: Loaded script at 'scripts/main.rhai'
2026-09-08T14:20:00Z  INFO framemc::network::listener: FrameMC listener bound to 0.0.0.0:25565 (online_mode: true)
```

At this stage, you have a live proxy listening for Minecraft connections.

---

## CLI Options & Flags

FrameMC keeps command-line arguments intentionally minimal:

```text
Usage: framemc [OPTIONS]

Options:
  -c, --config <PATH>  Path to configuration file [default: config.toml]
  -V, --version        Display version information
  -h, --help           Display help message
```

### Running with a custom configuration file
If you manage multiple environments or test nodes on one box:

```bash
./framemc -c /etc/framemc/staging.toml
```

### Adjusting log verbosity
FrameMC uses `tracing-subscriber` wired to your environment. To increase or filter log output:

```bash
# Debug logging across the entire proxy
RUST_LOG=debug ./framemc

# Focus strictly on network handshakes and script execution
RUST_LOG=framemc::network=debug,framemc::script=trace ./framemc
```

On Windows PowerShell:
```powershell
$env:RUST_LOG="debug"; .\framemc.exe
```

---

## Connecting Your First Backend

By default, FrameMC looks for a backend named `lobby` on `127.0.0.1:25566`.

Let's say you have a Paper server running locally on port `25568`:

1. Open `config.toml` in your text editor.
2. Point your default server to Paper and enable modern forwarding:

```toml
default_server = "paper"

[servers.paper]
address = "127.0.0.1"
port = 25568
forwarding_mode = "velocity_modern"
forwarding_secret = "replace_with_a_secure_random_token"
```

3. In your Paper server root, open `config/paper-global.yml`:

```yaml
proxies:
  velocity:
    enabled: true
    online-mode: true
    secret: "replace_with_a_secure_random_token"
```

4. Set `online-mode=false` in Paper's `server.properties` (the proxy handles Mojang authentication, so backend servers must not re-authenticate incoming connections).
5. Restart Paper, launch FrameMC, and join via your Minecraft client at `localhost:25565`.

For full setup guides covering Spigot, Fabric, SteelMC, and Vanilla, see the [Configuration Reference](CONFIGURATION.md).

---

## Troubleshooting Common Setup Traps

Here are the most frequent snags admins hit during initial setup and how to fix them quickly:

### 1. `Address already in use (os error 98 / 10048)`
- **Why**: Another process is already bound to port 25565 (often an existing Minecraft server, an old Velocity instance, or a zombie proxy process).
- **Fix**:
  - Linux: Run `sudo ss -tulpn | grep 25565` or `lsof -i :25565` to find the offending PID, then kill it.
  - Windows: Run `netstat -ano | findstr 25565` in PowerShell, then run `taskkill /PID <PID> /F`.
  - Alternatively, change `bind_port = 25577` in `config.toml`.

### 2. `Backend connection failed: Connection refused (os error 111 / 10061)`
- **Why**: The proxy tried connecting to the destination server (e.g. `127.0.0.1:25566`), but nothing is listening on that address and port.
- **Fix**: Verify your backend server is fully booted before connecting. Check that the port in `config.toml` matches the `server-port` in your backend's `server.properties`.

### 3. Paper kicks with `Unable to verify player details` or `Invalid signature`
- **Why**: The `forwarding_secret` in `config.toml` does not match `proxies.velocity.secret` in Paper's `paper-global.yml`.
- **Fix**: Ensure both strings match character-for-character. Watch out for accidental leading or trailing spaces and newlines. Also confirm `proxies.velocity.enabled` is set to `true`.

### 4. Client hangs on "Encrypting..." then disconnects
- **Why**: The proxy is running with `online_mode = true`, but the server machine cannot reach Mojang's session servers (`sessionserver.mojang.com`) over HTTPS due to firewall rules or outbound DNS failures.
- **Fix**: Test outbound connectivity with `curl https://sessionserver.mojang.com`. If you are developing locally without an active internet connection, set `online_mode = false` in `config.toml`.

### 5. Players get kicked when switching servers
- **Why**: Downstream backends have mismatched network configurations (e.g., one backend enforces network compression with a threshold of 256, while the other runs with compression disabled).
- **Fix**: FrameMC manages decoupled compression translation automatically. However, if a backend takes longer than 5 seconds to reply to the Configuration handshake, FrameMC aborts the switch to prevent the client connection from freezing. Ensure your backend isn't choking on massive world generation or main-thread lag during joins.

---

## Next Steps

- Check out the full [Configuration Guide](CONFIGURATION.md) for every option in `config.toml`.
- Learn how to write custom commands, whitelist logic, and routing rules in [Scripting with Rhai](SCRIPTING.md).
- Inspect the protocol wire flow and state machine design in [Architecture Specification](ARCHITECTURE.md).
