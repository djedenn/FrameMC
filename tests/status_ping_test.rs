use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::time::{timeout, Duration};

use framemc::config::ProxyConfig;
use framemc::network::codec::{read_packet, write_packet, DEFAULT_MAX_PACKET_SIZE};
use framemc::network::listener::{bind_listener, start_listener_on};
use framemc::protocol::handshake::{ConnectionState, HandshakePacket};
use framemc::protocol::packet::RawPacket;
use framemc::protocol::status::StatusResponse;
use framemc::protocol::varint::decode_varint;
use framemc::script::engine::ScriptHost;

#[tokio::test]
async fn test_full_status_ping_flow_over_tcp() {
    // 1. Prepare configuration with port 0 (ephemeral kernel-assigned port)
    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0,
        motd: "§aFrameMC Status Integration MOTD".to_string(),
        max_players: 777,
        online_mode: true,
        favicon: Some("data:image/png;base64,mockfavicon".to_string()),
        session_server_url: None,
        servers: HashMap::new(),
        default_server: "lobby".to_string(),
        script_path: "scripts/main.rhai".to_string(),
        plugins_dir: "plugins".to_string(),
    };

    // Bind listener to ephemeral port
    let listener = bind_listener(&config)
        .await
        .expect("Failed to bind ephemeral listener");
    let actual_port = listener
        .local_addr()
        .expect("Failed to get local addr")
        .port();
    config.bind_port = actual_port;
    let config = Arc::new(config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let script_host = Arc::new(ScriptHost::new());

    // Spawn server listener loop
    let server_handle = tokio::spawn(start_listener_on(
        listener,
        Arc::clone(&config),
        script_host,
        shutdown_rx,
    ));

    // 2. Client connects via real TCP stream
    let mut client = TcpStream::connect(format!("127.0.0.1:{actual_port}"))
        .await
        .expect("Client failed to connect to proxy TCP port");

    // 3. Client sends Handshake with next_state = Status
    let handshake = HandshakePacket {
        protocol_version: 765,
        server_address: "localhost".to_string(),
        server_port: actual_port,
        next_state: ConnectionState::Status,
    };
    write_packet(&mut client, &handshake.encode())
        .await
        .expect("Failed to send Handshake");

    // 4. Client sends StatusRequest (id = 0x00, empty payload)
    write_packet(&mut client, &RawPacket::new(0x00, Bytes::new()))
        .await
        .expect("Failed to send StatusRequest");

    // 5. Client receives StatusResponse (id = 0x00)
    let response_packet = timeout(
        Duration::from_secs(5),
        read_packet(&mut client, DEFAULT_MAX_PACKET_SIZE),
    )
    .await
    .expect("Timed out waiting for StatusResponse")
    .expect("Failed to read StatusResponse packet");

    assert_eq!(response_packet.id, 0x00);

    // Decode VarInt string length followed by JSON payload
    let mut cursor = &response_packet.payload[..];
    let json_len = decode_varint(&mut cursor).expect("Failed to decode JSON VarInt length");
    assert_eq!(cursor.len(), json_len as usize);
    let json_str = std::str::from_utf8(cursor).expect("StatusResponse payload is not valid UTF-8");

    let status: StatusResponse =
        serde_json::from_str(json_str).expect("StatusResponse JSON failed schema validation");

    assert_eq!(status.description.text, "§aFrameMC Status Integration MOTD");
    assert_eq!(status.players.max, 777);
    assert_eq!(status.players.online, 0);
    assert_eq!(status.version.protocol, 765);
    assert_eq!(
        status.favicon,
        Some("data:image/png;base64,mockfavicon".to_string())
    );
    assert!(!status.enforces_secure_chat);

    // 6. Client sends PingRequest with 64-bit timestamp
    let ping_timestamp: i64 = 1718999888777;
    let ping_packet = RawPacket::new(0x01, Bytes::copy_from_slice(&ping_timestamp.to_be_bytes()));
    write_packet(&mut client, &ping_packet)
        .await
        .expect("Failed to send PingRequest");

    // 7. Client receives PongResponse (id = 0x01)
    let pong_packet = timeout(
        Duration::from_secs(5),
        read_packet(&mut client, DEFAULT_MAX_PACKET_SIZE),
    )
    .await
    .expect("Timed out waiting for PongResponse")
    .expect("Failed to read PongResponse packet");

    assert_eq!(pong_packet.id, 0x01);
    assert_eq!(pong_packet.payload.len(), 8);
    let returned_timestamp = i64::from_be_bytes(pong_packet.payload[..8].try_into().unwrap());
    assert_eq!(
        returned_timestamp, ping_timestamp,
        "Pong timestamp must match ping timestamp exactly"
    );

    // 8. Clean up: shutdown server
    shutdown_tx
        .send(true)
        .expect("Failed to send shutdown signal");
    let _ = server_handle.await;
}
