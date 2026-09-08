# FrameMC Automated Test Suite & Protocol Verification

Every codec, state machine transition, and cryptographic routine in FrameMC is verified against official Minecraft protocol specifications (1.20.4 through 1.21.4+, protocols 764 to 776+). 

Tests do not merely inspect mocked in-memory structures—they bind real loopback TCP sockets to validate raw byte-for-byte serialization, cipher state preservation, framing alignment, and error handling on the wire.

---

## Test Execution Summary

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

To execute the complete test suite locally:

```bash
# Run all unit and integration tests
cargo test --all-targets

# Run tests with real-time test name output
cargo test --all-targets -- --nocapture
```

---

## 1. Cryptography & Session Authentication (9 Tests)

Covers RSA-1024 public key export in standard X.509 DER format, PKCS#1 v1.5 shared secret decryption, and Mojang's two's-complement SHA-1 server hash generation. We also verify that the AES-128-CFB8 stream cipher preserves its continuous keystream state across variable TCP chunk fragmentations (testing 1-byte, 7-byte, and 1,024-byte chunks).

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

Tests LEB128 VarInt and VarLong encodings across extreme boundaries (0, max 32-bit/64-bit bounds, negative values) and confirms that over-length VarInts fail closed immediately to prevent memory amplification attacks. Compression tests exercise zlib threshold transitions, raw wire verification against golden reference packets, and decompression bomb limits.

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

Validates Velocity modern forwarding signatures using HMAC-SHA256, null-byte host rewrites for legacy BungeeCord backends, and LoginSuccess packet schemas across protocol versions. Tests also confirm that unauthenticated clients cannot inject spoofed internal proxy channels, and verify clean disconnects when downstream backends misreport online-mode requirements.

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

Minecraft 1.20.2 separated configuration negotiation from login. These tests verify non-destructive capture of registry packets (biomes, dimensions, damage types) during active client connections, and test replaying cached codecs to downstream targets during cross-server transfers.

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

## 5. Play State Machine, Server Switching & Bridge (30 Tests)

Covers bidirectional TCP stream bridging with 5 MB bulk throughput transfers, client socket FIN propagation, and Brigadier command tree injection for `/server` and `/lobby`. Also tests failover mechanisms that intercept unexpected backend disconnects and route players back to the lobby instead of kicking them from the proxy.

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
| `test_is_login_play_packet_across_versions` | `routing::state_machine` | Login (Play) packet ID resolution across protocol versions | ✅ Passed |
| `test_is_login_play_packet_comprehensive` | `routing::state_machine` | Comprehensive validation of Login (Play) packet layouts | ✅ Passed |
| `test_extract_respawn_from_login_modern_776` | `routing::state_machine` | Respawn packet synthesized from downstream Login (Play) for 1.21.4+ | ✅ Passed |
| `test_respawn_packet_roundtrip` | `routing::state_machine` | Respawn packet serialization and parsing across versions | ✅ Passed |
| `test_respawn_packet_ids_across_versions` | `routing::state_machine` | Respawn packet IDs verified across 1.20.4, 1.20.6, 1.21, 1.21.4 | ✅ Passed |
| `test_respawn_packet_protocol_776_packet_id` | `routing::state_machine` | Exact packet ID verification for Protocol 776 | ✅ Passed |
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

Validates engine sandboxing under hostile script conditions: infinite loops terminated at 50,000 opcodes, recursion limits clamped at 32 frames, and exponential string builders rejected at 1,024 bytes. Also confirms that filesystem `import` statements are strictly blocked, tests in-memory key-value primitives, and verifies that all bundled `.rhai` plugins initialize cleanly.

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
| `test_all_20_popular_bungee_plugins_load_and_run` | `script::events` | Validates all bundled BungeeCord-equivalent Rhai plugins | ✅ Passed |

---

## 7. Configuration, Network Listener & Integration Tests (19 Tests)

Integration tests binding real loopback TCP sockets. These run end-to-end connection lifecycles: server list status queries (verifying base64 favicon delivery and ping/pong timestamp symmetry), RSA/AES authentication flows, and full bi-directional traffic bridging against a live SteelMC instance.

| Test Function | Location | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_default_template_matches_default_struct` | `config` | Generated default template matches `ProxyConfig` in-memory defaults | ✅ Passed |
| `test_forwarding_mode_parsing` | `config` | Parsing `velocity_modern`, `legacy_bungee`, and `none` from TOML | ✅ Passed |
| `test_load_or_create` | `config` | Reading existing config or bootstrapping default configuration | ✅ Passed |
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

---

## 8. Command-Line Interface & Application Lifecycle (4 Tests)

Verifies CLI argument handling for configuration overrides (`-c` and `--config`), version flags (`-V`, `--version`), help text output, and clean error exit codes when unrecognized flags or missing file paths are passed.

| Test Function | Target Module | Verification Scope | Status |
| :--- | :--- | :--- | :---: |
| `test_cli_args_default` | `main` | Default configuration resolution (`config.toml`) when no arguments supplied | ✅ Passed |
| `test_cli_args_config_flag` | `main` | Explicit `-c` and `--config` custom configuration file path parsing | ✅ Passed |
| `test_cli_args_version_and_help` | `main` | Clean short and long version (`-v`, `-V`, `--version`) and help (`-h`, `--help`) triggers | ✅ Passed |
| `test_cli_args_errors` | `main` | Rejection of missing configuration values and unrecognized CLI options | ✅ Passed |
