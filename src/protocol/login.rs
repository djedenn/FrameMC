use bytes::{Buf, BufMut, BytesMut};
use rand::Rng;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::error::ProxyError;
use crate::protocol::packet::RawPacket;
use crate::protocol::varint::{decode_varint, encode_varint, varint_size};

/// Initial client login packet (Packet ID: `0x00` in Login state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStartPacket {
    pub username: String,
    pub player_uuid: Uuid,
}

impl LoginStartPacket {
    pub fn new(username: impl Into<String>, player_uuid: Uuid) -> Self {
        Self {
            username: username.into(),
            player_uuid,
        }
    }

    /// Decodes a `LoginStartPacket` from a `RawPacket`.
    ///
    /// Validates that:
    /// - `packet.id == 0x00`
    /// - `username` is valid UTF-8 and does not exceed 16 characters
    /// - `player_uuid` is read from the remaining 16 bytes if present
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x00 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];

        // 1. Username (String 1..=16 chars: [a-zA-Z0-9_])
        let name_len = decode_varint(&mut cursor)?;
        if !(1..=16).contains(&name_len) {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Username byte length {name_len} is out of bounds (1..=16)"),
            )));
        }

        let name_len = name_len as usize;
        if cursor.remaining() < name_len {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading username",
            )));
        }

        let name_bytes = &cursor[..name_len];
        for &b in name_bytes {
            if !(b.is_ascii_alphanumeric() || b == b'_') {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Invalid character in username: {:?}", b as char),
                )));
            }
        }

        let username = std::str::from_utf8(name_bytes)
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in username",
                ))
            })?
            .to_string();
        cursor.advance(name_len);

        // 2. Player UUID (16 raw bytes)
        let player_uuid = if cursor.remaining() >= 16 {
            let mut uuid_bytes = [0u8; 16];
            cursor.copy_to_slice(&mut uuid_bytes);
            Uuid::from_bytes(uuid_bytes)
        } else {
            Uuid::nil()
        };

        Ok(Self {
            username,
            player_uuid,
        })
    }

    /// Encodes this `LoginStartPacket` into a `RawPacket`.
    pub fn encode(&self) -> RawPacket {
        let mut payload = BytesMut::new();
        encode_varint(self.username.len() as i32, &mut payload);
        payload.put_slice(self.username.as_bytes());
        payload.put_slice(self.player_uuid.as_bytes());
        RawPacket::new(0x00, payload.freeze())
    }
}

/// Server encryption request packet (Packet ID: `0x01` in Login state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionRequestPacket {
    pub server_id: String,
    pub public_key: Vec<u8>,
    pub verify_token: Vec<u8>,
    pub should_authenticate: bool,
}

impl EncryptionRequestPacket {
    pub fn new(server_id: impl Into<String>, public_key: Vec<u8>, verify_token: Vec<u8>) -> Self {
        Self {
            server_id: server_id.into(),
            public_key,
            verify_token,
            should_authenticate: true,
        }
    }

    pub fn with_should_authenticate(
        server_id: impl Into<String>,
        public_key: Vec<u8>,
        verify_token: Vec<u8>,
        should_authenticate: bool,
    ) -> Self {
        Self {
            server_id: server_id.into(),
            public_key,
            verify_token,
            should_authenticate,
        }
    }

    /// Generates an `EncryptionRequestPacket` with an empty server ID,
    /// the provided public key DER, a random 4-byte verification token,
    /// and `should_authenticate = true`.
    pub fn generate(public_key_der: &[u8]) -> Self {
        let mut verify_token = [0u8; 4];
        rand::thread_rng().fill(&mut verify_token);

        Self {
            server_id: String::new(),
            public_key: public_key_der.to_vec(),
            verify_token: verify_token.to_vec(),
            should_authenticate: true,
        }
    }

    /// Encodes this `EncryptionRequestPacket` into a `RawPacket` using legacy/default format (< 766).
    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    /// Encodes this `EncryptionRequestPacket` into a `RawPacket` according to the given protocol version.
    /// In modern Minecraft 1.20.5+ (protocol version >= 766), appends `should_authenticate` boolean.
    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        let mut payload = BytesMut::new();

        // 1. Server ID (String, VarInt length-prefixed)
        encode_varint(self.server_id.len() as i32, &mut payload);
        payload.put_slice(self.server_id.as_bytes());

        // 2. Public Key (byte array, VarInt length-prefixed)
        encode_varint(self.public_key.len() as i32, &mut payload);
        payload.put_slice(&self.public_key);

        // 3. Verify Token (byte array, VarInt length-prefixed)
        encode_varint(self.verify_token.len() as i32, &mut payload);
        payload.put_slice(&self.verify_token);

        // 4. Should Authenticate (Boolean, 1.20.5+ / protocol >= 766)
        if protocol_version >= 766 {
            payload.put_u8(if self.should_authenticate { 1 } else { 0 });
        }

