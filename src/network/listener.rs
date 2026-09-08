use rand::Rng;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use crate::config::ProxyConfig;
use crate::crypto::{
    mojang_sha1_digest, offline_profile, verify_session_with_url, EncryptedStream, PlayerProfile,
    RsaKeyManager, DEFAULT_MOJANG_SESSION_SERVER,
};
use crate::error::ProxyError;
use crate::network::codec::{
    encode_packet_with_compression, read_packet, read_packet_with_compression, write_packet,
    write_packet_with_compression, DisconnectPacket, DEFAULT_MAX_PACKET_SIZE,
};
use crate::protocol::configuration::{SessionRegistryCache, REGISTRY_DATA_PACKET_ID};
use crate::protocol::forwarding::connect_and_forward;
use crate::protocol::handshake::{ConnectionState, HandshakePacket};
use crate::protocol::is_protected_plugin_channel;
use crate::protocol::login::{
    encode_login_success_with_session, EncryptionRequestPacket, EncryptionResponsePacket,
    LoginAcknowledgedPacket, LoginStartPacket, LoginSuccessPacket,
};
use crate::protocol::status::handle_status_with_version;
use crate::protocol::varint::decode_varint;
use crate::routing::state_machine::{PlayStateMachine, PlayerSession, TcpBackendConnector};
use crate::script::engine::ScriptHost;
use crate::script::events::PlayerJoinEvent;
use uuid::Uuid;

#[inline]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// An active connection stream that may either be plain (unencrypted) or AES-128-CFB8 encrypted.
#[allow(clippy::large_enum_variant)]
pub enum ActiveStream<S> {
    Plain(S),
    Encrypted(EncryptedStream<S>),
}

impl<S> std::fmt::Debug for ActiveStream<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActiveStream::Plain(_) => write!(f, "ActiveStream::Plain(..)"),
            ActiveStream::Encrypted(_) => write!(f, "ActiveStream::Encrypted(..)"),
        }
    }
}

impl<S> ActiveStream<S> {
    pub fn is_encrypted(&self) -> bool {
        matches!(self, ActiveStream::Encrypted(_))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for ActiveStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ActiveStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            ActiveStream::Encrypted(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for ActiveStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ActiveStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            ActiveStream::Encrypted(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ActiveStream::Plain(s) => Pin::new(s).poll_flush(cx),
            ActiveStream::Encrypted(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ActiveStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            ActiveStream::Encrypted(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Awaits until the shutdown watch channel receives `true`.
/// Ensures no non-Send `Ref` guard is held across `.await` boundaries to preserve `Send` bounds.
pub async fn wait_for_shutdown(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Binds a `TcpListener` based on the provided configuration.
pub async fn bind_listener(config: &ProxyConfig) -> Result<TcpListener, ProxyError> {
    let addr = format!("{}:{}", config.bind_address, config.bind_port);
    let listener = TcpListener::bind(&addr).await?;
    Ok(listener)
}

/// Starts accepting client TCP connections on an existing `TcpListener` with default crypto and HTTP components.
pub async fn start_listener_on(
    listener: TcpListener,
    config: Arc<ProxyConfig>,
    script_host: Arc<ScriptHost>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ProxyError> {
    let rsa_manager = Arc::new(RsaKeyManager::new()?);
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| {
            ProxyError::AuthenticationFailed(format!("Failed to build HTTP client: {e}"))
        })?;

    start_listener_with_state_on(
        listener,
        config,
        rsa_manager,
        http_client,
        script_host,
        shutdown_rx,
    )
    .await
}

/// Starts accepting client TCP connections using a provided `RsaKeyManager` and `reqwest::Client`.
pub async fn start_listener_with_state_on(
    listener: TcpListener,
    config: Arc<ProxyConfig>,
    rsa_manager: Arc<RsaKeyManager>,
    http_client: reqwest::Client,
    script_host: Arc<ScriptHost>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ProxyError> {
    let max_conns = if config.max_players >= 0 {
        config.max_players as usize
    } else {
        1000
    };
    let connection_semaphore = Arc::new(tokio::sync::Semaphore::new(max_conns));

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, client_addr)) => {
                        // Immediately enable TCP_NODELAY on accept
                        if let Err(e) = stream.set_nodelay(true) {
                            tracing::warn!(%client_addr, "Failed to set TCP_NODELAY: {e}");
                        }

                        let permit = match Arc::clone(&connection_semaphore).try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                tracing::warn!(%client_addr, "Rejecting connection: max_players limit ({}) reached", config.max_players);
                                drop(stream);
                                continue;
                            }
                        };

                        let client_config = Arc::clone(&config);
                        let client_rsa = Arc::clone(&rsa_manager);
                        let client_http = http_client.clone();
                        let client_script = Arc::clone(&script_host);
                        let client_shutdown = shutdown_rx.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            handle_client(stream, client_addr, client_config, client_rsa, client_http, client_script, client_shutdown).await;
                        });
                    }
                    Err(e) => {
                        tracing::error!("Failed to accept incoming TCP connection: {e}");
                    }
                }
            }
            _ = wait_for_shutdown(&mut shutdown_rx) => {
                tracing::info!("Shutdown signal received, terminating TCP listener loop");
                break;
            }
        }
    }

    Ok(())
}

