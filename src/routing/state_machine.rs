use bytes::{Buf, BufMut, BytesMut};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::config::{BackendConfig, ProxyConfig};
use crate::crypto::PlayerProfile;
use crate::error::ProxyError;
use crate::network::codec::{
    read_packet_with_compression, write_packet_with_compression, DisconnectPacket,
    DEFAULT_MAX_PACKET_SIZE,
};
use crate::network::listener::wait_for_shutdown;
use crate::protocol::configuration::{SessionRegistryCache, REGISTRY_DATA_PACKET_ID};
use crate::protocol::packet::{is_protected_plugin_channel, RawPacket};
use crate::protocol::varint::{decode_varint, encode_varint, varint_size};
use crate::script::engine::ScriptHost;
use crate::script::events::{PlayerCommandEvent, PlayerTabCompleteEvent};

/// Packet ID for server-bound chat command in Play state (1.19+ / 1.20+).
pub const SERVERBOUND_CHAT_COMMAND_PACKET_ID: i32 = 0x04;

/// Packet ID for server-bound chat message in Play state.
pub const SERVERBOUND_CHAT_MESSAGE_PACKET_ID: i32 = 0x05;

/// Packet ID for client-bound respawn packet in Play state (1.20.2 - 1.20.4).
pub const RESPAWN_PACKET_ID: i32 = 0x45;

/// Packet ID for client-bound system chat message in Play state (1.20.2 - 1.20.4).
pub const SYSTEM_CHAT_MESSAGE_PACKET_ID: i32 = 0x69;

/// Packet ID for client-bound close container packet in Play state (1.20.2 - 1.21.4).
pub const CLOSE_CONTAINER_PACKET_ID: i32 = 0x12;

/// Packet ID for client-bound stop sound packet in Play state (1.20.2).
pub const STOP_SOUND_PACKET_ID: i32 = 0x66;

/// Packet ID for client-bound boss bar packet in Play state (1.20.2 - 1.21.4).
pub const BOSS_BAR_PACKET_ID: i32 = 0x0A;

/// Packet ID for client-bound scoreboard objective packet in Play state (1.20.2).
pub const SCOREBOARD_OBJECTIVE_PACKET_ID: i32 = 0x5A;

/// Packet ID for client-bound display objective packet in Play state (1.20.2).
pub const DISPLAY_OBJECTIVE_PACKET_ID: i32 = 0x53;

/// Respawn `data_to_keep` flag: keep attributes.
pub const KEEP_ATTRIBUTES: u8 = 0x01;

/// Respawn `data_to_keep` flag: keep entity metadata.
pub const KEEP_METADATA: u8 = 0x02;

/// Respawn `data_to_keep` flag: keep both attributes and metadata (0x01 | 0x02).
pub const KEEP_ALL_DATA: u8 = KEEP_ATTRIBUTES | KEEP_METADATA;

/// Stream trait representing any bidirectional asynchronous stream.
pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> AsyncStream for T {}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type BackendConnectResult<'a> =
    BoxFuture<'a, Result<(Box<dyn AsyncStream>, Option<usize>), ProxyError>>;

/// Abstraction for initiating backend server connections.
pub trait BackendConnector: Send + Sync {
    fn connect<'a>(
        &'a self,
        backend_name: &'a str,
        backend_config: &'a BackendConfig,
        profile: &'a PlayerProfile,
        client_ip: &'a str,
        protocol_version: i32,
    ) -> BackendConnectResult<'a>;
}

/// Default TCP backend connector calling `connect_and_forward` and completing Login state.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpBackendConnector;

impl BackendConnector for TcpBackendConnector {
    fn connect<'a>(
        &'a self,
        _backend_name: &'a str,
        backend_config: &'a BackendConfig,
        profile: &'a PlayerProfile,
        client_ip: &'a str,
        protocol_version: i32,
    ) -> BoxFuture<'a, Result<(Box<dyn AsyncStream>, Option<usize>), ProxyError>> {
        Box::pin(async move {
            let mut stream = crate::protocol::forwarding::connect_and_forward(
                backend_config,
                profile,
                client_ip,
                protocol_version,
            )
            .await?;

            // Read initial response packet from backend (handling SetCompression or LoginSuccess)
            let first_pkt =
                crate::network::codec::read_packet(&mut stream, DEFAULT_MAX_PACKET_SIZE).await?;
            let (compression_threshold, login_pkt) = if first_pkt.id == 0x03 {
                // SetCompression (0x03)
                let mut cursor = &first_pkt.payload[..];
                let thresh = decode_varint(&mut cursor)?;
                if thresh < 0 {
                    return Err(ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Negative compression threshold from backend",
                    )));
                }
                let threshold = Some(thresh as usize);
                let next_pkt =
                    read_packet_with_compression(&mut stream, DEFAULT_MAX_PACKET_SIZE, threshold)
                        .await?;
                (threshold, next_pkt)
            } else {
                (None, first_pkt)
            };

            if login_pkt.id == 0x01 {
                return Err(ProxyError::AuthenticationFailed(
                    "Downstream backend server has online-mode or encryption enabled! Set online-mode=false in backend configuration.".to_string(),
                ));
            }
            if login_pkt.id == 0x00 {
                let reason = String::from_utf8_lossy(&login_pkt.payload).to_string();
                return Err(ProxyError::AuthenticationFailed(format!(
                    "Backend rejected login: {reason}"
                )));
            }
            if login_pkt.id != 0x02 {
                return Err(ProxyError::InvalidPacketId(login_pkt.id));
            }

            // Backend accepted login!
            // If modern protocol (>= 764), send LoginAcknowledged (0x03) to transition backend to Configuration state
            if protocol_version >= 764 {
                let ack = crate::protocol::login::LoginAcknowledgedPacket::new();
                write_packet_with_compression(&mut stream, &ack.encode(), compression_threshold)
                    .await?;
                tokio::io::AsyncWriteExt::flush(&mut stream).await?;
            }

            Ok((
                Box::new(stream) as Box<dyn AsyncStream>,
                compression_threshold,
            ))
        })
    }
}

/// Serverbound chat command packet issued by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChatCommand {
    pub command: String,
}

impl ServerboundChatCommand {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }

    pub fn is_command_packet(id: i32, protocol_version: i32) -> bool {
        if protocol_version >= 768 {
            id == 0x05 || id == 0x06 || id == 0x07
        } else if protocol_version >= 766 {
            id == 0x04 || id == 0x05 || id == 0x06
        } else {
            id == 0x04 || id == 0x05 || id == 0x06 || id == SERVERBOUND_CHAT_COMMAND_PACKET_ID
        }
    }

    /// Decodes a command from either ChatCommand or ChatMessage.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        Self::decode_with_version(packet, 765)
    }

    pub fn decode_with_version(
        packet: &RawPacket,
        protocol_version: i32,
    ) -> Result<Self, ProxyError> {
        if !Self::is_command_packet(packet.id, protocol_version) {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];
        let len = decode_varint(&mut cursor)?;
        if len < 0 || cursor.remaining() < len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading command string",
            )));
        }

        let cmd_bytes = &cursor[..len as usize];
        let command = std::str::from_utf8(cmd_bytes)
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in command",
                ))
            })?
            .to_string();

        Ok(Self { command })
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let packet_id = if protocol_version >= 768 {
            0x05
        } else if protocol_version >= 766 {
            0x04
        } else {
            SERVERBOUND_CHAT_COMMAND_PACKET_ID
        };
        let mut payload = BytesMut::new();
        encode_varint(self.command.len() as i32, &mut payload);
        payload.put_slice(self.command.as_bytes());
        RawPacket::new(packet_id, payload.freeze())
    }

    /// Returns the command guaranteed to start with a leading slash `/`.
    pub fn normalized_command(&self) -> String {
        if self.command.starts_with('/') {
            self.command.clone()
        } else {
            format!("/{}", self.command)
        }
    }
}

/// Server-bound Tab-Complete / Command Suggestion Request packet (`0x0D` / `0x0F` depending on version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabCompleteRequestPacket {
    pub transaction_id: i32,
    pub text: String,
}

impl TabCompleteRequestPacket {
    pub fn new(transaction_id: i32, text: impl Into<String>) -> Self {
        Self {
            transaction_id,
            text: text.into(),
        }
    }

    pub fn is_tab_complete_request(id: i32, protocol_version: i32) -> bool {
        if protocol_version >= 768 {
            id == 0x0D || id == 0x0F
        } else if protocol_version >= 766 {
            id == 0x0B
        } else if protocol_version >= 764 {
            id == 0x0A
        } else {
            id == 0x09 || id == 0x08 || id == 0x06 || id == 0x05
        }
    }

    pub fn decode(packet: &RawPacket, protocol_version: i32) -> Result<Self, ProxyError> {
        if !Self::is_tab_complete_request(packet.id, protocol_version) {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];
        let transaction_id = decode_varint(&mut cursor)?;
        let text_len = decode_varint(&mut cursor)?;
        if text_len < 0 || cursor.remaining() < text_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading tab complete request text",
            )));
        }

        let text_bytes = &cursor[..text_len as usize];
        let text = std::str::from_utf8(text_bytes)
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in tab complete request text",
                ))
            })?
            .to_string();

        Ok(Self {
            transaction_id,
            text,
        })
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let packet_id = if protocol_version >= 775 {
            0x0F
        } else if protocol_version >= 768 {
            0x0D
        } else if protocol_version >= 766 {
            0x0B
        } else if protocol_version >= 764 {
            0x0A
        } else {
            0x06
        };

        let mut payload = BytesMut::new();
        encode_varint(self.transaction_id, &mut payload);
        encode_varint(self.text.len() as i32, &mut payload);
        payload.put_slice(self.text.as_bytes());
        RawPacket::new(packet_id, payload.freeze())
    }
}

/// Client-bound Tab-Complete / Command Suggestions Response packet (`0x10` or `0x0F` depending on version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabCompleteResponsePacket {
    pub transaction_id: i32,
    pub start: i32,
    pub length: i32,
    pub matches: Vec<String>,
}

impl TabCompleteResponsePacket {
    pub fn new(transaction_id: i32, start: i32, length: i32, matches: Vec<String>) -> Self {
        Self {
            transaction_id,
            start,
            length,
            matches,
        }
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 770 {
            0x0F
        } else if protocol_version >= 764 {
            0x10
        } else {
            0x0F
        }
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let packet_id = Self::packet_id_for_version(protocol_version);
        let mut payload = BytesMut::new();
        encode_varint(self.transaction_id, &mut payload);
        encode_varint(self.start, &mut payload);
        encode_varint(self.length, &mut payload);
        encode_varint(self.matches.len() as i32, &mut payload);
        for m in &self.matches {
            encode_varint(m.len() as i32, &mut payload);
            payload.put_slice(m.as_bytes());
            // tooltip option: 0 = None (no tooltip)
            payload.put_u8(0x00);
        }
        RawPacket::new(packet_id, payload.freeze())
    }

    pub fn decode(packet: &RawPacket, protocol_version: i32) -> Result<Self, ProxyError> {
        let expected_id = Self::packet_id_for_version(protocol_version);
        if packet.id != expected_id && packet.id != 0x0F && packet.id != 0x10 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];
        let transaction_id = decode_varint(&mut cursor)?;
        let start = decode_varint(&mut cursor)?;
        let length = decode_varint(&mut cursor)?;
        let count = decode_varint(&mut cursor)?;
        if !(0..=4096).contains(&count) {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid suggestion match count",
            )));
        }

        let mut matches = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let m_len = decode_varint(&mut cursor)?;
            if m_len < 0 || cursor.remaining() < m_len as usize {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading match string",
                )));
            }
            let m_bytes = &cursor[..m_len as usize];
            let match_str = std::str::from_utf8(m_bytes)
                .map_err(|_| {
                    ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid UTF-8 in match string",
                    ))
                })?
                .to_string();
            cursor.advance(m_len as usize);

            // tooltip option: 0 = None
            if cursor.has_remaining() {
                let _has_tooltip = cursor.get_u8() != 0;
            }

            matches.push(match_str);
        }

        Ok(Self {
            transaction_id,
            start,
            length,
            matches,
        })
    }
}

/// Client-bound Respawn packet (`0x45`) used to reset client world context and entity state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RespawnPacket {
    pub dimension_type: String,
    pub dimension_name: String,
    pub hashed_seed: i64,
    pub gamemode: u8,
    pub previous_gamemode: i8,
    pub is_debug: bool,
    pub is_flat: bool,
    pub data_kept: u8,
}

impl RespawnPacket {
    pub fn default_reset() -> Self {
        Self {
            dimension_type: "minecraft:overworld".to_string(),
            dimension_name: "minecraft:overworld".to_string(),
            hashed_seed: 0,
            gamemode: 0, // Survival
            previous_gamemode: -1,
            is_debug: false,
            is_flat: false,
            data_kept: 0, // Reset all entities and world state
        }
    }

    pub fn is_valid_respawn_id(id: i32) -> bool {
        matches!(
            id,
            0x52 | 0x4C | 0x47 | 0x45 | 0x43 | 0x3F | 0x3E | 0x3D | 0x39 | 0x35
        )
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 775 {
            0x52
        } else if protocol_version >= 768 {
            0x4C
        } else if protocol_version >= 766 {
            0x47
        } else if protocol_version >= 764 {
            0x45
        } else if protocol_version >= 762 {
            0x43
        } else if protocol_version == 761 {
            0x3F
        } else if protocol_version >= 759 {
            0x3E
        } else if protocol_version >= 755 {
            0x3D
        } else if protocol_version >= 751 {
            0x39
        } else if protocol_version >= 340 {
            0x35
        } else {
            RESPAWN_PACKET_ID
        }
    }

    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        Self::decode_with_version(packet, 765)
    }

    pub fn decode_with_version(
        packet: &RawPacket,
        protocol_version: i32,
    ) -> Result<Self, ProxyError> {
        let expected_id = Self::packet_id_for_version(protocol_version);
        if packet.id != expected_id && !Self::is_valid_respawn_id(packet.id) {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];

        if protocol_version >= 766 {
            // Modern SpawnInfo (protocol >= 766)
            // 1. dimension (VarInt index)
            let _dimension_idx = decode_varint(&mut cursor)?;
            // 2. dimension_name
            let dn_len = decode_varint(&mut cursor)?;
            if dn_len < 0 || cursor.remaining() < dn_len as usize {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading dimension_name",
                )));
            }
            let dimension_name = std::str::from_utf8(&cursor[..dn_len as usize])
                .map_err(|_| {
                    ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid UTF-8 in dimension_name",
                    ))
                })?
                .to_string();
            cursor.advance(dn_len as usize);

            // 3. hashed_seed
            if cursor.remaining() < 8 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading hashed_seed",
                )));
            }
            let hashed_seed = cursor.get_i64();

            // 4. gamemode
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading gamemode",
                )));
            }
            let gamemode = cursor.get_u8();

            // 5. previous_gamemode
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading previous_gamemode",
                )));
            }
            let prev_u8 = cursor.get_u8();
            let previous_gamemode = if prev_u8 == 255 { -1 } else { prev_u8 as i8 };

            // 6. is_debug
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading is_debug",
                )));
            }
            let is_debug = cursor.get_u8() != 0;

            // 7. is_flat
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading is_flat",
                )));
            }
            let is_flat = cursor.get_u8() != 0;

            // 8. death (Option)
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading death option",
                )));
            }
            let has_death = cursor.get_u8() != 0;
            if has_death {
                let death_dim_len = decode_varint(&mut cursor)?;
                if death_dim_len < 0 || cursor.remaining() < death_dim_len as usize + 8 {
                    return Err(ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "EOF reading death pos",
                    )));
                }
                cursor.advance(death_dim_len as usize + 8);
            }

            // 9. portal_cooldown
            let _portal_cooldown = decode_varint(&mut cursor)?;

            // 10. sea_level (>= 768)
            if protocol_version >= 768 {
                let _sea_level = decode_varint(&mut cursor)?;
            }

            // 11. data_kept
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading data_kept",
                )));
            }
            let data_kept = cursor.get_u8();

            Ok(Self {
                dimension_type: dimension_name.clone(),
                dimension_name,
                hashed_seed,
                gamemode,
                previous_gamemode,
                is_debug,
                is_flat,
                data_kept,
            })
        } else {
            // 1. dimension_type
            let dt_len = decode_varint(&mut cursor)?;
            if dt_len < 0 || cursor.remaining() < dt_len as usize {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading dimension_type",
                )));
            }
            let dimension_type = std::str::from_utf8(&cursor[..dt_len as usize])
                .map_err(|_| {
                    ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid UTF-8 in dimension_type",
                    ))
                })?
                .to_string();
            cursor.advance(dt_len as usize);

            // 2. dimension_name
            let dn_len = decode_varint(&mut cursor)?;
            if dn_len < 0 || cursor.remaining() < dn_len as usize {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading dimension_name",
                )));
            }
            let dimension_name = std::str::from_utf8(&cursor[..dn_len as usize])
                .map_err(|_| {
                    ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid UTF-8 in dimension_name",
                    ))
                })?
                .to_string();
            cursor.advance(dn_len as usize);

            // 3. hashed_seed
            if cursor.remaining() < 8 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading hashed_seed",
                )));
            }
            let hashed_seed = cursor.get_i64();

            // 4. gamemode
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading gamemode",
                )));
            }
            let gamemode = cursor.get_u8();

            // 5. previous_gamemode
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading previous_gamemode",
                )));
            }
            let previous_gamemode = cursor.get_i8();

            // 6. is_debug
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading is_debug",
                )));
            }
            let is_debug = cursor.get_u8() != 0;

            // 7. is_flat
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading is_flat",
                )));
            }
            let is_flat = cursor.get_u8() != 0;

            // 8. data_kept
            if cursor.remaining() < 1 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF reading data_kept",
                )));
            }
            let data_kept = cursor.get_u8();

            Ok(Self {
                dimension_type,
                dimension_name,
                hashed_seed,
                gamemode,
                previous_gamemode,
                is_debug,
                is_flat,
                data_kept,
            })
        }
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let mut payload = BytesMut::new();

        if protocol_version >= 766 {
            // Modern SpawnInfo (protocol >= 766)
            // 1. dimension (VarInt index, 0 = overworld)
            encode_varint(0, &mut payload);

            // 2. dimension_name (String)
            encode_varint(self.dimension_name.len() as i32, &mut payload);
            payload.put_slice(self.dimension_name.as_bytes());

            // 3. hashed_seed (i64)
            payload.put_i64(self.hashed_seed);

            // 4. gamemode (i8)
            payload.put_i8(self.gamemode as i8);

            // 5. previous_gamemode (u8, 255 for none)
            payload.put_u8(if self.previous_gamemode < 0 {
                255
            } else {
                self.previous_gamemode as u8
            });

            // 6. is_debug (bool)
            payload.put_u8(if self.is_debug { 1 } else { 0 });

            // 7. is_flat (bool)
            payload.put_u8(if self.is_flat { 1 } else { 0 });

            // 8. death (Option, 0 = None)
            payload.put_u8(0);

            // 9. portal_cooldown (VarInt)
            encode_varint(0, &mut payload);

            // 10. sea_level (VarInt, present in protocol >= 768)
            if protocol_version >= 768 {
                encode_varint(63, &mut payload);
            }

            // 11. copyMetadata / data_kept (u8)
            payload.put_u8(self.data_kept);
        } else {
            encode_varint(self.dimension_type.len() as i32, &mut payload);
            payload.put_slice(self.dimension_type.as_bytes());

            encode_varint(self.dimension_name.len() as i32, &mut payload);
            payload.put_slice(self.dimension_name.as_bytes());

            payload.put_i64(self.hashed_seed);
            payload.put_u8(self.gamemode);
            payload.put_i8(self.previous_gamemode);
            payload.put_u8(if self.is_debug { 1 } else { 0 });
            payload.put_u8(if self.is_flat { 1 } else { 0 });
            payload.put_u8(self.data_kept);

            // death_dimension (None = 0)
            payload.put_u8(0);
            // portal_cooldown
            encode_varint(0, &mut payload);
        }

        RawPacket::new(
            Self::packet_id_for_version(protocol_version),
            payload.freeze(),
        )
    }
}