        RawPacket::new(0x01, payload.freeze())
    }

    /// Decodes an `EncryptionRequestPacket` from a `RawPacket`.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x01 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];

        // 1. Server ID
        let server_id_len = decode_varint(&mut cursor)?;
        if server_id_len < 0 || cursor.remaining() < server_id_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading server_id",
            )));
        }
        let server_id_bytes = &cursor[..server_id_len as usize];
        let server_id = std::str::from_utf8(server_id_bytes)
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in server_id",
                ))
            })?
            .to_string();
        cursor.advance(server_id_len as usize);

        // 2. Public Key
        let pk_len = decode_varint(&mut cursor)?;
        if pk_len < 0 || cursor.remaining() < pk_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading public_key",
            )));
        }
        let public_key = cursor[..pk_len as usize].to_vec();
        cursor.advance(pk_len as usize);

        // 3. Verify Token
        let token_len = decode_varint(&mut cursor)?;
        if token_len < 0 || cursor.remaining() < token_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading verify_token",
            )));
        }
        let verify_token = cursor[..token_len as usize].to_vec();
        cursor.advance(token_len as usize);

        // 4. Should Authenticate (if protocol >= 766 or extra byte is present)
        let should_authenticate = if cursor.has_remaining() {
            cursor.get_u8() != 0
        } else {
            true
        };

        Ok(Self {
            server_id,
            public_key,
            verify_token,
            should_authenticate,
        })
    }
}

/// Client encryption response packet (Packet ID: `0x01` in Login state, Serverbound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionResponsePacket {
    pub shared_secret: Vec<u8>,
    pub verify_token: Vec<u8>,
}

impl Drop for EncryptionResponsePacket {
    fn drop(&mut self) {
        self.zeroize();
    }
}

impl zeroize::Zeroize for EncryptionResponsePacket {
    fn zeroize(&mut self) {
        self.shared_secret.zeroize();
        self.verify_token.zeroize();
    }
}

impl EncryptionResponsePacket {
    pub fn new(shared_secret: Vec<u8>, verify_token: Vec<u8>) -> Self {
        Self {
            shared_secret,
            verify_token,
        }
    }

    /// Encodes this `EncryptionResponsePacket` into a `RawPacket`.
    pub fn encode(&self) -> RawPacket {
        let mut payload = BytesMut::new();

        // 1. Shared secret (VarInt length prefixed byte array)
        encode_varint(self.shared_secret.len() as i32, &mut payload);
        payload.put_slice(&self.shared_secret);

        // 2. Verify token (VarInt length prefixed byte array)
        encode_varint(self.verify_token.len() as i32, &mut payload);
        payload.put_slice(&self.verify_token);

        RawPacket::new(0x01, payload.freeze())
    }

    /// Decodes an `EncryptionResponsePacket` from a `RawPacket`.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x01 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];

        // 1. Shared secret
        let ss_len = decode_varint(&mut cursor)?;
        if ss_len < 0 || cursor.remaining() < ss_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading encrypted shared_secret",
            )));
        }
        let shared_secret = cursor[..ss_len as usize].to_vec();
        cursor.advance(ss_len as usize);

        // 2. Verify token
        let vt_len = decode_varint(&mut cursor)?;
        if vt_len < 0 || cursor.remaining() < vt_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading encrypted verify_token",
            )));
        }
        let verify_token = cursor[..vt_len as usize].to_vec();
        cursor.advance(vt_len as usize);

        Ok(Self {
            shared_secret,
            verify_token,
        })
    }
}

/// Server login success packet (Packet ID: `0x02` in Login state, Clientbound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccessPacket {
    pub uuid: Uuid,
    pub username: String,
    pub properties: Vec<crate::crypto::mojang_auth::ProfileProperty>,
    pub session_id: Option<Uuid>,
}

impl LoginSuccessPacket {
    pub fn new(
        uuid: Uuid,
        username: impl Into<String>,
        properties: Vec<crate::crypto::mojang_auth::ProfileProperty>,
    ) -> Self {
        Self {
            uuid,
            username: username.into(),
            properties,
            session_id: None,
        }
    }

    pub fn with_session_id(
        uuid: Uuid,
        username: impl Into<String>,
        properties: Vec<crate::crypto::mojang_auth::ProfileProperty>,
        session_id: Option<Uuid>,
    ) -> Self {
        Self {
            uuid,
            username: username.into(),
            properties,
            session_id,
        }
    }

    /// Encodes this `LoginSuccessPacket` into a `RawPacket` using default modern protocol (1.20.4, 765).
    ///
    /// The wire format strictly enforces:
    /// 1. UUID: exactly 16 raw big-endian bytes (`buf.put_slice(uuid.as_bytes())`).
    /// 2. Username: Minecraft protocol string (VarInt length + UTF-8 bytes).
    /// 3. Properties Array: VarInt count (0 for offline / empty) + properties array.
    pub fn encode(&self) -> RawPacket {
        self.encode_with_version(765)
    }

