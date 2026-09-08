use crate::error::ProxyError;
use crate::protocol::packet::RawPacket;
use crate::protocol::varint::{decode_varint, encode_varint, read_varint, varint_size};
use bytes::{BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Default maximum uncompressed packet length (2 MiB).
pub const DEFAULT_MAX_PACKET_SIZE: usize = 2097152;

/// Login state `SetCompression` packet (Packet ID: `0x03`, Clientbound).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetCompressionPacket {
    pub threshold: i32,
}

impl SetCompressionPacket {
    pub fn new(threshold: i32) -> Self {
        Self { threshold }
    }

    pub fn encode(&self) -> RawPacket {
        let mut payload = BytesMut::new();
        encode_varint(self.threshold, &mut payload);
        RawPacket::new(0x03, payload.freeze())
    }

    pub fn decode(packet: &RawPacket) -> Result<Self, ProxyError> {
        if packet.id != 0x03 {
            return Err(ProxyError::InvalidPacketId(packet.id));
        }
        let mut cursor = &packet.payload[..];
        let threshold = decode_varint(&mut cursor)?;
        Ok(Self { threshold })
    }
}

/// Disconnect packet used during clean shutdown or connection termination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisconnectPacket {
    pub reason: String,
}

impl DisconnectPacket {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn encode_login(&self) -> RawPacket {
        let json = serde_json::json!({ "text": self.reason }).to_string();
        let mut payload = BytesMut::new();
        encode_varint(json.len() as i32, &mut payload);
        payload.put_slice(json.as_bytes());
        RawPacket::new(0x00, payload.freeze())
    }

    pub fn encode_config(&self, protocol_version: i32) -> RawPacket {
        let packet_id = if protocol_version >= 766 { 0x02 } else { 0x01 };
        if protocol_version >= 766 {
            let mut payload = BytesMut::new();
            payload.put_u8(0x0A); // TAG_Compound
            payload.put_u8(0x08); // TAG_String
            payload.put_u16(4); // name length "text"
            payload.put_slice(b"text");
            let reason_bytes = self.reason.as_bytes();
            payload.put_u16(reason_bytes.len() as u16);
            payload.put_slice(reason_bytes);
            payload.put_u8(0x00); // TAG_End
            RawPacket::new(packet_id, payload.freeze())
        } else {
            let json = serde_json::json!({ "text": self.reason }).to_string();
            let mut payload = BytesMut::new();
            encode_varint(json.len() as i32, &mut payload);
            payload.put_slice(json.as_bytes());
            RawPacket::new(packet_id, payload.freeze())
        }
    }

    pub fn encode_play(&self) -> RawPacket {
        let json = serde_json::json!({ "text": self.reason }).to_string();
        let mut payload = BytesMut::new();
        encode_varint(json.len() as i32, &mut payload);
        payload.put_slice(json.as_bytes());
        RawPacket::new(0x1B, payload.freeze())
    }

    pub fn encode_for_client(
        &self,
        protocol_version: i32,
        in_configuration_or_play: bool,
    ) -> RawPacket {
        if !in_configuration_or_play {
            self.encode_login()
        } else if protocol_version >= 764 {
            self.encode_config(protocol_version)
        } else {
            self.encode_play()
        }
    }
}

/// Reads and frames a raw Minecraft packet from an asynchronous stream.
pub async fn read_packet<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_size: usize,
) -> Result<RawPacket, ProxyError> {
    read_packet_with_compression(reader, max_size, None).await
}

/// Reads and frames a raw Minecraft packet from an asynchronous stream,
/// supporting Zlib decompression when `threshold` is configured.
pub async fn read_packet_with_compression<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_size: usize,
    threshold: Option<usize>,
) -> Result<RawPacket, ProxyError> {
    let length = read_varint(reader).await?;
    if length < 0 || (length as usize) > max_size {
        return Err(ProxyError::PacketTooLarge(length as usize));
    }
    if length == 0 {
        return Err(ProxyError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "packet length cannot be zero",
        )));
    }

    let total_len = length as usize;
    let mut body = BytesMut::with_capacity(total_len.min(64 * 1024));
    let mut remaining = total_len;
    while remaining > 0 {
        let chunk_size = remaining.min(64 * 1024);
        let prev_len = body.len();
        body.resize(prev_len + chunk_size, 0);
        reader.read_exact(&mut body[prev_len..]).await?;
        remaining -= chunk_size;
    }

    let mut cursor = body.freeze();

    if threshold.is_none() {
        let id = decode_varint(&mut cursor)?;
        let payload = cursor;
        return Ok(RawPacket { id, payload });
    }

    let data_length = decode_varint(&mut cursor)?;
    if data_length < 0 || (data_length as usize) > max_size {
        return Err(ProxyError::PacketTooLarge(data_length as usize));
    }

    if let Some(thresh) = threshold {
        if data_length > 0 && (data_length as usize) < thresh {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Decompressed data length {} is smaller than threshold {}",
                    data_length, thresh
                ),
            )));
        }
    }

    if data_length == 0 {
        // Uncompressed packet
        let id = decode_varint(&mut cursor)?;
        let payload = cursor;
        Ok(RawPacket { id, payload })
    } else {
        // Compressed packet
        use flate2::read::ZlibDecoder;
        use std::io::Read;

        let max_allowed = data_length as usize;
        let decoder = ZlibDecoder::new(&cursor[..]);
        let mut limited_decoder = decoder.take((max_allowed as u64) + 1);
        let mut decompressed = Vec::with_capacity(max_allowed.min(64 * 1024));
        limited_decoder.read_to_end(&mut decompressed)?;

        if decompressed.len() != max_allowed {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Decompressed length mismatch: expected {} bytes, got {}",
                    max_allowed,
                    decompressed.len()
                ),
            )));
        }

        let mut decomp_cursor = Bytes::from(decompressed);
        let id = decode_varint(&mut decomp_cursor)?;
        let payload = decomp_cursor;
        Ok(RawPacket { id, payload })
    }
}

