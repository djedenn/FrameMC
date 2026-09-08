use bytes::{BufMut, BytesMut};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::config::ProxyConfig;
use crate::error::ProxyError;
use crate::network::codec::{read_packet, write_packet, DEFAULT_MAX_PACKET_SIZE};
use crate::protocol::packet::RawPacket;
use crate::protocol::varint::encode_varint;

/// Minecraft version information displayed in the server list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub name: String,
    pub protocol: i32,
}

/// Player sample entry showing an individual player's name and UUID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSample {
    pub name: String,
    pub id: String,
}

/// Player count and sample information in the status response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayersInfo {
    pub max: i32,
    pub online: i32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample: Vec<PlayerSample>,
}

/// Chat component description (MOTD) for the status response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatDescription {
    pub text: String,
}

impl ChatDescription {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Official modern Minecraft Status JSON response payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub version: VersionInfo,
    pub players: PlayersInfo,
    pub description: ChatDescription,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    #[serde(rename = "enforcesSecureChat")]
    pub enforces_secure_chat: bool,
}

/// Handles the full Server List Ping lifecycle for a client in the `Status` state.
///
/// 1. Reads incoming `StatusRequest` (`id = 0x00`).
/// 2. Serializes `StatusResponse` into JSON and responds with `id = 0x00`.
/// 3. Reads optional incoming `PingRequest` (`id = 0x01`).
/// 4. Immediately echoes back `PongResponse` (`id = 0x01`) with the identical 64-bit payload.
pub async fn handle_status<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    config: &ProxyConfig,
) -> Result<(), ProxyError> {
    handle_status_with_version(stream, config, 765).await
}