/// Returns true if the given packet ID corresponds to the clientbound Login (Play) / JoinGame packet
/// for the specified protocol version.
pub fn is_login_play_packet(id: i32, protocol_version: i32) -> bool {
    if protocol_version >= 775 {
        id == 0x31
    } else if protocol_version >= 768 {
        id == 0x2C
    } else if protocol_version >= 766 {
        id == 0x2B
    } else if protocol_version >= 764 {
        id == 0x29
    } else if protocol_version >= 762 {
        id == 0x28
    } else if protocol_version == 761 {
        id == 0x24 // 1.19.3
    } else if protocol_version >= 759 {
        id == 0x25 // 1.19-1.19.2
    } else if protocol_version >= 755 {
        id == 0x26 // 1.17-1.18.2
    } else if protocol_version >= 735 {
        id == 0x24 // 1.16-1.16.5
    } else if protocol_version >= 573 {
        id == 0x26 // 1.15-1.15.2
    } else if protocol_version >= 393 {
        id == 0x25 // 1.13-1.14.4
    } else {
        id == 0x23 // <= 1.12.2
    }
}

/// Slices the Common Player Spawn Info from a backend Login (Play) packet and constructs
/// a matching client-bound Respawn packet with data_kept = 0 [R-01], [R-02], [R-11].
///
/// In Minecraft Java Edition (protocol >= 766), the layout of Login (Play) contains:
/// [entity_id: i32] [is_hardcore: bool] [dimension_names: String[]] [max_players: VarInt]
/// [view_dist: VarInt] [sim_dist: VarInt] [reduced_debug: bool] [show_respawn: bool]
/// [do_limited_crafting: bool] [Common Player Spawn Info...] [enforces_secure_chat: bool]
///
/// And clientbound Respawn layout is:
/// [Common Player Spawn Info...] [data_kept: u8]
///
/// Extracting this worldState directly preserves the backend's exact dimension registry index,
/// dimension name identifier, seed, gamemode, and sea level, ensuring zero client ghost collisions
/// or desync upon server transfers.
pub fn extract_respawn_from_login_with_data_kept(
    login_pkt: &RawPacket,
    protocol_version: i32,
    data_to_keep: u8,
) -> Option<RawPacket> {
    if protocol_version >= 766 {
        let mut cursor = &login_pkt.payload[..];
        // 1. entity_id (4 bytes i32)
        if cursor.remaining() < 4 {
            return None;
        }
        cursor.advance(4);

        // 2. is_hardcore (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 3. dimension_names (VarInt count + strings)
        let dim_count = decode_varint(&mut cursor).ok()?;
        if !(0..=1024).contains(&dim_count) {
            return None;
        }
        for _ in 0..dim_count {
            let str_len = decode_varint(&mut cursor).ok()?;
            if str_len < 0 || cursor.remaining() < str_len as usize {
                return None;
            }
            cursor.advance(str_len as usize);
        }

        // 4. max_players (VarInt)
        decode_varint(&mut cursor).ok()?;

        // 5. view_distance (VarInt)
        decode_varint(&mut cursor).ok()?;

        // 6. simulation_distance (VarInt)
        decode_varint(&mut cursor).ok()?;

        // 7. reduced_debug_info (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 8. show_respawn_screen (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 9. do_limited_crafting (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // We are now at the start of Common Player Spawn Info (SpawnInfo).
        // Parse SpawnInfo forward to accurately locate its exact boundary,
        // avoiding any assumptions about trailing Login (Play) fields (e.g. enforcesSecureChat, onlineMode in 26.2+).
        let spawn_info_start = login_pkt.payload.len() - cursor.remaining();

        // 1. dimension: VarInt
        decode_varint(&mut cursor).ok()?;

        // 2. dimension name: String (VarInt length + bytes)
        let name_len = decode_varint(&mut cursor).ok()?;
        if name_len < 0 || cursor.remaining() < name_len as usize {
            return None;
        }
        cursor.advance(name_len as usize);

        // 3. hashed_seed: i64 (8 bytes)
        if cursor.remaining() < 8 {
            return None;
        }
        cursor.advance(8);

        // 4. gamemode: u8 (1 byte)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 5. previous_gamemode: u8 / i8 (1 byte)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 6. is_debug: bool (1 byte)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 7. is_flat: bool (1 byte)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 8. has_death_location: bool (1 byte)
        if cursor.remaining() < 1 {
            return None;
        }
        let has_death = cursor.get_u8() != 0;
        if has_death {
            // GlobalPos: dimension identifier (String) + position (i64)
            let death_dim_len = decode_varint(&mut cursor).ok()?;
            if death_dim_len < 0 || cursor.remaining() < death_dim_len as usize + 8 {
                return None;
            }
            cursor.advance(death_dim_len as usize + 8);
        }

        // 9. portal_cooldown: VarInt
        decode_varint(&mut cursor).ok()?;

        // 10. sea_level: VarInt (present in protocol >= 768 / 1.21.2+)
        if protocol_version >= 768 {
            decode_varint(&mut cursor).ok()?;
        }

        let spawn_info_end = login_pkt.payload.len() - cursor.remaining();
        let spawn_info = &login_pkt.payload[spawn_info_start..spawn_info_end];

        // Respawn packet is SpawnInfo + dataToKeep: u8
        let mut respawn_payload = BytesMut::with_capacity(spawn_info.len() + 1);
        respawn_payload.put_slice(spawn_info);
        respawn_payload.put_u8(data_to_keep);

        let respawn_id = RespawnPacket::packet_id_for_version(protocol_version);
        Some(RawPacket::new(respawn_id, respawn_payload.freeze()))
    } else if protocol_version >= 764 {
        let mut cursor = &login_pkt.payload[..];
        // 1. entity_id (4 bytes i32)
        if cursor.remaining() < 4 {
            return None;
        }
        cursor.advance(4);

        // 2. is_hardcore (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 3. dimension_names (VarInt count + strings)
        let dim_count = decode_varint(&mut cursor).ok()?;
        if !(0..=1024).contains(&dim_count) {
            return None;
        }
        for _ in 0..dim_count {
            let str_len = decode_varint(&mut cursor).ok()?;
            if str_len < 0 || cursor.remaining() < str_len as usize {
                return None;
            }
            cursor.advance(str_len as usize);
        }

        // 4. max_players (VarInt)
        decode_varint(&mut cursor).ok()?;

        // 5. view_distance (VarInt)
        decode_varint(&mut cursor).ok()?;

        // 6. simulation_distance (VarInt)
        decode_varint(&mut cursor).ok()?;

        // 7. reduced_debug_info (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 8. show_respawn_screen (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // 9. do_limited_crafting (1 byte bool)
        if cursor.remaining() < 1 {
            return None;
        }
        cursor.advance(1);

        // In 1.20.2 - 1.20.4 (protocols 764-765), SpawnInfo fields:
        // 1. dimension_type: String
        let dt_len = decode_varint(&mut cursor).ok()?;
        if dt_len < 0 || cursor.remaining() < dt_len as usize {
            return None;
        }
        let dimension_type = std::str::from_utf8(&cursor[..dt_len as usize])
            .ok()?
            .to_string();
        cursor.advance(dt_len as usize);

        // 2. dimension_name: String
        let dn_len = decode_varint(&mut cursor).ok()?;
        if dn_len < 0 || cursor.remaining() < dn_len as usize {
            return None;
        }
        let dimension_name = std::str::from_utf8(&cursor[..dn_len as usize])
            .ok()?
            .to_string();
        cursor.advance(dn_len as usize);

        // 3. hashed_seed: i64
        if cursor.remaining() < 8 {
            return None;
        }
        let hashed_seed = cursor.get_i64();

        // 4. gamemode: u8
        if cursor.remaining() < 1 {
            return None;
        }
        let gamemode = cursor.get_u8();

        // 5. previous_gamemode: i8
        if cursor.remaining() < 1 {
            return None;
        }
        let previous_gamemode = cursor.get_i8();

        // 6. is_debug: bool
        if cursor.remaining() < 1 {
            return None;
        }
        let is_debug = cursor.get_u8() != 0;

        // 7. is_flat: bool
        if cursor.remaining() < 1 {
            return None;
        }
        let is_flat = cursor.get_u8() != 0;

        let respawn = RespawnPacket {
            dimension_type,
            dimension_name,
            hashed_seed,
            gamemode,
            previous_gamemode,
            is_debug,
            is_flat,
            data_kept: data_to_keep,
        };
        Some(respawn.encode_with_version(protocol_version))
    } else {
        let mut reset = RespawnPacket::default_reset();
        reset.data_kept = data_to_keep;
        Some(reset.encode_with_version(protocol_version))
    }
}

pub fn extract_respawn_from_login(
    login_pkt: &RawPacket,
    protocol_version: i32,
) -> Option<RawPacket> {
    extract_respawn_from_login_with_data_kept(login_pkt, protocol_version, 0x00)
}

/// Returns true if the packet ID corresponds to the clientbound DeclareCommands packet.
pub fn is_declare_commands_packet(id: i32, protocol_version: i32) -> bool {
    if protocol_version >= 770 {
        id == 0x10
    } else {
        id == 0x11
    }
}

/// Injects FrameMC proxy commands (`server`, `hub`, `lobby`, `steel`, etc.) into the backend's
/// `declare_commands` packet so that modern Minecraft clients (1.13+) highlight proxy commands in white
/// and display them directly in client-side autocomplete popups as the player types.
pub fn inject_proxy_commands_into_declare_commands(
    packet: &RawPacket,
    protocol_version: i32,
    custom_servers: &[String],
) -> Result<RawPacket, ProxyError> {
    if protocol_version < 393 {
        return Ok(packet.clone());
    }

    if !is_declare_commands_packet(packet.id, protocol_version)
        && packet.id != 0x10
        && packet.id != 0x11
    {
        return Ok(packet.clone());
    }

    let mut cursor = &packet.payload[..];
    let original_count = decode_varint(&mut cursor)?;
    if original_count <= 0 || cursor.is_empty() {
        return Ok(packet.clone());
    }

    // Root node: flags must be 0x00
    let root_flags = cursor.get_u8();
    if root_flags != 0x00 {
        return Ok(packet.clone());
    }

    let original_child_count = decode_varint(&mut cursor)?;
    if original_child_count < 0 || cursor.remaining() < original_child_count as usize {
        return Ok(packet.clone());
    }

    let mut original_children = Vec::with_capacity(original_child_count as usize);
    for _ in 0..original_child_count {
        original_children.push(decode_varint(&mut cursor)?);
    }

    // The remainder of the payload contains nodes 1..N-1 and the trailing rootIndex VarInt
    let rem = cursor.remaining();
    if rem < 1 {
        return Ok(packet.clone());
    }

    let mut root_index_varint = None;
    if cursor[rem - 1] == 0x00 {
        // Canonical multi-byte VarInts can NEVER end with 0x00.
        // A trailing 0x00 byte is guaranteed to be a 1-byte VarInt of value 0.
        root_index_varint = Some((1, 0));
    } else if (cursor[rem - 1] & 0x80) == 0 {
        let mut possible_lens = Vec::new();
        possible_lens.push(1);
        for len in 2..=5.min(rem) {
            if (cursor[rem - len] & 0x80) != 0 {
                possible_lens.push(len);
            } else {
                break;
            }
        }
        for &len in possible_lens.iter().rev() {
            let start = rem - len;
            let mut slice = &cursor[start..];
            if let Ok(val) = decode_varint(&mut slice) {
                if slice.is_empty()
                    && varint_size(val) == len
                    && val >= 0
                    && (val as usize) < original_count as usize
                {
                    root_index_varint = Some((len, val));
                    break;
                }
            }
        }
    }
    let (root_index_len, root_index) = match root_index_varint {
        Some((len, val)) => (len, val),
        None => return Ok(packet.clone()),
    };
    let rest_of_nodes = &cursor[..rem - root_index_len];

    // Build list of proxy commands to inject:
    // 1. "server" (with argument "name" that asks server for completions)
    // 2. "hub"
    // 3. "lobby"
    // 4. "steel"
    // Plus any custom configured backend server names!
    let mut root_literals = vec!["hub".to_string(), "lobby".to_string(), "steel".to_string()];
    for s in custom_servers {
        let name = s.trim().to_lowercase();
        if !name.is_empty() && name != "server" && !root_literals.contains(&name) {
            root_literals.push(name);
        }
    }

    let mut new_nodes_payload = BytesMut::new();
    let mut new_root_children = Vec::new();
    let mut current_index = original_count;

    // A) Literal node for "server" with child argument node for "<name>"
    let server_literal_idx = current_index;
    let server_arg_idx = current_index + 1;
    current_index += 2;
    new_root_children.push(server_literal_idx);

    // Literal node: "server"
    // flags: 0x01 (literal) | 0x04 (executable) = 0x05
    new_nodes_payload.put_u8(0x05);
    encode_varint(1, &mut new_nodes_payload); // 1 child
    encode_varint(server_arg_idx, &mut new_nodes_payload); // points to server_arg_idx
    encode_varint(6, &mut new_nodes_payload);
    new_nodes_payload.put_slice(b"server");

    // Argument node: "<name>"
    // flags: 0x02 (argument) | 0x04 (executable) | 0x10 (has_custom_suggestions) = 0x16
    new_nodes_payload.put_u8(0x16);
    encode_varint(0, &mut new_nodes_payload); // 0 children
    encode_varint(4, &mut new_nodes_payload);
    new_nodes_payload.put_slice(b"name");
    encode_varint(5, &mut new_nodes_payload); // parser: brigadier:string = 5
    encode_varint(0, &mut new_nodes_payload); // string type: SINGLE_WORD = 0
    let sug_type = "minecraft:ask_server";
    encode_varint(sug_type.len() as i32, &mut new_nodes_payload);
    new_nodes_payload.put_slice(sug_type.as_bytes());

    // B) Literal nodes for root shortcuts
    for literal in &root_literals {
        let lit_idx = current_index;
        current_index += 1;
        new_root_children.push(lit_idx);

        new_nodes_payload.put_u8(0x05); // literal, executable
        encode_varint(0, &mut new_nodes_payload); // 0 children
        encode_varint(literal.len() as i32, &mut new_nodes_payload);
        new_nodes_payload.put_slice(literal.as_bytes());
    }

    let new_total_count = current_index;

    // Assemble final payload
    let mut final_payload =
        BytesMut::with_capacity(packet.payload.len() + new_nodes_payload.len() + 64);
    encode_varint(new_total_count, &mut final_payload);

    // Node 0 (Root node)
    final_payload.put_u8(0x00); // flags
    encode_varint(
        (original_children.len() + new_root_children.len()) as i32,
        &mut final_payload,
    );
    for c in &original_children {
        encode_varint(*c, &mut final_payload);
    }
    for c in &new_root_children {
        encode_varint(*c, &mut final_payload);
    }

    // Existing nodes 1..N-1
    final_payload.put_slice(rest_of_nodes);

    // Injected proxy nodes
    final_payload.put_slice(&new_nodes_payload);

    // Trailing rootIndex
    encode_varint(root_index, &mut final_payload);

    Ok(RawPacket::new(packet.id, final_payload.freeze()))
}