/// Serializes and writes a raw Minecraft packet to an asynchronous stream.
pub async fn write_packet<W: AsyncWrite + Unpin>(
    writer: &mut W,
    packet: &RawPacket,
) -> Result<(), ProxyError> {
    write_packet_with_compression(writer, packet, None).await
}

/// Serializes a raw Minecraft packet into a framed buffer,
/// applying Zlib compression when `threshold` is `Some(thresh)` and payload size >= `thresh`.
pub fn encode_packet_with_compression(
    packet: &RawPacket,
    threshold: Option<usize>,
) -> Result<BytesMut, ProxyError> {
    match threshold {
        None => {
            let id_len = varint_size(packet.id);
            let total_length = id_len + packet.payload.len();
            let prefix_len = varint_size(total_length as i32);

            let mut buf = BytesMut::with_capacity(prefix_len + total_length);
            encode_varint(total_length as i32, &mut buf);
            encode_varint(packet.id, &mut buf);
            buf.extend_from_slice(&packet.payload);
            Ok(buf)
        }
        Some(thresh) => {
            let id_len = varint_size(packet.id);
            let uncompressed_len = id_len + packet.payload.len();

            if uncompressed_len < thresh {
                // Data length is 0 (1-byte VarInt = 0x00)
                let total_packet_len = 1 + uncompressed_len;
                let prefix_len = varint_size(total_packet_len as i32);

                let mut buf = BytesMut::with_capacity(prefix_len + total_packet_len);
                encode_varint(total_packet_len as i32, &mut buf);
                encode_varint(0, &mut buf); // Data Length = 0
                encode_varint(packet.id, &mut buf);
                buf.extend_from_slice(&packet.payload);
                Ok(buf)
            } else {
                use flate2::write::ZlibEncoder;
                use flate2::Compression;
                use std::io::Write;

                let mut uncompressed_buf = BytesMut::with_capacity(uncompressed_len);
                encode_varint(packet.id, &mut uncompressed_buf);
                uncompressed_buf.extend_from_slice(&packet.payload);

                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&uncompressed_buf)?;
                let compressed = encoder.finish()?;

                let data_length = uncompressed_len as i32;
                let data_len_size = varint_size(data_length);
                let total_packet_len = data_len_size + compressed.len();
                let prefix_len = varint_size(total_packet_len as i32);

                let mut buf = BytesMut::with_capacity(prefix_len + total_packet_len);
                encode_varint(total_packet_len as i32, &mut buf);
                encode_varint(data_length, &mut buf);
                buf.extend_from_slice(&compressed);
                Ok(buf)
            }
        }
    }
}

