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
use framemc::protocol::varint::{decode_varint, encode_varint};
use framemc::protocol::RawPacket;
use framemc::routing::state_machine::ServerboundChatCommand;
use framemc::script::engine::ScriptHost;

#[tokio::test]
async fn test_live_server_transfer_steelmc_and_paper() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("framemc=info")
        .try_init();

    // 1. Verify that backend servers are running
    let steel_lobby_up = TcpStream::connect("127.0.0.1:25566").await.is_ok();
    let steel_up = TcpStream::connect("127.0.0.1:25567").await.is_ok();
    let paper_up = TcpStream::connect("127.0.0.1:25568").await.is_ok();

    if !steel_lobby_up || !steel_up {
        println!("SteelMC servers (ports 25566, 25567) are not running; skipping live test");
        return;
    }

    println!(
        "Live backend status: Lobby(25566)={steel_lobby_up}, SteelMC(25567)={steel_up}, Paper(25568)={paper_up}"
    );

    // 2. Configure FrameMC proxy with live backends
    let mut servers = HashMap::new();
    servers.insert(
        "lobby".to_string(),
        BackendConfig {
            address: "127.0.0.1".to_string(),
            port: 25566,
            forwarding_mode: ForwardingMode::None,
            forwarding_secret: None,
        },
    );
    servers.insert(
        "steelmc".to_string(),
        BackendConfig {
            address: "127.0.0.1".to_string(),
            port: 25567,
            forwarding_mode: ForwardingMode::None,
            forwarding_secret: None,
        },
    );
    if paper_up {
        servers.insert(
            "paper".to_string(),
            BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 25568,
                forwarding_mode: ForwardingMode::VelocityModern,
                forwarding_secret: Some("framemc_secret_velocity_2026".to_string()),
            },
        );
    }

    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0, // OS assigned ephemeral port
        motd: "§aFrameMC Seamless Transfer Test".to_string(),
        max_players: 100,
        online_mode: false,
        favicon: None,
        session_server_url: None,
        servers,
        default_server: "lobby".to_string(),
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
    script_host.set_servers(
        config.servers.keys().cloned().collect(),
        config.default_server.clone(),
    );
    if std::path::Path::new(&config.script_path).exists() {
        let _ = script_host.reload(&config.script_path).await;
    }
    let _ = script_host.load_plugins_dir(&config.plugins_dir).await;

    let proxy_handle = tokio::spawn(start_listener_on(
        listener,
        config,
        script_host,
        shutdown_rx,
    ));

    println!("Spawned test FrameMC instance on port {proxy_port}");

    // 3. Connect simulated Minecraft client (protocol 776)
    let mut client_stream = TcpStream::connect(format!("127.0.0.1:{proxy_port}"))
        .await
        .expect("Failed to connect to test proxy");

    // Handshake
    let handshake = HandshakePacket::new(776, "127.0.0.1", proxy_port, ConnectionState::Login);
    write_packet(&mut client_stream, &handshake.encode())
        .await
        .unwrap();

    // LoginStart
    let client_uuid = Uuid::parse_str("50ffad28-75c9-35b1-af41-768e70a4d3e9").unwrap();
    let login_start = LoginStartPacket::new("v4mphire", client_uuid);
    write_packet(&mut client_stream, &login_start.encode())
        .await
        .unwrap();

    // Login response
    let first_pkt = read_packet(&mut client_stream, DEFAULT_MAX_PACKET_SIZE)
        .await
        .unwrap();
    let (threshold, success_pkt) = if first_pkt.id == 0x03 {
        let mut comp_cursor = &first_pkt.payload[..];
        let thresh = decode_varint(&mut comp_cursor).unwrap() as usize;
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
    println!("Client received LoginSuccess for player v4mphire");

    // LoginAcknowledged
    let ack = LoginAcknowledgedPacket::new();
    write_packet_with_compression(&mut client_stream, &ack.encode(), threshold)
        .await
        .unwrap();

    // Configuration phase
    let mut config_done = false;
    while !config_done {
        let pkt =
            read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, threshold)
                .await
                .unwrap();

        if pkt.id == 0x0E {
            // Known Packs
            let mut known_packs_resp = bytes::BytesMut::new();
            encode_varint(0, &mut known_packs_resp);
            let resp_pkt = RawPacket::new(0x07, known_packs_resp.freeze());
            write_packet_with_compression(&mut client_stream, &resp_pkt, threshold)
                .await
                .unwrap();
        } else if pkt.id == 0x03 {
            // FinishConfiguration
            let finish_ack = RawPacket::new(0x03, bytes::Bytes::new());
            write_packet_with_compression(&mut client_stream, &finish_ack, threshold)
                .await
                .unwrap();
            config_done = true;
        }
    }

    println!("Client completed Configuration state, now entering Play state on Lobby");

    // Play phase: receive initial JoinGame
    let first_play_pkt =
        read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, threshold)
            .await
            .unwrap();
    println!(
        "Client received initial Play packet id=0x{:02X} (len={})",
        first_play_pkt.id,
        first_play_pkt.payload.len()
    );

    // Read a few packets from lobby
    for _ in 0..5 {
        let _ =
            read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, threshold)
                .await;
    }

    // 4. Send /server steelmc to transfer from Lobby (25566) -> SteelMC (25567)
    println!(">>> Sending /server steelmc ...");
    let cmd = ServerboundChatCommand::new("server steelmc");
    let cmd_pkt = cmd.encode_with_version(776);
    write_packet_with_compression(&mut client_stream, &cmd_pkt, threshold)
        .await
        .unwrap();

    // Read packets until transfer occurs:
    // Client MUST receive clientbound Respawn packet (0x52 on protocol 776)
    let mut received_respawn_to_steelmc = false;
    let mut transferred_packets = 0;
    let start_steel = std::time::Instant::now();
    while start_steel.elapsed() < std::time::Duration::from_secs(10) {
        let res = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, threshold),
        )
        .await;

        match res {
            Ok(Ok(pkt)) => {
                transferred_packets += 1;
                println!(
                    "Client received packet id=0x{:02X} len={}",
                    pkt.id,
                    pkt.payload.len()
                );
                if pkt.id == 0x52 {
                    println!(">>> SUCCESS: Client received Respawn packet (0x52) during transfer to steelmc!");
                    received_respawn_to_steelmc = true;
                    break;
                }
            }
            Ok(Err(e)) => {
                println!("Error reading packet during steelmc transfer: {e:?}");
                break;
            }
            Err(_) => {
                // Wait up to 10s total
            }
        }
    }

    assert!(
        received_respawn_to_steelmc,
        "Client failed to receive clientbound Respawn (0x52) when transferring to steelmc (read {transferred_packets} packets)"
    );

    // 5. If Paper is running, test transfer to Paper (25568) with Velocity Modern Forwarding
    if paper_up {
        println!(">>> Sending /server paper ...");
        let cmd = ServerboundChatCommand::new("server paper");
        let cmd_pkt = cmd.encode_with_version(776);
        write_packet_with_compression(&mut client_stream, &cmd_pkt, threshold)
            .await
            .unwrap();

        let mut received_respawn_to_paper = false;
        let start = std::time::Instant::now();
        let mut i = 0;
        while start.elapsed() < std::time::Duration::from_secs(10) {
            let res = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                read_packet_with_compression(
                    &mut client_stream,
                    DEFAULT_MAX_PACKET_SIZE,
                    threshold,
                ),
            )
            .await;

            match res {
                Ok(Ok(pkt)) => {
                    println!(
                        "Client received during Paper transfer #{i}: id=0x{:02X} len={}",
                        pkt.id,
                        pkt.payload.len()
                    );
                    i += 1;
                    if pkt.id == 0x79 {
                        if let Ok(chat) =
                            framemc::routing::state_machine::SystemChatMessagePacket::decode(&pkt)
                        {
                            println!("Chat message: {}", chat.message);
                        }
                    }
                    if pkt.id == 0x52 {
                        println!(">>> SUCCESS: Client received Respawn packet (0x52) during transfer to Paper!");
                        received_respawn_to_paper = true;
                        break;
                    }
                }
                Ok(Err(e)) => {
                    println!("Error reading packet during Paper transfer: {e:?}");
                    break;
                }
                Err(_) => {
                    // Small slice timeout; continue waiting up to 10s total
                }
            }
        }

        assert!(
            received_respawn_to_paper,
            "Client failed to receive clientbound Respawn (0x52) when transferring to Paper"
        );
        println!(">>> Successfully transferred to Paper via Velocity Modern Forwarding!");
    }

    // Clean teardown
    drop(client_stream);
    let _ = shutdown_tx.send(true);
    let _ = proxy_handle.await;
    println!(">>> Test completed with 100% success!");
}