    /// Encodes this `LoginSuccessPacket` into a `RawPacket` according to the given protocol version.
    /// - Protocols >= 707 use raw 16 bytes for UUID; legacy (< 707) uses string UUID.
    /// - Protocols >= 776 append raw 16 bytes for session ID.
    /// - Follows standard modern Minecraft wire format: `[UUID (16 bytes)] [Username (String)] [Properties Count VarInt] [Session ID (16 bytes, >= 776)]`.
    pub fn encode_with_version(&self, protocol_version: i32) -> RawPacket {
        if protocol_version >= 707 {
            let mut payload = BytesMut::new();

            // 1. UUID: exactly 16 raw big-endian bytes
            payload.put_slice(self.uuid.as_bytes());

            // 2. Username: VarInt length + UTF-8 bytes
            encode_varint(self.username.len() as i32, &mut payload);
            payload.put_slice(self.username.as_bytes());

            // 3. Properties Array: VarInt count (always written; 0 if empty/offline)
            encode_varint(self.properties.len() as i32, &mut payload);
            for prop in &self.properties {
                encode_varint(prop.name.len() as i32, &mut payload);
                payload.put_slice(prop.name.as_bytes());

                encode_varint(prop.value.len() as i32, &mut payload);
                payload.put_slice(prop.value.as_bytes());

                if let Some(signature) = &prop.signature {
                    payload.put_u8(1);
                    encode_varint(signature.len() as i32, &mut payload);
                    payload.put_slice(signature.as_bytes());
                } else {
                    payload.put_u8(0);
                }
            }

            // 4. Session ID (UUID): 16 raw big-endian bytes in protocol >= 776
            if protocol_version >= 776 {
                let sid = self.session_id.unwrap_or_else(Uuid::new_v4);
                payload.put_slice(sid.as_bytes());
            }

            RawPacket::new(0x02, payload.freeze())
        } else {
            let mut payload = BytesMut::new();
            let uuid_str = self.uuid.hyphenated().to_string();
            encode_varint(uuid_str.len() as i32, &mut payload);
            payload.put_slice(uuid_str.as_bytes());

            encode_varint(self.username.len() as i32, &mut payload);
            payload.put_slice(self.username.as_bytes());

            RawPacket::new(0x02, payload.freeze())
        }
    }

    /// Encodes this `LoginSuccessPacket` into a complete framed wire packet:
    /// `[Packet Length: VarInt] [Packet ID: VarInt (0x02)] [Payload]`
    pub fn encode_frame(&self) -> BytesMut {
        self.encode_frame_with_version(765)
    }

    /// Encodes this `LoginSuccessPacket` into a complete framed wire packet for a specific protocol version:
    /// `[Packet Length: VarInt] [Packet ID: VarInt (0x02)] [Payload]`
    pub fn encode_frame_with_version(&self, protocol_version: i32) -> BytesMut {
        encode_frame_packet(&self.encode_with_version(protocol_version))
    }

    /// Decodes a `LoginSuccessPacket` from a framed byte slice:
    /// `[Packet Length: VarInt] [Packet ID: VarInt (0x02)] [Payload]`
    pub fn decode_frame(frame: &[u8]) -> Result<Self, ProxyError> {
        Self::decode_frame_with_version(frame, 765)
    }

