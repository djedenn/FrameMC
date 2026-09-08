# FrameMC Architecture & Protocol Specification

This document details the low-level architectural invariants, protocol wire layouts, and execution lifecycle of **FrameMC**—a high-performance, native Rust reverse proxy for Minecraft Java Edition.

If you are looking for configuration directives, setup guides, or test suites, see:
- [Getting Started Guide](GETTING_STARTED.md)
- [Configuration Reference](CONFIGURATION.md)
- [Rhai Scripting Guide](SCRIPTING.md)
- [Automated Test Suite & Protocol Verification](TESTING.md)

---

## 1. Architectural Directives

FrameMC enforces twelve non-negotiable architectural invariants across its codebase:

| Rule ID | Name | Specification |
| :--- | :--- | :--- |
| **[R-01]** | **Zero Custom Primitive Parsers** | Use `tokio::io::AsyncReadExt` and `bytes::{Buf, BufMut, BytesMut}` for VarInt, VarLong, and packet framing. Never invent ad-hoc byte streaming when established byte buffer abstractions exist. |
| **[R-02]** | **Zero-Copy Forwarding** | Once players enter the Play state, uninspected packet streams route directly using `tokio::io::copy_bidirectional` across raw socket file descriptors. |
| **[R-03]** | **Explicit Async Safety** | Never hold a `std::sync::Mutex` across an `.await` boundary. Use `tokio::sync` primitives or message-passing channels (`tokio::sync::mpsc`, `tokio::sync::watch`). |
| **[R-04]** | **Deterministic Script Limits** | The embedded Rhai scripting engine enforces hard runtime boundaries: max 50,000 operations, max call stack depth of 32, and max string allocation of 1,024 bytes. |
| **[R-05]** | **Pure Memory Handshakes** | Cryptographic secrets (AES shared secrets, RSA private keys, HMAC tokens) are zeroized on drop via the `zeroize` crate and never logged. |
| **[R-06]** | **Strict Version Decoupling** | Handshake protocol versions are dynamically extracted from incoming client packets and propagated to backend handshakes. Network routing state structures do not hardcode client versions. |
| **[R-07]** | **Velocity Modern Forwarding** | Modern downstream backends use the Velocity player info forwarding protocol (`velocity:player_info` channel with HMAC-SHA256). Legacy null-byte host appending is maintained solely for legacy BungeeCord backends. |
| **[R-08]** | **Fail-Closed Stream Termination** | Any decode failure, VarInt overflow, or unexpected EOF during Handshake, Login, or Configuration phases immediately terminates both client and backend socket handles. |
| **[R-09]** | **Non-Blocking Script Dispatch** | Script lifecycle hooks (`on_player_join`, `on_player_command`, `on_tab_complete`) must be fast-returning predicates. Background work or external network requests are offloaded to background worker channels. |
| **[R-10]** | **Registry Codec Caching** | Configuration state registry packets (`minecraft:dimension_type`, biomes, damage types) are intercepted and cached per backend during initial join to synthesize clean client respawns during transfers. |
| **[R-11]** | **Graceful Backend Re-routing** | If an active backend disconnects mid-game, FrameMC intercepts the Disconnect packet, suppresses the client-side disconnect screen, synthesizes a modern Respawn packet, and routes the player to the configured fallback server. |
| **[R-12]** | **Test-Driven Wire Verification** | Every protocol codec, packet framing routine, and crypto transformation is backed by unit tests parsing real byte vectors against official protocol specifications. |

---

## 2. Technical Protocol Wire Layouts

### 2.1 VarInt & VarLong Encoding
Minecraft frames all packet lengths and identifiers using variable-length LEB128 integers. Each byte contributes 7 payload bits and 1 continuation bit in the most significant bit (MSB):
- **MSB = 1**: Another byte follows.
- **MSB = 0**: Terminal byte.
- Max 5 bytes for 32-bit `VarInt`, max 10 bytes for 64-bit `VarLong`.

```text
Byte 0          Byte 1          Byte 2
[ 1 | bbbbbbb ] [ 1 | bbbbbbb ] [ 0 | bbbbbbb ]
  ▲               ▲               ▲
  Continuation    Continuation    Terminal (MSB = 0)
```

Because an unconstrained VarInt reader could consume unbounded memory if a hostile client keeps streaming bytes with the MSB set, FrameMC validates the 5-byte and 10-byte ceilings on every read before allocating or expanding buffers. Any sequence exceeding these limits terminates the connection immediately (`Fail-Closed`).

