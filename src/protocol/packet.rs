use bytes::Bytes;

/// Represents an unparsed Minecraft packet containing its packet ID and raw payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPacket {
    pub id: i32,
    pub payload: Bytes,
}

impl RawPacket {
    pub fn new(id: i32, payload: impl Into<Bytes>) -> Self {
        Self {
            id,
            payload: payload.into(),
        }
    }

    /// Encodes this packet into a framed Minecraft wire buffer:
    /// `[Packet Length: VarInt] [Packet ID: VarInt] [Payload]`
    pub fn frame(&self) -> bytes::BytesMut {
        let id_len = crate::protocol::varint::varint_size(self.id);
        let total_length = id_len + self.payload.len();
        let prefix_len = crate::protocol::varint::varint_size(total_length as i32);

        let mut buf = bytes::BytesMut::with_capacity(prefix_len + total_length);
        crate::protocol::varint::encode_varint(total_length as i32, &mut buf);
        crate::protocol::varint::encode_varint(self.id, &mut buf);
        buf.extend_from_slice(&self.payload);
        buf
    }
}

/// Returns true if the raw packet's payload begins with a protected proxy plugin channel name
/// such as `velocity:player_info` or `bungeecord:main`.
pub fn is_protected_plugin_channel(packet: &RawPacket) -> bool {
    use bytes::Buf;
    let mut cursor = &packet.payload[..];
    if let Ok(len) = crate::protocol::varint::decode_varint(&mut cursor) {
        if len > 0 && len <= 128 && cursor.remaining() >= len as usize {
            if let Ok(channel) = std::str::from_utf8(&cursor[..len as usize]) {
                let lower = channel.to_ascii_lowercase();
                if lower == "velocity:player_info"
                    || lower == "bungeecord:main"
                    || lower == "bungee:main"
                    || lower == "bungeecord"
                {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BufMut;

    #[test]
    fn test_is_protected_plugin_channel() {
        // velocity:player_info
        let mut p1 = bytes::BytesMut::new();
        crate::protocol::varint::encode_varint("velocity:player_info".len() as i32, &mut p1);
        p1.put_slice(b"velocity:player_info");
        let pkt1 = RawPacket::new(0x10, p1.freeze());
        assert!(is_protected_plugin_channel(&pkt1));

        // bungeecord:main
        let mut p2 = bytes::BytesMut::new();
        crate::protocol::varint::encode_varint("bungeecord:main".len() as i32, &mut p2);
        p2.put_slice(b"bungeecord:main");
        let pkt2 = RawPacket::new(0x12, p2.freeze());
        assert!(is_protected_plugin_channel(&pkt2));

        // Normal channel e.g. "minecraft:brand"
        let mut p3 = bytes::BytesMut::new();
        crate::protocol::varint::encode_varint("minecraft:brand".len() as i32, &mut p3);
        p3.put_slice(b"minecraft:brand");
        let pkt3 = RawPacket::new(0x10, p3.freeze());
        assert!(!is_protected_plugin_channel(&pkt3));
    }
}