/// Binds to `${config.bind_address}:${config.bind_port}` and serves incoming Minecraft connections.
pub async fn start_listener(
    config: Arc<ProxyConfig>,
    script_host: Arc<ScriptHost>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ProxyError> {
    let listener = bind_listener(&config).await?;
    tracing::info!("Proxy TCP listener active on {}", listener.local_addr()?);
    start_listener_on(listener, config, script_host, shutdown_rx).await
}

/// Handles an individual client connection lifecycle.
async fn handle_client(
    stream: TcpStream,
    client_addr: SocketAddr,
    config: Arc<ProxyConfig>,
    rsa_manager: Arc<RsaKeyManager>,
    http_client: reqwest::Client,
    script_host: Arc<ScriptHost>,
    shutdown_rx: watch::Receiver<bool>,
) {
    if let Err(err) = process_connection(
        stream,
        client_addr,
        config,
        &rsa_manager,
        &http_client,
        script_host,
        shutdown_rx,
    )
    .await
    {
        tracing::warn!(%client_addr, "Client connection error: {err}");
    }
}

/// Handles the authentication phase of the Minecraft login sequence.
/// Performs RSA decryption and Mojang authentication in online mode,
/// or generates offline UUID in offline mode, leaving the client in Login state.
pub async fn handle_login_auth<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: ActiveStream<S>,
    client_ip: &str,
    handshake: &HandshakePacket,
    config: &ProxyConfig,
    rsa_manager: &RsaKeyManager,
    http_client: &reqwest::Client,
) -> Result<(ActiveStream<S>, PlayerProfile), ProxyError> {
    // 1. Read initial Login packet: LoginStart (0x00) with 10s timeout
    let raw_packet = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE),
    )
    .await
    .map_err(|_| {
        ProxyError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "LoginStart read timed out",
        ))
    })??;
    let login_start = LoginStartPacket::decode(&raw_packet)?;

    tracing::debug!(
        username = %login_start.username,
        player_uuid = %login_start.player_uuid,
        online_mode = config.online_mode,
        "Processing LoginStart packet"
    );

    let profile = if config.online_mode {
        // 2. Generate 4-byte random verification token
        let mut verify_token = [0u8; 4];
        rand::thread_rng().fill(&mut verify_token);

        // 3. Send EncryptionRequestPacket (0x01)
        let enc_req = EncryptionRequestPacket::new(
            "",
            rsa_manager.public_key_der().to_vec(),
            verify_token.to_vec(),
        );
        write_packet(
            &mut stream,
            &enc_req.encode_with_version(handshake.protocol_version),
        )
        .await?;

        // 4. Read EncryptionResponsePacket (0x01) with 10s timeout
        let resp_raw = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE),
        )
        .await
        .map_err(|_| {
            ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "EncryptionResponse read timed out",
            ))
        })??;
        let enc_resp = EncryptionResponsePacket::decode(&resp_raw)?;

        // 5. Decrypt shared secret and verification token with RSA private key in blocking thread
        let rsa = rsa_manager.clone();
        let sec_cipher = enc_resp.shared_secret.clone();
        let tok_cipher = enc_resp.verify_token.clone();
        let (decrypted_secret, decrypted_token) = tokio::task::spawn_blocking(move || {
            let sec = rsa.decrypt(&sec_cipher);
            let tok = rsa.decrypt(&tok_cipher);
            (sec, tok)
        })
        .await
        .map_err(|e| ProxyError::CryptoError(format!("RSA decryption task panicked: {e}")))?;
        let decrypted_secret = zeroize::Zeroizing::new(decrypted_secret?);
        let decrypted_token = decrypted_token?;

        // 6. Assert returned verification token matches sent token (constant-time check)
        if !constant_time_eq(&decrypted_token, &verify_token) {
            return Err(ProxyError::AuthenticationFailed(
                "Encryption verification token mismatch".to_string(),
            ));
        }

        // Validate shared secret length for AES-128
        if decrypted_secret.len() != 16 {
            return Err(ProxyError::AuthenticationFailed(format!(
                "Invalid shared secret length: {} bytes (expected 16)",
                decrypted_secret.len()
            )));
        }

        let mut shared_secret_arr = zeroize::Zeroizing::new([0u8; 16]);
        shared_secret_arr.copy_from_slice(&decrypted_secret);
        drop(decrypted_secret); // [R-05] Wipe secret bytes from local memory immediately

        // 7. Calculate Mojang SHA-1 negative carry hash
        let server_hash =
            mojang_sha1_digest("", &shared_secret_arr[..], rsa_manager.public_key_der());

        // 8. Verify session with Mojang hasJoined endpoint
        let endpoint_url = config
            .session_server_url
            .as_deref()
            .unwrap_or(DEFAULT_MOJANG_SESSION_SERVER);

        let verified_profile = match verify_session_with_url(
            &login_start.username,
            &server_hash,
            client_ip,
            http_client,
            endpoint_url,
        )
        .await
        {
            Ok(p) => p,
            Err(e) => {
                let inner_stream = match stream {
                    ActiveStream::Plain(s) => s,
                    ActiveStream::Encrypted(s) => s.into_inner(),
                };
                let mut encrypted_stream = EncryptedStream::new(inner_stream, &shared_secret_arr);

                let disconnect = DisconnectPacket::new(format!("§cFailed to verify username: {e}"));
                let _ = write_packet(&mut encrypted_stream, &disconnect.encode_login()).await;
                let _ = encrypted_stream.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;

                return Err(e);
            }
        };

        // Ensure verified profile name matches requested login username case-insensitively
        if !verified_profile
            .name
            .eq_ignore_ascii_case(&login_start.username)
        {
            return Err(ProxyError::AuthenticationFailed(format!(
                "Username mismatch: client requested '{}', Mojang authenticated as '{}'",
                login_start.username, verified_profile.name
            )));
        }

        // 9. Transition stream into EncryptedStream
        let inner_stream = match stream {
            ActiveStream::Plain(s) => s,
            ActiveStream::Encrypted(_) => {
                return Err(ProxyError::AuthenticationFailed(
                    "Stream is already encrypted".to_string(),
                ));
            }
        };

        let encrypted_stream = EncryptedStream::new(inner_stream, &shared_secret_arr);
        stream = ActiveStream::Encrypted(encrypted_stream);

        verified_profile
    } else {
        // Offline mode: generate offline profile with v3 MD5 UUID
        offline_profile(&login_start.username)
    };

    Ok((stream, profile))
}

