# FrameMC Automated Test Suite & Protocol Verification

Every codec, state machine transition, and cryptographic routine in FrameMC is verified against official Minecraft protocol specifications (1.20.4 through 1.21.4+, protocols 764 to 776+). 

Tests do not merely inspect mocked in-memory structures—they bind real loopback TCP sockets to validate raw byte-for-byte serialization, cipher state preservation, framing alignment, and error handling on the wire.

For architecture details, setup steps, or configuration options, see:
- [Architecture & Invariants](ARCHITECTURE.md)
- [Getting Started Guide](GETTING_STARTED.md)
- [Configuration Reference](CONFIGURATION.md)
- [Rhai Scripting Guide](SCRIPTING.md)

## Table of Contents
- [Test Execution Summary](#test-execution-summary)
- [Developer Test Commands](#developer-test-commands)
- [1. Cryptography & Session Authentication (9 Tests)](#1-cryptography--session-authentication-9-tests)
- [2. Protocol Wire Formats, Handshake & Compression (24 Tests)](#2-protocol-wire-formats-handshake--compression-24-tests)
- [3. Login Authentication & Forwarding Handshakes (25 Tests)](#3-login-authentication--forwarding-handshakes-25-tests)
- [4. Modern Configuration & Registry Caching (7 Tests)](#4-modern-configuration--registry-caching-7-tests)
- [5. Play State Machine, Server Switching & Bridge (46 Tests)](#5-play-state-machine-server-switching--bridge-46-tests)
- [6. Sandboxed Rhai Scripting Engine & Plugins (18 Tests)](#6-sandboxed-rhai-scripting-engine--plugins-18-tests)
- [7. Configuration, Network Listener & Integration Tests (22 Tests)](#7-configuration-network-listener--integration-tests-22-tests)
- [8. Command-Line Interface & Application Lifecycle (6 Tests)](#8-command-line-interface--application-lifecycle-6-tests)

---

## Test Execution Summary

```text
===============================================================================
Total Test Invocations:   157
Passed:                   157
Failed:                     0
Ignored / Filtered:         0
Success Rate:             100%
Linter Compliance:        cargo clippy --all-targets -- -D warnings (0 warnings)
Formatting Compliance:    cargo fmt --check (100% compliant)
Tested Protocols:         Minecraft 1.20.4 through 1.21.4+ (Protocols 764 – 776+)
===============================================================================
```

## Developer Test Commands

### Bash (Linux / macOS):
```bash
# Run all 157 tests
cargo test --all-targets

# Run tests with real-time names and stdout/stderr output
cargo test --all-targets -- --nocapture

# Run a specific test by name filter
cargo test test_mojang_sha1_known_vectors

# Run only integration tests in tests/
cargo test --test '*'

# Run with debug tracing enabled for network inspection
RUST_LOG=framemc=debug cargo test test_full_status_ping_flow_over_tcp -- --nocapture
```

### PowerShell (Windows):
```powershell
# Run all 152 tests
cargo test --all-targets

# Run tests with verbose output
cargo test --all-targets -- --nocapture

# Run a specific test by name filter
cargo test test_mojang_sha1_known_vectors

# Run with debug tracing enabled
$env:RUST_LOG="framemc=debug"; cargo test test_full_status_ping_flow_over_tcp -- --nocapture
```

---

## 1. Cryptography & Session Authentication (9 Tests)

Authentication is where proxies often introduce subtle security bugs or memory leaks. Here we verify two critical aspects: first, that our RSA-1024 public key export in SubjectPublicKeyInfo DER format and PKCS#1 v1.5 decryption match Mojang's specification. Second, that our AES-128-CFB8 stream cipher preserves its continuous keystream state across variable TCP chunk fragmentations (testing 1-byte, 7-byte, and 1,024-byte chunks).

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_rsa_key_manager_der_export` | `crypto` | Export of 1024-bit RSA public key in standard X.509 SubjectPublicKeyInfo DER format | ✅ Passed |
| `test_rsa_encrypt_decrypt_roundtrip_16_bytes` | `crypto` | PKCS#1 v1.5 encryption and decryption roundtrip of 16-byte symmetric shared secrets | ✅ Passed |
| `test_encrypted_stream_bidirectional_roundtrip` | `crypto::aes` | Bidirectional stream cipher roundtrip with independent encryptor/decryptor states | ✅ Passed |
| `test_wire_bytes_are_actually_encrypted` | `crypto::aes` | Verifies on-wire byte stream differs from plaintext and contains no raw leakage | ✅ Passed |
| `test_multiple_consecutive_chunks_preserve_cipher_state` | `crypto::aes` | Variable chunk sizes (1 byte, 7 bytes, 1024 bytes) preserving continuous CFB8 keystream | ✅ Passed |
| `test_mojang_sha1_known_vectors` | `crypto::mojang_auth` | Known Mojang SHA-1 test vectors with two's-complement signed hex string representation | ✅ Passed |
| `test_mojang_sha1_combined_components` | `crypto::mojang_auth` | Multi-byte combined server ID, shared secret, and DER public key hashing | ✅ Passed |
| `test_offline_profile_generation` | `crypto::mojang_auth` | Deterministic MD5 UUID v3 generation matching Minecraft offline player spec | ✅ Passed |
| `test_player_profile_json_deserialization` | `crypto::mojang_auth` | Session server JSON profile parsing with property arrays (textures, skins, signatures) | ✅ Passed |

---

## 2. Protocol Wire Formats, Handshake & Compression (24 Tests)

Minecraft's VarInt format uses 7 bits per byte with the most significant bit as a continuation flag. An unconstrained parser could allow an attacker to stream bytes with the MSB set until memory runs out. We test boundary vectors (0, max 32-bit/64-bit bounds, negatives) and confirm that over-length VarInts fail closed immediately. We also verify zlib threshold handling and decompression bomb limits.

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_wire_specification_vectors` | `protocol::varint` | Boundary vectors: 0, 1, 127, 128, 255, 2147483647, -1, -2147483648 | ✅ Passed |
| `test_varlong_roundtrip_and_overflow` | `protocol::varint` | 64-bit VarLong encoding and 10-byte wire limit enforcement | ✅ Passed |
| `test_buffer_underflow` | `protocol::varint` | Truncated byte buffers fail closed with immediate `BufferUnderflow` error | ✅ Passed |
| `test_varint_overflow` | `protocol::varint` | VarInts exceeding 5 bytes are rejected to prevent memory exhaustion attacks | ✅ Passed |
| `test_decode_standard_vanilla_handshake` | `protocol::handshake` | Clean decoding of standard vanilla client Handshake packet (0x00) | ✅ Passed |
| `test_decode_status_handshake` | `protocol::handshake` | Handshake with `next_state = 1` routed to Server List Ping handler | ✅ Passed |
| `test_decode_bungeecord_forward_string` | `protocol::handshake` | Null-separated host string parsing (`host\0ip\0uuid`) | ✅ Passed |
| `test_truncated_handshake` | `protocol::handshake` | Malformed/incomplete handshake wire streams return cleanly rejected errors | ✅ Passed |
| `test_invalid_next_state` | `protocol::handshake` | Handshakes with state outside [1, 2] rejected immediately | ✅ Passed |
| `test_invalid_packet_id` | `protocol::handshake` | Non-zero packet ID during Handshake state fails closed | ✅ Passed |
| `test_write_packet` | `network::codec` | Direct packet framing with length prepending | ✅ Passed |
| `test_uncompressed_packet_roundtrip_below_threshold` | `network::codec` | Zero uncompressed length header when packet size < compression threshold | ✅ Passed |
| `test_compressed_packet_roundtrip_above_threshold` | `network::codec` | Zlib compression, uncompressed length header, and deflated payload roundtrip | ✅ Passed |
| `test_compressed_packet_below_threshold_rejected` | `network::codec` | Deflated packets below declared threshold rejected as invalid wire layout | ✅ Passed |
| `test_decompression_bomb_clamped` | `network::codec` | Malicious decompression payloads exceeding safe limits clamped | ✅ Passed |
| `test_encode_packet_with_compression_exact_wire_bytes` | `network::codec` | Byte-for-byte wire verification against known reference packets | ✅ Passed |
| `test_roundtrip_write_then_read` | `network::codec` | Framing codec write-to-read integrity across mock duplex streams | ✅ Passed |
| `test_read_back_to_back_packets` | `network::codec` | Consecutive back-to-back packets read sequentially without byte drift | ✅ Passed |
| `test_packet_too_large_rejection` | `network::codec` | Payloads exceeding max packet limit (> 2 MiB) rejected before allocation | ✅ Passed |
| `test_set_compression_packet_roundtrip` | `network::codec` | Login `SetCompression` (0x03) threshold serialization and parsing | ✅ Passed |
| `test_disconnect_packet_encoding` | `network::codec` | Text component disconnect packet serialization | ✅ Passed |
| `test_status_json_schema` | `protocol::status` | JSON response matches Minecraft 1.20+ client schema with player samples | ✅ Passed |
| `test_ping_pong_timestamp_symmetry` | `protocol::status` | 64-bit client payload in PingRequest (0x01) mirrored exactly in Pong (0x01) | ✅ Passed |
| `test_client_disconnect_after_status_response` | `protocol::status` | Graceful TCP socket shutdown after status payload delivery | ✅ Passed |

---

## 3. Login Authentication & Forwarding Handshakes (25 Tests)

Forwarding player identity to backend servers requires strict HMAC-SHA256 signing under the modern Velocity specification. These tests validate signature computation, null-byte host rewrites for legacy BungeeCord, and LoginSuccess packet schemas across protocol versions. We also verify that unauthenticated clients cannot inject spoofed internal proxy channels.

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_login_start_decode_and_encode` | `protocol::login` | LoginStart (0x00) username and UUID extraction across protocol versions | ✅ Passed |
| `test_login_start_empty_username` | `protocol::login` | Empty username validation and immediate socket drop | ✅ Passed |
| `test_login_start_username_too_long` | `protocol::login` | Usernames exceeding 16 characters rejected at gate | ✅ Passed |
| `test_login_start_invalid_chars` | `protocol::login` | Usernames containing illegal characters rejected | ✅ Passed |
| `test_login_packet_wrong_id` | `protocol::login` | Unexpected packet IDs during Login state rejected | ✅ Passed |
| `test_login_acknowledged_packet` | `protocol::login` | Modern Configuration transition packet (0x03) wire framing | ✅ Passed |
| `test_login_success_legacy_version_roundtrip` | `protocol::login` | Pre-1.20.2 LoginSuccess (0x02) serialization without modern session ID | ✅ Passed |
| `test_login_success_modern_roundtrip_with_properties` | `protocol::login` | LoginSuccess with textures, skins, and Mojang cryptographic signatures | ✅ Passed |
| `test_login_success_golden_bytes_player_nil_uuid` | `protocol::login` | Exact byte comparison against golden fixture for offline Steve profile | ✅ Passed |
| `test_login_success_steelmc_golden_bytes` | `protocol::login` | Golden byte match against SteelMC high-performance server expectations | ✅ Passed |
| `test_login_success_protocol_776_standard_wire_layout` | `protocol::login` | Session ID and properties inclusion for 1.21.4+ (Protocol 776) | ✅ Passed |
| `test_login_success_offline_player_frame_serialization` | `protocol::login` | Offline UUID v3 profile serialization | ✅ Passed |
| `test_encode_login_success_modern_offline_exact_bytes` | `protocol::login` | Byte-for-byte modern offline LoginSuccess layout verification | ✅ Passed |
| `test_encryption_request_generate_and_roundtrip` | `protocol::login` | Generation of 4-byte verify tokens and public key payload | ✅ Passed |
| `test_encryption_request_modern_version_776_should_authenticate` | `protocol::login` | Authentication flag enforcement for Protocol 776+ | ✅ Passed |
| `test_encryption_response_roundtrip` | `protocol::login` | Decryption of client shared secret and verify token match | ✅ Passed |
| `test_velocity_hmac_and_payload_layout` | `protocol::forwarding` | HMAC-SHA256 signature matching Velocity specification (`velocity:player_info`) | ✅ Passed |
| `test_velocity_missing_secret_error` | `protocol::forwarding` | Backend defined with `velocity_modern` without secret fails closed | ✅ Passed |
| `test_velocity_backend_online_mode_true_warning_error` | `protocol::forwarding` | Catching misconfigured downstream backends requesting authentication | ✅ Passed |
| `test_dispatch_forwarding_velocity_flow` | `protocol::forwarding` | Successful negotiation of `velocity:player_info` login plugin message | ✅ Passed |
| `test_dispatch_forwarding_bungeecord_flow` | `protocol::forwarding` | Handshake host modification for legacy BungeeCord backends | ✅ Passed |
| `test_bungeecord_handshake_formatting` | `protocol::forwarding` | Host formatting: `host\0client_ip\0uuid` | ✅ Passed |
| `test_login_plugin_request_response_codecs` | `protocol::forwarding` | Login plugin query and response packet serialization | ✅ Passed |
| `test_connect_and_forward_tcp` | `protocol::forwarding` | Real TCP connection and forwarding negotiation loop | ✅ Passed |
| `test_is_protected_plugin_channel` | `protocol::packet` | Rejection of spoofed proxy channels from unauthenticated clients | ✅ Passed |

---

## 4. Modern Configuration & Registry Caching (7 Tests)

Minecraft 1.20.2 overhauled connection handshakes by introducing an independent Configuration state. If a proxy does not intercept and cache dimension codecs and registry tags, transferring between servers running different world types causes the client to desync or crash. These tests verify non-destructive capture and replay of registry packets.

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_finish_configuration_packet` | `protocol::configuration` | FinishConfiguration (0x02 / 0x03) packet serialization | ✅ Passed |
| `test_registry_entries_roundtrip` | `protocol::configuration` | Dimension and biome registry entries serialization and parsing | ✅ Passed |
| `test_registry_data_encode_decode_roundtrip` | `protocol::configuration` | Full registry data packet encoding/decoding roundtrip | ✅ Passed |
| `test_invalid_packet_id_rejection` | `protocol::configuration` | Malformed packet IDs rejected during configuration exchange | ✅ Passed |
| `test_session_cache_capture_without_corruption` | `protocol::configuration` | Non-destructive registry capture during live client connection | ✅ Passed |
| `test_cache_replay_to_stream` | `protocol::configuration` | Replaying cached registry codecs to downstream server on switch | ✅ Passed |
| `test_process_clientbound_packet_actions` | `protocol::configuration` | Parsing clientbound configuration packets into cache actions | ✅ Passed |

---

## 5. Play State Machine, Server Switching & Bridge (46 Tests)

Once players enter the Play state, FrameMC routes commands and switches servers without dropping the client socket. We verify Brigadier command tree injection (so `/server` and `/lobby` appear in the client's tab-completion HUD), mid-game transfers across different compression thresholds, state sanitization (closing container GUIs, stopping active audio, clearing scoreboards and bossbars), and failover routing when an active backend crashes unexpectedly.

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_bridge_bidirectional_5mb_integrity` | `network::bridge` | 5 Megabytes of continuous bidirectional packet data streamed without loss | ✅ Passed |
| `test_bridge_early_client_eof_shuts_down_backend` | `network::bridge` | Client socket FIN cleanly shuts down backend socket | ✅ Passed |
| `test_bridge_shutdown_signal_terminates_immediately` | `network::bridge` | Immediate clean teardown on proxy shutdown broadcast | ✅ Passed |
| `test_chat_command_packet_roundtrip` | `routing::state_machine` | Chat command packet (0x04) serialization across protocols | ✅ Passed |
| `test_command_packet_detection_across_protocols` | `routing::state_machine` | Identifying command packets across versions 764 through 776+ | ✅ Passed |
| `test_command_interception_and_server_switch` | `routing::state_machine` | Intercepting `/server <target>` and initiating server transition | ✅ Passed |
| `test_command_cancellation_with_message` | `routing::state_machine` | Cancelling command execution and returning custom proxy message | ✅ Passed |
| `test_command_passthrough_untouched` | `routing::state_machine` | Non-proxy commands passed to backend without byte modification | ✅ Passed |
| `test_server_transfer_with_different_compression_thresholds` | `routing::state_machine` | Transfer from compressed backend (256) to uncompressed backend | ✅ Passed |
| `test_switch_server_configuration_timeout` | `routing::state_machine` | Unresponsive target backend cleanly times out without dropping client | ✅ Passed |
| `test_switch_server_graceful_connect_failure` | `routing::state_machine` | Connection refusal on switch returns error message, keeping player connected | ✅ Passed |
| `test_unexpected_backend_disconnect_failsafe_reroute` | `routing::state_machine` | Backend crash intercepts Disconnect, suppresses client kick, reroutes to lobby | ✅ Passed |
| `test_inject_proxy_commands_into_declare_commands` | `routing::state_machine` | Brigadier command tree injection for `/server`, `/lobby`, `/hub` | ✅ Passed |
| `test_inject_proxy_commands_with_multibyte_root_index` | `routing::state_machine` | Brigadier root index spanning multi-byte VarInt values | ✅ Passed |
| `test_inject_proxy_commands_legacy_version_unmodified` | `routing::state_machine` | Legacy protocol versions bypass Brigadier tree manipulation safely | ✅ Passed |
| `test_inject_proxy_commands_malformed_root_index_unmodified` | `routing::state_machine` | Malformed command trees preserved intact without panic | ✅ Passed |
| `test_is_declare_commands_packet_across_versions` | `routing::state_machine` | Exact DeclareCommands packet ID mapping across all protocol versions | ✅ Passed |
| `test_handle_backend_packet_injects_declare_commands` | `routing::state_machine` | DeclareCommands interception in Play state and command injection | ✅ Passed |
| `test_handle_backend_packet_injects_declare_commands_compressed` | `routing::state_machine` | DeclareCommands interception with compression threshold active | ✅ Passed |
| `test_is_login_play_packet_across_versions` | `routing::state_machine` | Login (Play) packet ID resolution across protocol versions | ✅ Passed |
| `test_is_login_play_packet_comprehensive` | `routing::state_machine` | Comprehensive validation of Login (Play) packet layouts | ✅ Passed |
| `test_extract_respawn_from_login_modern_776` | `routing::state_machine` | Respawn packet synthesized from downstream Login (Play) for 1.21.4+ | ✅ Passed |
| `test_extract_respawn_from_login_with_data_kept_modern` | `routing::state_machine` | Preserving SpawnInfo and dataToKeep flag (KEEP_ALL_DATA, KEEP_ATTRIBUTES, KEEP_METADATA) for 1.21.4+ | ✅ Passed |
| `test_extract_respawn_from_login_with_data_kept_765_custom_world` | `routing::state_machine` | Dimension extraction and respawn synthesis for 1.20.2 - 1.20.4 (Protocols 764-765) | ✅ Passed |
| `test_respawn_packet_roundtrip` | `routing::state_machine` | Respawn packet serialization and parsing across versions | ✅ Passed |
| `test_respawn_packet_ids_across_versions` | `routing::state_machine` | Respawn packet IDs verified across 1.20.4, 1.20.6, 1.21, 1.21.4 | ✅ Passed |
| `test_respawn_packet_protocol_776_packet_id` | `routing::state_machine` | Exact packet ID verification for Protocol 776 | ✅ Passed |
| `test_boss_bar_packet_codecs_across_versions` | `routing::state_machine` | BossBarPacket (0x0A/0x0B) remove action serialization across versions | ✅ Passed |
| `test_scoreboard_objective_codecs_across_versions` | `routing::state_machine` | ScoreboardObjectivePacket remove action serialization across versions | ✅ Passed |
| `test_display_objective_codecs_across_versions` | `routing::state_machine` | DisplayObjectivePacket clear slot serialization across versions | ✅ Passed |
| `test_close_container_packet_codecs_across_versions` | `routing::state_machine` | CloseContainerPacket (0x12/0x11/0x0F/0x10) window close codec across versions | ✅ Passed |
| `test_stop_sound_packet_codecs_across_versions` | `routing::state_machine` | StopSoundPacket (0x66/0x68/0x6A/0x71) audio stop flags and sound identifiers | ✅ Passed |
| `test_seamless_transfer_gui_sanitization` | `routing::state_machine` | Automated container window closure on server switch | ✅ Passed |
| `test_seamless_transfer_audio_cleanup` | `routing::state_machine` | Automated audio loop termination on server switch | ✅ Passed |
| `test_seamless_transfer_bossbar_and_scoreboard_teardown` | `routing::state_machine` | Complete cleanup of lingering bossbars and scoreboard objectives | ✅ Passed |
| `test_seamless_transfer_display_objective_teardown` | `routing::state_machine` | Clearing active display objective slots on server switch | ✅ Passed |
| `test_clear_display_objective_helper` | `routing::state_machine` | Direct programmatic invocation of clear_display_objective | ✅ Passed |
| `test_manual_sanitize_screen_and_stop_audio_helpers` | `routing::state_machine` | Direct programmatic invocation of sanitize_screen and stop_audio | ✅ Passed |
| `test_system_chat_message_roundtrip` | `routing::state_machine` | System chat message packet serialization (JSON component) | ✅ Passed |
| `test_system_chat_message_protocol_776_nbt_roundtrip` | `routing::state_machine` | NBT-encoded system chat component serialization for 1.21.4+ | ✅ Passed |
| `test_system_chat_packet_ids_across_versions` | `routing::state_machine` | System chat packet IDs verified across protocol range | ✅ Passed |
| `test_tab_complete_request_packet_encode_decode` | `routing::state_machine` | Tab complete request packet (0x0B) serialization | ✅ Passed |
| `test_tab_complete_response_packet_encode_decode` | `routing::state_machine` | Tab complete response packet (0x11) serialization | ✅ Passed |
| `test_play_state_machine_tab_complete_interception` | `routing::state_machine` | Intercepting tab complete requests and serving proxy server suggestions | ✅ Passed |
| `test_play_state_machine_tab_complete_passthrough` | `routing::state_machine` | Non-proxy tab completions forwarded to backend server | ✅ Passed |
| `test_protected_plugin_channel_dropped` | `routing::state_machine` | Malicious client injection of proxy-internal plugin channels dropped | ✅ Passed |

---

## 6. Sandboxed Rhai Scripting Engine & Plugins (18 Tests)

Scripts should never be able to freeze the Tokio reactor or chew through memory. We test the engine under hostile conditions: infinite loops terminated at 50,000 opcodes, recursion limits clamped at 32 frames, and exponential string builders rejected at 1,024 bytes. We also confirm that filesystem `import` statements are strictly blocked, test in-memory key-value primitives, and verify that the bundled `server_switcher.rhai` plugin initializes and routes cleanly.

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_infinite_loop_halts_with_operations_limit` | `script::engine` | `while true` loop terminated after 50,000 opcodes with `EvalError` | ✅ Passed |
| `test_recursion_depth_limit` | `script::engine` | Infinite function recursion halted cleanly at depth 32 | ✅ Passed |
| `test_max_string_size_limit` | `script::engine` | Exponential string doubling rejected at 1,024 bytes | ✅ Passed |
| `test_external_file_import_is_disabled` | `script::engine` | Sandboxed engine rejects `import` statements accessing filesystem | ✅ Passed |
| `test_logging_functions_and_call_fn` | `script::engine` | Proxy log functions (`proxy_info`, `proxy_warn`, `proxy_error`) | ✅ Passed |
| `test_reload_from_file` | `script::engine` | Hot-reloading Rhai scripts from disk at runtime | ✅ Passed |
| `test_plugin_directory_loading_and_server_queries` | `script::engine` | Scanning and loading multiple `.rhai` scripts from `plugins/` | ✅ Passed |
| `test_kv_store_and_timestamps` | `script::engine` | Key-value store (`kv_set`, `kv_get`) and Unix timestamp utilities | ✅ Passed |
| `test_main_rhai_join_event` | `script::events` | Default join hook evaluating player join event maps | ✅ Passed |
| `test_main_rhai_hub_and_lobby_reroute` | `script::events` | Routing logic for `/hub` and `/lobby` shortcuts | ✅ Passed |
| `test_main_rhai_steel_reroute` | `script::events` | Routing logic for `/steel` shortcut command | ✅ Passed |
| `test_main_rhai_forbidden_commands` | `script::events` | Blocking forbidden commands with custom denial messages | ✅ Passed |
| `test_main_rhai_passthrough_command` | `script::events` | Unhandled commands returning `cancel: false` | ✅ Passed |
| `test_server_switcher_plugin_commands` | `script::events` | `/server`, `/server <name>`, and invalid target handling | ✅ Passed |
| `test_server_switcher_tab_complete` | `script::events` | Dynamic tab completion matching configured backend server names | ✅ Passed |
| `test_default_fallback_when_hooks_missing` | `script::events` | Safe defaults when scripts omit hook function definitions | ✅ Passed |
| `test_plugin_security_and_edge_cases` | `script::events` | Malformed events, non-map return types, and script error isolation | ✅ Passed |
| `test_server_switcher_plugin_loading_and_runtime` | `script::events` | Validates directory-loaded server switcher plugin lifecycle & commands | ✅ Passed |

---

## 7. Configuration, Network Listener & Integration Tests (21 Tests)

These integration tests bind real loopback TCP sockets on localhost. They execute end-to-end connection lifecycles: server list status queries (verifying base64 favicon delivery and ping/pong timestamp symmetry), RSA/AES authentication flows, and full bi-directional traffic bridging against a live SteelMC instance.

| Test Function | Location | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_default_template_matches_default_struct` | `config` | Generated default template matches `ProxyConfig` in-memory defaults | ✅ Passed |
| `test_forwarding_mode_parsing` | `config` | Parsing `velocity_modern`, `legacy_bungee`, and `none` from TOML | ✅ Passed |
| `test_load_or_create` | `config` | Reading existing config or bootstrapping default configuration | ✅ Passed |
| `test_load_or_create_nested_subdirectory_cross_platform` | `config` | Recursive parent directory creation and nested config bootstrapping | ✅ Passed |
| `test_resolve_favicon_uri_auto_detect` | `config` | Automatically detecting and encoding `server-icon.png` | ✅ Passed |
| `test_resolve_favicon_uri_data_uri` | `config` | Parsing raw `data:image/png;base64,...` URIs | ✅ Passed |
| `test_resolve_favicon_uri_file_path` | `config` | Resolving filesystem image paths to base64 favicon data | ✅ Passed |
| `test_toml_roundtrip` | `config` | Full TOML serialization and deserialization roundtrip | ✅ Passed |
| `test_error_display` | `error` | Formatted display strings for all `FrameError` variants | ✅ Passed |
| `test_offline_mode_login_flow` | `network::listener` | Offline mode handshake and login transition on listener socket | ✅ Passed |
| `test_online_mode_token_mismatch_fails_closed` | `network::listener` | RSA verify token forgery fails closed immediately | ✅ Passed |
| `test_modern_configuration_transition_flow` | `network::listener` | Full transition from Login to Configuration phase | ✅ Passed |
| `test_compressed_backend_synchronization_flow` | `network::listener` | Compression negotiation and threshold synchronization | ✅ Passed |
| `test_full_flow_with_live_steelmc` | `tests/full_live_test.rs` | Complete end-to-end handshake, login, config, and play stream relay | ✅ Passed |
| `test_full_status_ping_flow_over_tcp` | `tests/status_ping_test.rs` | Real TCP StatusRequest $\rightarrow$ StatusResponse (base64 favicon) $\rightarrow$ Ping/Pong | ✅ Passed |
| `test_full_online_mode_login_and_encryption_over_tcp` | `tests/login_auth_test.rs` | Real TCP RSA encryption handshake + encrypted AES-128-CFB8 stream | ✅ Passed |
| `test_online_mode_token_mismatch_fails_closed_over_tcp` | `tests/login_auth_test.rs` | Real TCP token forgery detected and socket dropped immediately | ✅ Passed |
| `test_offline_mode_login_over_tcp` | `tests/login_auth_test.rs` | Real TCP offline mode handshake and LoginSuccess exchange | ✅ Passed |
| `test_offline_mode_login_protocol_776_over_tcp` | `tests/login_auth_test.rs` | Real TCP Protocol 776 (1.21.4+) offline login flow | ✅ Passed |
| `test_online_mode_case_insensitive_username_matches` | `tests/login_auth_test.rs` | Real TCP login with mixed-case username matching Mojang profile | ✅ Passed |
| `test_live_server_transfer_steelmc_and_paper` | `tests/transfer_live_test.rs` | Live loopback server switching with compression negotiation and Brigadier tree injection | ✅ Passed |

---

## 8. Command-Line Interface & Application Lifecycle (6 Tests)

Verifies CLI argument handling for configuration overrides (`-c` and `--config`), version flags (`-V`, `--version`), help text output, and clean error exit codes when unrecognized flags or missing file paths are passed.

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_cli_args_default` | `main` | Default configuration resolution (`config.toml`) when no arguments supplied | ✅ Passed |
| `test_cli_args_config_flag` | `main` | Explicit `-c` and `--config` custom configuration file path parsing | ✅ Passed |
| `test_cli_args_version_and_help` | `main` | Clean short and long version (`-v`, `-V`, `--version`) and help (`-h`, `--help`) triggers | ✅ Passed |
| `test_cli_args_errors` | `main` | Rejection of missing configuration values and unrecognized CLI options | ✅ Passed |
| `test_shutdown_channel_broadcast` | `main` | Asynchronous watch broadcast for graceful termination across all runtime tasks | ✅ Passed |
| `test_wait_for_shutdown_signal_does_not_prematurely_trigger` | `main` | OS signal listener remains active without premature termination until actual signal arrival | ✅ Passed |