### 2.2 Mojang SHA-1 Negative Hash
When `online_mode = true`, client authentication requires calculating a specialized hash over the empty server ID string, the shared secret negotiated via RSA, and the proxy's public key in DER format:

```text
digest = SHA-1( "" + shared_secret + public_key_der )
```

Minecraft formats this digest not as standard raw hex, but as a big-endian signed two's-complement integer (an idiosyncratic artifact of Java's `BigInteger(byte[]).toString(16)`). If the most significant bit is set (negative), the two's-complement value is prepended with a `-` sign in the hex string sent to Mojang's session servers.

### 2.3 Velocity Modern Forwarding Wire Layout
Modern downstream servers (Paper, Purpur, Folia, FabricProxy-Lite) receive client identity and profile properties via the `velocity:player_info` login plugin message channel:

```text
+-------------------------------------------------------------+
|                     HMAC-SHA256 (32 Bytes)                  |
|  HMAC signature over all remaining payload bytes using     |
|  the shared secret configured in config.toml                |
+-------------------------------------------------------------+
| Forwarding Version (VarInt = 4)                             |
+-------------------------------------------------------------+
| Remote Client IP Address (UTF-8 String)                     |
+-------------------------------------------------------------+
| Player UUID (16 Raw Big-Endian Bytes)                       |
+-------------------------------------------------------------+
| Player Username (UTF-8 String)                              |
+-------------------------------------------------------------+
| Properties Count (VarInt)                                   |
|   ├── Name (UTF-8 String, e.g. "textures")                  |
|   ├── Value (UTF-8 String)                                  |
|   └── Signature (Optional UTF-8 String)                     |
+-------------------------------------------------------------+
```

The HMAC signature protects the player payload from forged UUIDs or spoofed IP addresses. If the backend fails to verify the HMAC token, the connection is rejected before the player enters the world.

---

## 3. Dynamic Server Switching Flow

When a player triggers a server transfer (through `/server <target>` or a script redirect):

```text
Client                      FrameMC Proxy                  Target Backend
  │                              │                               │
  │── (In Play State) ──────────>│                               │
  │   /server lobby              │── Connect TCP ───────────────>│
  │                              │<── SetCompression (0x03) ─────│ (optional)
  │                              │── LoginAcknowledged (0x03) ──>│
  │                              │<── Config Phase Handshake ────│
  │                              │── FinishConfig (0x02) ───────>│
  │<── Synthesized Respawn (0x4B)│                               │
  │                              │<── Login (Play) (0x2B/0x29) ──│
  │══════════════════════════════╪═══════════════════════════════│
  │       Direct Zero-Copy Forwarding (copy_bidirectional)       │
  │══════════════════════════════╪═══════════════════════════════│
```

1. **Independent TCP Handshake**: FrameMC connects to the target backend and completes Handshake and Login states independently without touching the existing client socket.
2. **Compression Decoupling**: If the target backend requires compression or uses a different threshold than the client, FrameMC manages separate compression contexts per socket without corrupting deflate streams.
3. **Configuration Phase Negotiation**: FrameMC consumes the target backend's Configuration state packets (`KnownPacks`, `RegistryData`, `UpdateTags`) and updates its cached registry state.
4. **Respawn Packet Synthesis**: A clientbound `Respawn` packet is generated using the target backend's dimension data, cleanly switching the client's world rendering without triggering a disconnect screen.
5. **Play Stream Splicing**: Both sockets transition to `tokio::io::copy_bidirectional`. Gameplay packets flow directly through kernel socket buffers without user-space allocation overhead.

---

## 4. Play Bridge & Socket Splicing Design

Once a connection enters the `Play` state, FrameMC steps back from packet parsing. 

Traditional Java proxies keep an active Netty channel pipeline alive throughout the entire session. Every inbound movement, chunk update, and entity metadata packet is read into a JVM byte buffer, parsed into an object, matched against channel handlers, and written back to the outbound socket. This creates high allocations on the hot path and continuous young-gen garbage collector churn.

FrameMC's bridge uses Tokio's `copy_bidirectional`:
- **Kernel-Level Relaying**: Operating system socket buffers transfer bytes directly with minimal context switching.
- **Immediate FIN Teardown**: When either the client closes their connection or the backend shuts down, EOF signals propagate immediately to the counterpart socket, avoiding lingering half-open sockets or file descriptor leaks.
- **Zero Heap Churn**: Zero packet structures are allocated in user-space during raw gameplay streaming, maintaining an idle RSS footprint of ~12–22 MB even under load.