/// Executes the full Login state lifecycle, including RSA encryption handshake, Mojang authentication,
/// and sending LoginSuccess.
pub async fn handle_login<S: AsyncRead + AsyncWrite + Unpin>(
    stream: ActiveStream<S>,
    client_ip: &str,
    handshake: &HandshakePacket,
    config: &ProxyConfig,
    rsa_manager: &RsaKeyManager,
    http_client: &reqwest::Client,
) -> Result<(ActiveStream<S>, PlayerProfile), ProxyError> {
    let (mut stream, profile) = handle_login_auth(
        stream,
        client_ip,
        handshake,
        config,
        rsa_manager,
        http_client,
    )
    .await?;

    // 10. Send LoginSuccessPacket (0x02) containing verified UUID, username, and properties
    let session_id = if handshake.protocol_version >= 776 {
        Some(Uuid::new_v4())
    } else {
        None
    };
    let raw_login_success = encode_login_success_with_session(
        profile.id,
        &profile.name,
        &profile.properties,
        session_id,
        handshake.protocol_version,
    );
    write_packet(&mut stream, &raw_login_success).await?;
    stream.flush().await?;

    tracing::info!(
        username = %profile.name,
        uuid = %profile.id,
        session_id = ?session_id,
        protocol_version = handshake.protocol_version,
        "LoginSuccess sent to client"
    );

    Ok((stream, profile))
}

