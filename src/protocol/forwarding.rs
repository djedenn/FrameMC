use bytes::{Buf, BufMut, Bytes, BytesMut};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::config::{BackendConfig, ForwardingMode};
use crate::crypto::PlayerProfile;
use crate::error::ProxyError;
use crate::network::codec::{read_packet, write_packet, DEFAULT_MAX_PACKET_SIZE};
use crate::protocol::handshake::{ConnectionState, HandshakePacket};
use crate::protocol::login::LoginStartPacket;
use crate::protocol::packet::RawPacket;
use crate::protocol::varint::{decode_varint, encode_varint};

/// Standard plugin channel for Velocity modern player information forwarding.
pub const VELOCITY_FORWARDING_CHANNEL: &str = "velocity:player_info";

/// Modern forwarding protocol version 4.
pub const VELOCITY_FORWARDING_VERSION: i32 = 4;

/// Backend login plugin request packet (Packet ID: `0x04` in Login state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPluginRequestPacket {
    pub message_id: i32,
    pub channel: String,
    pub data: Bytes,
}

impl LoginPluginRequestPacket {
    pub fn new(message_id: i32, channel: impl Into<String>, data: Bytes) -> Self {
        Self {
            message_id,
            channel: channel.into(),
            data,
        }
    }

    /// Encodes a `LoginPluginRequestPacket` into a `RawPacket` (ID: `0x04`).
    pub fn encode(&self) -> RawPacket {
        let mut payload = BytesMut::new();
        encode_varint(self.message_id, &mut payload);
        encode_varint(self.channel.len() as i32, &mut payload);
        payload.put_slice(self.channel.as_bytes());
        payload.put_slice(&self.data);
        RawPacket::new(0x04, payload.freeze())
    }

    /// Decodes a `LoginPluginRequestPacket` from a `RawPacket`.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x04 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];

        let message_id = decode_varint(&mut cursor)?;
        let channel_len = decode_varint(&mut cursor)?;
        if channel_len < 0 || cursor.remaining() < channel_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading channel in LoginPluginRequest",
            )));
        }
        let channel = std::str::from_utf8(&cursor[..channel_len as usize])
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in channel",
                ))
            })?
            .to_string();
        cursor.advance(channel_len as usize);

        let data = Bytes::copy_from_slice(cursor);

        Ok(Self {
            message_id,
            channel,
            data,
        })
    }
}

/// Proxy login plugin response packet (Packet ID: `0x02` in Login state, Serverbound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPluginResponsePacket {
    pub message_id: i32,
    pub successful: bool,
    pub data: Bytes,
}

impl LoginPluginResponsePacket {
    pub fn new(message_id: i32, successful: bool, data: Bytes) -> Self {
        Self {
            message_id,
            successful,
            data,
        }
    }

    /// Encodes a `LoginPluginResponsePacket` into a `RawPacket` (ID: `0x02`).
    pub fn encode(&self) -> RawPacket {
        let mut payload = BytesMut::new();
        encode_varint(self.message_id, &mut payload);
        payload.put_u8(if self.successful { 1 } else { 0 });
        if self.successful {
            payload.put_slice(&self.data);
        }
        RawPacket::new(0x02, payload.freeze())
    }

    /// Decodes a `LoginPluginResponsePacket` from a `RawPacket`.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x02 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];

        let message_id = decode_varint(&mut cursor)?;
        if !cursor.has_remaining() {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading successful flag in LoginPluginResponse",
            )));
        }
        let successful = cursor.get_u8() != 0;
        let data = if successful && cursor.has_remaining() {
            Bytes::copy_from_slice(cursor)
        } else {
            Bytes::new()
        };

        Ok(Self {
            message_id,
            successful,
            data,
        })
    }
}