/// Serializes and writes a raw Minecraft packet to an asynchronous stream,
/// applying Zlib compression when `threshold` is `Some(thresh)` and payload size >= `thresh`.
pub async fn write_packet_with_compression<W: AsyncWrite + Unpin>(
    writer: &mut W,
    packet: &RawPacket,
    threshold: Option<usize>,
) -> Result<(), ProxyError> {
    let buf = encode_packet_with_compression(packet, threshold)?;
    writer.write_all(&buf).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use tokio_test::io::Builder;

    #[tokio::test]
    async fn test_read_back_to_back_packets() {
        // Construct Packet 1: Handshake (id: 0x00, payload: [0x02, 0x01, 0x7F])
        // Packet ID 0x00 is 1 byte, payload is 3 bytes -> total length = 4
        // Wire: [0x04 (len), 0x00 (id), 0x02, 0x01, 0x7F]
        let packet1_bytes = vec![0x04, 0x00, 0x02, 0x01, 0x7F];

        // Construct Packet 2: Ping (id: 0x01, payload: [0xAA, 0xBB, 0xCC, 0xDD])
        // Packet ID 0x01 is 1 byte, payload is 4 bytes -> total length = 5
        // Wire: [0x05 (len), 0x01 (id), 0xAA, 0xBB, 0xCC, 0xDD]
        let packet2_bytes = vec![0x05, 0x01, 0xAA, 0xBB, 0xCC, 0xDD];

        // Construct Packet 3: Disconnect (id: 0x40, payload: [0x01, 0x02])
        // Packet ID 0x40 is 1 byte, payload is 2 bytes -> total length = 3
        // Wire: [0x03 (len), 0x40 (id), 0x01, 0x02]
        let packet3_bytes = vec![0x03, 0x40, 0x01, 0x02];

        let mut combined_stream = Vec::new();
        combined_stream.extend_from_slice(&packet1_bytes);
        combined_stream.extend_from_slice(&packet2_bytes);
        combined_stream.extend_from_slice(&packet3_bytes);

        let mut mock = Builder::new().read(&combined_stream).build();

        // 1. Read packet 1
        let p1 = read_packet(&mut mock, DEFAULT_MAX_PACKET_SIZE)
            .await
            .expect("Failed to read packet 1");
        assert_eq!(p1.id, 0x00);
        assert_eq!(&p1.payload[..], &[0x02, 0x01, 0x7F]);

        // 2. Read packet 2
        let p2 = read_packet(&mut mock, DEFAULT_MAX_PACKET_SIZE)
            .await
            .expect("Failed to read packet 2");
        assert_eq!(p2.id, 0x01);
        assert_eq!(&p2.payload[..], &[0xAA, 0xBB, 0xCC, 0xDD]);

        // 3. Read packet 3
        let p3 = read_packet(&mut mock, DEFAULT_MAX_PACKET_SIZE)
            .await
            .expect("Failed to read packet 3");
        assert_eq!(p3.id, 0x40);
        assert_eq!(&p3.payload[..], &[0x01, 0x02]);
    }

    #[tokio::test]
    async fn test_write_packet() {
        let packet = RawPacket::new(0x02, vec![0x10, 0x20, 0x30]);
        // Packet ID 0x02 (1 byte), payload 3 bytes -> total len = 4
        // Expected wire bytes: [0x04 (len), 0x02 (id), 0x10, 0x20, 0x30]
        let expected = vec![0x04, 0x02, 0x10, 0x20, 0x30];

        let mut mock = Builder::new().write(&expected).build();

        write_packet(&mut mock, &packet)
            .await
            .expect("Failed to write packet");
    }

    #[tokio::test]
    async fn test_packet_too_large_rejection() {
        // Construct header claiming length of 100 bytes
        let mut stream = BytesMut::new();
        encode_varint(100, &mut stream);

        let mut mock = Builder::new().read(&stream).build();

        // Max size is 50, so reading 100 bytes must be rejected
        match read_packet(&mut mock, 50).await {
            Err(ProxyError::PacketTooLarge(len)) => assert_eq!(len, 100),
            other => panic!("Expected PacketTooLarge(100), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_roundtrip_write_then_read() {
        let original = RawPacket::new(0x25, vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03]);

        let mut buffer = Vec::new();
        write_packet(&mut buffer, &original)
            .await
            .expect("Failed to write");

        let mut cursor = &buffer[..];
        let decoded = read_packet(&mut cursor, DEFAULT_MAX_PACKET_SIZE)
            .await
            .expect("Failed to read");

        assert_eq!(original, decoded);
    }

    #[tokio::test]
    async fn test_compressed_packet_roundtrip_above_threshold() {
        let large_payload = vec![0x42u8; 1024]; // 1 KB payload > 256 threshold
        let original = RawPacket::new(0x21, large_payload);

        let mut buffer = Vec::new();
        write_packet_with_compression(&mut buffer, &original, Some(256))
            .await
            .expect("Failed to write compressed packet");

        // Assert wire size is significantly smaller due to Zlib compression
        assert!(buffer.len() < original.payload.len());

        let mut cursor = &buffer[..];
        let decoded = read_packet_with_compression(&mut cursor, DEFAULT_MAX_PACKET_SIZE, Some(256))
            .await
            .expect("Failed to read compressed packet");

        assert_eq!(original, decoded);
    }

    #[tokio::test]
    async fn test_uncompressed_packet_roundtrip_below_threshold() {
        let small_payload = vec![0x01, 0x02, 0x03, 0x04]; // 4 bytes < 256 threshold
        let original = RawPacket::new(0x0F, small_payload);

        let mut buffer = Vec::new();
        write_packet_with_compression(&mut buffer, &original, Some(256))
            .await
            .expect("Failed to write small packet");

        let mut cursor = &buffer[..];
        let decoded = read_packet_with_compression(&mut cursor, DEFAULT_MAX_PACKET_SIZE, Some(256))
            .await
            .expect("Failed to read small packet");

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_set_compression_packet_roundtrip() {
        let pkt = SetCompressionPacket::new(256);
        let raw = pkt.encode();
        assert_eq!(raw.id, 0x03);

        let decoded = SetCompressionPacket::decode(&raw).expect("Decode failed");
        assert_eq!(decoded.threshold, 256);
    }

    #[test]
    fn test_disconnect_packet_encoding() {
        let disconnect = DisconnectPacket::new("§cServer is restarting");
        let login_raw = disconnect.encode_login();
        assert_eq!(login_raw.id, 0x00);
        assert!(String::from_utf8_lossy(&login_raw.payload).contains("restarting"));

        let play_raw = disconnect.encode_play();
        assert_eq!(play_raw.id, 0x1B);
        assert!(String::from_utf8_lossy(&play_raw.payload).contains("restarting"));

        // Modern 1.21.4 (protocol 776 >= 766) configuration disconnect
        let config_776 = disconnect.encode_config(776);
        assert_eq!(config_776.id, 0x02);
        assert!(String::from_utf8_lossy(&config_776.payload).contains("text"));
        assert!(String::from_utf8_lossy(&config_776.payload).contains("restarting"));

        // 1.20.4 (protocol 765 < 766) configuration disconnect
        let config_765 = disconnect.encode_config(765);
        assert_eq!(config_765.id, 0x01);
        assert!(String::from_utf8_lossy(&config_765.payload).contains("restarting"));

        // encode_for_client helper
        let client_login = disconnect.encode_for_client(776, false);
        assert_eq!(client_login.id, 0x00);
        let client_config = disconnect.encode_for_client(776, true);
        assert_eq!(client_config.id, 0x02);
        let client_legacy_play = disconnect.encode_for_client(763, true);
        assert_eq!(client_legacy_play.id, 0x1B);
    }

    #[test]
    fn test_encode_packet_with_compression_exact_wire_bytes() {
        // SetCompression packet (ID: 0x03, payload: [0x80, 0x02] = 256)
        let set_comp = SetCompressionPacket::new(256).encode();
        let uncompressed_wire = encode_packet_with_compression(&set_comp, None).unwrap();
        assert_eq!(&uncompressed_wire[..], &[0x03, 0x03, 0x80, 0x02]);

        // Small packet under compression threshold 256:
        // total length = 1 (data len 0x00) + 1 (id 0x02) + 4 (payload) = 6
        let small_pkt = RawPacket::new(0x02, vec![0x10, 0x20, 0x30, 0x40]);
        let wire_under_thresh = encode_packet_with_compression(&small_pkt, Some(256)).unwrap();
        assert_eq!(
            &wire_under_thresh[..],
            &[0x06, 0x00, 0x02, 0x10, 0x20, 0x30, 0x40]
        );
    }

    #[tokio::test]
    async fn test_compressed_packet_below_threshold_rejected() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        // Construct a compressed packet claiming data_length = 100 (< 256 threshold)
        let uncompressed = vec![0x42u8; 100];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&uncompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut body = BytesMut::new();
        encode_varint(100, &mut body); // data_length = 100 < 256
        body.put_slice(&compressed);

        let mut wire = BytesMut::new();
        encode_varint(body.len() as i32, &mut wire);
        wire.put_slice(&body);

        let mut cursor = &wire[..];
        let res =
            read_packet_with_compression(&mut cursor, DEFAULT_MAX_PACKET_SIZE, Some(256)).await;
        assert!(
            res.is_err(),
            "Expected error for compressed packet with data_length < threshold"
        );
    }

    #[tokio::test]
    async fn test_decompression_bomb_clamped() {
        use flate2::write::ZlibEncoder;
        use flate2::Compression;
        use std::io::Write;

        // Construct a compressed payload that expands to 1000 bytes, but claim data_length = 100
        let uncompressed = vec![0x42u8; 1000];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&uncompressed).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut body = BytesMut::new();
        encode_varint(100, &mut body); // Lie about data_length: claims 100
        body.put_slice(&compressed);

        let mut wire = BytesMut::new();
        encode_varint(body.len() as i32, &mut wire);
        wire.put_slice(&body);

        let mut cursor = &wire[..];
        // With threshold = 50, data_length 100 >= 50, but decompression expands past 100
        let res =
            read_packet_with_compression(&mut cursor, DEFAULT_MAX_PACKET_SIZE, Some(50)).await;
        assert!(
            res.is_err(),
            "Expected error for decompression bomb exceeding claimed data_length"
        );
    }
}