/// Processes the initial protocol handshake and routes to Status or Login states.
async fn process_connection(
    mut stream: TcpStream,
    client_addr: SocketAddr,
    config: Arc<ProxyConfig>,
    rsa_manager: &RsaKeyManager,
    http_client: &reqwest::Client,
    script_host: Arc<ScriptHost>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ProxyError> {
    // 1. Read initial packet: Handshake with 10s timeout
    let raw_packet = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE),
    )
    .await
    .map_err(|_| {
        ProxyError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Handshake read timed out",
        ))
    })??;
    let handshake = HandshakePacket::decode(&raw_packet)?;

    tracing::debug!(
        %client_addr,
        protocol_version = handshake.protocol_version,
        server_address = handshake.server_address,
        server_port = handshake.server_port,
        next_state = ?handshake.next_state,
        "Received Handshake"
    );

    // 2. Dispatch based on next_state
    match handshake.next_state {
        ConnectionState::Status => {
            // Execute server list ping flow and close connection cleanly
            handle_status_with_version(&mut stream, &config, handshake.protocol_version).await?;
            Ok(())
        }
        ConnectionState::Login => {
            let client_ip = client_addr.ip().to_string();
            let (mut active_stream, profile) = handle_login_auth(
                ActiveStream::Plain(stream),
                &client_ip,
                &handshake,
                &config,
                rsa_manager,
                http_client,
            )
            .await?;

            // Evaluate Rhai on_player_join hooks [R-04], [R-09]
            let join_event = PlayerJoinEvent::new(
                &profile.name,
                profile.id.to_string(),
                &client_ip,
                handshake.protocol_version,
            );
            let join_decision = script_host.eval_join(join_event).await;
            if !join_decision.allow {
                let reason = if join_decision.disconnect_reason.is_empty() {
                    "Disconnected by proxy script".to_string()
                } else {
                    join_decision.disconnect_reason
                };
                tracing::warn!(%client_addr, player = %profile.name, %reason, "Player rejected by Rhai join hook");
                let disconnect = DisconnectPacket::new(reason);
                let _ = write_packet(&mut active_stream, &disconnect.encode_login()).await;
                let _ = active_stream.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                return Err(ProxyError::AuthenticationFailed(
                    "Rejected by script".to_string(),
                ));
            }

            let target_server_name = if !join_decision.target_server.is_empty()
                && config.servers.contains_key(&join_decision.target_server)
            {
                join_decision.target_server
            } else {
                config.default_server.clone()
            };

            let backend_config = match config.servers.get(&target_server_name) {
                Some(cfg) => cfg.clone(),
                None => {
                    tracing::error!(%client_addr, server = %target_server_name, "Target server not found in configuration");
                    let raw_login_success = encode_login_success_with_session(
                        profile.id,
                        &profile.name,
                        &profile.properties,
                        Some(Uuid::new_v4()),
                        handshake.protocol_version,
                    );
                    let _ = write_packet(&mut active_stream, &raw_login_success).await;
                    let _ = active_stream.flush().await;

                    let disconnect = DisconnectPacket::new(format!(
                        "§cTarget server '{target_server_name}' not configured"
                    ));
                    let pkt = disconnect.encode_for_client(handshake.protocol_version, true);
                    let _ = write_packet(&mut active_stream, &pkt).await;
                    let _ = active_stream.flush().await;
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    return Err(ProxyError::ConfigError(format!(
                        "Target server '{target_server_name}' not found"
                    )));
                }
            };

            // 11. Connect to configured backend server immediately after client auth
            let mut backend_stream = match connect_and_forward(
                &backend_config,
                &profile,
                &client_ip,
                handshake.protocol_version,
            )
            .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::error!(%client_addr, server = %target_server_name, "Failed to connect to backend: {e}");
                    let disconnect = DisconnectPacket::new(format!(
                        "§cCould not connect to {target_server_name}: {e}"
                    ));
                    let _ = write_packet(&mut active_stream, &disconnect.encode_login()).await;
                    let _ = active_stream.flush().await;
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    return Err(e);
                }
            };

            // 12. Read backend's response packet (handling SetCompression or LoginSuccess)
            let mut backend_login_pkt = match read_packet(
                &mut backend_stream,
                DEFAULT_MAX_PACKET_SIZE,
            )
            .await
            {
                Ok(pkt) => pkt,
                Err(e) => {
                    tracing::error!(%client_addr, server = %target_server_name, "Failed to read login packet from backend: {e}");
                    let disconnect =
                        DisconnectPacket::new(format!("§cBackend connection lost: {e}"));
                    let _ = write_packet(&mut active_stream, &disconnect.encode_login()).await;
                    let _ = active_stream.flush().await;
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    return Err(e);
                }
            };

            let mut compression_threshold: Option<usize> = None;

            if backend_login_pkt.id == 0x03 {
                // SetCompression from backend
                let mut cursor = &backend_login_pkt.payload[..];
                let thresh = decode_varint(&mut cursor)?;
                if thresh < 0 {
                    return Err(ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Negative compression threshold from backend",
                    )));
                }
                let threshold = thresh as usize;
                compression_threshold = Some(threshold);
                tracing::info!(%client_addr, threshold, "Backend enabled compression; synchronizing with client");

                // Forward SetCompression (0x03) uncompressed to client
                write_packet(&mut active_stream, &backend_login_pkt).await?;
                let _ = active_stream.flush().await;

                backend_login_pkt = match read_packet_with_compression(
                    &mut backend_stream,
                    DEFAULT_MAX_PACKET_SIZE,
                    compression_threshold,
                )
                .await
                {
                    Ok(pkt) => pkt,
                    Err(e) => {
                        tracing::error!(%client_addr, server = %target_server_name, "Failed to read compressed login packet from backend: {e}");
                        let disconnect =
                            DisconnectPacket::new(format!("§cBackend connection lost: {e}"));
                        let _ = write_packet_with_compression(
                            &mut active_stream,
                            &disconnect.encode_login(),
                            compression_threshold,
                        )
                        .await;
                        let _ = active_stream.flush().await;
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                        return Err(e);
                    }
                };
            }

            if backend_login_pkt.id == 0x01 {
                let msg = "Downstream backend server has online-mode or encryption enabled! Set encryption=false in the backend server configuration.";
                tracing::error!(%client_addr, server = %target_server_name, "{msg}");
                let disconnect = DisconnectPacket::new(format!("§c{msg}"));
                let _ = write_packet_with_compression(
                    &mut active_stream,
                    &disconnect.encode_login(),
                    compression_threshold,
                )
                .await;
                let _ = active_stream.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                return Err(ProxyError::AuthenticationFailed(msg.to_string()));
            }
            if backend_login_pkt.id == 0x00 {
                let reason = String::from_utf8_lossy(&backend_login_pkt.payload).to_string();
                tracing::warn!(%client_addr, server = %target_server_name, "Backend rejected login: {reason}");
                let disconnect = DisconnectPacket::new(&reason);
                let _ = write_packet_with_compression(
                    &mut active_stream,
                    &disconnect.encode_login(),
                    compression_threshold,
                )
                .await;
                let _ = active_stream.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                return Err(ProxyError::AuthenticationFailed(format!(
                    "Backend disconnected during login: {reason}"
                )));
            }
            if backend_login_pkt.id != 0x02 {
                let msg = format!(
                    "Backend sent unexpected login packet 0x{:02X}",
                    backend_login_pkt.id
                );
                tracing::error!(%client_addr, server = %target_server_name, "{msg}");
                let disconnect = DisconnectPacket::new(format!("§c{msg}"));
                let _ = write_packet_with_compression(
                    &mut active_stream,
                    &disconnect.encode_login(),
                    compression_threshold,
                )
                .await;
                let _ = active_stream.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                return Err(ProxyError::InvalidPacketId(backend_login_pkt.id));
            }

            // Backend accepted! Send LoginSuccessPacket (0x02) to client
            let backend_success = LoginSuccessPacket::decode_with_version(
                &backend_login_pkt,
                handshake.protocol_version,
            )
            .ok();
            let session_id = backend_success
                .and_then(|s| s.session_id)
                .unwrap_or_else(Uuid::new_v4);

            let raw_login_success = encode_login_success_with_session(
                profile.id,
                &profile.name,
                &profile.properties,
                Some(session_id),
                handshake.protocol_version,
            );
            let success_wire =
                encode_packet_with_compression(&raw_login_success, compression_threshold)?;
            active_stream.write_all(&success_wire).await?;
            active_stream.flush().await?;

            tracing::info!(
                username = %profile.name,
                uuid = %profile.id,
                session_id = ?session_id,
                protocol_version = handshake.protocol_version,
                "LoginSuccess sent to client"
            );

            let mut registry_cache = SessionRegistryCache::new();

            // 13. State Transition: Configuration (>= 764) or direct Play (< 764)
            if handshake.protocol_version >= 764 {
                // Await LoginAcknowledged (0x03) from client before switching state with 10s timeout
                let client_ack_raw = tokio::select! {
                    res = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        read_packet_with_compression(&mut active_stream, DEFAULT_MAX_PACKET_SIZE, compression_threshold)
                    ) => {
                        match res {
                            Ok(packet_res) => packet_res?,
                            Err(_) => return Err(ProxyError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "LoginAcknowledged read timed out"))),
                        }
                    }
                    _ = wait_for_shutdown(&mut shutdown_rx) => {
                        let disconnect = DisconnectPacket::new("§cServer is shutting down");
                        let pkt = disconnect.encode_for_client(handshake.protocol_version, true);
                        let _ = write_packet_with_compression(&mut active_stream, &pkt, compression_threshold).await;
                        let _ = active_stream.flush().await;
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                        return Ok(());
                    }
                };
                let _ = LoginAcknowledgedPacket::decode(&client_ack_raw)?;
                tracing::info!(%client_addr, "Received LoginAcknowledged from client");

                // Forward LoginAcknowledged (0x03) to backend to transition backend to Configuration state
                let ack = LoginAcknowledgedPacket::new();
                write_packet_with_compression(
                    &mut backend_stream,
                    &ack.encode(),
                    compression_threshold,
                )
                .await?;
                let _ = backend_stream.flush().await;
                tracing::info!(%client_addr, "Forwarded LoginAcknowledged to backend; entering Configuration state");

                // 14. Configuration State Bridging
                let mut client_finished_config = false;
                let mut backend_finished_config = false;

                let backend_finish_id = if handshake.protocol_version >= 766 {
                    0x03
                } else {
                    0x02
                };
                let backend_disconnect_id = if handshake.protocol_version >= 766 {
                    0x02
                } else {
                    0x01
                };

                while !client_finished_config || !backend_finished_config {
                    tokio::select! {
                        backend_res = read_packet_with_compression(&mut backend_stream, DEFAULT_MAX_PACKET_SIZE, compression_threshold) => {
                            let pkt = backend_res?;
                            if is_protected_plugin_channel(&pkt) {
                                tracing::warn!(%client_addr, "Dropping backend packet targeting protected plugin channel");
                                continue;
                            }
                            if pkt.id == REGISTRY_DATA_PACKET_ID || (handshake.protocol_version < 766 && pkt.id == 0x05) {
                                let _ = registry_cache.cache_packet(&pkt);
                            }
                            if pkt.id == backend_finish_id {
                                backend_finished_config = true;
                                tracing::info!(%client_addr, "Backend sent FinishConfiguration (0x{:02X})", pkt.id);
                            }
                            if pkt.id == backend_disconnect_id {
                                tracing::warn!(%client_addr, "Backend disconnected client during Configuration state");
                                write_packet_with_compression(&mut active_stream, &pkt, compression_threshold).await?;
                                let _ = active_stream.flush().await;
                                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                                return Ok(());
                            }
                            write_packet_with_compression(&mut active_stream, &pkt, compression_threshold).await?;
                            let _ = active_stream.flush().await;
                        }
                        client_res = read_packet_with_compression(&mut active_stream, DEFAULT_MAX_PACKET_SIZE, compression_threshold) => {
                            let pkt = client_res?;
                            if is_protected_plugin_channel(&pkt) {
                                tracing::warn!(%client_addr, "Dropping client packet targeting protected plugin channel");
                                continue;
                            }
                            // Serverbound FinishConfiguration is 0x03 in 1.20.5+ (>= 766) or 0x02 in 1.20.2-1.20.4 (< 766)
                            if (handshake.protocol_version >= 766 && pkt.id == 0x03)
                                || (handshake.protocol_version < 766 && pkt.id == 0x02)
                            {
                                client_finished_config = true;
                                tracing::info!(%client_addr, "Client sent FinishConfiguration (0x{:02X})", pkt.id);
                            }
                            write_packet_with_compression(&mut backend_stream, &pkt, compression_threshold).await?;
                            let _ = backend_stream.flush().await;
                        }
                        _ = wait_for_shutdown(&mut shutdown_rx) => {
                            tracing::info!(%client_addr, "Shutdown signal received during Configuration state");
                            let disconnect = DisconnectPacket::new("§cServer is shutting down");
                            let pkt = disconnect.encode_for_client(handshake.protocol_version, true);
                            let _ = write_packet_with_compression(&mut active_stream, &pkt, compression_threshold).await;
                            let _ = active_stream.flush().await;
                            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                            return Ok(());
                        }
                    }
                }
            }

            tracing::info!(
                %client_addr,
                username = %profile.name,
                uuid = %profile.id,
                server = %target_server_name,
                "Configuration state complete; handing off to Play state machine"
            );

            // 15. Transition to ConnectionState::Play and hand off to PlayStateMachine [R-02], [R-10], [R-11]
            let session = PlayerSession::new(
                profile,
                client_ip,
                handshake.protocol_version,
                target_server_name,
                registry_cache,
            );
            let connector = Arc::new(TcpBackendConnector);
            let mut state_machine = PlayStateMachine::new(
                Box::new(active_stream),
                Box::new(backend_stream),
                connector,
                session,
                config,
                script_host,
                compression_threshold,
            );
            state_machine.run(shutdown_rx).await?;

            Ok(())
        }
        other_state => Err(ProxyError::InvalidConnectionState(match other_state {
            ConnectionState::Handshake => 0,
            ConnectionState::Status => 1,
            ConnectionState::Login => 2,
            ConnectionState::Configuration => 3,
            ConnectionState::Play => 4,
            ConnectionState::Closed => 5,
        })),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::login::{encode_login_success, LoginSuccessPacket};
    use crate::protocol::packet::RawPacket;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_offline_mode_login_flow() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let mut client_stream = ActiveStream::Plain(client_io);

        let config = ProxyConfig {
            online_mode: false,
            ..Default::default()
        };

        let rsa = RsaKeyManager::new().unwrap();
        let http_client = reqwest::Client::new();
        let handshake = HandshakePacket::new(765, "localhost", 25565, ConnectionState::Login);

        let server_task = tokio::spawn(async move {
            handle_login(
                ActiveStream::Plain(server_io),
                "127.0.0.1",
                &handshake,
                &config,
                &rsa,
                &http_client,
            )
            .await
        });

        // 1. Client sends LoginStart
        let login_start = LoginStartPacket::new("Steve", Uuid::nil());
        write_packet(&mut client_stream, &login_start.encode())
            .await
            .unwrap();

        // 2. Client receives LoginSuccess
        let success_raw = read_packet(&mut client_stream, DEFAULT_MAX_PACKET_SIZE)
            .await
            .unwrap();
        let login_success = LoginSuccessPacket::decode_with_version(&success_raw, 765).unwrap();

        assert_eq!(login_success.username, "Steve");
        let expected_uuid = offline_profile("Steve").id;
        assert_eq!(login_success.uuid, expected_uuid);

        let (server_stream, server_profile) = server_task.await.unwrap().unwrap();
        assert_eq!(server_profile.name, "Steve");
        assert_eq!(server_profile.id, expected_uuid);
        assert!(!server_stream.is_encrypted());
    }

    #[tokio::test]
    async fn test_online_mode_token_mismatch_fails_closed() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let mut client_stream = ActiveStream::Plain(client_io);

        let config = ProxyConfig {
            online_mode: true,
            ..Default::default()
        };

        let rsa = Arc::new(RsaKeyManager::new().unwrap());
        let http_client = reqwest::Client::new();
        let handshake = HandshakePacket::new(765, "localhost", 25565, ConnectionState::Login);

        let rsa_clone = Arc::clone(&rsa);
        let server_task = tokio::spawn(async move {
            handle_login(
                ActiveStream::Plain(server_io),
                "127.0.0.1",
                &handshake,
                &config,
                &rsa_clone,
                &http_client,
            )
            .await
        });

        // 1. Client sends LoginStart
        let login_start = LoginStartPacket::new("Alex", Uuid::nil());
        write_packet(&mut client_stream, &login_start.encode())
            .await
            .unwrap();

        // 2. Client receives EncryptionRequest
        let enc_req_raw = read_packet(&mut client_stream, DEFAULT_MAX_PACKET_SIZE)
            .await
            .unwrap();
        let _enc_req = EncryptionRequestPacket::decode(&enc_req_raw).unwrap();

        // 3. Client replies with an INVALID verify token
        let shared_secret = vec![0x42; 16];
        let bad_token = vec![0x99, 0x99, 0x99, 0x99]; // does not match enc_req.verify_token
        let encrypted_secret = rsa.encrypt(&shared_secret).unwrap();
        let encrypted_bad_token = rsa.encrypt(&bad_token).unwrap();

        let enc_resp = EncryptionResponsePacket::new(encrypted_secret, encrypted_bad_token);
        write_packet(&mut client_stream, &enc_resp.encode())
            .await
            .unwrap();

        // 4. Server task MUST fail with AuthenticationFailed
        let server_result = server_task.await.unwrap();
        assert!(
            server_result.is_err(),
            "Server must fail closed on verify token mismatch"
        );
        match server_result {
            Err(ProxyError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("verification token mismatch"));
            }
            Ok(_) => panic!("Expected AuthenticationFailed error, got Ok"),
            Err(e) => panic!("Expected AuthenticationFailed error, got other error: {e}"),
        }
    }

    #[tokio::test]
    async fn test_modern_configuration_transition_flow() {
        // Mock backend listener on a random local port
        let mock_backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_port = mock_backend.local_addr().unwrap().port();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        let mut config = ProxyConfig {
            online_mode: false,
            ..Default::default()
        };
        config.servers.insert(
            "lobby".to_string(),
            crate::config::BackendConfig {
                address: "127.0.0.1".to_string(),
                port: backend_port,
                forwarding_mode: crate::config::ForwardingMode::None,
                forwarding_secret: None,
            },
        );

        let rsa = Arc::new(RsaKeyManager::new().unwrap());
        let http_client = reqwest::Client::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Spawn mock backend task
        let backend_task = tokio::spawn(async move {
            let (mut stream, _) = mock_backend.accept().await.unwrap();
            // 1. Read Handshake from proxy
            let hs = read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            assert_eq!(hs.id, 0x00);
            // 2. Read LoginStart from proxy
            let ls = read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            assert_eq!(ls.id, 0x00);
            // 3. Backend sends LoginSuccess (0x02)
            let success = encode_login_success(Uuid::nil(), "Steve", &[], 765);
            write_packet(&mut stream, &success).await.unwrap();
            // 4. Backend reads LoginAcknowledged (0x03)
            let ack = read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            assert_eq!(ack.id, 0x03);
            // 5. In Configuration state, backend sends FinishConfiguration (0x02)
            let finish = RawPacket::new(0x02, bytes::Bytes::new());
            write_packet(&mut stream, &finish).await.unwrap();
            // 6. Backend reads client's FinishConfiguration (0x02 for 765)
            let client_finish = read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            assert_eq!(client_finish.id, 0x02);
            // Reached Play state cleanly
        });

        let script_host = Arc::new(ScriptHost::new());
        let config = Arc::new(config);
        let server_task = tokio::spawn(async move {
            let (stream, client_addr) = listener.accept().await.unwrap();
            process_connection(
                stream,
                client_addr,
                config,
                &rsa,
                &http_client,
                script_host,
                shutdown_rx,
            )
            .await
        });

        let mut client_stream = TcpStream::connect(server_addr).await.unwrap();

        // 1. Client sends Handshake (protocol 765, Login state)
        let handshake = HandshakePacket::new(765, "localhost", 25565, ConnectionState::Login);
        write_packet(&mut client_stream, &handshake.encode())
            .await
            .unwrap();

        // 2. Client sends LoginStart
        let login_start = LoginStartPacket::new("Steve", Uuid::nil());
        write_packet(&mut client_stream, &login_start.encode())
            .await
            .unwrap();

        // 3. Client receives LoginSuccess
        let success_raw = read_packet(&mut client_stream, DEFAULT_MAX_PACKET_SIZE)
            .await
            .unwrap();
        assert_eq!(success_raw.id, 0x02);
        let success = LoginSuccessPacket::decode_with_version(&success_raw, 765).unwrap();
        assert_eq!(success.username, "Steve");
        assert!(success.properties.is_empty());

        // 4. Client sends LoginAcknowledged (0x03)
        let ack = LoginAcknowledgedPacket::new();
        write_packet(&mut client_stream, &ack.encode())
            .await
            .unwrap();

        // 5. Client receives FinishConfiguration (0x02) forwarded from backend
        let finish_raw = read_packet(&mut client_stream, DEFAULT_MAX_PACKET_SIZE)
            .await
            .unwrap();
        assert_eq!(finish_raw.id, 0x02);

        // 6. Client responds with FinishConfiguration (0x02 for 765)
        write_packet(
            &mut client_stream,
            &RawPacket::new(0x02, bytes::Bytes::new()),
        )
        .await
        .unwrap();

        // 7. Drop client stream to end Play bridge cleanly
        drop(client_stream);
        backend_task.await.unwrap();
        let _ = server_task.await.unwrap();
        let _ = shutdown_tx.send(true);
    }

    #[tokio::test]
    async fn test_compressed_backend_synchronization_flow() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();

        let mock_backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_port = mock_backend.local_addr().unwrap().port();

        let mut config = ProxyConfig {
            online_mode: false,
            ..Default::default()
        };
        config.servers.insert(
            "lobby".to_string(),
            crate::config::BackendConfig {
                address: "127.0.0.1".to_string(),
                port: backend_port,
                forwarding_mode: crate::config::ForwardingMode::None,
                forwarding_secret: None,
            },
        );

        let rsa = Arc::new(RsaKeyManager::new().unwrap());
        let http_client = reqwest::Client::new();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Spawn mock backend that enables compression threshold 256
        let backend_task = tokio::spawn(async move {
            let (mut stream, _) = mock_backend.accept().await.unwrap();
            // 1. Read Handshake from proxy
            let hs = read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            assert_eq!(hs.id, 0x00);
            // 2. Read LoginStart from proxy
            let ls = read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            assert_eq!(ls.id, 0x00);

            // 3. Backend sends SetCompression (0x03) with threshold 256
            let mut thresh_payload = bytes::BytesMut::new();
            crate::protocol::varint::encode_varint(256, &mut thresh_payload);
            let set_comp = RawPacket::new(0x03, thresh_payload.freeze());
            write_packet(&mut stream, &set_comp).await.unwrap();

            // 4. Backend sends LoginSuccess (0x02) compressed
            let success = encode_login_success(Uuid::nil(), "Steve", &[], 765);
            write_packet_with_compression(&mut stream, &success, Some(256))
                .await
                .unwrap();

            // 5. Backend reads LoginAcknowledged (0x03) compressed
            let ack = read_packet_with_compression(&mut stream, DEFAULT_MAX_PACKET_SIZE, Some(256))
                .await
                .unwrap();
            assert_eq!(ack.id, 0x03);

            // 6. Backend sends FinishConfiguration (0x02) compressed
            let finish = RawPacket::new(0x02, bytes::Bytes::new());
            write_packet_with_compression(&mut stream, &finish, Some(256))
                .await
                .unwrap();

            // 7. Backend reads client's FinishConfiguration compressed
            let client_finish =
                read_packet_with_compression(&mut stream, DEFAULT_MAX_PACKET_SIZE, Some(256))
                    .await
                    .unwrap();
            assert_eq!(client_finish.id, 0x02);
        });

        let script_host = Arc::new(ScriptHost::new());
        let config = Arc::new(config);
        let server_task = tokio::spawn(async move {
            let (stream, client_addr) = listener.accept().await.unwrap();
            process_connection(
                stream,
                client_addr,
                config,
                &rsa,
                &http_client,
                script_host,
                shutdown_rx,
            )
            .await
        });

        let mut client_stream = TcpStream::connect(server_addr).await.unwrap();

        // 1. Client sends Handshake
        let handshake = HandshakePacket::new(765, "localhost", 25565, ConnectionState::Login);
        write_packet(&mut client_stream, &handshake.encode())
            .await
            .unwrap();

        // 2. Client sends LoginStart
        let login_start = LoginStartPacket::new("Steve", Uuid::nil());
        write_packet(&mut client_stream, &login_start.encode())
            .await
            .unwrap();

        // 3. Client receives SetCompression (0x03) uncompressed
        let comp_raw = read_packet(&mut client_stream, DEFAULT_MAX_PACKET_SIZE)
            .await
            .unwrap();
        assert_eq!(comp_raw.id, 0x03);
        let mut cursor = &comp_raw.payload[..];
        let threshold = decode_varint(&mut cursor).unwrap() as usize;
        assert_eq!(threshold, 256);

        // 4. Client receives LoginSuccess (0x02) compressed
        let success_raw =
            read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, Some(256))
                .await
                .unwrap();
        assert_eq!(success_raw.id, 0x02);
        let success = LoginSuccessPacket::decode_with_version(&success_raw, 765).unwrap();
        assert_eq!(success.username, "Steve");

        // 5. Client sends LoginAcknowledged (0x03) compressed
        let ack = LoginAcknowledgedPacket::new();
        write_packet_with_compression(&mut client_stream, &ack.encode(), Some(256))
            .await
            .unwrap();

        // 6. Client receives FinishConfiguration (0x02) compressed
        let finish_raw =
            read_packet_with_compression(&mut client_stream, DEFAULT_MAX_PACKET_SIZE, Some(256))
                .await
                .unwrap();
        assert_eq!(finish_raw.id, 0x02);

        // 7. Client sends FinishConfiguration (0x02) compressed
        write_packet_with_compression(
            &mut client_stream,
            &RawPacket::new(0x02, bytes::Bytes::new()),
            Some(256),
        )
        .await
        .unwrap();

        drop(client_stream);
        backend_task.await.unwrap();
        let _ = server_task.await.unwrap();
        let _ = shutdown_tx.send(true);
    }
}
