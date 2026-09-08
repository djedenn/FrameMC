use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use uuid::Uuid;

use framemc::config::ProxyConfig;
use framemc::crypto::offline_profile;
use framemc::crypto::EncryptedStream;
use framemc::network::codec::{read_packet, write_packet, DEFAULT_MAX_PACKET_SIZE};
use framemc::network::listener::{bind_listener, start_listener_on};
use framemc::protocol::handshake::{ConnectionState, HandshakePacket};
use framemc::protocol::login::{
    EncryptionRequestPacket, EncryptionResponsePacket, LoginStartPacket, LoginSuccessPacket,
};
use framemc::script::engine::ScriptHost;

/// Spawns a lightweight in-process HTTP server emulating Mojang's session server `hasJoined` endpoint.
async fn start_mock_mojang_session_server() -> (String, tokio::sync::oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind mock Mojang HTTP server");
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/hasJoined");

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                accept_res = listener.accept() => {
                    let (mut stream, _) = match accept_res {
                        Ok(conn) => conn,
                        Err(_) => break,
                    };

                    tokio::spawn(async move {
                        let mut buf = [0u8; 2048];
                        let _ = stream.read(&mut buf).await;

                        let json_body = r#"{
                            "id": "069a79f444e34726a9be254cc4d37b01",
                            "name": "Steve",
                            "properties": [
                                {
                                    "name": "textures",
                                    "value": "mock_textures_base64_payload",
                                    "signature": "mock_signature_base64"
                                }
                            ]
                        }"#;

                        let http_response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            json_body.len(),
                            json_body
                        );

                        let _ = stream.write_all(http_response.as_bytes()).await;
                        let _ = stream.flush().await;
                    });
                }
                _ = &mut shutdown_rx => break,
            }
        }
    });

    (url, shutdown_tx)
}

