use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::watch;
use uuid::Uuid;

use framemc::config::{BackendConfig, ForwardingMode, ProxyConfig};
use framemc::network::codec::{
    read_packet, read_packet_with_compression, write_packet, write_packet_with_compression,
    DEFAULT_MAX_PACKET_SIZE,
};
use framemc::network::listener::{bind_listener, start_listener_on};
use framemc::protocol::handshake::{ConnectionState, HandshakePacket};
use framemc::protocol::login::{LoginAcknowledgedPacket, LoginStartPacket, LoginSuccessPacket};
use framemc::protocol::RawPacket;
use framemc::script::engine::ScriptHost;

#[tokio::test]
async fn test_full_flow_with_live_steelmc() {
    // Check if SteelMC is running
    if TcpStream::connect("127.0.0.1:25567").await.is_err() {
        println!("SteelMC is not running on 25567; skipping integration test");
        return;
    }

    let mut servers = HashMap::new();
    servers.insert(
        "steelmc".to_string(),
        BackendConfig {
            address: "127.0.0.1".to_string(),
            port: 25567,
            forwarding_mode: ForwardingMode::None,
            forwarding_secret: None,
        },
    );

    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0, // OS assigned
        motd: "§aFrameMC Test".to_string(),
        max_players: 100,
        online_mode: false,
        favicon: None,
        session_server_url: None,
        servers,
        default_server: "steelmc".to_string(),
        script_path: "scripts/main.rhai".to_string(),
        plugins_dir: "plugins".to_string(),
    };

    let listener = bind_listener(&config)
        .await
        .expect("Failed to bind proxy listener");
    let proxy_port = listener.local_addr().unwrap().port();
    config.bind_port = proxy_port;
    let config = Arc::new(config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let script_host = Arc::new(ScriptHost::new());
    let proxy_handle = tokio::spawn(start_listener_on(
        listener,
        config,
        script_host,
        shutdown_rx,
    ));

    // Now connect simulated Lunar Client (protocol 776) to FrameMC
    let mut client_stream = TcpStream::connect(format!("127.0.0.1:{proxy_port}"))
        .await
        .unwrap();

    // 1. Handshake
    let handshake = HandshakePacket::new(776, "localhost", proxy_port, ConnectionState::Login);
    write_packet(&mut client_stream, &handshake.encode())
        .await
        .unwrap();

    // 2. LoginStart
    let client_uuid = Uuid::parse_str("50ffad28-75c9-35b1-af41-768e70a4d3e9").unwrap();
    let login_start = LoginStartPacket::new("v4mphire", client_uuid);
    write_packet(&mut client_stream, &login_start.encode())
        .await
        .unwrap();

    // 3. Receive initial response: SetCompression (0x03) or direct LoginSuccess (0x02)
    let first_pkt = read_packet(&mut client_stream, DEFAULT_MAX_PACKET_SIZE)
        .await
        .unwrap();
    let (threshold, success_pkt) = if first_pkt.id == 0x03 {
        let mut comp_cursor = &first_pkt.payload[..];
        let thresh = framemc::protocol::varint::decode_varint(&mut comp_cursor).unwrap() as usize;
        let success =
            read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, Some(thresh))
                .await
                .unwrap();
        (Some(thresh), success)
    } else {
        (None, first_pkt)
    };

    assert_eq!(success_pkt.id, 0x02, "Expected LoginSuccess (0x02)");
    let decoded_success = LoginSuccessPacket::decode_with_version(&success_pkt, 776).unwrap();
    assert_eq!(decoded_success.username, "v4mphire");
    assert!(
        decoded_success.session_id.is_some(),
        "Expected session_id in LoginSuccess for protocol 776"
    );
    println!(
        "Received LoginSuccess with session_id: {:?}",
        decoded_success.session_id
    );

    // 5. Send LoginAcknowledged
    let ack = LoginAcknowledgedPacket::new();
    write_packet_with_compression(&mut client_stream, &ack.encode(), threshold)
        .await
        .unwrap();
    println!("Sent LoginAcknowledged");

    // 6. Configuration phase
    let mut config_done = false;
    while !config_done {
        let pkt =
            read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, threshold)
                .await
                .unwrap();
        println!(
            "Client received config pkt id=0x{:02X}, len={}",
            pkt.id,
            pkt.payload.len()
        );
        if pkt.id == 0x02 {
            println!(
                "Configuration Disconnect packet payload: {}",
                String::from_utf8_lossy(&pkt.payload)
            );
        }

        if pkt.id == 0x0E {
            // Known packs: respond with Serverbound Known Packs (0x07)
            println!("Client replying to Known Packs (0x0E) with Serverbound Known Packs (0x07)");
            // Empty known packs response: VarInt count = 0
            let mut known_packs_resp = bytes::BytesMut::new();
            framemc::protocol::varint::encode_varint(0, &mut known_packs_resp);
            let resp_pkt = RawPacket::new(0x07, known_packs_resp.freeze());
            write_packet_with_compression(&mut client_stream, &resp_pkt, threshold)
                .await
                .unwrap();
        } else if pkt.id == 0x03 {
            // Clientbound FinishConfiguration (0x03)
            println!("Client received FinishConfiguration (0x03), sending Serverbound FinishConfiguration (0x03)");
            let finish_ack = RawPacket::new(0x03, bytes::Bytes::new());
            write_packet_with_compression(&mut client_stream, &finish_ack, threshold)
                .await
                .unwrap();
            config_done = true;
        }
    }

    // 7. Play phase: client should receive JoinGame (Login (play)) packet from backend!
    println!("Waiting for Play state packet...");
    let play_pkt =
        read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, threshold)
            .await
            .unwrap();
    println!(
        "Successfully received Play state packet! id=0x{:02X}, len={}",
        play_pkt.id,
        play_pkt.payload.len()
    );

    drop(client_stream);
    let _ = shutdown_tx.send(true);
    let _ = proxy_handle.await;
}
