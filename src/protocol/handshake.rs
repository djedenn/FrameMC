use crate::error::ProxyError;
use crate::protocol::packet::RawPacket;
use crate::protocol::varint::{decode_varint, encode_varint};
use bytes::{Buf, BufMut, BytesMut};

/// Protocol connection states throughout the lifecycle of a Minecraft connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
    Closed,
}

/// Represents the initial Handshake packet sent by a client (Packet ID: `0x00`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakePacket {
    pub protocol_version: i32,
    pub server_address: String,
    pub server_port: u16,
    pub next_state: ConnectionState,
}

impl HandshakePacket {
    pub fn new(
        protocol_version: i32,
        server_address: impl Into<String>,
        server_port: u16,
        next_state: ConnectionState,
    ) -> Self {
        Self {
            protocol_version,
            server_address: server_address.into(),
            server_port,
            next_state,
        }
    }

    /// Decodes a Handshake packet from a `RawPacket`.
    ///
    /// Validates that:
    /// - `packet.id == 0x00`
    /// - `protocol_version` is a valid VarInt
    /// - `server_address` is a valid VarInt-prefixed UTF-8 string
    /// - `server_port` is an unsigned 16-bit big-endian integer
    /// - `next_state` is 1 (`Status`) or 2 (`Login`), returning `ProxyError::InvalidConnectionState` otherwise.
    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x00 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }

        let mut cursor = &packet.payload[..];

        // 1. Protocol Version (VarInt)
        let protocol_version = decode_varint(&mut cursor)?;

        // 2. Server Address (VarInt length + UTF-8 string)
        let addr_len = decode_varint(&mut cursor)?;
        if addr_len < 0 {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "negative server_address length",
            )));
        }
        let addr_len_usize = addr_len as usize;
        if cursor.remaining() < addr_len_usize {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF while reading server_address",
            )));
        }
        let addr_bytes = &cursor[..addr_len_usize];
        let server_address = std::str::from_utf8(addr_bytes)
            .map_err(|_| {
                ProxyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "server_address is not valid UTF-8",
                ))
            })?
            .to_string();
        cursor.advance(addr_len_usize);

        // 3. Server Port (unsigned 16-bit big-endian)
        if cursor.remaining() < 2 {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected EOF while reading server_port",
            )));
        }
        let server_port = cursor.get_u16();

        // 4. Next State (VarInt: 1 = Status, 2 = Login)
        let next_state_raw = decode_varint(&mut cursor)?;
        let next_state = match next_state_raw {
            1 => ConnectionState::Status,
            2 => ConnectionState::Login,
            other => return Err(ProxyError::InvalidConnectionState(other)),
        };

        Ok(Self {
            protocol_version,
            server_address,
            server_port,
            next_state,
        })
    }

    /// Encodes this HandshakePacket into a `RawPacket` (Packet ID: `0x00`).
    pub fn encode(&self) -> RawPacket {
        let mut payload = BytesMut::new();

        encode_varint(self.protocol_version, &mut payload);
        encode_varint(self.server_address.len() as i32, &mut payload);
        payload.put_slice(self.server_address.as_bytes());
        payload.put_u16(self.server_port);

        let next_state_val = match self.next_state {
            ConnectionState::Status => 1,
            ConnectionState::Login => 2,
            _ => 0,
        };
        encode_varint(next_state_val, &mut payload);

        RawPacket::new(0x00, payload.freeze())
    }

    /// Returns the clean hostname portion of `server_address`, stripping any
    /// legacy BungeeCord null-separated metadata (`host\0ip\0uuid...`).
    pub fn clean_hostname(&self) -> &str {
        self.server_address
            .split('\0')
            .next()
            .unwrap_or(&self.server_address)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_decode_standard_vanilla_handshake() {
        // Build vanilla handshake packet
        // protocol_version: 765 (1.20.4) -> [0xFD, 0x05]
        // server_address: "mc.example.com" -> len 14 [0x0E] + "mc.example.com"
        // server_port: 25565 -> [0x63, 0xDD]
        // next_state: 2 (Login) -> [0x02]
        let mut payload = BytesMut::new();
        encode_varint(765, &mut payload);
        encode_varint(14, &mut payload);
        payload.put_slice(b"mc.example.com");
        payload.put_u16(25565);
        encode_varint(2, &mut payload);

        let raw = RawPacket::new(0x00, payload.freeze());
        let handshake = HandshakePacket::decode(&raw).expect("Failed to decode vanilla handshake");

        assert_eq!(handshake.protocol_version, 765);
        assert_eq!(handshake.server_address, "mc.example.com");
        assert_eq!(handshake.clean_hostname(), "mc.example.com");
        assert_eq!(handshake.server_port, 25565);
        assert_eq!(handshake.next_state, ConnectionState::Login);

        // Verify encode roundtrip
        let re_encoded = handshake.encode();
        assert_eq!(raw, re_encoded);
    }

    #[test]
    fn test_decode_status_handshake() {
        let mut payload = BytesMut::new();
        encode_varint(763, &mut payload);
        encode_varint(9, &mut payload);
        payload.put_slice(b"localhost");
        payload.put_u16(25565);
        encode_varint(1, &mut payload); // Status

        let raw = RawPacket::new(0x00, payload.freeze());
        let handshake = HandshakePacket::decode(&raw).expect("Failed to decode status handshake");

        assert_eq!(handshake.protocol_version, 763);
        assert_eq!(handshake.server_address, "localhost");
        assert_eq!(handshake.clean_hostname(), "localhost");
        assert_eq!(handshake.server_port, 25565);
        assert_eq!(handshake.next_state, ConnectionState::Status);
    }

    #[test]
    fn test_decode_bungeecord_forward_string() {
        // BungeeCord host format: "hostname\0remote_ip\0player_uuid"
        let bungeecord_host =
            "play.server.net\x00192.168.1.100\x00069a79f444e34726a9be254cc4d37b01";

        let mut payload = BytesMut::new();
        encode_varint(765, &mut payload);
        encode_varint(bungeecord_host.len() as i32, &mut payload);
        payload.put_slice(bungeecord_host.as_bytes());
        payload.put_u16(25565);
        encode_varint(2, &mut payload); // Login

        let raw = RawPacket::new(0x00, payload.freeze());
        let handshake =
            HandshakePacket::decode(&raw).expect("Failed to decode BungeeCord handshake");

        assert_eq!(handshake.protocol_version, 765);
        assert_eq!(handshake.server_address, bungeecord_host);
        assert_eq!(handshake.clean_hostname(), "play.server.net");
        assert_eq!(handshake.server_port, 25565);
        assert_eq!(handshake.next_state, ConnectionState::Login);
    }

    #[test]
    fn test_invalid_packet_id() {
        let raw = RawPacket::new(0x01, Bytes::from_static(&[0x00]));
        match HandshakePacket::decode(&raw) {
            Err(ProxyError::InvalidPacketId(id)) => assert_eq!(id, 0x01),
            other => panic!("Expected InvalidPacketId, got {:?}", other),
        }
    }

    #[test]
    fn test_invalid_next_state() {
        let mut payload = BytesMut::new();
        encode_varint(765, &mut payload);
        encode_varint(9, &mut payload);
        payload.put_slice(b"localhost");
        payload.put_u16(25565);
        encode_varint(3, &mut payload); // Invalid state 3

        let raw = RawPacket::new(0x00, payload.freeze());
        match HandshakePacket::decode(&raw) {
            Err(ProxyError::InvalidConnectionState(state)) => assert_eq!(state, 3),
            other => panic!("Expected InvalidConnectionState(3), got {:?}", other),
        }
    }

    #[test]
    fn test_truncated_handshake() {
        // Missing port and next state
        let mut payload = BytesMut::new();
        encode_varint(765, &mut payload);
        encode_varint(9, &mut payload);
        payload.put_slice(b"localhost");

        let raw = RawPacket::new(0x00, payload.freeze());
        match HandshakePacket::decode(&raw) {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("Expected UnexpectedEof, got {:?}", other),
        }
    }
}