    /// Decodes a `LoginSuccessPacket` from a framed byte slice for a specific protocol version:
    /// `[Packet Length: VarInt] [Packet ID: VarInt (0x02)] [Payload]`
    pub fn decode_frame_with_version(
        frame: &[u8],
        protocol_version: i32,
    ) -> Result<Self, ProxyError> {
        let mut cursor = frame;
        let total_length = decode_varint(&mut cursor)?;
        if cursor.remaining() < total_length as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading framed packet",
            )));
        }
        let packet_id = decode_varint(&mut cursor)?;
        if packet_id != 0x02 {
            return Err(ProxyError::InvalidPacketId(packet_id));
        }
        let raw = RawPacket::new(packet_id, bytes::Bytes::copy_from_slice(cursor));
        Self::decode_with_version(&raw, protocol_version)
    }

    /// Decodes a `LoginSuccessPacket` from a `RawPacket` using default modern protocol (1.20.4, 765).
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        Self::decode_with_version(packet, 765)
    }

    /// Decodes a `LoginSuccessPacket` from a `RawPacket` for a specific protocol version.
    pub fn decode_with_version(
        packet: &RawPacket,
        protocol_version: i32,
    ) -> Result<Self, ProxyError> {
        if packet.id != 0x02 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];

        // 1. UUID
        let uuid = if protocol_version >= 707 {
            if cursor.remaining() < 16 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading UUID bytes",
                )));
            }
            let mut uuid_bytes = [0u8; 16];
            cursor.copy_to_slice(&mut uuid_bytes);
            Uuid::from_bytes(uuid_bytes)
        } else {
            let len = decode_varint(&mut cursor)?;
            if len < 0 || cursor.remaining() < len as usize {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading UUID string",
                )));
            }
            let uuid_str = std::str::from_utf8(&cursor[..len as usize]).map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in UUID string",
                ))
            })?;
            cursor.advance(len as usize);
            Uuid::parse_str(uuid_str).map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UUID format",
                ))
            })?
        };

        // 2. Username
        let name_len = decode_varint(&mut cursor)?;
        if name_len < 0 || cursor.remaining() < name_len as usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading username",
            )));
        }
        let username = std::str::from_utf8(&cursor[..name_len as usize])
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid UTF-8 in username",
                ))
            })?
            .to_string();
        cursor.advance(name_len as usize);

        // 3. Properties (if protocol >= 759 and cursor has remaining data)
        let mut properties = Vec::new();
        if protocol_version >= 759 && cursor.has_remaining() {
            let prop_count = decode_varint(&mut cursor)?;
            if prop_count < 0 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "negative property count",
                )));
            }
            for _ in 0..prop_count {
                // Name
                let n_len = decode_varint(&mut cursor)?;
                if n_len < 0 || cursor.remaining() < n_len as usize {
                    return Err(ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "EOF reading property name",
                    )));
                }
                let name = std::str::from_utf8(&cursor[..n_len as usize])
                    .map_err(|_| {
                        ProxyError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid UTF-8 in property name",
                        ))
                    })?
                    .to_string();
                cursor.advance(n_len as usize);

                // Value
                let v_len = decode_varint(&mut cursor)?;
                if v_len < 0 || cursor.remaining() < v_len as usize {
                    return Err(ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "EOF reading property value",
                    )));
                }
                let value = std::str::from_utf8(&cursor[..v_len as usize])
                    .map_err(|_| {
                        ProxyError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "invalid UTF-8 in property value",
                        ))
                    })?
                    .to_string();
                cursor.advance(v_len as usize);

                // Is signed
                if !cursor.has_remaining() {
                    return Err(ProxyError::Io(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "EOF reading property is_signed",
                    )));
                }
                let is_signed = cursor.get_u8() != 0;
                let signature = if is_signed {
                    let s_len = decode_varint(&mut cursor)?;
                    if s_len < 0 || cursor.remaining() < s_len as usize {
                        return Err(ProxyError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "EOF reading property signature",
                        )));
                    }
                    let sig = std::str::from_utf8(&cursor[..s_len as usize])
                        .map_err(|_| {
                            ProxyError::Io(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "invalid UTF-8 in property signature",
                            ))
                        })?
                        .to_string();
                    cursor.advance(s_len as usize);
                    Some(sig)
                } else {
                    None
                };

                properties.push(crate::crypto::mojang_auth::ProfileProperty {
                    name,
                    value,
                    signature,
                });
            }
        }

        // 4. Session ID: exactly 16 raw bytes if protocol >= 776
        let session_id = if protocol_version >= 776 {
            if cursor.remaining() < 16 {
                return Err(ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unexpected EOF reading session_id in LoginSuccess",
                )));
            }
            let mut sid_bytes = [0u8; 16];
            cursor.copy_to_slice(&mut sid_bytes);
            Some(Uuid::from_bytes(sid_bytes))
        } else {
            None
        };

        if cursor.has_remaining() {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Unexpected trailing data ({} bytes) in LoginSuccess packet",
                    cursor.remaining()
                ),
            )));
        }

        Ok(Self {
            uuid,
            username,
            properties,
            session_id,
        })
    }
}

/// Encodes a `RawPacket` into a framed Minecraft wire buffer:
/// `[Packet Length: VarInt] [Packet ID: VarInt] [Payload]`
pub fn encode_frame_packet(packet: &RawPacket) -> BytesMut {
    let id_len = varint_size(packet.id);
    let total_length = id_len + packet.payload.len();
    let prefix_len = varint_size(total_length as i32);

    let mut buf = BytesMut::with_capacity(prefix_len + total_length);
    encode_varint(total_length as i32, &mut buf);
    encode_varint(packet.id, &mut buf);
    buf.extend_from_slice(&packet.payload);
    buf
}

/// Encodes a `LoginSuccess` packet according to modern or legacy wire format.
///
/// In modern Minecraft Java Edition:
/// - `UUID`: 16 raw bytes
/// - `Username`: String (prefixed with VarInt length)
/// - `Number of Properties`: VarInt (must encode at least `0` if empty; never omitted)
/// - Properties: `Name` (String), `Value` (String), `is_signed` (Boolean), optional `Signature` (String).
/// - `Session ID`: 16 raw bytes (appended in 26.2+ / protocol version >= 776).
pub fn encode_login_success(
    uuid: Uuid,
    username: &str,
    properties: &[crate::crypto::mojang_auth::ProfileProperty],
    protocol_version: i32,
) -> RawPacket {
    let packet = LoginSuccessPacket::new(uuid, username, properties.to_vec());
    packet.encode_with_version(protocol_version)
}