/// Constructs the raw Velocity modern forwarding payload buffer.
///
/// Layout:
/// - Version: VarInt = 4
/// - Remote Address: String (client IP)
/// - Player UUID: 16 raw bytes
/// - Username: String
/// - Properties Array: VarInt count + entries (Name, Value, HasSignature, Signature)
pub fn create_velocity_forwarding_payload(profile: &PlayerProfile, client_ip: &str) -> Bytes {
    let mut buf = BytesMut::new();

    // 1. Forwarding Version (VarInt = 4)
    encode_varint(VELOCITY_FORWARDING_VERSION, &mut buf);

    // 2. Remote Address (String)
    encode_varint(client_ip.len() as i32, &mut buf);
    buf.put_slice(client_ip.as_bytes());

    // 3. Player UUID (16 raw bytes)
    buf.put_slice(profile.id.as_bytes());

    // 4. Player Name (String)
    encode_varint(profile.name.len() as i32, &mut buf);
    buf.put_slice(profile.name.as_bytes());

    // 5. Properties Array
    encode_varint(profile.properties.len() as i32, &mut buf);
    for prop in &profile.properties {
        // Name
        encode_varint(prop.name.len() as i32, &mut buf);
        buf.put_slice(prop.name.as_bytes());

        // Value
        encode_varint(prop.value.len() as i32, &mut buf);
        buf.put_slice(prop.value.as_bytes());

        // Has Signature boolean & optional signature string
        if let Some(signature) = &prop.signature {
            buf.put_u8(1);
            encode_varint(signature.len() as i32, &mut buf);
            buf.put_slice(signature.as_bytes());
        } else {
            buf.put_u8(0);
        }
    }

    buf.freeze()
}

/// Computes the 32-byte HMAC-SHA256 signature for a Velocity forwarding payload.
pub fn calculate_velocity_hmac(secret: &[u8], payload: &[u8]) -> Result<[u8; 32], ProxyError> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|e| ProxyError::CryptoError(format!("Invalid HMAC-SHA256 key: {e}")))?;
    mac.update(payload);
    let result = mac.finalize();
    let bytes: [u8; 32] = result.into_bytes().into();
    Ok(bytes)
}

/// Constructs the complete Velocity modern forwarding data payload (`[32-byte HMAC] + [Payload]`).
pub fn create_velocity_response_data(
    secret: &str,
    profile: &PlayerProfile,
    client_ip: &str,
) -> Result<Bytes, ProxyError> {
    let payload = create_velocity_forwarding_payload(profile, client_ip);
    let hmac_sig = calculate_velocity_hmac(secret.as_bytes(), &payload)?;

    let mut data = BytesMut::with_capacity(32 + payload.len());
    data.put_slice(&hmac_sig);
    data.put_slice(&payload);
    Ok(data.freeze())
}

/// Constructs the legacy BungeeCord handshake server address string:
/// `{target_host}\0{client_ip}\0{uuid_without_hyphens}\0{properties_json}`
pub fn create_bungeecord_handshake_address(
    target_host: &str,
    client_ip: &str,
    profile: &PlayerProfile,
) -> Result<String, ProxyError> {
    let uuid_without_hyphens = profile.id.simple().to_string();
    let properties_json = serde_json::to_string(&profile.properties).map_err(|e| {
        ProxyError::ConfigError(format!(
            "Failed to serialize profile properties for BungeeCord forwarding: {e}"
        ))
    })?;

    Ok(format!(
        "{}\0{}\0{}\0{}",
        target_host, client_ip, uuid_without_hyphens, properties_json
    ))
}

/// Dispatches initial Handshake and Login packets and performs forwarding negotiation.
pub async fn dispatch_forwarding<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    backend: &BackendConfig,
    profile: &PlayerProfile,
    client_ip: &str,
    protocol_version: i32,
) -> Result<(), ProxyError> {
    // 1. Construct Handshake server_address according to forwarding mode
    let handshake_addr = match backend.forwarding_mode {
        ForwardingMode::LegacyBungee => {
            create_bungeecord_handshake_address(&backend.address, client_ip, profile)?
        }
        ForwardingMode::VelocityModern | ForwardingMode::None => backend.address.clone(),
    };

    let handshake = HandshakePacket::new(
        protocol_version,
        handshake_addr,
        backend.port,
        ConnectionState::Login,
    );
    write_packet(stream, &handshake.encode()).await?;

    // 2. Dispatch LoginStart packet
    let login_start = LoginStartPacket::new(&profile.name, profile.id);
    write_packet(stream, &login_start.encode()).await?;

    // 3. Handle Velocity Modern Forwarding if configured
    if backend.forwarding_mode == ForwardingMode::VelocityModern {
        let secret = backend.forwarding_secret.as_deref().ok_or_else(|| {
            ProxyError::ConfigError(format!(
                "Backend server at {}:{} requires 'forwarding_secret' for velocity_modern forwarding",
                backend.address, backend.port
            ))
        })?;

        // Read packet from backend with 10s timeout
        let incoming = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            read_packet(stream, DEFAULT_MAX_PACKET_SIZE),
        )
        .await
        .map_err(|_| {
            ProxyError::BackendConnectionFailed(
                "Timeout waiting for backend forwarding response".to_string(),
            )
        })??;
        if incoming.id == 0x04 {
            // LoginPluginRequest
            let req = LoginPluginRequestPacket::decode(&incoming)?;
            if req.channel == VELOCITY_FORWARDING_CHANNEL {
                let forwarding_data = create_velocity_response_data(secret, profile, client_ip)?;
                let resp = LoginPluginResponsePacket::new(req.message_id, true, forwarding_data);
                write_packet(stream, &resp.encode()).await?;
            } else {
                return Err(ProxyError::AuthenticationFailed(format!(
                    "Unexpected LoginPluginRequest channel: '{}', expected '{}'",
                    req.channel, VELOCITY_FORWARDING_CHANNEL
                )));
            }
        } else if incoming.id == 0x01 {
            tracing::warn!(
                "Downstream backend server has online-mode=true enabled! Set online-mode=false in the backend server configuration."
            );
            return Err(ProxyError::AuthenticationFailed(
                "Downstream backend server has online-mode=true enabled! Set online-mode=false in the backend server configuration.".to_string(),
            ));
        } else if incoming.id == 0x00 {
            // Disconnect packet from backend
            let reason = String::from_utf8_lossy(&incoming.payload).to_string();
            return Err(ProxyError::AuthenticationFailed(format!(
                "Backend rejected connection during login: {reason}"
            )));
        } else {
            return Err(ProxyError::AuthenticationFailed(format!(
                "Expected LoginPluginRequest (0x04) from backend, got packet 0x{:02X}",
                incoming.id
            )));
        }
    }

    Ok(())
}