#[tokio::test]
async fn test_full_online_mode_login_and_encryption_over_tcp() {
    // 1. Start mock Mojang HTTP session server
    let (mock_session_url, _mock_guard) = start_mock_mojang_session_server().await;

    // 2. Configure proxy in online mode with session URL pointing to mock
    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0,
        motd: "§aFrameMC Login Integration MOTD".to_string(),
        max_players: 100,
        online_mode: true,
        favicon: None,
        session_server_url: Some(mock_session_url),
        servers: HashMap::new(),
        default_server: "lobby".to_string(),
        script_path: "scripts/main.rhai".to_string(),
        plugins_dir: "plugins".to_string(),
    };

    let listener = bind_listener(&config)
        .await
        .expect("Failed to bind proxy listener");
    let actual_port = listener.local_addr().unwrap().port();
    config.bind_port = actual_port;
    let config = Arc::new(config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let script_host = Arc::new(ScriptHost::new());
    let _server_handle = tokio::spawn(start_listener_on(
        listener,
        config,
        script_host,
        shutdown_rx,
    ));

    // 3. Client connects via TCP
    let mut client_tcp = TcpStream::connect(format!("127.0.0.1:{actual_port}"))
        .await
        .expect("Client failed to connect to proxy TCP port");
    client_tcp.set_nodelay(true).unwrap();

    // 4. Client sends Handshake (protocol 765, Login state)
    let handshake = HandshakePacket::new(765, "localhost", actual_port, ConnectionState::Login);
    let mut handshake_bytes = bytes::BytesMut::new();
    framemc::protocol::varint::encode_varint(handshake.protocol_version, &mut handshake_bytes);
    framemc::protocol::varint::encode_varint(
        handshake.server_address.len() as i32,
        &mut handshake_bytes,
    );
    handshake_bytes.extend_from_slice(handshake.server_address.as_bytes());
    handshake_bytes.extend_from_slice(&handshake.server_port.to_be_bytes());
    framemc::protocol::varint::encode_varint(2, &mut handshake_bytes); // 2 = Login

    let handshake_packet = framemc::protocol::RawPacket::new(0x00, handshake_bytes.freeze());
    write_packet(&mut client_tcp, &handshake_packet)
        .await
        .unwrap();

    // 5. Client sends LoginStartPacket
    let client_uuid = Uuid::nil();
    let login_start = LoginStartPacket::new("Steve", client_uuid);
    write_packet(&mut client_tcp, &login_start.encode())
        .await
        .unwrap();

    // 6. Client receives EncryptionRequestPacket
    let enc_req_raw = read_packet(&mut client_tcp, DEFAULT_MAX_PACKET_SIZE)
        .await
        .expect("Failed to read EncryptionRequest from server");
    assert_eq!(enc_req_raw.id, 0x01);
    let enc_req =
        EncryptionRequestPacket::decode(&enc_req_raw).expect("Failed to decode EncryptionRequest");

    assert_eq!(enc_req.server_id, "");
    assert_eq!(enc_req.verify_token.len(), 4);

    // 7. Client generates 16-byte shared secret and encrypts secret + token with proxy RSA public key
    let mut shared_secret = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut shared_secret);

    let rsa_public_key = RsaPublicKey::from_public_key_der(&enc_req.public_key)
        .expect("Client failed to parse proxy RSA public key DER");

    let mut rng = rand::thread_rng();
    let encrypted_secret = rsa_public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &shared_secret)
        .expect("RSA encryption of shared secret failed");
    let encrypted_token = rsa_public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &enc_req.verify_token)
        .expect("RSA encryption of verify token failed");

    // 8. Client sends EncryptionResponsePacket
    let enc_resp = EncryptionResponsePacket::new(encrypted_secret, encrypted_token);
    write_packet(&mut client_tcp, &enc_resp.encode())
        .await
        .unwrap();

    // 9. Client transitions TCP stream into AES-128-CFB8 EncryptedStream
    let mut encrypted_client = EncryptedStream::new(client_tcp, &shared_secret);

    // 10. Client reads LoginSuccessPacket over encrypted stream
    let success_raw = read_packet(&mut encrypted_client, DEFAULT_MAX_PACKET_SIZE)
        .await
        .expect("Failed to read LoginSuccess over encrypted channel");
    assert_eq!(success_raw.id, 0x02);

    let login_success = LoginSuccessPacket::decode_with_version(&success_raw, 765)
        .expect("Failed to decode LoginSuccess packet");

    // 11. Assert returned profile matches Mojang session mock response
    assert_eq!(login_success.username, "Steve");
    assert_eq!(
        login_success.uuid,
        Uuid::parse_str("069a79f4-44e3-4726-a9be-254cc4d37b01").unwrap()
    );
    assert_eq!(login_success.properties.len(), 1);
    assert_eq!(login_success.properties[0].name, "textures");
    assert_eq!(
        login_success.properties[0].value,
        "mock_textures_base64_payload"
    );
    assert_eq!(
        login_success.properties[0].signature,
        Some("mock_signature_base64".to_string())
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_online_mode_token_mismatch_fails_closed_over_tcp() {
    let (mock_session_url, _mock_guard) = start_mock_mojang_session_server().await;

    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0,
        motd: "§aFrameMC Mismatch Test MOTD".to_string(),
        max_players: 100,
        online_mode: true,
        favicon: None,
        session_server_url: Some(mock_session_url),
        servers: HashMap::new(),
        default_server: "lobby".to_string(),
        script_path: "scripts/main.rhai".to_string(),
        plugins_dir: "plugins".to_string(),
    };

    let listener = bind_listener(&config).await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    config.bind_port = actual_port;
    let config = Arc::new(config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let script_host = Arc::new(ScriptHost::new());
    let _server_handle = tokio::spawn(start_listener_on(
        listener,
        config,
        script_host,
        shutdown_rx,
    ));

    let mut client_tcp = TcpStream::connect(format!("127.0.0.1:{actual_port}"))
        .await
        .unwrap();

    // Handshake
    let mut handshake_bytes = bytes::BytesMut::new();
    framemc::protocol::varint::encode_varint(765, &mut handshake_bytes);
    framemc::protocol::varint::encode_varint(9, &mut handshake_bytes);
    handshake_bytes.extend_from_slice(b"localhost");
    handshake_bytes.extend_from_slice(&actual_port.to_be_bytes());
    framemc::protocol::varint::encode_varint(2, &mut handshake_bytes);
    write_packet(
        &mut client_tcp,
        &framemc::protocol::RawPacket::new(0x00, handshake_bytes.freeze()),
    )
    .await
    .unwrap();

    // LoginStart
    let login_start = LoginStartPacket::new("Alex", Uuid::nil());
    write_packet(&mut client_tcp, &login_start.encode())
        .await
        .unwrap();

    // EncryptionRequest
    let enc_req_raw = read_packet(&mut client_tcp, DEFAULT_MAX_PACKET_SIZE)
        .await
        .unwrap();
    let enc_req = EncryptionRequestPacket::decode(&enc_req_raw).unwrap();

    let rsa_public_key = RsaPublicKey::from_public_key_der(&enc_req.public_key).unwrap();
    let mut rng = rand::thread_rng();
    let mut shared_secret = [0u8; 16];
    rng.fill_bytes(&mut shared_secret);

    // Provide invalid verify token
    let bad_token = vec![0xFF, 0xFE, 0xFD, 0xFC];
    let encrypted_secret = rsa_public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &shared_secret)
        .unwrap();
    let encrypted_bad_token = rsa_public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &bad_token)
        .unwrap();

    let enc_resp = EncryptionResponsePacket::new(encrypted_secret, encrypted_bad_token);
    write_packet(&mut client_tcp, &enc_resp.encode())
        .await
        .unwrap();

    // The proxy MUST terminate the connection immediately.
    // Reading further on client_tcp should result in EOF (0 bytes or UnexpectedEof).
    let mut encrypted_client = EncryptedStream::new(client_tcp, &shared_secret);
    let read_result = read_packet(&mut encrypted_client, DEFAULT_MAX_PACKET_SIZE).await;
    assert!(
        read_result.is_err(),
        "Server must close stream immediately upon verify token mismatch"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_offline_mode_login_over_tcp() {
    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0,
        motd: "§aFrameMC Offline MOTD".to_string(),
        max_players: 100,
        online_mode: false,
        favicon: None,
        session_server_url: None,
        servers: HashMap::new(),
        default_server: "lobby".to_string(),
        script_path: "scripts/main.rhai".to_string(),
        plugins_dir: "plugins".to_string(),
    };

    let listener = bind_listener(&config).await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    config.bind_port = actual_port;
    let config = Arc::new(config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let script_host = Arc::new(ScriptHost::new());
    let _server_handle = tokio::spawn(start_listener_on(
        listener,
        config,
        script_host,
        shutdown_rx,
    ));

    let mut client_tcp = TcpStream::connect(format!("127.0.0.1:{actual_port}"))
        .await
        .unwrap();

    // Handshake
    let mut handshake_bytes = bytes::BytesMut::new();
    framemc::protocol::varint::encode_varint(765, &mut handshake_bytes);
    framemc::protocol::varint::encode_varint(9, &mut handshake_bytes);
    handshake_bytes.extend_from_slice(b"localhost");
    handshake_bytes.extend_from_slice(&actual_port.to_be_bytes());
    framemc::protocol::varint::encode_varint(2, &mut handshake_bytes);
    write_packet(
        &mut client_tcp,
        &framemc::protocol::RawPacket::new(0x00, handshake_bytes.freeze()),
    )
    .await
    .unwrap();

    // LoginStart
    let login_start = LoginStartPacket::new("OfflineSteve", Uuid::nil());
    write_packet(&mut client_tcp, &login_start.encode())
        .await
        .unwrap();

    // In offline mode, proxy immediately replies with LoginSuccess over plaintext TCP
    let success_raw = read_packet(&mut client_tcp, DEFAULT_MAX_PACKET_SIZE)
        .await
        .unwrap();
    assert_eq!(success_raw.id, 0x02);

    let login_success = LoginSuccessPacket::decode_with_version(&success_raw, 765).unwrap();
    assert_eq!(login_success.username, "OfflineSteve");
    assert_eq!(login_success.uuid, offline_profile("OfflineSteve").id);
    assert!(login_success.properties.is_empty());

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_offline_mode_login_protocol_776_over_tcp() {
    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0,
        motd: "§aFrameMC Offline MOTD 776".to_string(),
        max_players: 100,
        online_mode: false,
        favicon: None,
        session_server_url: None,
        servers: HashMap::new(),
        default_server: "lobby".to_string(),
        script_path: "scripts/main.rhai".to_string(),
        plugins_dir: "plugins".to_string(),
    };

    let listener = bind_listener(&config).await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    config.bind_port = actual_port;
    let config = Arc::new(config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let script_host = Arc::new(ScriptHost::new());
    let _server_handle = tokio::spawn(start_listener_on(
        listener,
        config,
        script_host,
        shutdown_rx,
    ));

    let mut client_tcp = TcpStream::connect(format!("127.0.0.1:{actual_port}"))
        .await
        .unwrap();

    // Handshake with Protocol 776
    let mut handshake_bytes = bytes::BytesMut::new();
    framemc::protocol::varint::encode_varint(776, &mut handshake_bytes);
    framemc::protocol::varint::encode_varint(9, &mut handshake_bytes);
    handshake_bytes.extend_from_slice(b"localhost");
    handshake_bytes.extend_from_slice(&actual_port.to_be_bytes());
    framemc::protocol::varint::encode_varint(2, &mut handshake_bytes);
    write_packet(
        &mut client_tcp,
        &framemc::protocol::RawPacket::new(0x00, handshake_bytes.freeze()),
    )
    .await
    .unwrap();

    // LoginStart
    let login_start = LoginStartPacket::new("OfflineSteve776", Uuid::nil());
    write_packet(&mut client_tcp, &login_start.encode())
        .await
        .unwrap();

    // Proxy replies with LoginSuccess over plaintext TCP
    let success_raw = read_packet(&mut client_tcp, DEFAULT_MAX_PACKET_SIZE)
        .await
        .unwrap();
    assert_eq!(success_raw.id, 0x02);

    let login_success = LoginSuccessPacket::decode_with_version(&success_raw, 776).unwrap();
    assert_eq!(login_success.username, "OfflineSteve776");
    assert_eq!(login_success.uuid, offline_profile("OfflineSteve776").id);
    assert!(login_success.properties.is_empty());
    assert!(
        login_success.session_id.is_some(),
        "Protocol 776 must include session_id"
    );
    assert_ne!(
        login_success.session_id.unwrap(),
        Uuid::nil(),
        "session_id should not be nil"
    );

    let _ = shutdown_tx.send(true);
}

#[tokio::test]
async fn test_online_mode_case_insensitive_username_matches() {
    let (mock_session_url, _mock_guard) = start_mock_mojang_session_server().await;

    let mut config = ProxyConfig {
        bind_address: "127.0.0.1".to_string(),
        bind_port: 0,
        motd: "§aFrameMC Case Insensitive Test MOTD".to_string(),
        max_players: 100,
        online_mode: true,
        favicon: None,
        session_server_url: Some(mock_session_url),
        servers: HashMap::new(),
        default_server: "lobby".to_string(),
        script_path: "scripts/main.rhai".to_string(),
        plugins_dir: "plugins".to_string(),
    };

    let listener = bind_listener(&config).await.unwrap();
    let actual_port = listener.local_addr().unwrap().port();
    config.bind_port = actual_port;
    let config = Arc::new(config);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let script_host = Arc::new(ScriptHost::new());
    let _server_handle = tokio::spawn(start_listener_on(
        listener,
        config,
        script_host,
        shutdown_rx,
    ));

    let mut client_tcp = TcpStream::connect(format!("127.0.0.1:{actual_port}"))
        .await
        .unwrap();

    // Handshake
    let mut handshake_bytes = bytes::BytesMut::new();
    framemc::protocol::varint::encode_varint(765, &mut handshake_bytes);
    framemc::protocol::varint::encode_varint(9, &mut handshake_bytes);
    handshake_bytes.extend_from_slice(b"localhost");
    handshake_bytes.extend_from_slice(&actual_port.to_be_bytes());
    framemc::protocol::varint::encode_varint(2, &mut handshake_bytes);
    write_packet(
        &mut client_tcp,
        &framemc::protocol::RawPacket::new(0x00, handshake_bytes.freeze()),
    )
    .await
    .unwrap();

    // Client requests "sTeVe" (mixed-case) while Mojang mock returns "Steve"
    let login_start = LoginStartPacket::new("sTeVe", Uuid::nil());
    write_packet(&mut client_tcp, &login_start.encode())
        .await
        .unwrap();

    // EncryptionRequest
    let enc_req_raw = read_packet(&mut client_tcp, DEFAULT_MAX_PACKET_SIZE)
        .await
        .unwrap();
    let enc_req = EncryptionRequestPacket::decode(&enc_req_raw).unwrap();

    let rsa_public_key = RsaPublicKey::from_public_key_der(&enc_req.public_key).unwrap();
    let mut rng = rand::thread_rng();
    let mut shared_secret = [0u8; 16];
    rng.fill_bytes(&mut shared_secret);

    let encrypted_secret = rsa_public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &shared_secret)
        .unwrap();
    let encrypted_token = rsa_public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &enc_req.verify_token)
        .unwrap();

    let enc_resp = EncryptionResponsePacket::new(encrypted_secret, encrypted_token);
    write_packet(&mut client_tcp, &enc_resp.encode())
        .await
        .unwrap();

    // Client transitions into EncryptedStream
    let mut encrypted_client = EncryptedStream::new(client_tcp, &shared_secret);

    // Read LoginSuccessPacket
    let success_raw = read_packet(&mut encrypted_client, DEFAULT_MAX_PACKET_SIZE)
        .await
        .unwrap();
    assert_eq!(success_raw.id, 0x02);
    let login_success = LoginSuccessPacket::decode_with_version(&success_raw, 765).unwrap();
    assert_eq!(login_success.username, "Steve");

    let _ = shutdown_tx.send(true);
}