/// Encodes a `LoginSuccess` packet with an explicit session ID.
pub fn encode_login_success_with_session(
    uuid: Uuid,
    username: &str,
    properties: &[crate::crypto::mojang_auth::ProfileProperty],
    session_id: Option<Uuid>,
    protocol_version: i32,
) -> RawPacket {
    let packet =
        LoginSuccessPacket::with_session_id(uuid, username, properties.to_vec(), session_id);
    packet.encode_with_version(protocol_version)
}

/// Encodes a `LoginSuccess` packet directly into a complete framed wire buffer:
/// `[Packet Length: VarInt] [Packet ID: VarInt (0x02)] [Payload]`
pub fn encode_login_success_frame(
    uuid: Uuid,
    username: &str,
    properties: &[crate::crypto::mojang_auth::ProfileProperty],
    protocol_version: i32,
) -> BytesMut {
    let packet = LoginSuccessPacket::new(uuid, username, properties.to_vec());
    packet.encode_frame_with_version(protocol_version)
}

/// Encodes a `LoginSuccess` packet with an explicit session ID directly into a complete framed wire buffer.
pub fn encode_login_success_frame_with_session(
    uuid: Uuid,
    username: &str,
    properties: &[crate::crypto::mojang_auth::ProfileProperty],
    session_id: Option<Uuid>,
    protocol_version: i32,
) -> BytesMut {
    let packet =
        LoginSuccessPacket::with_session_id(uuid, username, properties.to_vec(), session_id);
    packet.encode_frame_with_version(protocol_version)
}

/// Client login acknowledged packet (Packet ID: `0x03` in Login state, Serverbound, 1.20.2+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoginAcknowledgedPacket;