/// Connects to a downstream backend server and negotiates player information forwarding.
pub async fn connect_and_forward(
    backend: &BackendConfig,
    profile: &PlayerProfile,
    client_ip: &str,
    protocol_version: i32,
) -> Result<TcpStream, ProxyError> {
    let target = format!("{}:{}", backend.address, backend.port);
    let stream_res = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(&target),
    )
    .await
    .map_err(|_| {
        ProxyError::BackendConnectionFailed(format!("Connection to backend {target} timed out"))
    })?;
    let mut stream = stream_res.map_err(|e| {
        ProxyError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to connect to backend server at {target}: {e}"),
        ))
    })?;

    if let Err(e) = stream.set_nodelay(true) {
        tracing::warn!(target = %target, "Failed to set TCP_NODELAY on backend connection: {e}");
    }

    dispatch_forwarding(&mut stream, backend, profile, client_ip, protocol_version).await?;

    Ok(stream)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::crypto::ProfileProperty;
    use uuid::Uuid;

    fn sample_profile() -> PlayerProfile {
        PlayerProfile {
            id: Uuid::parse_str("069a79f4-44e3-4726-a9be-254cc4d37b01").unwrap(),
            name: "Steve".to_string(),
            properties: vec![ProfileProperty {
                name: "textures".to_string(),
                value: "sample_base64_texture".to_string(),
                signature: Some("sample_sig".to_string()),
            }],
        }
    }

    #[test]
    fn test_velocity_hmac_and_payload_layout() {
        let profile = sample_profile();
        let client_ip = "192.168.1.50";
        let secret = "my_secure_shared_forwarding_secret";

        let payload = create_velocity_forwarding_payload(&profile, client_ip);

        // Verify payload layout:
        let mut cursor = &payload[..];

        // 1. Version (VarInt = 4)
        let version = decode_varint(&mut cursor).unwrap();
        assert_eq!(version, 4);

        // 2. Remote IP (String)
        let ip_len = decode_varint(&mut cursor).unwrap();
        assert_eq!(ip_len, client_ip.len() as i32);
        let ip_str = std::str::from_utf8(&cursor[..ip_len as usize]).unwrap();
        assert_eq!(ip_str, client_ip);
        cursor.advance(ip_len as usize);

        // 3. UUID (16 raw bytes)
        let mut uuid_bytes = [0u8; 16];
        cursor.copy_to_slice(&mut uuid_bytes);
        assert_eq!(Uuid::from_bytes(uuid_bytes), profile.id);

        // 4. Username (String)
        let name_len = decode_varint(&mut cursor).unwrap();
        assert_eq!(name_len, profile.name.len() as i32);
        let name_str = std::str::from_utf8(&cursor[..name_len as usize]).unwrap();
        assert_eq!(name_str, "Steve");
        cursor.advance(name_len as usize);

        // 5. Properties Array
        let prop_count = decode_varint(&mut cursor).unwrap();
        assert_eq!(prop_count, 1);

        let p_name_len = decode_varint(&mut cursor).unwrap();
        let p_name = std::str::from_utf8(&cursor[..p_name_len as usize]).unwrap();
        assert_eq!(p_name, "textures");
        cursor.advance(p_name_len as usize);

        let p_val_len = decode_varint(&mut cursor).unwrap();
        let p_val = std::str::from_utf8(&cursor[..p_val_len as usize]).unwrap();
        assert_eq!(p_val, "sample_base64_texture");
        cursor.advance(p_val_len as usize);

        assert_eq!(cursor.get_u8(), 1); // has signature
        let p_sig_len = decode_varint(&mut cursor).unwrap();
        let p_sig = std::str::from_utf8(&cursor[..p_sig_len as usize]).unwrap();
        assert_eq!(p_sig, "sample_sig");
        cursor.advance(p_sig_len as usize);

        assert_eq!(cursor.remaining(), 0);

        // Verify full HMAC response data
        let response_data = create_velocity_response_data(secret, &profile, client_ip).unwrap();
        assert_eq!(response_data.len(), 32 + payload.len());

        let sig = &response_data[..32];
        let attached_payload = &response_data[32..];
        assert_eq!(attached_payload, &payload[..]);

        let expected_hmac = calculate_velocity_hmac(secret.as_bytes(), &payload).unwrap();
        assert_eq!(sig, &expected_hmac[..]);
    }

    #[test]
    fn test_bungeecord_handshake_formatting() {
        let profile = sample_profile();
        let client_ip = "10.0.0.42";
        let target_host = "lobby.internal";

        let address =
            create_bungeecord_handshake_address(target_host, client_ip, &profile).unwrap();

        let parts: Vec<&str> = address.split('\0').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "lobby.internal");
        assert_eq!(parts[1], "10.0.0.42");
        assert_eq!(parts[2], "069a79f444e34726a9be254cc4d37b01"); // No hyphens

        let deserialized_props: Vec<ProfileProperty> = serde_json::from_str(parts[3]).unwrap();
        assert_eq!(deserialized_props, profile.properties);
    }

    #[test]
    fn test_login_plugin_request_response_codecs() {
        let req = LoginPluginRequestPacket::new(
            42,
            "velocity:player_info",
            Bytes::from_static(b"test_query"),
        );
        let raw = req.encode();
        assert_eq!(raw.id, 0x04);

        let decoded = LoginPluginRequestPacket::decode(&raw).unwrap();
        assert_eq!(decoded.message_id, 42);
        assert_eq!(decoded.channel, "velocity:player_info");
        assert_eq!(&decoded.data[..], b"test_query");

        let resp =
            LoginPluginResponsePacket::new(42, true, Bytes::from_static(b"response_payload"));
        let raw_resp = resp.encode();
        assert_eq!(raw_resp.id, 0x02);

        let decoded_resp = LoginPluginResponsePacket::decode(&raw_resp).unwrap();
        assert_eq!(decoded_resp.message_id, 42);
        assert!(decoded_resp.successful);
        assert_eq!(&decoded_resp.data[..], b"response_payload");
    }

    #[tokio::test]
    async fn test_dispatch_forwarding_velocity_flow() {
        let (mut client_io, mut server_io) = tokio::io::duplex(4096);
        let profile = sample_profile();
        let secret = "velocity_secret_key";

        let backend = BackendConfig {
            address: "127.0.0.1".to_string(),
            port: 25566,
            forwarding_mode: ForwardingMode::VelocityModern,
            forwarding_secret: Some(secret.to_string()),
        };

        // Backend mock task
        let backend_task = tokio::spawn(async move {
            // 1. Read Handshake
            let hs_raw = read_packet(&mut server_io, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let hs = HandshakePacket::decode(&hs_raw).unwrap();
            assert_eq!(hs.server_address, "127.0.0.1");
            assert_eq!(hs.protocol_version, 765);

            // 2. Read LoginStart
            let ls_raw = read_packet(&mut server_io, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let ls = LoginStartPacket::decode(&ls_raw).unwrap();
            assert_eq!(ls.username, "Steve");

            // 3. Backend sends LoginPluginRequest (0x04)
            let req = LoginPluginRequestPacket::new(101, VELOCITY_FORWARDING_CHANNEL, Bytes::new());
            write_packet(&mut server_io, &req.encode()).await.unwrap();

            // 4. Backend reads LoginPluginResponse (0x02)
            let resp_raw = read_packet(&mut server_io, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let resp = LoginPluginResponsePacket::decode(&resp_raw).unwrap();
            assert_eq!(resp.message_id, 101);
            assert!(resp.successful);

            // Verify signature
            let sig = &resp.data[..32];
            let payload = &resp.data[32..];
            let expected_hmac = calculate_velocity_hmac(secret.as_bytes(), payload).unwrap();
            assert_eq!(sig, &expected_hmac[..]);
        });

        // Proxy dispatch
        dispatch_forwarding(&mut client_io, &backend, &profile, "127.0.0.1", 765)
            .await
            .unwrap();

        backend_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_dispatch_forwarding_bungeecord_flow() {
        let (mut client_io, mut server_io) = tokio::io::duplex(4096);
        let profile = sample_profile();

        let backend = BackendConfig {
            address: "127.0.0.1".to_string(),
            port: 25567,
            forwarding_mode: ForwardingMode::LegacyBungee,
            forwarding_secret: None,
        };

        let backend_task = tokio::spawn(async move {
            // 1. Read Handshake
            let hs_raw = read_packet(&mut server_io, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let hs = HandshakePacket::decode(&hs_raw).unwrap();
            assert!(hs.server_address.contains('\0'));
            let parts: Vec<&str> = hs.server_address.split('\0').collect();
            assert_eq!(parts[0], "127.0.0.1");
            assert_eq!(parts[1], "192.168.1.99");
            assert_eq!(parts[2], "069a79f444e34726a9be254cc4d37b01");

            // 2. Read LoginStart
            let ls_raw = read_packet(&mut server_io, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let ls = LoginStartPacket::decode(&ls_raw).unwrap();
            assert_eq!(ls.username, "Steve");
        });

        dispatch_forwarding(&mut client_io, &backend, &profile, "192.168.1.99", 765)
            .await
            .unwrap();

        backend_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_velocity_missing_secret_error() {
        let (mut client_io, _server_io) = tokio::io::duplex(4096);
        let profile = sample_profile();

        let backend = BackendConfig {
            address: "127.0.0.1".to_string(),
            port: 25568,
            forwarding_mode: ForwardingMode::VelocityModern,
            forwarding_secret: None, // Missing secret
        };

        let result =
            dispatch_forwarding(&mut client_io, &backend, &profile, "127.0.0.1", 765).await;
        assert!(result.is_err());
        match result {
            Err(ProxyError::ConfigError(msg)) => {
                assert!(msg.contains("requires 'forwarding_secret'"));
            }
            other => panic!("Expected ConfigError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_connect_and_forward_tcp() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let profile = sample_profile();

        let backend = BackendConfig {
            address: "127.0.0.1".to_string(),
            port,
            forwarding_mode: ForwardingMode::None,
            forwarding_secret: None,
        };

        let server_task = tokio::spawn(async move {
            let (mut server_stream, _) = listener.accept().await.unwrap();
            // Read Handshake
            let hs_raw = read_packet(&mut server_stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let hs = HandshakePacket::decode(&hs_raw).unwrap();
            assert_eq!(hs.server_address, "127.0.0.1");

            // Read LoginStart
            let ls_raw = read_packet(&mut server_stream, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let ls = LoginStartPacket::decode(&ls_raw).unwrap();
            assert_eq!(ls.username, "Steve");
        });

        let client_stream = connect_and_forward(&backend, &profile, "127.0.0.1", 765)
            .await
            .unwrap();
        drop(client_stream);

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_velocity_backend_online_mode_true_warning_error() {
        let (mut client_io, mut server_io) = tokio::io::duplex(4096);
        let profile = sample_profile();

        let backend = BackendConfig {
            address: "127.0.0.1".to_string(),
            port: 25567,
            forwarding_mode: ForwardingMode::VelocityModern,
            forwarding_secret: Some("test_secret".to_string()),
        };

        let backend_task = tokio::spawn(async move {
            // Read Handshake and LoginStart
            let _ = read_packet(&mut server_io, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            let _ = read_packet(&mut server_io, DEFAULT_MAX_PACKET_SIZE)
                .await
                .unwrap();
            // Backend sends packet 0x01 (EncryptionRequest) because online-mode=true
            write_packet(
                &mut server_io,
                &RawPacket::new(0x01, bytes::Bytes::from_static(b"mock_enc_req")),
            )
            .await
            .unwrap();
        });

        let result =
            dispatch_forwarding(&mut client_io, &backend, &profile, "127.0.0.1", 765).await;
        assert!(result.is_err());
        match result {
            Err(ProxyError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("online-mode=true enabled"));
            }
            other => panic!("Expected AuthenticationFailed, got {:?}", other),
        }

        backend_task.await.unwrap();
    }
}
