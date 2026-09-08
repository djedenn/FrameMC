# Contributing to FrameMC

Thank you for your interest in contributing to FrameMC! FrameMC is a high-performance native Rust reverse proxy for Minecraft networks designed for deterministic performance, zero-copy forwarding, and zero JVM garbage collection pauses.

## Table of Contents
- [Code Architecture Directives](#code-architecture-directives)
- [Development Setup](#development-setup)
- [Testing & Quality Assurance](#testing--quality-assurance)
- [Submitting Pull Requests](#submitting-pull-requests)
- [Code Style & Conventions](#code-style--conventions)

---

## Code Architecture Directives

When contributing code to FrameMC, adhere to our core engineering invariants:

1. **Zero Custom Primitive Parsers**: Always use `tokio::io::AsyncReadExt` and `bytes::{Buf, BufMut, BytesMut}`. Never implement custom ad-hoc bit-shifting VarInt/VarLong parsers if an existing module utility covers it.
2. **Zero-Copy Play Relaying**: In the `Play` state, uninspected packets are routed directly using `tokio::io::copy_bidirectional` or split duplex streams without user-space re-serialization.
3. **Async Safety**: Never hold a standard `std::sync::Mutex` across an `.await` point. Use `tokio::sync::Mutex` or message channels (`tokio::sync::mpsc`).
4. **Memory Hygiene & Cryptography**: Sensitive cryptographic keys (shared secrets, private keys, HMAC tokens) must be zeroized on drop using the `zeroize` crate and never logged.
5. **Strict Protocol Decoupling**: Read the handshake protocol version, preserve it down to the backend, and never hardcode client versions in network routing structures.
6. **Velocity Modern Forwarding**: Modern backend forwarding must follow the Velocity HMAC-SHA256 specification (`velocity:player_info`). Legacy null-byte host appending is only used when explicitly configured for BungeeCord backends.
7. **Fail-Closed Stream Termination**: Any framing error, decompression error, or unexpected EOF during Handshake, Login, or Configuration must terminate client and backend socket handles immediately.
8. **Deterministic Script Limits**: Embedded Rhai scripts enforce maximum 50,000 opcodes, recursion call stack depth of 32, and maximum string sizes of 1,024 bytes.

---

## Development Setup

### Prerequisites
- [Rust 1.80+](https://www.rust-lang.org/tools/install) (stable toolchain)
- Cargo

### Clone & Build
```bash
git clone https://github.com/djedenn/FrameMC.git
cd FrameMC
cargo build --release
```

For detailed architecture notes and setup guides, refer to the [Documentation Suite](docs/GETTING_STARTED.md).

---

## Testing & Quality Assurance

FrameMC maintains 100% test pass rates across both unit and end-to-end integration tests. Before opening a pull request, ensure all checks pass:

### 1. Run Unit & Integration Tests
```bash
cargo test --all-targets -- --nocapture
```

### 2. Run Clippy Linter
No warnings are permitted:
```bash
cargo clippy --all-targets -- -D warnings
```

### 3. Check Code Formatting
Code must follow standard `rustfmt` conventions:
```bash
cargo fmt --check
```

---

## Submitting Pull Requests

1. **Fork & Branch**: Create a feature branch with a descriptive name (`git checkout -b feat/my-feature` or `git checkout -b fix/issue-desc`).
2. **Write Tests First**: Any new feature or bugfix must include unit tests parsing real byte vectors or integration tests exercising state transitions.
3. **Commit Messages**: Follow conventional commits (`feat:`, `fix:`, `refactor:`, `docs:`, `test:`).
4. **Clean Diff**: Keep pull requests focused on a single change. Avoid unrelated reformatting or dependency additions.

---

## Code Style & Conventions

- Format code using `cargo fmt` prior to committing.
- Avoid `.unwrap()` and `.expect()` in non-test production code. Propagate errors via `Result<T, FrameError>` and handle fallbacks gracefully.
- Group imports logically: standard library first, external crates second, internal crate modules third.