impl LoginAcknowledgedPacket {
    pub fn new() -> Self {
        Self
    }

    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x03 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        Ok(Self)
    }

    pub fn encode(&self) -> RawPacket {
        RawPacket::new(0x03, bytes::Bytes::new())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::crypto::mojang_auth::ProfileProperty;

    #[test]
    fn test_login_start_decode_and_encode() {
        let player_uuid = Uuid::parse_str("069a79f4-44e3-4726-a9be-254cc4d37b01").unwrap();
        let login_start = LoginStartPacket::new("Steve", player_uuid);

        let raw = login_start.encode();
        assert_eq!(raw.id, 0x00);

        let decoded = LoginStartPacket::decode(&raw).expect("Failed to decode LoginStart");
        assert_eq!(decoded.username, "Steve");
        assert_eq!(decoded.player_uuid, player_uuid);
        assert_eq!(decoded, login_start);
    }

    #[test]
    fn test_login_start_username_too_long() {
        let mut payload = BytesMut::new();
        let long_name = "A_Very_Long_Player_Name_Over_16_Chars";
        encode_varint(long_name.len() as i32, &mut payload);
        payload.put_slice(long_name.as_bytes());
        payload.put_slice(Uuid::nil().as_bytes());

        let raw = RawPacket::new(0x00, payload.freeze());
        match LoginStartPacket::decode(&raw) {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
            other => panic!(
                "Expected InvalidData error for long username, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_login_start_empty_username() {
        let mut payload = BytesMut::new();
        encode_varint(0, &mut payload); // length 0
        payload.put_slice(Uuid::nil().as_bytes());

        let raw = RawPacket::new(0x00, payload.freeze());
        match LoginStartPacket::decode(&raw) {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
            other => panic!(
                "Expected InvalidData error for empty username, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_login_start_invalid_chars() {
        let mut payload = BytesMut::new();
        let bad_name = "Steve!";
        encode_varint(bad_name.len() as i32, &mut payload);
        payload.put_slice(bad_name.as_bytes());
        payload.put_slice(Uuid::nil().as_bytes());

        let raw = RawPacket::new(0x00, payload.freeze());
        match LoginStartPacket::decode(&raw) {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
            other => panic!(
                "Expected InvalidData error for username with symbols, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn test_encryption_request_generate_and_roundtrip() {
        let dummy_der = vec![0x30, 0x81, 0x9F, 0x30, 0x0D, 0x06, 0x09];
        let enc_req = EncryptionRequestPacket::generate(&dummy_der);

        assert_eq!(enc_req.server_id, "");
        assert_eq!(enc_req.public_key, dummy_der);
        assert_eq!(enc_req.verify_token.len(), 4);

        let raw = enc_req.encode();
        assert_eq!(raw.id, 0x01);

        let decoded =
            EncryptionRequestPacket::decode(&raw).expect("Failed to decode EncryptionRequest");
        assert_eq!(decoded, enc_req);
    }

    #[test]
    fn test_encryption_request_modern_version_776_should_authenticate() {
        let dummy_der = vec![0x30, 0x81, 0x9F, 0x30, 0x0D, 0x06, 0x09];
        let enc_req = EncryptionRequestPacket::generate(&dummy_der);

        // Protocol 765 (< 766) should NOT have should_authenticate byte
        let raw_765 = enc_req.encode_with_version(765);
        assert_eq!(raw_765.id, 0x01);

        // Protocol 776 (>= 766, Minecraft 1.21.4) MUST have should_authenticate byte (0x01)
        let raw_776 = enc_req.encode_with_version(776);
        assert_eq!(raw_776.id, 0x01);
        assert_eq!(raw_776.payload.len(), raw_765.payload.len() + 1);
        assert_eq!(*raw_776.payload.last().unwrap(), 0x01);

        let decoded = EncryptionRequestPacket::decode(&raw_776)
            .expect("Failed to decode EncryptionRequest 776");
        assert!(decoded.should_authenticate);
        assert_eq!(decoded, enc_req);
    }

    #[test]
    fn test_encryption_response_roundtrip() {
        let shared_secret = vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let verify_token = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let resp = EncryptionResponsePacket::new(shared_secret.clone(), verify_token.clone());

        let raw = resp.encode();
        assert_eq!(raw.id, 0x01);

        let decoded =
            EncryptionResponsePacket::decode(&raw).expect("Failed to decode EncryptionResponse");
        assert_eq!(decoded.shared_secret, shared_secret);
        assert_eq!(decoded.verify_token, verify_token);
    }

    #[test]
    fn test_login_success_modern_roundtrip_with_properties() {
        let uuid = Uuid::parse_str("069a79f4-44e3-4726-a9be-254cc4d37b01").unwrap();
        let properties = vec![
            ProfileProperty {
                name: "textures".to_string(),
                value: "base64_texture_payload".to_string(),
                signature: Some("signature_payload".to_string()),
            },
            ProfileProperty {
                name: "custom_prop".to_string(),
                value: "val".to_string(),
                signature: None,
            },
        ];

        let login_success = LoginSuccessPacket::new(uuid, "Steve", properties);

        // Test modern encode (packet ID 0x02)
        let raw = login_success.encode();
        assert_eq!(raw.id, 0x02);

        let decoded = LoginSuccessPacket::decode_with_version(&raw, 765)
            .expect("Failed to decode LoginSuccess 765");
        assert_eq!(decoded.uuid, uuid);
        assert_eq!(decoded.username, "Steve");
        assert_eq!(decoded.properties.len(), 2);
        assert_eq!(decoded.properties[0].name, "textures");
        assert_eq!(
            decoded.properties[0].signature,
            Some("signature_payload".to_string())
        );
        assert_eq!(decoded.properties[1].name, "custom_prop");
        assert_eq!(decoded.properties[1].signature, None);
    }

    #[test]
    fn test_login_success_legacy_version_roundtrip() {
        let uuid = Uuid::parse_str("069a79f4-44e3-4726-a9be-254cc4d37b01").unwrap();
        let login_success = LoginSuccessPacket::new(uuid, "Alex", Vec::new());

        // Test 1.12.2 (protocol 340 < 707)
        let raw = login_success.encode_with_version(340);
        assert_eq!(raw.id, 0x02);

        let decoded = LoginSuccessPacket::decode_with_version(&raw, 340)
            .expect("Failed to decode LoginSuccess 340");
        assert_eq!(decoded.uuid, uuid);
        assert_eq!(decoded.username, "Alex");
        assert!(decoded.properties.is_empty());
    }

    #[test]
    fn test_encode_login_success_modern_offline_exact_bytes() {
        let uuid = Uuid::from_bytes([
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ]);
        let username = "Steve";
        let properties = Vec::<ProfileProperty>::new();

        for protocol in [764, 765, 766, 767] {
            let raw = encode_login_success(uuid, username, &properties, protocol);
            assert_eq!(raw.id, 0x02);

            let expected_bytes: [u8; 23] = [
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
                0x0f, 0x10, 5, b'S', b't', b'e', b'v', b'e',
                0x00, // VarInt 0: empty properties array (never omitted)
            ];

            assert_eq!(raw.payload.len(), 23);
            assert_eq!(&raw.payload[..], &expected_bytes[..]);

            let decoded = LoginSuccessPacket::decode_with_version(&raw, protocol)
                .expect("Failed to decode modern offline LoginSuccess");
            assert_eq!(decoded.uuid, uuid);
            assert_eq!(decoded.username, "Steve");
            assert!(decoded.properties.is_empty());
        }
    }

    #[test]
    fn test_login_success_offline_player_frame_serialization() {
        let uuid = Uuid::nil();
        let username = "TestPlayer";
        let properties = Vec::<ProfileProperty>::new();

        let login_success = LoginSuccessPacket::new(uuid, username, properties);
        let frame = login_success.encode_frame();

        let mut cursor = &frame[..];

        // 1. Total packet length header matches the buffer length
        let packet_length =
            decode_varint(&mut cursor).expect("Failed to decode packet length header");
        assert_eq!(
            packet_length as usize,
            cursor.len(),
            "Total packet length header must match the remaining buffer length"
        );

        // 2. Total packet ID is 0x02
        let packet_id = decode_varint(&mut cursor).expect("Failed to decode packet ID");
        assert_eq!(packet_id, 0x02, "Total packet ID must be 0x02");

        // 3. The next 16 bytes are all 0x00
        assert_eq!(
            &cursor[..16],
            &[0u8; 16],
            "The next 16 bytes must all be 0x00"
        );
        cursor.advance(16);

        // 4. The next byte is 0x0A (length 10 for 'TestPlayer'), followed by 'TestPlayer'
        assert_eq!(
            cursor[0], 0x0A,
            "The next byte must be 0x0A (length 10 for TestPlayer)"
        );
        cursor.advance(1);
        assert_eq!(&cursor[..10], b"TestPlayer", "Followed by 'TestPlayer'");
        cursor.advance(10);

        // 5. The subsequent byte is 0x00 (0 properties)
        assert_eq!(
            cursor[0], 0x00,
            "The subsequent byte must be 0x00 (0 properties)"
        );
        cursor.advance(1);

        // Ensure no extra trailing bytes
        assert_eq!(
            cursor.len(),
            0,
            "No trailing bytes allowed in LoginSuccess frame"
        );

        // Roundtrip frame decoding
        let decoded = LoginSuccessPacket::decode_frame(&frame)
            .expect("Failed to decode framed LoginSuccess packet");
        assert_eq!(decoded.uuid, uuid);
        assert_eq!(decoded.username, "TestPlayer");
        assert!(decoded.properties.is_empty());
    }

    #[test]
    fn test_login_success_golden_bytes_player_nil_uuid() {
        let uuid = Uuid::nil();
        let username = "Player";
        let properties = Vec::<ProfileProperty>::new();

        let login_success = LoginSuccessPacket::new(uuid, username, properties);
        let frame = login_success.encode_frame();

        // The EXACT complete frame bytes sent over the wire (26 bytes total):
        // - Total Frame Length: 0x19 (25 bytes in decimal)
        // - Packet ID: 0x02 (LoginSuccess)
        // - UUID: 16 bytes of 0x00
        // - Username Length: 0x06
        // - Username ASCII: 0x50, 0x6C, 0x61, 0x79, 0x65, 0x72 ("Player")
        // - Properties Count: 0x00 (VarInt 0)
        let expected_frame: [u8; 26] = [
            0x19, // Total Frame Length: 25 bytes
            0x02, // Packet ID: 0x02 (LoginSuccess)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // UUID: 16 bytes of 0x00
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, // Username length: 6
            0x50, 0x6C, 0x61, 0x79, 0x65, 0x72, // "Player" in ASCII
            0x00, // Properties count: 0
        ];

        // 1. Verify complete frame matches expected 26-byte buffer
        assert_eq!(&frame[..], &expected_frame[..]);

        // 2. Verify total frame length header is 0x19 (25 bytes)
        assert_eq!(frame[0], 0x19);

        // 3. Verify the exact 25-byte sequence following the length prefix
        let payload_and_id = &frame[1..];
        assert_eq!(payload_and_id.len(), 25);
        assert_eq!(payload_and_id, &expected_frame[1..]);

        // 4. Verify encode_login_success produces identical framed bytes
        let raw = encode_login_success(uuid, "Player", &[], 765);
        assert_eq!(&raw.frame()[..], &expected_frame[..]);

        // 5. Verify decoding roundtrip
        let decoded = LoginSuccessPacket::decode_frame(&frame).unwrap();
        assert_eq!(decoded.uuid, uuid);
        assert_eq!(decoded.username, "Player");
        assert!(decoded.properties.is_empty());
    }

    #[test]
    fn test_login_acknowledged_packet() {
        let ack = LoginAcknowledgedPacket::new();
        let raw = ack.encode();
        assert_eq!(raw.id, 0x03);
        assert!(raw.payload.is_empty());

        let decoded =
            LoginAcknowledgedPacket::decode(&raw).expect("Failed to decode LoginAcknowledged");
        assert_eq!(decoded, LoginAcknowledgedPacket);

        let invalid = RawPacket::new(0x02, bytes::Bytes::new());
        assert!(LoginAcknowledgedPacket::decode(&invalid).is_err());
    }

    #[test]
    fn test_login_packet_wrong_id() {
        let raw = RawPacket::new(0x05, bytes::Bytes::new());
        assert!(LoginStartPacket::decode(&raw).is_err());
        assert!(EncryptionRequestPacket::decode(&raw).is_err());
        assert!(EncryptionResponsePacket::decode(&raw).is_err());
        assert!(LoginSuccessPacket::decode(&raw).is_err());
        assert!(LoginAcknowledgedPacket::decode(&raw).is_err());
    }

    #[test]
    fn test_login_success_protocol_776_standard_wire_layout() {
        let uuid = Uuid::nil();
        let username = "TestUser"; // 8-character username
        let properties = Vec::<ProfileProperty>::new();
        let session_id = Uuid::nil();

        // 1. Verify encode_login_success for protocol 776 (1.21.4 / 26.2)
        let raw =
            encode_login_success_with_session(uuid, username, &properties, Some(session_id), 776);
        assert_eq!(raw.id, 0x02);

        // Standard payload breakdown for 8-char username with 0 properties and 16-byte session_id:
        // - UUID: 16 bytes (0x00 * 16)
        // - Username length VarInt: 1 byte (0x08)
        // - Username ASCII: 8 bytes ("TestUser")
        // - Properties count VarInt: 1 byte (0x00)
        // - Session ID: 16 bytes (0x00 * 16)
        // Total payload = 16 + 1 + 8 + 1 + 16 = 42 bytes.
        assert_eq!(raw.payload.len(), 42);

        // 2. Wire frame verification:
        // Total packet length = Packet ID (1 byte) + Payload (42 bytes) = 43 bytes (0x2B).
        let frame = raw.frame();
        assert_eq!(
            frame[0], 0x2B,
            "Total packet length VarInt must be 0x2B (43 bytes) for 8-char username with session_id"
        );
        assert_eq!(frame[1], 0x02, "Packet ID must be 0x02");
        assert_eq!(
            frame.len(),
            44,
            "Total frame length (header + payload) must be 44 bytes"
        );

        let expected_frame: [u8; 44] = [
            0x2B, // Total Frame Length: 43 bytes
            0x02, // Packet ID: 0x02 (LoginSuccess)
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // UUID: 16 bytes of 0x00
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, // Username length: 8
            b'T', b'e', b's', b't', b'U', b's', b'e', b'r', // "TestUser" in ASCII
            0x00, // Properties count: 0
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Session ID: 16 bytes of 0x00
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(&frame[..], &expected_frame[..]);

        // 3. Roundtrip decode with protocol 776
        let decoded = LoginSuccessPacket::decode_with_version(&raw, 776)
            .expect("Failed to decode LoginSuccess for protocol 776");
        assert_eq!(decoded.uuid, uuid);
        assert_eq!(decoded.username, "TestUser");
        assert_eq!(decoded.session_id, Some(session_id));
        assert!(decoded.properties.is_empty());

        // 4. Also verify decode_frame_with_version for protocol 776
        let decoded_frame = LoginSuccessPacket::decode_frame_with_version(&frame, 776)
            .expect("Failed to decode framed LoginSuccess for protocol 776");
        assert_eq!(decoded_frame.uuid, uuid);
        assert_eq!(decoded_frame.username, "TestUser");
        assert_eq!(decoded_frame.session_id, Some(session_id));
        assert!(decoded_frame.properties.is_empty());
    }

    #[test]
    fn test_login_success_steelmc_golden_bytes() {
        // Exact byte vector received from SteelMC backend on protocol 776:
        // [0x2C (packet len), 0x00 (data len: uncompressed), 0x02 (packet id), UUID (16B), name len (1B), "v4mphire" (8B), props (0x00), session UUID (16B)]
        let steelmc_raw_bytes: [u8; 45] = [
            0x2C, 0x00, 0x02, 0xE6, 0xCC, 0x35, 0x58, 0xEE, 0xD4, 0x3D, 0x34, 0xB4, 0xBF, 0x2B,
            0x2C, 0x9D, 0x27, 0xE9, 0xD0, 0x08, b'v', b'4', b'm', b'p', b'h', b'i', b'r', b'e',
            0x00, 0x2E, 0xDE, 0xEC, 0xC1, 0xD9, 0x40, 0x46, 0x6A, 0xBC, 0x45, 0x79, 0x63, 0xE1,
            0x7D, 0x3B, 0xBF,
        ];

        // Decode uncompressed packet payload from frame
        // Packet ID is 0x02, payload is remaining 42 bytes
        let raw = RawPacket::new(0x02, bytes::Bytes::copy_from_slice(&steelmc_raw_bytes[3..]));
        let decoded = LoginSuccessPacket::decode_with_version(&raw, 776)
            .expect("Failed to decode SteelMC LoginSuccess");

        let expected_uuid = Uuid::parse_str("e6cc3558-eed4-3d34-b4bf-2b2c9d27e9d0").unwrap();
        let expected_session = Uuid::parse_str("2edeecc1-d940-466a-bc45-7963e17d3bbf").unwrap();

        assert_eq!(decoded.uuid, expected_uuid);
        assert_eq!(decoded.username, "v4mphire");
        assert!(decoded.properties.is_empty());
        assert_eq!(decoded.session_id, Some(expected_session));
    }
}