/// Handles the Server List Ping lifecycle with an explicit protocol version.
pub async fn handle_status_with_version<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    config: &ProxyConfig,
    protocol_version: i32,
) -> Result<(), ProxyError> {
    // 1. Read first packet: expect StatusRequest (id = 0x00) with 10s timeout
    let request_packet = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_packet(stream, DEFAULT_MAX_PACKET_SIZE),
    )
    .await
    .map_err(|_| {
        ProxyError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Status request timed out",
        ))
    })??;
    if request_packet.id != 0x00 {
        return Err(ProxyError::InvalidPacketId(request_packet.id));
    }

    // 2. Build StatusResponse using pre-cached favicon Data URI
    let status_response = StatusResponse {
        version: VersionInfo {
            name: "FrameMC".to_string(),
            protocol: protocol_version,
        },
        players: PlayersInfo {
            max: config.max_players,
            online: 0,
            sample: Vec::new(),
        },
        description: ChatDescription::new(config.motd.clone()),
        favicon: config.favicon.clone(),
        enforces_secure_chat: false,
    };

    let json_str = serde_json::to_string(&status_response).map_err(|e| {
        ProxyError::ConfigError(format!("Failed to serialize status response: {e}"))
    })?;

    // Encode JSON string as a VarInt-prefixed Minecraft String
    let mut response_payload = BytesMut::with_capacity(5 + json_str.len());
    encode_varint(json_str.len() as i32, &mut response_payload);
    response_payload.put_slice(json_str.as_bytes());

    write_packet(stream, &RawPacket::new(0x00, response_payload.freeze())).await?;

    // 3. Read subsequent packet: PingRequest (id = 0x01) or graceful disconnect with 10s timeout
    let ping_read = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_packet(stream, DEFAULT_MAX_PACKET_SIZE),
    )
    .await;

    match ping_read {
        Ok(Ok(ping_packet)) => {
            if ping_packet.id != 0x01 {
                return Err(ProxyError::InvalidPacketId(ping_packet.id));
            }
            if ping_packet.payload.len() < 8 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "ping packet payload must contain 8 bytes (i64)",
                )));
            }
            // 4. Echo back identical 64-bit payload
            write_packet(stream, &RawPacket::new(0x01, ping_packet.payload)).await?;
            Ok(())
        }
        Ok(Err(ProxyError::Io(ref e))) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            // Client closed stream after receiving status response without pinging
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // Optional ping timed out; client received status response and left stream idle
            Ok(())
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::varint::decode_varint;
    use bytes::Bytes;
    use std::collections::HashMap;

    #[test]
    fn test_status_json_schema() {
        let response = StatusResponse {
            version: VersionInfo {
                name: "1.20.4".to_string(),
                protocol: 765,
            },
            players: PlayersInfo {
                max: 100,
                online: 5,
                sample: vec![PlayerSample {
                    name: "Player1".to_string(),
                    id: "069a79f4-44e3-4726-a9be-254cc4d37b01".to_string(),
                }],
            },
            description: ChatDescription::new("§aA Minecraft Server"),
            favicon: Some("data:image/png;base64,samplefavicon".to_string()),
            enforces_secure_chat: false,
        };

        let json = serde_json::to_string(&response).expect("Failed to serialize StatusResponse");

        // Verify required keys
        assert!(json.contains("\"version\":{\"name\":\"1.20.4\",\"protocol\":765}"));
        assert!(json.contains("\"players\":{"));
        assert!(json.contains("\"max\":100"));
        assert!(json.contains("\"online\":5"));
        assert!(json.contains(
            "\"sample\":[{\"name\":\"Player1\",\"id\":\"069a79f4-44e3-4726-a9be-254cc4d37b01\"}]"
        ));
        assert!(json.contains("\"description\":{\"text\":\"§aA Minecraft Server\"}"));
        assert!(json.contains("\"favicon\":\"data:image/png;base64,samplefavicon\""));
        assert!(json.contains("\"enforcesSecureChat\":false"));

        // Roundtrip deserialization check
        let deserialized: StatusResponse =
            serde_json::from_str(&json).expect("Failed to deserialize StatusResponse");
        assert_eq!(response, deserialized);
    }

    #[tokio::test]
    async fn test_ping_pong_timestamp_symmetry() {
        let config = ProxyConfig {
            bind_address: "0.0.0.0".to_string(),
            bind_port: 25565,
            motd: "§bFrameMC Test MOTD".to_string(),
            max_players: 500,
            online_mode: true,
            favicon: None,
            session_server_url: None,
            servers: HashMap::new(),
            default_server: "lobby".to_string(),
            script_path: "scripts/main.rhai".to_string(),
            plugins_dir: "plugins".to_string(),
        };

        let timestamp: i64 = 1718000000123;

        // Test handle_status using an in-memory duplex stream
        let (mut client, mut server) = tokio::io::duplex(4096);

        let server_handle = tokio::spawn(async move { handle_status(&mut server, &config).await });

        // Client sends StatusRequest
        write_packet(&mut client, &RawPacket::new(0x00, Bytes::new()))
            .await
            .expect("Failed to send StatusRequest");

        // Client reads StatusResponse
        let status_packet = read_packet(&mut client, DEFAULT_MAX_PACKET_SIZE)
            .await
            .expect("Failed to read StatusResponse");
        assert_eq!(status_packet.id, 0x00);

        // Decode JSON from StatusResponse payload
        let mut cursor = &status_packet.payload[..];
        let json_len = decode_varint(&mut cursor).expect("Failed to read json length");
        let json_bytes = &cursor[..json_len as usize];
        let json_str = std::str::from_utf8(json_bytes).expect("Invalid UTF-8 JSON");
        let parsed_status: StatusResponse =
            serde_json::from_str(json_str).expect("Failed to parse StatusResponse JSON");

        assert_eq!(parsed_status.description.text, "§bFrameMC Test MOTD");
        assert_eq!(parsed_status.players.max, 500);
        assert_eq!(parsed_status.players.online, 0);
        assert!(!parsed_status.enforces_secure_chat);

        // Client sends PingRequest with timestamp
        let ping_payload = Bytes::copy_from_slice(&timestamp.to_be_bytes());
        write_packet(&mut client, &RawPacket::new(0x01, ping_payload))
            .await
            .expect("Failed to send PingRequest");

        // Client reads PongResponse
        let pong_packet = read_packet(&mut client, DEFAULT_MAX_PACKET_SIZE)
            .await
            .expect("Failed to read PongResponse");
        assert_eq!(pong_packet.id, 0x01);
        assert_eq!(pong_packet.payload.len(), 8);

        // Assert timestamp symmetry
        let returned_timestamp = i64::from_be_bytes(pong_packet.payload[..8].try_into().unwrap());
        assert_eq!(
            returned_timestamp, timestamp,
            "Pong timestamp must match ping timestamp exactly"
        );

        // Await server completion
        server_handle
            .await
            .expect("Server task panicked")
            .expect("handle_status failed");
    }

    #[tokio::test]
    async fn test_client_disconnect_after_status_response() {
        let config = ProxyConfig::default();
        let (mut client, mut server) = tokio::io::duplex(4096);

        let server_handle = tokio::spawn(async move { handle_status(&mut server, &config).await });

        // Client sends StatusRequest
        write_packet(&mut client, &RawPacket::new(0x00, Bytes::new()))
            .await
            .unwrap();

        // Client reads StatusResponse
        let status_packet = read_packet(&mut client, DEFAULT_MAX_PACKET_SIZE)
            .await
            .unwrap();
        assert_eq!(status_packet.id, 0x00);

        // Client closes stream immediately without sending Ping
        drop(client);

        // Server should complete gracefully with Ok(())
        let res = server_handle.await.expect("Server panicked");
        assert!(res.is_ok(), "Expected Ok on client disconnect after status");
    }
}