/// Client-bound System Chat Message packet sent to display notification or error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemChatMessagePacket {
    pub message: String,
    pub overlay: bool,
}

impl SystemChatMessagePacket {
    pub fn new(message: impl Into<String>, overlay: bool) -> Self {
        Self {
            message: message.into(),
            overlay,
        }
    }

    pub fn is_valid_system_chat_id(id: i32) -> bool {
        matches!(id, 0x79 | 0x73 | 0x6C | 0x67 | 0x64 | 0x60 | 0x5F | 0x69)
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 775 {
            0x79
        } else if protocol_version >= 768 {
            0x73
        } else if protocol_version >= 766 {
            0x6C
        } else if protocol_version >= 764 {
            SYSTEM_CHAT_MESSAGE_PACKET_ID
        } else if protocol_version == 763 {
            0x67
        } else if protocol_version == 762 {
            0x64
        } else if protocol_version == 761 {
            0x60
        } else if protocol_version >= 759 {
            0x5F
        } else {
            SYSTEM_CHAT_MESSAGE_PACKET_ID
        }
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let mut payload = BytesMut::new();
        if protocol_version >= 766 {
            // Modern Minecraft (1.20.5+ / protocol >= 766): anonymous NBT compound containing "text"
            // [0x0a (TAG_Compound), 0x08 (TAG_String), 0x00, 0x04, 't', 'e', 'x', 't',
            //  len_hi, len_lo, msg_bytes..., 0x00 (TAG_End), overlay as u8]
            payload.put_u8(0x0a); // TAG_Compound
            payload.put_u8(0x08); // TAG_String
            payload.put_u16(4); // key len
            payload.put_slice(b"text");
            let msg_bytes = self.message.as_bytes();
            payload.put_u16(msg_bytes.len() as u16);
            payload.put_slice(msg_bytes);
            payload.put_u8(0x00); // TAG_End
            payload.put_u8(if self.overlay { 1 } else { 0 });
        } else {
            // Legacy Minecraft (< 766): JSON string with VarInt length prefix + overlay boolean
            let json_str = serde_json::json!({ "text": self.message }).to_string();
            encode_varint(json_str.len() as i32, &mut payload);
            payload.put_slice(json_str.as_bytes());
            payload.put_u8(if self.overlay { 1 } else { 0 });
        }
        RawPacket::new(
            Self::packet_id_for_version(protocol_version),
            payload.freeze(),
        )
    }

    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        Self::decode_with_version(packet, 765)
    }

    pub fn decode_with_version(
        packet: &RawPacket,
        _protocol_version: i32,
    ) -> Result<Self, ProxyError> {
        let expected_id = Self::packet_id_for_version(_protocol_version);
        if packet.id != expected_id && !Self::is_valid_system_chat_id(packet.id) {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];
        if cursor.is_empty() {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "empty system chat message payload",
            )));
        }

        // Check if payload starts with 0x0A (TAG_Compound) for anonymous NBT
        if cursor[0] == 0x0A {
            cursor.advance(1); // skip TAG_Compound
            let mut message = String::new();
            while cursor.has_remaining() {
                let tag_type = cursor.get_u8();
                if tag_type == 0x00 {
                    // TAG_End
                    break;
                }
                if cursor.remaining() < 2 {
                    break;
                }
                let key_len = cursor.get_u16() as usize;
                if cursor.remaining() < key_len {
                    break;
                }
                let key = std::str::from_utf8(&cursor[..key_len]).unwrap_or("");
                cursor.advance(key_len);

                if tag_type == 0x08 {
                    // TAG_String
                    if cursor.remaining() < 2 {
                        break;
                    }
                    let str_len = cursor.get_u16() as usize;
                    if cursor.remaining() < str_len {
                        break;
                    }
                    let val = std::str::from_utf8(&cursor[..str_len]).unwrap_or("");
                    cursor.advance(str_len);
                    if key == "text" {
                        message = val.to_string();
                    }
                }
            }
            let overlay = if cursor.has_remaining() {
                cursor.get_u8() != 0
            } else {
                false
            };
            return Ok(Self { message, overlay });
        }

        // Otherwise JSON string format
        let len = decode_varint(&mut cursor)?;
        if len < 0 || cursor.remaining() < len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading system chat message JSON",
            )));
        }
        let json_bytes = &cursor[..len as usize];
        let json_str = std::str::from_utf8(json_bytes).map_err(|_| {
            ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid UTF-8 in message JSON",
            ))
        })?;
        cursor.advance(len as usize);

        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| ProxyError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        let message = parsed
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or(json_str)
            .to_string();

        let overlay = if cursor.has_remaining() {
            cursor.get_u8() != 0
        } else {
            false
        };

        Ok(Self { message, overlay })
    }
}

/// Client-bound Close Container / Window packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseContainerPacket {
    pub window_id: u8,
}

impl CloseContainerPacket {
    pub fn new(window_id: u8) -> Self {
        Self { window_id }
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 764 {
            CLOSE_CONTAINER_PACKET_ID
        } else if protocol_version >= 762 {
            0x11
        } else if protocol_version == 761 {
            0x0F
        } else if protocol_version >= 759 {
            0x10
        } else {
            CLOSE_CONTAINER_PACKET_ID
        }
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let id = Self::packet_id_for_version(protocol_version);
        let mut payload = BytesMut::with_capacity(1);
        payload.put_u8(self.window_id);
        RawPacket::new(id, payload.freeze())
    }

    pub fn decode(packet: &RawPacket, protocol_version: i32) -> Result<Self, ProxyError> {
        let expected = Self::packet_id_for_version(protocol_version);
        if packet.id != expected {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];
        if cursor.remaining() < 1 {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected EOF reading window_id in CloseContainer",
            )));
        }
        let window_id = cursor.get_u8();
        Ok(Self { window_id })
    }
}

/// Client-bound Stop Sound packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopSoundPacket {
    pub flags: u8,
    pub source: Option<i32>,
    pub sound: Option<String>,
}

impl StopSoundPacket {
    pub fn all() -> Self {
        Self {
            flags: 0,
            source: None,
            sound: None,
        }
    }

    pub fn with_source(source: i32) -> Self {
        Self {
            flags: 1,
            source: Some(source),
            sound: None,
        }
    }

    pub fn with_sound(sound: impl Into<String>) -> Self {
        Self {
            flags: 2,
            source: None,
            sound: Some(sound.into()),
        }
    }

    pub fn with_source_and_sound(source: i32, sound: impl Into<String>) -> Self {
        Self {
            flags: 3,
            source: Some(source),
            sound: Some(sound.into()),
        }
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 768 {
            0x71
        } else if protocol_version >= 766 {
            0x6A
        } else if protocol_version >= 765 {
            0x68
        } else if protocol_version >= 764 {
            STOP_SOUND_PACKET_ID
        } else if protocol_version >= 762 {
            0x63
        } else if protocol_version == 761 {
            0x5F
        } else if protocol_version >= 759 {
            0x61
        } else {
            STOP_SOUND_PACKET_ID
        }
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let id = Self::packet_id_for_version(protocol_version);
        let mut payload = BytesMut::new();
        payload.put_u8(self.flags);
        if self.flags & 1 != 0 {
            if let Some(src) = self.source {
                encode_varint(src, &mut payload);
            }
        }
        if self.flags & 2 != 0 {
            if let Some(ref snd) = self.sound {
                encode_varint(snd.len() as i32, &mut payload);
                payload.put_slice(snd.as_bytes());
            }
        }
        RawPacket::new(id, payload.freeze())
    }

    pub fn decode(packet: &RawPacket, protocol_version: i32) -> Result<Self, ProxyError> {
        let expected = Self::packet_id_for_version(protocol_version);
        if packet.id != expected {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];
        if cursor.remaining() < 1 {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected EOF reading flags in StopSound",
            )));
        }
        let flags = cursor.get_u8();
        let source = if flags & 1 != 0 {
            Some(decode_varint(&mut cursor)?)
        } else {
            None
        };
        let sound = if flags & 2 != 0 {
            let len = decode_varint(&mut cursor)?;
            if len < 0 || cursor.remaining() < len as usize {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Unexpected EOF reading sound in StopSound",
                )));
            }
            let s = std::str::from_utf8(&cursor[..len as usize])
                .map_err(|_| {
                    ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid UTF-8 in sound",
                    ))
                })?
                .to_string();
            cursor.advance(len as usize);
            Some(s)
        } else {
            None
        };

        Ok(Self {
            flags,
            source,
            sound,
        })
    }
}

/// Client-bound Boss Bar packet (used for synchronization and clean removal on transfer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BossBarPacket {
    pub uuid: [u8; 16],
    pub action: i32,
}

impl BossBarPacket {
    pub const ACTION_ADD: i32 = 0;
    pub const ACTION_REMOVE: i32 = 1;
    pub const ACTION_UPDATE_HEALTH: i32 = 2;
    pub const ACTION_UPDATE_TITLE: i32 = 3;
    pub const ACTION_UPDATE_STYLE: i32 = 4;
    pub const ACTION_UPDATE_FLAGS: i32 = 5;

    pub fn remove(uuid: [u8; 16]) -> Self {
        Self {
            uuid,
            action: Self::ACTION_REMOVE,
        }
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 764 {
            BOSS_BAR_PACKET_ID
        } else if protocol_version >= 762 {
            0x0B
        } else {
            BOSS_BAR_PACKET_ID
        }
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let id = Self::packet_id_for_version(protocol_version);
        let mut payload = BytesMut::with_capacity(17);
        payload.put_slice(&self.uuid);
        encode_varint(self.action, &mut payload);
        RawPacket::new(id, payload.freeze())
    }

    pub fn decode(packet: &RawPacket, protocol_version: i32) -> Result<Self, ProxyError> {
        let expected = Self::packet_id_for_version(protocol_version);
        if packet.id != expected {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];
        if cursor.remaining() < 16 {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected EOF reading UUID in BossBar",
            )));
        }
        let mut uuid = [0u8; 16];
        cursor.copy_to_slice(&mut uuid);
        let action = decode_varint(&mut cursor)?;
        Ok(Self { uuid, action })
    }
}

/// Client-bound Scoreboard Objective packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreboardObjectivePacket {
    pub name: String,
    pub action: u8,
}

impl ScoreboardObjectivePacket {
    pub const ACTION_CREATE: u8 = 0;
    pub const ACTION_REMOVE: u8 = 1;
    pub const ACTION_UPDATE_DISPLAY_TEXT: u8 = 2;

    pub fn remove(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            action: Self::ACTION_REMOVE,
        }
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 768 {
            0x64
        } else if protocol_version >= 766 {
            0x5E
        } else if protocol_version >= 765 {
            0x5C
        } else if protocol_version >= 764 {
            SCOREBOARD_OBJECTIVE_PACKET_ID
        } else if protocol_version >= 762 {
            0x58
        } else if protocol_version == 761 {
            0x54
        } else {
            0x56
        }
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let id = Self::packet_id_for_version(protocol_version);
        let mut payload = BytesMut::new();
        encode_varint(self.name.len() as i32, &mut payload);
        payload.put_slice(self.name.as_bytes());
        payload.put_u8(self.action);
        RawPacket::new(id, payload.freeze())
    }

    pub fn decode(packet: &RawPacket, protocol_version: i32) -> Result<Self, ProxyError> {
        let expected = Self::packet_id_for_version(protocol_version);
        if packet.id != expected {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];
        let len = decode_varint(&mut cursor)?;
        if len < 0 || cursor.remaining() < len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected EOF reading name in ScoreboardObjective",
            )));
        }
        let name = std::str::from_utf8(&cursor[..len as usize])
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid UTF-8 in ScoreboardObjective name",
                ))
            })?
            .to_string();
        cursor.advance(len as usize);
        if cursor.remaining() < 1 {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected EOF reading action in ScoreboardObjective",
            )));
        }
        let action = cursor.get_u8();
        Ok(Self { name, action })
    }
}

/// Client-bound Set Display Objective packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayObjectivePacket {
    pub position: i32,
    pub name: String,
}

impl DisplayObjectivePacket {
    pub const POSITION_LIST: i32 = 0;
    pub const POSITION_SIDEBAR: i32 = 1;
    pub const POSITION_BELOW_NAME: i32 = 2;

    pub fn clear(position: i32) -> Self {
        Self {
            position,
            name: String::new(),
        }
    }

    pub fn packet_id_for_version(protocol_version: i32) -> i32 {
        if protocol_version >= 768 {
            0x5C
        } else if protocol_version >= 766 {
            0x57
        } else if protocol_version >= 765 {
            0x55
        } else if protocol_version >= 764 {
            DISPLAY_OBJECTIVE_PACKET_ID
        } else if protocol_version >= 762 {
            0x51
        } else if protocol_version == 761 {
            0x4D
        } else {
            0x4F
        }
    }

    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let id = Self::packet_id_for_version(protocol_version);
        let mut payload = BytesMut::new();
        encode_varint(self.position, &mut payload);
        encode_varint(self.name.len() as i32, &mut payload);
        payload.put_slice(self.name.as_bytes());
        RawPacket::new(id, payload.freeze())
    }

    pub fn decode(packet: &RawPacket, protocol_version: i32) -> Result<Self, ProxyError> {
        let expected = Self::packet_id_for_version(protocol_version);
        if packet.id != expected {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];
        let position = decode_varint(&mut cursor)?;
        let len = decode_varint(&mut cursor)?;
        if len < 0 || cursor.remaining() < len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Unexpected EOF reading name in DisplayObjective",
            )));
        }
        let name = std::str::from_utf8(&cursor[..len as usize])
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid UTF-8 in DisplayObjective name",
                ))
            })?
            .to_string();
        cursor.advance(len as usize);
        Ok(Self { position, name })
    }
}

/// Returns true if the packet ID corresponds to clientbound Open Screen / Window.
pub fn is_open_window_packet(id: i32, protocol_version: i32) -> bool {
    if protocol_version >= 768 {
        id == 0x35
    } else if protocol_version >= 766 {
        id == 0x33
    } else if protocol_version >= 764 {
        id == 0x31
    } else if protocol_version >= 762 {
        id == 0x30
    } else if protocol_version == 761 {
        id == 0x2D
    } else {
        id == 0x2E
    }
}

/// Returns true if the packet ID corresponds to clientbound sound effects.
pub fn is_sound_packet(id: i32, protocol_version: i32) -> bool {
    if protocol_version >= 768 {
        id == 0x6F || id == 0x6E
    } else if protocol_version >= 766 {
        id == 0x68 || id == 0x67
    } else if protocol_version >= 765 {
        id == 0x66 || id == 0x65
    } else if protocol_version >= 764 {
        id == 0x64 || id == 0x63
    } else {
        id == 0x61 || id == 0x60
    }
}

/// Returns true if the packet ID corresponds to serverbound Close Container / Window.
pub fn is_serverbound_close_container_packet(id: i32, protocol_version: i32) -> bool {
    if protocol_version >= 768 {
        id == 0x11
    } else if protocol_version >= 766 {
        id == 0x0F
    } else if protocol_version >= 764 {
        id == 0x0E
    } else if protocol_version >= 762 {
        id == 0x0D
    } else {
        id == 0x0C || id == 0x0A || id == 0x09
    }
}

/// State tracking an authenticated player session during routing.
#[derive(Debug, Clone)]
pub struct PlayerSession {
    pub profile: PlayerProfile,
    pub client_ip: String,
    pub protocol_version: i32,
    pub current_server: String,
    pub registry_cache: SessionRegistryCache,
}

impl PlayerSession {
    pub fn new(
        profile: PlayerProfile,
        client_ip: impl Into<String>,
        protocol_version: i32,
        current_server: impl Into<String>,
        registry_cache: SessionRegistryCache,
    ) -> Self {
        Self {
            profile,
            client_ip: client_ip.into(),
            protocol_version,
            current_server: current_server.into(),
            registry_cache,
        }
    }
}

