# FrameMC Architecture & Engineering Specification

This document details the low-level architectural invariants, protocol wire layouts, and execution lifecycle of **FrameMC**—a high-performance, native Rust reverse proxy for Minecraft Java Edition.

---

## 1. Architectural Directives

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
Minecraft uses variable-length integers where each byte provides 7 bits of payload and 1 continuation bit (MSB):
- **MSB = 1**: More bytes follow.
- **MSB = 0**: Terminal byte.
- Max 5 bytes for 32-bit `VarInt`, max 10 bytes for 64-bit `VarLong`.

```text
Byte 0          Byte 1          Byte 2
[ 1 | bbbbbbb ] [ 1 | bbbbbbb ] [ 0 | bbbbbbb ]
  ▲               ▲               ▲
  Continuation    Continuation    Terminal (MSB = 0)
```

### 2.2 Mojang SHA-1 Negative Hash
Authentication in `online_mode = true` requires calculating a server hash over the empty server ID string, the shared secret, and the proxy's public key in DER format:
```text
digest = SHA-1( "" + shared_secret + public_key_der )
```
The digest is formatted as a two's-complement big-endian signed integer. If the most significant bit is set (negative), the two's complement value is prepended with a `-` sign.

### 2.3 Velocity Modern Forwarding Packet
Velocity modern player info forwarding uses the plugin message channel `velocity:player_info`:

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

---

## 3. Dynamic Server Switching Flow

When a player triggers a server switch (via `/server <target>` or a Rhai script):

```
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

1. **New Backend Handshake & Login**: FrameMC connects to the target backend and completes Handshake and Login states independently.
2. **Compression Decoupling**: If the target backend requires compression or uses a different compression threshold than the client, FrameMC manages separate compression contexts per socket without corrupting deflate streams.
3. **Configuration Phase Negotiation**: FrameMC consumes the target backend's Configuration state packets (`KnownPacks`, `RegistryData`, `UpdateTags`) and caches updated registries.
4. **Respawn Packet Synthesis**: A clientbound `RespawnPacket` is generated using the target backend's dimension data, cleanly switching the client's world rendering without a disconnect screen.
5. **Play Stream Splicing**: Sockets transition to the bidirectional raw I/O bridge.