/// Play state machine coordinating packet interception, Rhai evaluation,
/// mid-session server transfers, and disconnect recovery [R-02], [R-10], [R-11].
pub struct PlayStateMachine {
    pub client: Box<dyn AsyncStream>,
    pub backend: Option<Box<dyn AsyncStream>>,
    pub connector: Arc<dyn BackendConnector>,
    pub session: PlayerSession,
    pub config: Arc<ProxyConfig>,
    pub script_host: Arc<ScriptHost>,
    pub client_compression_threshold: Option<usize>,
    pub backend_compression_threshold: Option<usize>,
    /// Backwards-compatible alias for client_compression_threshold
    pub compression_threshold: Option<usize>,
    pub server_transferred: bool,
    pub open_container_id: Option<u8>,
    pub has_active_audio: bool,
    pub active_boss_bars: std::collections::HashSet<[u8; 16]>,
    pub active_scoreboard_objectives: std::collections::HashSet<String>,
    pub active_display_slots: std::collections::HashSet<i32>,
}

impl PlayStateMachine {
    pub fn new(
        client: Box<dyn AsyncStream>,
        backend: Box<dyn AsyncStream>,
        connector: Arc<dyn BackendConnector>,
        session: PlayerSession,
        config: Arc<ProxyConfig>,
        script_host: Arc<ScriptHost>,
        compression_threshold: Option<usize>,
    ) -> Self {
        Self {
            client,
            backend: Some(backend),
            connector,
            session,
            config,
            script_host,
            client_compression_threshold: compression_threshold,
            backend_compression_threshold: compression_threshold,
            compression_threshold,
            server_transferred: false,
            open_container_id: None,
            has_active_audio: false,
            active_boss_bars: std::collections::HashSet::new(),
            active_scoreboard_objectives: std::collections::HashSet::new(),
            active_display_slots: std::collections::HashSet::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_thresholds(
        client: Box<dyn AsyncStream>,
        backend: Box<dyn AsyncStream>,
        connector: Arc<dyn BackendConnector>,
        session: PlayerSession,
        config: Arc<ProxyConfig>,
        script_host: Arc<ScriptHost>,
        client_compression_threshold: Option<usize>,
        backend_compression_threshold: Option<usize>,
    ) -> Self {
        Self {
            client,
            backend: Some(backend),
            connector,
            session,
            config,
            script_host,
            client_compression_threshold,
            backend_compression_threshold,
            compression_threshold: client_compression_threshold,
            server_transferred: false,
            open_container_id: None,
            has_active_audio: false,
            active_boss_bars: std::collections::HashSet::new(),
            active_scoreboard_objectives: std::collections::HashSet::new(),
            active_display_slots: std::collections::HashSet::new(),
        }
    }

    /// Executes the server transfer sequence:
    /// 1. Connects to new target backend via `connector.connect`.
    /// 2. Performs configuration handshake with target backend (capturing any new registries).
    /// 3. Synthesizes and sends client-bound `Respawn` packet.
    /// 4. Safely terminates old backend connection.
    /// 5. Replaces backend stream with new connection.
    pub async fn switch_server(&mut self, target_server: &str) -> Result<bool, ProxyError> {
        let backend_config = match self.config.servers.get(target_server) {
            Some(cfg) => cfg,
            None => {
                tracing::warn!("Target server '{}' not found in config", target_server);
                let msg = format!("§cServer '{}' not found.", target_server);
                let chat_pkt = SystemChatMessagePacket::new(msg, false)
                    .encode_with_version(self.session.protocol_version);
                write_packet_with_compression(
                    &mut self.client,
                    &chat_pkt,
                    self.client_compression_threshold,
                )
                .await?;
                let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                return Ok(false);
            }
        };

        tracing::info!(
            player = %self.session.profile.name,
            from = %self.session.current_server,
            to = %target_server,
            "Initiating server transfer sequence"
        );

        // 1. Connect to new target backend (performs handshake, login, and acknowledges login if modern)
        let (mut new_backend, new_backend_threshold) = match self
            .connector
            .connect(
                target_server,
                backend_config,
                &self.session.profile,
                &self.session.client_ip,
                self.session.protocol_version,
            )
            .await
        {
            Ok(res) => res,
            Err(e) => {
                tracing::error!("Failed to connect to target server '{target_server}': {e}");
                let msg = format!("§cCould not connect to {target_server}: {e}");
                let chat_pkt = SystemChatMessagePacket::new(msg, false)
                    .encode_with_version(self.session.protocol_version);
                let _ = write_packet_with_compression(
                    &mut self.client,
                    &chat_pkt,
                    self.client_compression_threshold,
                )
                .await;
                let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                return Ok(false);
            }
        };

        // 2. Modern configuration handshake with target backend (protocol >= 764)
        if self.session.protocol_version >= 764 {
            let finish_id = if self.session.protocol_version >= 766 {
                0x03
            } else {
                0x02
            };
            let disconnect_id = if self.session.protocol_version >= 766 {
                0x02
            } else {
                0x01
            };

            loop {
                let packet_res = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    read_packet_with_compression(
                        &mut new_backend,
                        DEFAULT_MAX_PACKET_SIZE,
                        new_backend_threshold,
                    ),
                )
                .await;

                let packet = match packet_res {
                    Ok(Ok(pkt)) => pkt,
                    Ok(Err(e)) => {
                        tracing::error!(
                            "Error reading configuration packet from {target_server}: {e}"
                        );
                        let msg = format!("§cCould not connect to {target_server}: {e}");
                        let chat_pkt = SystemChatMessagePacket::new(msg, false)
                            .encode_with_version(self.session.protocol_version);
                        let _ = write_packet_with_compression(
                            &mut self.client,
                            &chat_pkt,
                            self.client_compression_threshold,
                        )
                        .await;
                        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                        return Ok(false);
                    }
                    Err(_) => {
                        tracing::error!(
                            "Timeout reading configuration from target server '{target_server}'"
                        );
                        let msg =
                            format!("§cCould not connect to {target_server}: connection timed out");
                        let chat_pkt = SystemChatMessagePacket::new(msg, false)
                            .encode_with_version(self.session.protocol_version);
                        let _ = write_packet_with_compression(
                            &mut self.client,
                            &chat_pkt,
                            self.client_compression_threshold,
                        )
                        .await;
                        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                        return Ok(false);
                    }
                };

                if packet.id == REGISTRY_DATA_PACKET_ID
                    || (self.session.protocol_version < 766 && packet.id == 0x05)
                {
                    let _ = self.session.registry_cache.cache_packet(&packet);
                } else if self.session.protocol_version >= 766 && packet.id == 0x0E {
                    // Clientbound Known Packs: respond with Serverbound Known Packs (0x07) with 0 packs (VarInt count = 0)
                    let mut known_packs_resp = BytesMut::new();
                    encode_varint(0, &mut known_packs_resp);
                    let resp_pkt = RawPacket::new(0x07, known_packs_resp.freeze());
                    if let Err(e) = write_packet_with_compression(
                        &mut new_backend,
                        &resp_pkt,
                        new_backend_threshold,
                    )
                    .await
                    {
                        tracing::error!("Failed to write known packs to {target_server}: {e}");
                        let msg = format!("§cCould not connect to {target_server}: {e}");
                        let chat_pkt = SystemChatMessagePacket::new(msg, false)
                            .encode_with_version(self.session.protocol_version);
                        let _ = write_packet_with_compression(
                            &mut self.client,
                            &chat_pkt,
                            self.client_compression_threshold,
                        )
                        .await;
                        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                        return Ok(false);
                    }
                    let _ = tokio::io::AsyncWriteExt::flush(&mut new_backend).await;
                } else if packet.id == 0x04 {
                    // KeepAlive in configuration state: echo back
                    if let Err(e) = write_packet_with_compression(
                        &mut new_backend,
                        &packet,
                        new_backend_threshold,
                    )
                    .await
                    {
                        tracing::error!("Failed to echo keepalive to {target_server}: {e}");
                        let msg = format!("§cCould not connect to {target_server}: {e}");
                        let chat_pkt = SystemChatMessagePacket::new(msg, false)
                            .encode_with_version(self.session.protocol_version);
                        let _ = write_packet_with_compression(
                            &mut self.client,
                            &chat_pkt,
                            self.client_compression_threshold,
                        )
                        .await;
                        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                        return Ok(false);
                    }
                    let _ = tokio::io::AsyncWriteExt::flush(&mut new_backend).await;
                } else if packet.id == 0x05 {
                    // Ping in configuration state: reply with Pong (0x05)
                    if let Err(e) = write_packet_with_compression(
                        &mut new_backend,
                        &packet,
                        new_backend_threshold,
                    )
                    .await
                    {
                        tracing::error!("Failed to reply pong to {target_server}: {e}");
                        let msg = format!("§cCould not connect to {target_server}: {e}");
                        let chat_pkt = SystemChatMessagePacket::new(msg, false)
                            .encode_with_version(self.session.protocol_version);
                        let _ = write_packet_with_compression(
                            &mut self.client,
                            &chat_pkt,
                            self.client_compression_threshold,
                        )
                        .await;
                        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                        return Ok(false);
                    }
                    let _ = tokio::io::AsyncWriteExt::flush(&mut new_backend).await;
                } else if packet.id == finish_id {
                    // Backend finished configuration; acknowledge with FinishConfiguration
                    let finish = RawPacket::new(finish_id, bytes::Bytes::new());
                    if let Err(e) = write_packet_with_compression(
                        &mut new_backend,
                        &finish,
                        new_backend_threshold,
                    )
                    .await
                    {
                        tracing::error!(
                            "Failed to acknowledge finish configuration to {target_server}: {e}"
                        );
                        let msg = format!("§cCould not connect to {target_server}: {e}");
                        let chat_pkt = SystemChatMessagePacket::new(msg, false)
                            .encode_with_version(self.session.protocol_version);
                        let _ = write_packet_with_compression(
                            &mut self.client,
                            &chat_pkt,
                            self.client_compression_threshold,
                        )
                        .await;
                        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                        return Ok(false);
                    }
                    let _ = tokio::io::AsyncWriteExt::flush(&mut new_backend).await;
                    break;
                } else if packet.id == disconnect_id {
                    let reason = String::from_utf8_lossy(&packet.payload).to_string();
                    tracing::error!("Backend disconnected during configuration: {reason}");
                    let msg = format!("§cCould not connect to {target_server}: {reason}");
                    let chat_pkt = SystemChatMessagePacket::new(msg, false)
                        .encode_with_version(self.session.protocol_version);
                    let _ = write_packet_with_compression(
                        &mut self.client,
                        &chat_pkt,
                        self.client_compression_threshold,
                    )
                    .await;
                    let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                    return Ok(false);
                }
            }
        }

        // 3. Safely terminate connection to previous backend and clean up client state from old server
        if let Some(mut old_backend) = self.backend.take() {
            let _ = tokio::io::AsyncWriteExt::shutdown(&mut old_backend).await;
        }

        // Clean up client state from old server (screen, audio, scoreboard, bossbar)
        let _ = self.teardown_transferred_state().await;

        // 4. Reattach streams, update current server, and mark transfer pending Login (Play) [R-11]
        self.backend = Some(new_backend);
        self.backend_compression_threshold = new_backend_threshold;
        self.compression_threshold = self.client_compression_threshold;
        self.session.current_server = target_server.to_string();
        self.server_transferred = true;

        tracing::info!(
            player = %self.session.profile.name,
            server = %self.session.current_server,
            "Server transfer handshake completed; awaiting backend Login (Play) packet"
        );

        Ok(true)
    }

    /// Handles an incoming packet from the client.
    /// Intercepts chat commands, evaluates against Rhai, and triggers transfers.
    pub async fn handle_client_packet(&mut self, packet: RawPacket) -> Result<bool, ProxyError> {
        if is_protected_plugin_channel(&packet) {
            tracing::warn!("Dropping client packet on protected channel");
            return Ok(true);
        }

        if is_serverbound_close_container_packet(packet.id, self.session.protocol_version) {
            self.open_container_id = None;
        }

        let is_command_packet =
            ServerboundChatCommand::is_command_packet(packet.id, self.session.protocol_version);

        if is_command_packet {
            if let Ok(cmd_packet) =
                ServerboundChatCommand::decode_with_version(&packet, self.session.protocol_version)
            {
                // If it came as a chat_message packet, only intercept if it begins with '/'
                let is_chat_packet = (self.session.protocol_version < 766 && packet.id == 0x06)
                    || (self.session.protocol_version >= 766 && packet.id == 0x09);
                let should_intercept = !is_chat_packet || cmd_packet.command.starts_with('/');

                if should_intercept {
                    let normalized = cmd_packet.normalized_command();
                    let event = PlayerCommandEvent::with_server(
                        &self.session.profile.name,
                        &normalized,
                        &self.session.current_server,
                    );
                    let cmd_res = self.script_host.eval_command(event).await;

                    // Send notification message if returned by script
                    if !cmd_res.send_message.is_empty() {
                        let chat_pkt = SystemChatMessagePacket::new(&cmd_res.send_message, false)
                            .encode_with_version(self.session.protocol_version);
                        write_packet_with_compression(
                            &mut self.client,
                            &chat_pkt,
                            self.client_compression_threshold,
                        )
                        .await?;
                        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                    }

                    // Handle server reroute if requested
                    if !cmd_res.reroute_server.is_empty() {
                        let _ = self.switch_server(&cmd_res.reroute_server).await?;
                        return Ok(true);
                    }

                    // If command was cancelled (e.g. /server or /stop or /op), drop it
                    if cmd_res.cancel {
                        return Ok(true);
                    }
                }
            }
        }

        // Intercept Tab-Complete / Command Suggestion Requests
        let is_tab_complete = TabCompleteRequestPacket::is_tab_complete_request(
            packet.id,
            self.session.protocol_version,
        );
        if is_tab_complete {
            if let Ok(req) =
                TabCompleteRequestPacket::decode(&packet, self.session.protocol_version)
            {
                let event = PlayerTabCompleteEvent::new(
                    &self.session.profile.name,
                    &req.text,
                    &self.session.current_server,
                );
                let mut suggestions = self.script_host.eval_tab_complete(event).await;

                // Built-in proxy fallback suggestions if plugins returned empty
                if suggestions.is_empty() {
                    let cmd = req.text.as_str();
                    if let Some(arg) = cmd.strip_prefix("/server ") {
                        let arg = arg.trim();
                        for s_name in self.config.servers.keys() {
                            if arg.is_empty() || s_name.starts_with(arg) {
                                suggestions.push(s_name.clone());
                            }
                        }
                    } else if cmd == "/server" {
                        suggestions.push("/server".to_string());
                    } else if cmd == "/" {
                        suggestions.push("/server".to_string());
                        suggestions.push("/hub".to_string());
                        suggestions.push("/lobby".to_string());
                    } else if "/server".starts_with(cmd)
                        || "/hub".starts_with(cmd)
                        || "/lobby".starts_with(cmd)
                    {
                        if "/server".starts_with(cmd) {
                            suggestions.push("/server".to_string());
                        }
                        if "/hub".starts_with(cmd) {
                            suggestions.push("/hub".to_string());
                        }
                        if "/lobby".starts_with(cmd) {
                            suggestions.push("/lobby".to_string());
                        }
                    }
                }

                if !suggestions.is_empty() {
                    let (start, length) = if let Some(last_space) = req.text.rfind(' ') {
                        (
                            (last_space + 1) as i32,
                            (req.text.len() - last_space - 1) as i32,
                        )
                    } else {
                        (0, req.text.len() as i32)
                    };

                    let resp_pkt = TabCompleteResponsePacket::new(
                        req.transaction_id,
                        start,
                        length,
                        suggestions,
                    )
                    .encode_with_version(self.session.protocol_version);

                    write_packet_with_compression(
                        &mut self.client,
                        &resp_pkt,
                        self.client_compression_threshold,
                    )
                    .await?;
                    let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                    return Ok(true);
                }
            }
        }

        // Unintercepted packets forward untouched to current backend [R-02]
        if let Some(ref mut backend) = self.backend {
            write_packet_with_compression(backend, &packet, self.backend_compression_threshold)
                .await?;
            let _ = tokio::io::AsyncWriteExt::flush(backend).await;
        }

        Ok(true)
    }

    /// Handles an incoming packet from the current backend.
    /// Suppresses client kick on Disconnect packet and initiates failsafe fallback [R-11].
    pub async fn handle_backend_packet(&mut self, packet: RawPacket) -> Result<bool, ProxyError> {
        if is_protected_plugin_channel(&packet) {
            tracing::warn!("Dropping backend packet on protected channel");
            return Ok(true);
        }

        // Track open container window ID if backend opens a screen
        if is_open_window_packet(packet.id, self.session.protocol_version) {
            let mut cursor = &packet.payload[..];
            if let Ok(win_id) = decode_varint(&mut cursor) {
                self.open_container_id = Some(win_id as u8);
            }
        } else if packet.id
            == CloseContainerPacket::packet_id_for_version(self.session.protocol_version)
        {
            self.open_container_id = None;
        }

        // Track sound packets to know if audio cleanup is necessary on transfer
        if is_sound_packet(packet.id, self.session.protocol_version) {
            self.has_active_audio = true;
        } else if packet.id == StopSoundPacket::packet_id_for_version(self.session.protocol_version)
            && packet.payload.first().copied() == Some(0)
        {
            self.has_active_audio = false;
        }

        // Track active boss bars
        if packet.id == BossBarPacket::packet_id_for_version(self.session.protocol_version) {
            if let Ok(bb) = BossBarPacket::decode(&packet, self.session.protocol_version) {
                if bb.action == BossBarPacket::ACTION_ADD {
                    self.active_boss_bars.insert(bb.uuid);
                } else if bb.action == BossBarPacket::ACTION_REMOVE {
                    self.active_boss_bars.remove(&bb.uuid);
                }
            }
        }

        // Track active scoreboard objectives
        if packet.id
            == ScoreboardObjectivePacket::packet_id_for_version(self.session.protocol_version)
        {
            if let Ok(obj) =
                ScoreboardObjectivePacket::decode(&packet, self.session.protocol_version)
            {
                if obj.action == ScoreboardObjectivePacket::ACTION_CREATE {
                    self.active_scoreboard_objectives.insert(obj.name);
                } else if obj.action == ScoreboardObjectivePacket::ACTION_REMOVE {
                    self.active_scoreboard_objectives.remove(&obj.name);
                }
            }
        }

        // Track active display objective slots (e.g. sidebar, list, below_name)
        if packet.id == DisplayObjectivePacket::packet_id_for_version(self.session.protocol_version)
        {
            if let Ok(disp) = DisplayObjectivePacket::decode(&packet, self.session.protocol_version)
            {
                if disp.name.is_empty() {
                    self.active_display_slots.remove(&disp.position);
                } else {
                    self.active_display_slots.insert(disp.position);
                }
            }
        }

        // Disconnect packet in Play state is typically 0x1A or 0x1B
        let is_disconnect = packet.id == 0x1A || packet.id == 0x1B;
        if is_disconnect {
            tracing::warn!(
                player = %self.session.profile.name,
                server = %self.session.current_server,
                "Downstream backend disconnected mid-game. Suppressing client kick [R-11]"
            );

            if self.session.current_server != self.config.default_server {
                let fallback = self.config.default_server.clone();
                let notice = format!(
                    "§eDisconnected from {}. Reconnecting to fallback server...",
                    self.session.current_server
                );
                let chat_pkt = SystemChatMessagePacket::new(notice, false)
                    .encode_with_version(self.session.protocol_version);
                let _ = write_packet_with_compression(
                    &mut self.client,
                    &chat_pkt,
                    self.client_compression_threshold,
                )
                .await;
                let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                return self.switch_server(&fallback).await;
            } else {
                return Err(ProxyError::BackendConnectionFailed(
                    "Fallback server disconnected".into(),
                ));
            }
        }

        // Check for Login (Play) packet (0x31 on 775+, 0x2C on 768+, 0x2B on 766+, etc.)
        if is_login_play_packet(packet.id, self.session.protocol_version) {
            tracing::info!(
                player = %self.session.profile.name,
                server = %self.session.current_server,
                packet_id = packet.id,
                server_transferred = self.server_transferred,
                "Received Login (Play) packet from backend"
            );

            // Forward Login (Play) to client first
            write_packet_with_compression(
                &mut self.client,
                &packet,
                self.client_compression_threshold,
            )
            .await?;
            let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;

            // If this is following a mid-session server transfer, immediately follow Login (Play)
            // with a client-bound Respawn packet matching the new world state [R-11].
            // Modern protocols (>= 764) preserve player attributes (0x01) and metadata (0x02) via
            // KEEP_ALL_DATA (0x03) to completely prevent the death/dirt loading screen flash.
            if self.server_transferred {
                self.server_transferred = false;
                let data_kept = if self.session.protocol_version >= 764 {
                    KEEP_ALL_DATA
                } else {
                    0x01
                };
                let respawn_pkt = extract_respawn_from_login_with_data_kept(
                    &packet,
                    self.session.protocol_version,
                    data_kept,
                )
                .unwrap_or_else(|| {
                    let mut reset = RespawnPacket::default_reset();
                    reset.data_kept = data_kept;
                    reset.encode_with_version(self.session.protocol_version)
                });
                write_packet_with_compression(
                    &mut self.client,
                    &respawn_pkt,
                    self.client_compression_threshold,
                )
                .await?;
                let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                tracing::info!(
                    player = %self.session.profile.name,
                    server = %self.session.current_server,
                    data_kept = data_kept,
                    "Dispatched matching Respawn packet to client following server transfer"
                );
            }

            return Ok(true);
        }

        // Intercept Declare Commands packet (0x10 on 770+, 0x11 on < 770) to inject proxy commands into client graph
        if is_declare_commands_packet(packet.id, self.session.protocol_version) {
            let server_names: Vec<String> = self.config.servers.keys().cloned().collect();
            let injected_pkt = inject_proxy_commands_into_declare_commands(
                &packet,
                self.session.protocol_version,
                &server_names,
            )
            .unwrap_or(packet);
            write_packet_with_compression(
                &mut self.client,
                &injected_pkt,
                self.client_compression_threshold,
            )
            .await?;
            let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
            tracing::info!(
                player = %self.session.profile.name,
                "Injected FrameMC proxy commands into DeclareCommands packet sent to client"
            );
            return Ok(true);
        }

        // Normal packet: forward to client
        write_packet_with_compression(&mut self.client, &packet, self.client_compression_threshold)
            .await?;
        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
        Ok(true)
    }

    /// Handles an unexpected EOF from the current backend stream.
    /// In compliance with [R-11], catches disconnect, suppresses client kick, and routes to fallback server.
    pub async fn handle_backend_eof(&mut self) -> Result<bool, ProxyError> {
        tracing::warn!(
            player = %self.session.profile.name,
            server = %self.session.current_server,
            "Backend closed connection unexpectedly. Recovering to default server [R-11]"
        );

        if self.session.current_server != self.config.default_server {
            let fallback = self.config.default_server.clone();
            let notice = format!(
                "§eLost connection to {}. Returning to fallback server...",
                self.session.current_server
            );
            let chat_pkt = SystemChatMessagePacket::new(notice, false)
                .encode_with_version(self.session.protocol_version);
            let _ = write_packet_with_compression(
                &mut self.client,
                &chat_pkt,
                self.client_compression_threshold,
            )
            .await;
            let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
            self.switch_server(&fallback).await
        } else {
            Ok(false)
        }
    }

    /// Forcefully closes any open container GUI on the client.
    pub async fn sanitize_screen(&mut self, window_id: u8) -> Result<(), ProxyError> {
        self.open_container_id = None;
        let pkt =
            CloseContainerPacket::new(window_id).encode_with_version(self.session.protocol_version);
        write_packet_with_compression(&mut self.client, &pkt, self.client_compression_threshold)
            .await?;
        tokio::io::AsyncWriteExt::flush(&mut self.client).await?;
        Ok(())
    }

    /// Stops all audio playing on the client by sending a StopSound packet.
    pub async fn stop_audio(&mut self) -> Result<(), ProxyError> {
        self.has_active_audio = false;
        let pkt = StopSoundPacket::all().encode_with_version(self.session.protocol_version);
        write_packet_with_compression(&mut self.client, &pkt, self.client_compression_threshold)
            .await?;
        tokio::io::AsyncWriteExt::flush(&mut self.client).await?;
        Ok(())
    }

    /// Clears an active scoreboard display slot (e.g. sidebar, list, below_name) on the client.
    pub async fn clear_display_objective(&mut self, position: i32) -> Result<(), ProxyError> {
        self.active_display_slots.remove(&position);
        let pkt = DisplayObjectivePacket::clear(position)
            .encode_with_version(self.session.protocol_version);
        write_packet_with_compression(&mut self.client, &pkt, self.client_compression_threshold)
            .await?;
        tokio::io::AsyncWriteExt::flush(&mut self.client).await?;
        Ok(())
    }

    /// Resets lingering server-side state (container GUIs, audio loops, scoreboards, bossbars).
    pub async fn teardown_transferred_state(&mut self) -> Result<(), ProxyError> {
        if let Some(window_id) = self.open_container_id.take() {
            let close_pkt = CloseContainerPacket::new(window_id)
                .encode_with_version(self.session.protocol_version);
            let _ = write_packet_with_compression(
                &mut self.client,
                &close_pkt,
                self.client_compression_threshold,
            )
            .await;
        }

        if self.has_active_audio {
            self.has_active_audio = false;
            let stop_sound_pkt =
                StopSoundPacket::all().encode_with_version(self.session.protocol_version);
            let _ = write_packet_with_compression(
                &mut self.client,
                &stop_sound_pkt,
                self.client_compression_threshold,
            )
            .await;
        }

        for obj_name in self.active_scoreboard_objectives.drain() {
            let remove_pkt = ScoreboardObjectivePacket::remove(&obj_name)
                .encode_with_version(self.session.protocol_version);
            let _ = write_packet_with_compression(
                &mut self.client,
                &remove_pkt,
                self.client_compression_threshold,
            )
            .await;
        }

        for slot in self.active_display_slots.drain() {
            let clear_pkt = DisplayObjectivePacket::clear(slot)
                .encode_with_version(self.session.protocol_version);
            let _ = write_packet_with_compression(
                &mut self.client,
                &clear_pkt,
                self.client_compression_threshold,
            )
            .await;
        }

        for bossbar_uuid in self.active_boss_bars.drain() {
            let remove_pkt = BossBarPacket::remove(bossbar_uuid)
                .encode_with_version(self.session.protocol_version);
            let _ = write_packet_with_compression(
                &mut self.client,
                &remove_pkt,
                self.client_compression_threshold,
            )
            .await;
        }

        let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
        Ok(())
    }

    /// Runs the Play state packet routing loop until disconnect or shutdown.
    pub async fn run(
        &mut self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), ProxyError> {
        tracing::info!(
            player = %self.session.profile.name,
            server = %self.session.current_server,
            "Entering Play state routing loop with packet inspection"
        );

        loop {
            let backend_stream = match self.backend.as_mut() {
                Some(b) => b,
                None => {
                    tracing::warn!("No active backend in PlayStateMachine");
                    break;
                }
            };

            tokio::select! {
                client_res = read_packet_with_compression(&mut self.client, DEFAULT_MAX_PACKET_SIZE, self.client_compression_threshold) => {
                    match client_res {
                        Ok(packet) => {
                            if !self.handle_client_packet(packet).await? {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::debug!(player = %self.session.profile.name, "Client disconnected from Play state: {e}");
                            break;
                        }
                    }
                }
                backend_res = read_packet_with_compression(backend_stream, DEFAULT_MAX_PACKET_SIZE, self.backend_compression_threshold) => {
                    match backend_res {
                        Ok(packet) => {
                            if is_protected_plugin_channel(&packet) {
                                tracing::warn!("Dropping backend packet on protected channel");
                                continue;
                            }
                            if !self.handle_backend_packet(packet).await? {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                player = %self.session.profile.name,
                                server = %self.session.current_server,
                                "Backend stream error / EOF: {e}"
                            );
                            let recovered = self.handle_backend_eof().await?;
                            if !recovered {
                                break;
                            }
                        }
                    }
                }
                _ = wait_for_shutdown(&mut shutdown_rx) => {
                    tracing::info!(player = %self.session.profile.name, "Shutdown signal received during Play state");
                    let disconnect = DisconnectPacket::new("§cServer is shutting down");
                    let pkt = disconnect.encode_for_client(self.session.protocol_version, true);
                    let _ = write_packet_with_compression(&mut self.client, &pkt, self.client_compression_threshold).await;
                    let _ = tokio::io::AsyncWriteExt::flush(&mut self.client).await;
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    break;
                }
            }
        }

        let _ = tokio::io::AsyncWriteExt::shutdown(&mut self.client).await;
        if let Some(mut backend) = self.backend.take() {
            let _ = tokio::io::AsyncWriteExt::shutdown(&mut backend).await;
        }

        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::network::codec::{read_packet, write_packet};
    use crate::protocol::configuration::FinishConfigurationPacket;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tokio::io::duplex;
    use uuid::Uuid;

    /// Mock backend connector returning pre-configured duplex streams.
    #[allow(clippy::type_complexity)]
    pub struct MockBackendConnector {
        streams: Mutex<HashMap<String, Vec<(Box<dyn AsyncStream>, Option<usize>)>>>,
    }

    impl Default for MockBackendConnector {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockBackendConnector {
        pub fn new() -> Self {
            Self {
                streams: Mutex::new(HashMap::new()),
            }
        }

        pub fn register(&self, server: &str, stream: Box<dyn AsyncStream>) {
            self.register_with_compression(server, stream, None);
        }

        pub fn register_with_compression(
            &self,
            server: &str,
            stream: Box<dyn AsyncStream>,
            threshold: Option<usize>,
        ) {
            let mut lock = self.streams.lock().unwrap();
            lock.entry(server.to_string())
                .or_default()
                .push((stream, threshold));
        }
    }

    impl BackendConnector for MockBackendConnector {
        fn connect<'a>(
            &'a self,
            backend_name: &'a str,
            _backend_config: &'a BackendConfig,
            _profile: &'a PlayerProfile,
            _client_ip: &'a str,
            _protocol_version: i32,
        ) -> BoxFuture<'a, Result<(Box<dyn AsyncStream>, Option<usize>), ProxyError>> {
            Box::pin(async move {
                let mut lock = self.streams.lock().unwrap();
                if let Some(list) = lock.get_mut(backend_name) {
                    if !list.is_empty() {
                        return Ok(list.remove(0));
                    }
                }
                Err(ProxyError::BackendConnectionFailed(format!(
                    "No mock stream for {backend_name}"
                )))
            })
        }
    }

    fn sample_session() -> PlayerSession {
        PlayerSession::new(
            PlayerProfile {
                id: Uuid::parse_str("069a79f4-44e3-4726-a9be-254cc4d37b01").unwrap(),
                name: "Steve".to_string(),
                properties: vec![],
            },
            "127.0.0.1",
            765,
            "lobby",
            SessionRegistryCache::new(),
        )
    }

    fn sample_config() -> ProxyConfig {
        let mut servers = HashMap::new();
        servers.insert(
            "lobby".to_string(),
            BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 25566,
                forwarding_mode: crate::config::ForwardingMode::None,
                forwarding_secret: None,
            },
        );
        servers.insert(
            "steelmc".to_string(),
            BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 25567,
                forwarding_mode: crate::config::ForwardingMode::None,
                forwarding_secret: None,
            },
        );
        ProxyConfig {
            bind_address: "127.0.0.1".to_string(),
            bind_port: 25565,
            motd: "Sample".to_string(),
            max_players: 100,
            online_mode: false,
            favicon: None,
            session_server_url: None,
            servers,
            default_server: "lobby".to_string(),
            script_path: "scripts/main.rhai".to_string(),
            plugins_dir: "plugins".to_string(),
        }
    }

    #[test]
    fn test_chat_command_packet_roundtrip() {
        let cmd = ServerboundChatCommand::new("hub");
        let raw = cmd.encode();
        assert_eq!(raw.id, SERVERBOUND_CHAT_COMMAND_PACKET_ID);

        let decoded = ServerboundChatCommand::decode(&raw).expect("Decode failed");
        assert_eq!(decoded.command, "hub");
        assert_eq!(decoded.normalized_command(), "/hub");
    }

    #[test]
    fn test_respawn_packet_roundtrip() {
        let respawn = RespawnPacket::default_reset();
        let raw = respawn.encode();
        assert_eq!(raw.id, RESPAWN_PACKET_ID);

        let decoded = RespawnPacket::decode(&raw).expect("Decode failed");
        assert_eq!(decoded, respawn);
        assert_eq!(decoded.dimension_type, "minecraft:overworld");
        assert_eq!(decoded.data_kept, 0);
    }

    #[test]
    fn test_system_chat_message_roundtrip() {
        let msg = SystemChatMessagePacket::new("§aWelcome to FrameMC", false);
        let raw = msg.encode();
        assert_eq!(raw.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);

        let decoded = SystemChatMessagePacket::decode(&raw).expect("Decode failed");
        assert_eq!(decoded.message, "§aWelcome to FrameMC");
        assert!(!decoded.overlay);
    }

    #[test]
    fn test_system_chat_message_protocol_776_nbt_roundtrip() {
        let msg =
            SystemChatMessagePacket::new("§6[FrameMC] §eYou are currently on: §asteelmc", false);
        let raw_776 = msg.encode_with_version(776);
        assert_eq!(
            raw_776.id, 0x79,
            "Protocol 776 system chat packet ID must be 0x79 (121)"
        );
        assert_eq!(
            raw_776.payload[0], 0x0A,
            "Must begin with TAG_Compound for anonymous NBT"
        );

        let decoded = SystemChatMessagePacket::decode(&raw_776).expect("Decode 776 failed");
        assert_eq!(
            decoded.message,
            "§6[FrameMC] §eYou are currently on: §asteelmc"
        );
        assert!(!decoded.overlay);
    }

    #[test]
    fn test_respawn_packet_protocol_776_packet_id() {
        let respawn = RespawnPacket::default_reset();
        let raw_776 = respawn.encode_with_version(776);
        assert_eq!(
            raw_776.id, 0x52,
            "Protocol 776 respawn packet ID must be 0x52 (82)"
        );
    }

    #[test]
    fn test_command_packet_detection_across_protocols() {
        assert!(ServerboundChatCommand::is_command_packet(0x05, 776));
        assert!(ServerboundChatCommand::is_command_packet(0x06, 776));
        assert!(ServerboundChatCommand::is_command_packet(0x07, 776));
        assert!(!ServerboundChatCommand::is_command_packet(0x04, 776));

        assert!(ServerboundChatCommand::is_command_packet(0x05, 768));
        assert!(ServerboundChatCommand::is_command_packet(0x04, 765));
    }

    #[tokio::test]
    async fn test_command_interception_and_server_switch() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        // Setup mock target backend for "steelmc"
        let (mut steel_backend, steel_server_end) = duplex(65536);
        connector.register("steelmc", Box::new(steel_server_end));

        // Background task to simulate configuration finish from steelmc backend
        tokio::spawn(async move {
            let finish = FinishConfigurationPacket::new().encode();
            write_packet(&mut steel_backend, &finish)
                .await
                .expect("Write finish config failed");
            let _ = read_packet(&mut steel_backend, 65536).await;
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut steel_backend, &mut buf).await;
        });

        let (client_writer, client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Client issues /steel
        let steel_cmd = ServerboundChatCommand::new("/steel").encode();
        let handled = sm
            .handle_client_packet(steel_cmd)
            .await
            .expect("Handle client packet failed");
        assert!(handled);

        // Verify state machine updated current server to steelmc
        assert_eq!(sm.session.current_server, "steelmc");

        // Verify client received system message
        let mut reader = client_reader;
        let pkt1 = read_packet(&mut reader, 65536)
            .await
            .expect("Read pkt1 failed");
        assert_eq!(pkt1.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);
        let chat = SystemChatMessagePacket::decode(&pkt1).unwrap();
        assert!(chat.message.contains("SteelMC"));

        assert!(sm.server_transferred);

        // Backend sends Login (Play) (0x29 on 765)
        let login_pkt = RawPacket::new(
            0x29,
            Bytes::from_static(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        );
        sm.handle_backend_packet(login_pkt)
            .await
            .expect("Handle backend login failed");

        // Client receives Login (Play) first
        let pkt2 = read_packet(&mut reader, 65536)
            .await
            .expect("Read pkt2 failed");
        assert_eq!(pkt2.id, 0x29);

        // Client receives Respawn immediately following Login (Play)
        let pkt3 = read_packet(&mut reader, 65536)
            .await
            .expect("Read pkt3 failed");
        assert_eq!(pkt3.id, RESPAWN_PACKET_ID);
        assert!(!sm.server_transferred);
    }

    #[tokio::test]
    async fn test_command_passthrough_untouched() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        let (client_writer, _client_reader) = duplex(65536);
        let (mut backend_reader, backend_writer) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(backend_writer),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Client issues /msg Notch Hello
        let msg_cmd = ServerboundChatCommand::new("/msg Notch Hello").encode();
        sm.handle_client_packet(msg_cmd.clone())
            .await
            .expect("Handle packet failed");

        // Assert backend received the exact uncancelled command packet untouched
        let received = read_packet(&mut backend_reader, 65536)
            .await
            .expect("Backend read failed");
        assert_eq!(received, msg_cmd);
    }

    #[tokio::test]
    async fn test_command_cancellation_with_message() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        let (client_writer, mut client_reader) = duplex(65536);
        let (_backend_reader, backend_writer) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(backend_writer),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Client issues unauthorized /stop
        let stop_cmd = ServerboundChatCommand::new("/stop").encode();
        sm.handle_client_packet(stop_cmd)
            .await
            .expect("Handle packet failed");

        // Client receives permission denied message
        let pkt = read_packet(&mut client_reader, 65536)
            .await
            .expect("Read client failed");
        assert_eq!(pkt.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);
        let chat = SystemChatMessagePacket::decode(&pkt).unwrap();
        assert!(chat.message.contains("permission"));
    }

    #[tokio::test]
    async fn test_unexpected_backend_disconnect_failsafe_reroute() {
        let host = Arc::new(ScriptHost::new());
        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        // Setup mock fallback stream for "lobby"
        let (mut lobby_backend, lobby_server_end) = duplex(65536);
        connector.register("lobby", Box::new(lobby_server_end));

        tokio::spawn(async move {
            let finish = FinishConfigurationPacket::new().encode();
            write_packet(&mut lobby_backend, &finish)
                .await
                .expect("Write finish config failed");
            let _ = read_packet(&mut lobby_backend, 65536).await;
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut lobby_backend, &mut buf).await;
        });

        let (client_writer, mut client_reader) = duplex(65536);
        let (_steel_reader, steel_writer) = duplex(65536);

        let mut session = sample_session();
        session.current_server = "steelmc".to_string(); // Connected to steelmc

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(steel_writer),
            connector,
            session,
            config,
            host,
            None,
        );

        // SteelMC unexpectedly disconnects
        let recovered = sm.handle_backend_eof().await.expect("Handle EOF failed");
        assert!(recovered);

        // Assert player rerouted to fallback "lobby" without kick [R-11]
        assert_eq!(sm.session.current_server, "lobby");

        // Client receives fallback notification
        let pkt1 = read_packet(&mut client_reader, 65536)
            .await
            .expect("Read client pkt1 failed");
        assert_eq!(pkt1.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);
        let chat = SystemChatMessagePacket::decode(&pkt1).unwrap();
        assert!(chat.message.contains("fallback"));

        assert!(sm.server_transferred);

        // Fallback server sends Login (Play) (0x29 on 765)
        let login_pkt = RawPacket::new(
            0x29,
            Bytes::from_static(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        );
        sm.handle_backend_packet(login_pkt)
            .await
            .expect("Handle backend login failed");

        let pkt2 = read_packet(&mut client_reader, 65536)
            .await
            .expect("Read client pkt2 failed");
        assert_eq!(pkt2.id, 0x29);

        let pkt3 = read_packet(&mut client_reader, 65536)
            .await
            .expect("Read client pkt3 failed");
        assert_eq!(pkt3.id, RESPAWN_PACKET_ID);
        assert!(!sm.server_transferred);
    }

    #[tokio::test]
    async fn test_server_transfer_with_different_compression_thresholds() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        // Target server "steelmc" has NO compression (threshold = None)
        let (mut steel_backend, steel_server_end) = duplex(65536);
        connector.register_with_compression("steelmc", Box::new(steel_server_end), None);

        // Background task on steel_backend: sends uncompressed FinishConfiguration
        tokio::spawn(async move {
            let finish = FinishConfigurationPacket::new().encode();
            write_packet(&mut steel_backend, &finish)
                .await
                .expect("Write finish config failed");
            let _ = read_packet(&mut steel_backend, 65536).await; // reads uncompressed ack
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut steel_backend, &mut buf).await;
        });

        let (client_writer, client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        // Client and initial backend ("lobby") have compression threshold = Some(256)
        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            Some(256),
        );

        // Client issues /steel
        let steel_cmd = ServerboundChatCommand::new("/steel").encode();
        let handled = sm
            .handle_client_packet(steel_cmd)
            .await
            .expect("Handle client packet failed");
        assert!(handled);

        assert_eq!(sm.session.current_server, "steelmc");
        assert_eq!(sm.client_compression_threshold, Some(256));
        assert_eq!(sm.backend_compression_threshold, None);

        // Verify client received system message WITH compression (Some(256))
        let mut reader = client_reader;
        let pkt1 = read_packet_with_compression(&mut reader, 65536, Some(256))
            .await
            .expect("Read pkt1 failed");
        assert_eq!(pkt1.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);

        assert!(sm.server_transferred);

        // Target server sends Login (Play) (0x29 on 765)
        let login_pkt = RawPacket::new(
            0x29,
            Bytes::from_static(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        );
        sm.handle_backend_packet(login_pkt)
            .await
            .expect("Handle backend login failed");

        // Client receives Login (Play) with client compression
        let pkt2 = read_packet_with_compression(&mut reader, 65536, Some(256))
            .await
            .expect("Read pkt2 failed");
        assert_eq!(pkt2.id, 0x29);

        // Client receives Respawn with client compression
        let pkt3 = read_packet_with_compression(&mut reader, 65536, Some(256))
            .await
            .expect("Read pkt3 failed");
        assert_eq!(pkt3.id, RESPAWN_PACKET_ID);
        assert!(!sm.server_transferred);
    }

    #[test]
    fn test_is_login_play_packet_across_versions() {
        assert!(is_login_play_packet(0x31, 776));
        assert!(is_login_play_packet(0x31, 775));
        assert!(is_login_play_packet(0x2C, 768));
        assert!(is_login_play_packet(0x2C, 774));
        assert!(is_login_play_packet(0x2B, 766));
        assert!(is_login_play_packet(0x2B, 767));
        assert!(is_login_play_packet(0x29, 764));
        assert!(is_login_play_packet(0x29, 765));
        assert!(is_login_play_packet(0x28, 763));
        assert!(!is_login_play_packet(0x00, 776));
        assert!(!is_login_play_packet(0x52, 776));
    }

    #[test]
    fn test_extract_respawn_from_login_modern_776() {
        // Construct synthetic Login (Play) packet for version 776 (id = 0x31)
        let mut payload = BytesMut::new();
        // 1. entity_id: i32 (4 bytes)
        payload.put_i32(42);
        // 2. is_hardcore: bool (1 byte)
        payload.put_u8(0);
        // 3. dimension_names: VarInt count + string
        encode_varint(1, &mut payload);
        encode_varint(19, &mut payload);
        payload.put_slice(b"minecraft:overworld");
        // 4. max_players: VarInt
        encode_varint(100, &mut payload);
        // 5. view_distance: VarInt
        encode_varint(12, &mut payload);
        // 6. simulation_distance: VarInt
        encode_varint(10, &mut payload);
        // 7. reduced_debug_info: bool
        payload.put_u8(0);
        // 8. show_respawn_screen: bool
        payload.put_u8(1);
        // 9. do_limited_crafting: bool
        payload.put_u8(0);

        // 10. Common Player Spawn Info:
        let spawn_info_start = payload.len();
        // dimension: VarInt (index 0)
        encode_varint(0, &mut payload);
        // dimension_name: string
        encode_varint(19, &mut payload);
        payload.put_slice(b"minecraft:overworld");
        // hashed_seed: i64
        payload.put_i64(9876543210i64);
        // gamemode: u8
        payload.put_u8(0);
        // previous_gamemode: i8
        payload.put_i8(-1);
        // is_debug: bool
        payload.put_u8(0);
        // is_flat: bool
        payload.put_u8(0);
        // death_location: bool
        payload.put_u8(0);
        // portal_cooldown: VarInt
        encode_varint(0, &mut payload);
        // sea_level: VarInt
        encode_varint(63, &mut payload);
        let spawn_info_end = payload.len();

        // 11. online_mode: bool (1 byte, 26.2+ / protocol 776)
        payload.put_u8(0);
        // 12. enforces_secure_chat: bool (1 byte)
        payload.put_u8(0);

        let login_pkt = RawPacket::new(0x31, payload.freeze());
        let expected_spawn_info = &login_pkt.payload[spawn_info_start..spawn_info_end];

        let respawn_pkt =
            extract_respawn_from_login(&login_pkt, 776).expect("Respawn extraction failed");
        assert_eq!(respawn_pkt.id, 0x52); // Respawn ID on protocol >= 775
        assert_eq!(respawn_pkt.payload.len(), expected_spawn_info.len() + 1);
        assert_eq!(
            &respawn_pkt.payload[..expected_spawn_info.len()],
            expected_spawn_info
        );
        assert_eq!(respawn_pkt.payload[expected_spawn_info.len()], 0x00); // data_kept = 0
    }

    #[test]
    fn test_tab_complete_request_packet_encode_decode() {
        for proto in [765, 768, 776] {
            let original = TabCompleteRequestPacket::new(123, "/server ");
            let raw = original.encode_with_version(proto);
            assert!(TabCompleteRequestPacket::is_tab_complete_request(
                raw.id, proto
            ));
            let decoded = TabCompleteRequestPacket::decode(&raw, proto).expect("decode failed");
            assert_eq!(decoded.transaction_id, 123);
            assert_eq!(decoded.text, "/server ");
        }
    }

    #[test]
    fn test_tab_complete_response_packet_encode_decode() {
        for proto in [765, 768, 776] {
            let original = TabCompleteResponsePacket::new(
                456,
                8,
                0,
                vec!["lobby".to_string(), "steelmc".to_string()],
            );
            let raw = original.encode_with_version(proto);
            let decoded = TabCompleteResponsePacket::decode(&raw, proto).expect("decode failed");
            assert_eq!(decoded.transaction_id, 456);
            assert_eq!(decoded.start, 8);
            assert_eq!(decoded.length, 0);
            assert_eq!(decoded.matches, vec!["lobby", "steelmc"]);
        }
    }

    #[tokio::test]
    async fn test_play_state_machine_tab_complete_interception() {
        let (client_remote, mut client_local) = duplex(1024);
        let (backend_remote, mut backend_local) = duplex(1024);

        let connector = Arc::new(MockBackendConnector::new());
        let mut session = sample_session();
        session.protocol_version = 776;
        let config = Arc::new(sample_config());
        let script_host = Arc::new(ScriptHost::new());
        script_host.set_servers(
            vec!["lobby".to_string(), "steelmc".to_string()],
            "lobby".to_string(),
        );

        let mut sm = PlayStateMachine::new(
            Box::new(client_remote),
            Box::new(backend_remote),
            connector,
            session,
            config,
            script_host,
            None,
        );

        // Client requests tab completion for "/server "
        let req_pkt = TabCompleteRequestPacket::new(77, "/server ").encode_with_version(776);
        let handled = sm
            .handle_client_packet(req_pkt)
            .await
            .expect("handle_client_packet failed");
        assert!(handled);

        // Client should receive the TabCompleteResponse packet
        let resp_raw = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            read_packet(&mut client_local, DEFAULT_MAX_PACKET_SIZE),
        )
        .await
        .expect("timed out waiting for client response")
        .expect("client did not receive response");

        let resp = TabCompleteResponsePacket::decode(&resp_raw, 776)
            .expect("failed to decode response packet");
        assert_eq!(resp.transaction_id, 77);
        assert_eq!(resp.start, 8);
        assert_eq!(resp.length, 0);
        assert!(resp.matches.contains(&"lobby".to_string()));
        assert!(resp.matches.contains(&"steelmc".to_string()));

        // Backend stream should have received NOTHING
        let backend_recv = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            read_packet(&mut backend_local, DEFAULT_MAX_PACKET_SIZE),
        )
        .await;
        assert!(
            backend_recv.is_err(),
            "Backend should not have received any packets"
        );
    }

    #[tokio::test]
    async fn test_play_state_machine_tab_complete_passthrough() {
        let (client_remote, _client_local) = duplex(1024);
        let (backend_remote, mut backend_local) = duplex(1024);

        let connector = Arc::new(MockBackendConnector::new());
        let mut session = sample_session();
        session.protocol_version = 776;
        let config = Arc::new(sample_config());
        let script_host = Arc::new(ScriptHost::new());

        let mut sm = PlayStateMachine::new(
            Box::new(client_remote),
            Box::new(backend_remote),
            connector,
            session,
            config,
            script_host,
            None,
        );

        // Client requests tab completion for an unintercepted vanilla command
        let req_pkt =
            TabCompleteRequestPacket::new(88, "/give @p diamond ").encode_with_version(776);
        let handled = sm
            .handle_client_packet(req_pkt.clone())
            .await
            .expect("handle_client_packet failed");
        assert!(handled);

        // Backend stream MUST receive the packet untouched [R-02]
        let forwarded = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            read_packet(&mut backend_local, DEFAULT_MAX_PACKET_SIZE),
        )
        .await
        .expect("timed out waiting for forwarded packet")
        .expect("backend did not receive forwarded packet");

        assert_eq!(forwarded.id, req_pkt.id);
        assert_eq!(forwarded.payload, req_pkt.payload);
    }

    #[test]
    fn test_inject_proxy_commands_into_declare_commands() {
        // Minimal synthetic DeclareCommands packet:
        // Node 0: Root (flags = 0, child_count = 1, children = [1])
        // Node 1: Literal "help" (flags = 0x05, child_count = 0, name = "help")
        // rootIndex: 0
        let mut original_payload = BytesMut::new();
        encode_varint(2, &mut original_payload); // 2 nodes
                                                 // Node 0
        original_payload.put_u8(0x00);
        encode_varint(1, &mut original_payload); // 1 child
        encode_varint(1, &mut original_payload); // child index 1
                                                 // Node 1 ("help")
        original_payload.put_u8(0x05); // literal, executable
        encode_varint(0, &mut original_payload); // 0 children
        encode_varint(4, &mut original_payload);
        original_payload.put_slice(b"help");
        // rootIndex
        original_payload.put_u8(0x00);

        let orig_pkt = RawPacket::new(0x10, original_payload.freeze());
        let injected =
            inject_proxy_commands_into_declare_commands(&orig_pkt, 776, &["steelmc".to_string()])
                .expect("injection failed");

        let mut cursor = &injected.payload[..];
        let new_count = decode_varint(&mut cursor).unwrap();
        // 2 original + 6 new (server, name, hub, lobby, steel, steelmc) = 8
        assert_eq!(new_count, 8);

        // Node 0
        assert_eq!(cursor.get_u8(), 0x00);
        let root_children_count = decode_varint(&mut cursor).unwrap();
        // 1 original + 5 root commands = 6
        assert_eq!(root_children_count, 6);
        let mut root_children = Vec::new();
        for _ in 0..root_children_count {
            root_children.push(decode_varint(&mut cursor).unwrap());
        }
        assert_eq!(root_children[0], 1); // "help"
        assert_eq!(root_children[1], 2); // "server"
        assert_eq!(root_children[2], 4); // "hub"
        assert_eq!(root_children[3], 5); // "lobby"
        assert_eq!(root_children[4], 6); // "steel"
        assert_eq!(root_children[5], 7); // "steelmc"

        // Last byte of injected packet must be rootIndex = 0
        assert_eq!(injected.payload[injected.payload.len() - 1], 0x00);
    }

    #[test]
    fn test_is_login_play_packet_comprehensive() {
        assert!(is_login_play_packet(0x31, 776));
        assert!(is_login_play_packet(0x31, 775));
        assert!(is_login_play_packet(0x2C, 768));
        assert!(is_login_play_packet(0x2B, 766));
        assert!(is_login_play_packet(0x29, 764));
        assert!(is_login_play_packet(0x28, 762));
        assert!(is_login_play_packet(0x24, 761));
        assert!(is_login_play_packet(0x25, 759));
        assert!(is_login_play_packet(0x26, 755));
        assert!(is_login_play_packet(0x24, 735));
        assert!(is_login_play_packet(0x26, 573)); // 1.15
        assert!(is_login_play_packet(0x25, 393)); // 1.13
        assert!(is_login_play_packet(0x23, 340)); // 1.12.2 Join Game
        assert!(!is_login_play_packet(0x26, 340)); // 1.12.2 Entity Look & Relative Move is NOT Join Game
        assert!(!is_login_play_packet(0x24, 340)); // 1.12.2 Map is NOT Join Game
        assert!(!is_login_play_packet(0x25, 340)); // 1.12.2 Entity Relative Move is NOT Join Game
        assert!(!is_login_play_packet(0x00, 776));
        assert!(!is_login_play_packet(0x99, 761));
    }

    #[test]
    fn test_system_chat_packet_ids_across_versions() {
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(776), 0x79);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(775), 0x79);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(768), 0x73);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(766), 0x6C);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(764), 0x69);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(763), 0x67);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(762), 0x64);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(761), 0x60);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(759), 0x5F);
        assert_eq!(SystemChatMessagePacket::packet_id_for_version(754), 0x69);

        // Verify valid packet ids decoded
        for (proto, id) in [
            (776, 0x79),
            (768, 0x73),
            (766, 0x6C),
            (764, 0x69),
            (762, 0x64),
            (761, 0x60),
            (759, 0x5F),
            (754, 0x69),
        ] {
            let msg = SystemChatMessagePacket::new("Test message", false);
            let encoded = msg.encode_with_version(proto);
            assert_eq!(encoded.id, id);
            let decoded = SystemChatMessagePacket::decode_with_version(&encoded, proto).unwrap();
            assert_eq!(decoded.message, "Test message");
        }
    }

    #[test]
    fn test_respawn_packet_ids_across_versions() {
        assert_eq!(RespawnPacket::packet_id_for_version(776), 0x52);
        assert_eq!(RespawnPacket::packet_id_for_version(775), 0x52);
        assert_eq!(RespawnPacket::packet_id_for_version(768), 0x4C);
        assert_eq!(RespawnPacket::packet_id_for_version(766), 0x47);
        assert_eq!(RespawnPacket::packet_id_for_version(764), 0x45);
        assert_eq!(RespawnPacket::packet_id_for_version(762), 0x43);
        assert_eq!(RespawnPacket::packet_id_for_version(761), 0x3F);
        assert_eq!(RespawnPacket::packet_id_for_version(759), 0x3E);
        assert_eq!(RespawnPacket::packet_id_for_version(755), 0x3D);
        assert_eq!(RespawnPacket::packet_id_for_version(751), 0x39);
        assert_eq!(RespawnPacket::packet_id_for_version(340), 0x35);
        assert_eq!(RespawnPacket::packet_id_for_version(338), 0x45);

        // Test modern decode (776)
        let respawn = RespawnPacket::default_reset();
        let encoded_776 = respawn.encode_with_version(776);
        assert_eq!(encoded_776.id, 0x52);
        let decoded_776 = RespawnPacket::decode_with_version(&encoded_776, 776).unwrap();
        assert_eq!(decoded_776.dimension_name, "minecraft:overworld");
        assert_eq!(decoded_776.data_kept, 0);

        // Test legacy decode (765)
        let encoded_765 = respawn.encode_with_version(765);
        assert_eq!(encoded_765.id, 0x45);
        let decoded_765 = RespawnPacket::decode_with_version(&encoded_765, 765).unwrap();
        assert_eq!(decoded_765.dimension_name, "minecraft:overworld");
        assert_eq!(decoded_765.data_kept, 0);
    }

    #[test]
    fn test_inject_proxy_commands_with_multibyte_root_index() {
        // Construct synthetic DeclareCommands packet where rootIndex is 200 (encoded as 0xC8, 0x01)
        let mut original_payload = BytesMut::new();
        encode_varint(250, &mut original_payload); // 250 nodes
                                                   // Node 0
        original_payload.put_u8(0x00);
        encode_varint(1, &mut original_payload);
        encode_varint(1, &mut original_payload);
        // Node 1
        original_payload.put_u8(0x05);
        encode_varint(0, &mut original_payload);
        encode_varint(4, &mut original_payload);
        original_payload.put_slice(b"help");

        // Trailing rootIndex = 200 (requires 2 bytes)
        encode_varint(200, &mut original_payload);

        let orig_pkt = RawPacket::new(0x10, original_payload.freeze());
        let injected =
            inject_proxy_commands_into_declare_commands(&orig_pkt, 776, &["lobby".to_string()])
                .expect("injection failed");

        // Verify root index is preserved at end as 200
        let mut tail_slice = &injected.payload[injected.payload.len() - 2..];
        let decoded_root_index = decode_varint(&mut tail_slice).expect("decode root index");
        assert_eq!(decoded_root_index, 200);
        assert!(tail_slice.is_empty());
    }

    #[tokio::test]
    async fn test_protected_plugin_channel_dropped() {
        let (client_writer, _client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());
        let host = Arc::new(ScriptHost::new());

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Create packet on "velocity:player_info"
        let mut payload = BytesMut::new();
        let channel = "velocity:player_info";
        encode_varint(channel.len() as i32, &mut payload);
        payload.put_slice(channel.as_bytes());
        payload.put_slice(b"secret payload data");
        let pkt = RawPacket::new(0x17, payload.freeze());

        // Clientbound packet should be dropped
        let handled_client = sm.handle_client_packet(pkt.clone()).await.unwrap();
        assert!(handled_client);

        // Backend packet should be dropped
        let handled_backend = sm.handle_backend_packet(pkt).await.unwrap();
        assert!(handled_backend);
    }

    #[tokio::test]
    async fn test_switch_server_graceful_connect_failure() {
        let (client_writer, mut client_reader) = duplex(65536);
        let (mut backend_writer, lobby_backend) = duplex(65536);

        let mut config = ProxyConfig::default();
        config.servers.insert(
            "offline_server".to_string(),
            crate::config::BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 59999,
                forwarding_mode: crate::config::ForwardingMode::None,
                forwarding_secret: None,
            },
        );
        let config = Arc::new(config);
        let connector = Arc::new(MockBackendConnector::new()); // doesn't have "offline_server" registered
        let host = Arc::new(ScriptHost::new());

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Try switching to offline server
        let switched = sm.switch_server("offline_server").await.unwrap();
        assert!(
            !switched,
            "switch_server must return Ok(false) on connection failure"
        );

        // Verify client received error chat notification
        let chat_raw = read_packet(&mut client_reader, 65536)
            .await
            .expect("Read chat error");
        let chat = SystemChatMessagePacket::decode(&chat_raw).unwrap();
        assert!(chat.message.contains("Could not connect to offline_server"));

        // Verify backend is still intact
        assert!(sm.backend.is_some());
        // Verify backend can still receive packets
        let dummy = RawPacket::new(0x00, bytes::Bytes::from_static(b"test"));
        write_packet(&mut backend_writer, &dummy).await.unwrap();
    }

    #[test]
    fn test_inject_proxy_commands_malformed_root_index_unmodified() {
        let mut original_payload = BytesMut::new();
        encode_varint(10, &mut original_payload);
        original_payload.put_u8(0x00); // flags
        encode_varint(0, &mut original_payload); // 0 children
                                                 // Malformed trailing byte with MSB set (invalid VarInt terminator)
        original_payload.put_u8(0x80);

        let orig_pkt = RawPacket::new(0x10, original_payload.freeze());
        let result =
            inject_proxy_commands_into_declare_commands(&orig_pkt, 776, &["lobby".to_string()])
                .expect("should not error");
        assert_eq!(result.payload, orig_pkt.payload);
    }

    #[test]
    fn test_inject_proxy_commands_legacy_version_unmodified() {
        let mut original_payload = BytesMut::new();
        encode_varint(10, &mut original_payload);
        original_payload.put_u8(0x00);
        encode_varint(0, &mut original_payload);
        encode_varint(0, &mut original_payload);

        let orig_pkt = RawPacket::new(0x11, original_payload.freeze());
        // Protocol 340 (1.12.2) does not support DeclareCommands
        let result =
            inject_proxy_commands_into_declare_commands(&orig_pkt, 340, &["lobby".to_string()])
                .expect("should not error");
        assert_eq!(result.payload, orig_pkt.payload);
    }

    #[tokio::test]
    async fn test_switch_server_configuration_timeout() {
        tokio::time::pause();

        let (client_writer, mut client_reader) = duplex(65536);
        let (_lobby_backend_writer, lobby_backend) = duplex(65536);
        let (_hanging_backend_writer, hanging_backend) = duplex(65536);

        let mut config = ProxyConfig::default();
        config.servers.insert(
            "hanging_server".to_string(),
            crate::config::BackendConfig {
                address: "127.0.0.1".to_string(),
                port: 59998,
                forwarding_mode: crate::config::ForwardingMode::None,
                forwarding_secret: None,
            },
        );
        let config = Arc::new(config);
        let connector = Arc::new(MockBackendConnector::new());
        connector.register("hanging_server", Box::new(hanging_backend));
        let host = Arc::new(ScriptHost::new());

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        let switch_task = tokio::spawn(async move { sm.switch_server("hanging_server").await });

        // Advance simulated time past the 10-second timeout
        tokio::time::advance(std::time::Duration::from_secs(11)).await;

        let switched = switch_task
            .await
            .unwrap()
            .expect("switch_server should not fatal error");
        assert!(!switched, "switch_server must return Ok(false) on timeout");

        // Verify client received timeout chat notification
        let chat_raw = read_packet(&mut client_reader, 65536)
            .await
            .expect("Read chat error");
        let chat = SystemChatMessagePacket::decode(&chat_raw).unwrap();
        assert!(chat.message.contains("connection timed out"));
    }

    #[test]
    fn test_close_container_packet_codecs_across_versions() {
        for proto in [759, 761, 762, 763, 764, 765, 766, 768, 775, 776] {
            let pkt = CloseContainerPacket::new(5);
            let raw = pkt.encode_with_version(proto);
            assert_eq!(raw.id, CloseContainerPacket::packet_id_for_version(proto));
            let decoded = CloseContainerPacket::decode(&raw, proto).expect("decode failed");
            assert_eq!(decoded.window_id, 5);
        }

        assert_eq!(CloseContainerPacket::packet_id_for_version(776), 0x12);
        assert_eq!(CloseContainerPacket::packet_id_for_version(775), 0x12);
        assert_eq!(CloseContainerPacket::packet_id_for_version(768), 0x12);
        assert_eq!(CloseContainerPacket::packet_id_for_version(765), 0x12);
        assert_eq!(CloseContainerPacket::packet_id_for_version(761), 0x0F);

        // Invalid packet ID returns error
        let bad_raw = RawPacket::new(0x00, Bytes::from_static(&[5]));
        assert!(CloseContainerPacket::decode(&bad_raw, 765).is_err());

        // Empty payload returns error
        let empty_raw = RawPacket::new(0x12, Bytes::new());
        assert!(CloseContainerPacket::decode(&empty_raw, 765).is_err());
    }

    #[test]
    fn test_stop_sound_packet_codecs_across_versions() {
        for proto in [759, 761, 763, 764, 765, 766, 768, 775, 776] {
            // Test all sounds stop
            let stop_all = StopSoundPacket::all();
            let raw_all = stop_all.encode_with_version(proto);
            assert_eq!(raw_all.id, StopSoundPacket::packet_id_for_version(proto));
            assert_eq!(&raw_all.payload[..], &[0x00]);
            let dec_all = StopSoundPacket::decode(&raw_all, proto).expect("decode all failed");
            assert_eq!(dec_all.flags, 0);
            assert_eq!(dec_all.source, None);
            assert_eq!(dec_all.sound, None);

            // Test with source
            let stop_src = StopSoundPacket::with_source(2);
            let raw_src = stop_src.encode_with_version(proto);
            let dec_src = StopSoundPacket::decode(&raw_src, proto).expect("decode source failed");
            assert_eq!(dec_src.flags, 1);
            assert_eq!(dec_src.source, Some(2));
            assert_eq!(dec_src.sound, None);

            // Test with sound identifier
            let stop_snd = StopSoundPacket::with_sound("minecraft:music.game");
            let raw_snd = stop_snd.encode_with_version(proto);
            let dec_snd = StopSoundPacket::decode(&raw_snd, proto).expect("decode sound failed");
            assert_eq!(dec_snd.flags, 2);
            assert_eq!(dec_snd.source, None);
            assert_eq!(dec_snd.sound.as_deref(), Some("minecraft:music.game"));

            // Test with both source and sound identifier
            let stop_both = StopSoundPacket::with_source_and_sound(1, "minecraft:ambient.cave");
            let raw_both = stop_both.encode_with_version(proto);
            let dec_both = StopSoundPacket::decode(&raw_both, proto).expect("decode both failed");
            assert_eq!(dec_both.flags, 3);
            assert_eq!(dec_both.source, Some(1));
            assert_eq!(dec_both.sound.as_deref(), Some("minecraft:ambient.cave"));
        }

        assert_eq!(StopSoundPacket::packet_id_for_version(776), 0x71);
        assert_eq!(StopSoundPacket::packet_id_for_version(768), 0x71);
        assert_eq!(StopSoundPacket::packet_id_for_version(766), 0x6A);
        assert_eq!(StopSoundPacket::packet_id_for_version(765), 0x68);
        assert_eq!(StopSoundPacket::packet_id_for_version(764), 0x66);
    }

    #[test]
    fn test_boss_bar_packet_codecs_across_versions() {
        let test_uuid = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        for proto in [760, 762, 764, 765, 768, 775, 776] {
            let pkt = BossBarPacket::remove(test_uuid);
            let raw = pkt.encode_with_version(proto);
            assert_eq!(raw.id, BossBarPacket::packet_id_for_version(proto));
            let decoded = BossBarPacket::decode(&raw, proto).expect("decode bossbar failed");
            assert_eq!(decoded.uuid, test_uuid);
            assert_eq!(decoded.action, BossBarPacket::ACTION_REMOVE);
        }

        assert_eq!(BossBarPacket::packet_id_for_version(776), 0x0A);
        assert_eq!(BossBarPacket::packet_id_for_version(768), 0x0A);
        assert_eq!(BossBarPacket::packet_id_for_version(763), 0x0B);

        // Truncated UUID error
        let truncated = RawPacket::new(0x0A, Bytes::from_static(&[1, 2, 3]));
        assert!(BossBarPacket::decode(&truncated, 765).is_err());
    }

    #[test]
    fn test_scoreboard_objective_codecs_across_versions() {
        for proto in [759, 761, 763, 764, 765, 766, 768, 775, 776] {
            let pkt = ScoreboardObjectivePacket::remove("sidebar_stats");
            let raw = pkt.encode_with_version(proto);
            assert_eq!(
                raw.id,
                ScoreboardObjectivePacket::packet_id_for_version(proto)
            );
            let decoded =
                ScoreboardObjectivePacket::decode(&raw, proto).expect("decode scoreboard failed");
            assert_eq!(decoded.name, "sidebar_stats");
            assert_eq!(decoded.action, ScoreboardObjectivePacket::ACTION_REMOVE);
        }

        assert_eq!(ScoreboardObjectivePacket::packet_id_for_version(776), 0x64);
        assert_eq!(ScoreboardObjectivePacket::packet_id_for_version(768), 0x64);
        assert_eq!(ScoreboardObjectivePacket::packet_id_for_version(766), 0x5E);
        assert_eq!(ScoreboardObjectivePacket::packet_id_for_version(765), 0x5C);
        assert_eq!(ScoreboardObjectivePacket::packet_id_for_version(764), 0x5A);
    }

    #[test]
    fn test_display_objective_codecs_across_versions() {
        for proto in [759, 761, 763, 764, 765, 766, 768, 775, 776] {
            let pkt = DisplayObjectivePacket::clear(DisplayObjectivePacket::POSITION_SIDEBAR);
            let raw = pkt.encode_with_version(proto);
            assert_eq!(raw.id, DisplayObjectivePacket::packet_id_for_version(proto));
            let decoded = DisplayObjectivePacket::decode(&raw, proto)
                .expect("decode display objective failed");
            assert_eq!(decoded.position, 1);
            assert_eq!(decoded.name, "");
        }

        assert_eq!(DisplayObjectivePacket::packet_id_for_version(776), 0x5C);
        assert_eq!(DisplayObjectivePacket::packet_id_for_version(768), 0x5C);
        assert_eq!(DisplayObjectivePacket::packet_id_for_version(766), 0x57);
        assert_eq!(DisplayObjectivePacket::packet_id_for_version(765), 0x55);
        assert_eq!(DisplayObjectivePacket::packet_id_for_version(764), 0x53);
    }

    #[test]
    fn test_extract_respawn_from_login_with_data_kept_modern() {
        let mut payload = BytesMut::new();
        payload.put_i32(100); // entity_id
        payload.put_u8(0); // is_hardcore
        encode_varint(1, &mut payload); // dim count
        encode_varint(19, &mut payload);
        payload.put_slice(b"minecraft:overworld");
        encode_varint(50, &mut payload); // max_players
        encode_varint(16, &mut payload); // view_distance
        encode_varint(12, &mut payload); // simulation_distance
        payload.put_u8(0); // reduced_debug_info
        payload.put_u8(1); // show_respawn_screen
        payload.put_u8(0); // do_limited_crafting

        // SpawnInfo:
        let spawn_start = payload.len();
        encode_varint(0, &mut payload); // dim
        encode_varint(19, &mut payload);
        payload.put_slice(b"minecraft:overworld");
        payload.put_i64(123456789i64); // hashed_seed
        payload.put_u8(0); // gamemode
        payload.put_i8(-1); // prev gamemode
        payload.put_u8(0); // is_debug
        payload.put_u8(0); // is_flat
        payload.put_u8(0); // death
        encode_varint(0, &mut payload); // portal cooldown
        encode_varint(63, &mut payload); // sea level
        let spawn_end = payload.len();

        payload.put_u8(0); // online_mode
        payload.put_u8(0); // enforces_secure_chat

        let login_pkt = RawPacket::new(0x31, payload.freeze());
        let spawn_info_bytes = &login_pkt.payload[spawn_start..spawn_end];

        // 1. KEEP_ALL_DATA (0x03)
        let respawn_all = extract_respawn_from_login_with_data_kept(&login_pkt, 776, KEEP_ALL_DATA)
            .expect("extract failed");
        assert_eq!(respawn_all.id, 0x52);
        assert_eq!(respawn_all.payload[spawn_info_bytes.len()], KEEP_ALL_DATA);

        // 2. KEEP_ATTRIBUTES (0x01)
        let respawn_attr =
            extract_respawn_from_login_with_data_kept(&login_pkt, 776, KEEP_ATTRIBUTES)
                .expect("extract failed");
        assert_eq!(
            respawn_attr.payload[spawn_info_bytes.len()],
            KEEP_ATTRIBUTES
        );

        // 3. KEEP_METADATA (0x02)
        let respawn_meta =
            extract_respawn_from_login_with_data_kept(&login_pkt, 776, KEEP_METADATA)
                .expect("extract failed");
        assert_eq!(respawn_meta.payload[spawn_info_bytes.len()], KEEP_METADATA);
    }

    #[tokio::test]
    async fn test_seamless_transfer_gui_sanitization() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        let (mut steel_backend, steel_server_end) = duplex(65536);
        connector.register("steelmc", Box::new(steel_server_end));

        tokio::spawn(async move {
            let finish = FinishConfigurationPacket::new().encode();
            write_packet(&mut steel_backend, &finish)
                .await
                .expect("Write finish config failed");
            let _ = read_packet(&mut steel_backend, 65536).await;
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut steel_backend, &mut buf).await;
        });

        let (client_writer, mut client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Backend opens a chest container (window_id = 7)
        let mut open_window_payload = BytesMut::new();
        encode_varint(7, &mut open_window_payload); // windowId = 7
        encode_varint(0, &mut open_window_payload); // chest inventory type
        encode_varint(4, &mut open_window_payload); // title length
        open_window_payload.put_slice(b"Menu");
        let open_pkt = RawPacket::new(0x31, open_window_payload.freeze()); // 0x31 on 765
        sm.handle_backend_packet(open_pkt).await.unwrap();
        assert_eq!(sm.open_container_id, Some(7));
        let pkt_open_forwarded = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_open_forwarded.id, 0x31);

        // Player switches server via /steel
        let steel_cmd = ServerboundChatCommand::new("/steel").encode();
        sm.handle_client_packet(steel_cmd).await.unwrap();

        // 1. Read SystemChatMessage
        let pkt1 = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt1.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);

        // 2. Client receives CloseContainerPacket with window_id = 7 to cleanly sanitize GUI!
        let pkt_close = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_close.id, CLOSE_CONTAINER_PACKET_ID);
        let close_dec = CloseContainerPacket::decode(&pkt_close, 765).unwrap();
        assert_eq!(close_dec.window_id, 7);

        // 3. Backend sends Login (Play)
        let login_pkt = RawPacket::new(
            0x29,
            Bytes::from_static(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        );
        sm.handle_backend_packet(login_pkt).await.unwrap();

        // 4. Client receives Login (Play)
        let pkt_login = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_login.id, 0x29);

        // 5. Client receives Respawn
        let pkt_respawn = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_respawn.id, RESPAWN_PACKET_ID);
        let respawn_dec = RespawnPacket::decode(&pkt_respawn).unwrap();
        assert_eq!(respawn_dec.data_kept, KEEP_ALL_DATA);
    }

    #[tokio::test]
    async fn test_seamless_transfer_audio_cleanup() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        let (mut steel_backend, steel_server_end) = duplex(65536);
        connector.register("steelmc", Box::new(steel_server_end));

        tokio::spawn(async move {
            let finish = FinishConfigurationPacket::new().encode();
            write_packet(&mut steel_backend, &finish)
                .await
                .expect("Write finish config failed");
            let _ = read_packet(&mut steel_backend, 65536).await;
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut steel_backend, &mut buf).await;
        });

        let (client_writer, mut client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Backend plays a sound effect (0x66 on protocol 765)
        let sound_pkt = RawPacket::new(0x66, Bytes::from_static(&[0x01, 0x00]));
        sm.handle_backend_packet(sound_pkt).await.unwrap();
        assert!(sm.has_active_audio);

        // Client drains forwarded sound packet
        let _ = read_packet(&mut client_reader, 65536).await.unwrap();

        // Player switches server via /steel
        let steel_cmd = ServerboundChatCommand::new("/steel").encode();
        sm.handle_client_packet(steel_cmd).await.unwrap();

        // 1. Read SystemChatMessage
        let pkt1 = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt1.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);

        // 2. Client receives StopSoundPacket::all() to silence lingering sounds!
        let pkt_stop = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_stop.id, StopSoundPacket::packet_id_for_version(765));
        let stop_dec = StopSoundPacket::decode(&pkt_stop, 765).unwrap();
        assert_eq!(stop_dec.flags, 0); // Stops all sounds!

        // 3. Backend sends Login (Play)
        let login_pkt = RawPacket::new(
            0x29,
            Bytes::from_static(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]),
        );
        sm.handle_backend_packet(login_pkt).await.unwrap();

        // 4. Client receives Login (Play)
        let pkt_login = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_login.id, 0x29);

        // 5. Client receives Respawn
        let pkt_respawn = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_respawn.id, RESPAWN_PACKET_ID);
    }

    #[tokio::test]
    async fn test_seamless_transfer_bossbar_and_scoreboard_teardown() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        let (mut steel_backend, steel_server_end) = duplex(65536);
        connector.register("steelmc", Box::new(steel_server_end));

        tokio::spawn(async move {
            let finish = FinishConfigurationPacket::new().encode();
            write_packet(&mut steel_backend, &finish)
                .await
                .expect("Write finish config failed");
            let _ = read_packet(&mut steel_backend, 65536).await;
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut steel_backend, &mut buf).await;
        });

        let (client_writer, mut client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Backend adds a BossBar
        let boss_uuid = [42u8; 16];
        let mut bb_payload = BytesMut::new();
        bb_payload.put_slice(&boss_uuid);
        encode_varint(BossBarPacket::ACTION_ADD, &mut bb_payload);
        let bb_pkt = RawPacket::new(0x0A, bb_payload.freeze());
        sm.handle_backend_packet(bb_pkt).await.unwrap();
        assert!(sm.active_boss_bars.contains(&boss_uuid));
        let _ = read_packet(&mut client_reader, 65536).await.unwrap(); // drain forwarded

        // Backend creates a Scoreboard objective
        let mut sb_payload = BytesMut::new();
        encode_varint(7, &mut sb_payload);
        sb_payload.put_slice(b"sidebar");
        sb_payload.put_u8(ScoreboardObjectivePacket::ACTION_CREATE);
        let sb_pkt = RawPacket::new(0x5C, sb_payload.freeze());
        sm.handle_backend_packet(sb_pkt).await.unwrap();
        assert!(sm.active_scoreboard_objectives.contains("sidebar"));
        let _ = read_packet(&mut client_reader, 65536).await.unwrap(); // drain forwarded

        // Player switches server via /steel
        let steel_cmd = ServerboundChatCommand::new("/steel").encode();
        sm.handle_client_packet(steel_cmd).await.unwrap();

        // 1. Read SystemChatMessage
        let pkt1 = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt1.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);

        // 2. Client receives ScoreboardObjectivePacket REMOVE
        let pkt_sb = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_sb.id, 0x5C);
        let sb_dec = ScoreboardObjectivePacket::decode(&pkt_sb, 765).unwrap();
        assert_eq!(sb_dec.name, "sidebar");
        assert_eq!(sb_dec.action, ScoreboardObjectivePacket::ACTION_REMOVE);

        // 3. Client receives BossBarPacket REMOVE
        let pkt_bb = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt_bb.id, 0x0A);
        let bb_dec = BossBarPacket::decode(&pkt_bb, 765).unwrap();
        assert_eq!(bb_dec.uuid, boss_uuid);
        assert_eq!(bb_dec.action, BossBarPacket::ACTION_REMOVE);

        assert!(sm.active_boss_bars.is_empty());
        assert!(sm.active_scoreboard_objectives.is_empty());
    }

    #[tokio::test]
    async fn test_manual_sanitize_screen_and_stop_audio_helpers() {
        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());
        let host = Arc::new(ScriptHost::new());
        let (client_writer, mut client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        sm.sanitize_screen(0).await.expect("sanitize_screen failed");
        let close_pkt = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(close_pkt.id, CLOSE_CONTAINER_PACKET_ID);
        let close = CloseContainerPacket::decode(&close_pkt, 765).unwrap();
        assert_eq!(close.window_id, 0);

        sm.stop_audio().await.expect("stop_audio failed");
        let stop_pkt = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(stop_pkt.id, StopSoundPacket::packet_id_for_version(765));
        let stop = StopSoundPacket::decode(&stop_pkt, 765).unwrap();
        assert_eq!(stop.flags, 0);
    }

    #[test]
    fn test_extract_respawn_from_login_with_data_kept_765_custom_world() {
        // Construct synthetic Login (Play) packet for protocol 765 (1.20.4)
        let mut payload = BytesMut::new();
        // 1. entity_id: i32
        payload.put_i32(77);
        // 2. is_hardcore: bool
        payload.put_u8(0);
        // 3. dimension_names: count + string
        encode_varint(1, &mut payload);
        encode_varint(20, &mut payload);
        payload.put_slice(b"minecraft:the_nether");
        // 4. max_players: VarInt
        encode_varint(100, &mut payload);
        // 5. view_distance: VarInt
        encode_varint(10, &mut payload);
        // 6. simulation_distance: VarInt
        encode_varint(10, &mut payload);
        // 7. reduced_debug_info: bool
        payload.put_u8(0);
        // 8. show_respawn_screen: bool
        payload.put_u8(1);
        // 9. do_limited_crafting: bool
        payload.put_u8(0);

        // SpawnInfo:
        // 1. dimension_type: String
        encode_varint(20, &mut payload);
        payload.put_slice(b"minecraft:the_nether");
        // 2. dimension_name: String
        encode_varint(20, &mut payload);
        payload.put_slice(b"minecraft:the_nether");
        // 3. hashed_seed: i64
        payload.put_i64(987654321i64);
        // 4. gamemode: u8 (1 = Creative)
        payload.put_u8(1);
        // 5. previous_gamemode: i8 (-1 = None)
        payload.put_i8(-1);
        // 6. is_debug: bool
        payload.put_u8(0);
        // 7. is_flat: bool
        payload.put_u8(0);
        // 8. has_death_location: bool
        payload.put_u8(0);
        // 9. portal_cooldown: VarInt
        encode_varint(0, &mut payload);

        let login_pkt = RawPacket::new(0x29, payload.freeze());

        let respawn = extract_respawn_from_login_with_data_kept(&login_pkt, 765, KEEP_ALL_DATA)
            .expect("Extract 765 respawn failed");
        assert_eq!(respawn.id, 0x45); // RESPAWN_PACKET_ID on 764/765

        let decoded = RespawnPacket::decode_with_version(&respawn, 765).unwrap();
        assert_eq!(decoded.dimension_type, "minecraft:the_nether");
        assert_eq!(decoded.dimension_name, "minecraft:the_nether");
        assert_eq!(decoded.hashed_seed, 987654321);
        assert_eq!(decoded.gamemode, 1);
        assert_eq!(decoded.previous_gamemode, -1);
        assert_eq!(decoded.data_kept, KEEP_ALL_DATA);
    }

    #[tokio::test]
    async fn test_seamless_transfer_display_objective_teardown() {
        let host = Arc::new(ScriptHost::new());
        host.reload("scripts/main.rhai")
            .await
            .expect("Load scripts failed");

        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());

        let (mut steel_backend, steel_server_end) = duplex(65536);
        connector.register("steelmc", Box::new(steel_server_end));

        tokio::spawn(async move {
            let finish = FinishConfigurationPacket::new().encode();
            write_packet(&mut steel_backend, &finish)
                .await
                .expect("Write finish config failed");
            let _ = read_packet(&mut steel_backend, 65536).await;
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut steel_backend, &mut buf).await;
        });

        let (client_writer, mut client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        // Backend sets display objective for SIDEBAR (position 1)
        let set_disp = DisplayObjectivePacket {
            position: DisplayObjectivePacket::POSITION_SIDEBAR,
            name: "sidebar_obj".to_string(),
        }
        .encode_with_version(765);
        sm.handle_backend_packet(set_disp).await.unwrap();
        assert!(sm
            .active_display_slots
            .contains(&DisplayObjectivePacket::POSITION_SIDEBAR));
        let _ = read_packet(&mut client_reader, 65536).await.unwrap(); // drain forwarded

        // Player switches server via /steel
        let steel_cmd = ServerboundChatCommand::new("/steel").encode();
        sm.handle_client_packet(steel_cmd).await.unwrap();

        // 1. Read SystemChatMessage
        let pkt1 = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt1.id, SYSTEM_CHAT_MESSAGE_PACKET_ID);

        // 2. Client receives DisplayObjectivePacket CLEAR for SIDEBAR (position 1)
        let pkt_disp = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(
            pkt_disp.id,
            DisplayObjectivePacket::packet_id_for_version(765)
        );
        let disp_dec = DisplayObjectivePacket::decode(&pkt_disp, 765).unwrap();
        assert_eq!(disp_dec.position, DisplayObjectivePacket::POSITION_SIDEBAR);
        assert!(
            disp_dec.name.is_empty(),
            "DisplayObjective clear must have empty name"
        );

        assert!(sm.active_display_slots.is_empty());
    }

    #[tokio::test]
    async fn test_clear_display_objective_helper() {
        let config = Arc::new(ProxyConfig::default());
        let connector = Arc::new(MockBackendConnector::new());
        let host = Arc::new(ScriptHost::new());
        let (client_writer, mut client_reader) = duplex(65536);
        let (_lobby_client, lobby_backend) = duplex(65536);

        let mut sm = PlayStateMachine::new(
            Box::new(client_writer),
            Box::new(lobby_backend),
            connector,
            sample_session(),
            config,
            host,
            None,
        );

        sm.active_display_slots.insert(1);
        sm.clear_display_objective(1)
            .await
            .expect("clear_display_objective failed");
        assert!(sm.active_display_slots.is_empty());

        let pkt = read_packet(&mut client_reader, 65536).await.unwrap();
        assert_eq!(pkt.id, DisplayObjectivePacket::packet_id_for_version(765));
        let decoded = DisplayObjectivePacket::decode(&pkt, 765).unwrap();
        assert_eq!(decoded.position, 1);
        assert!(decoded.name.is_empty());
    }
}
