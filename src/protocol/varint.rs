use crate::error::ProxyError;
use bytes::{Buf, BufMut};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Reads a Minecraft VarInt from an asynchronous stream.
/// Returns `ProxyError::VarIntOverflow` if the VarInt exceeds 5 bytes.
pub async fn read_varint<R: AsyncRead + Unpin>(reader: &mut R) -> Result<i32, ProxyError> {
    let mut value = 0u32;
    let mut position = 0;
    let mut bytes_read = 0;

    loop {
        let current_byte = reader.read_u8().await?;
        bytes_read += 1;

        if bytes_read == 5 && (current_byte & 0xF0) != 0 {
            return Err(ProxyError::VarIntOverflow);
        }

        value |= ((current_byte & 0x7F) as u32).wrapping_shl(position);

        if (current_byte & 0x80) == 0 {
            break;
        }

        if bytes_read >= 5 {
            return Err(ProxyError::VarIntOverflow);
        }

        position += 7;
    }

    Ok(value as i32)
}

/// Reads a Minecraft VarLong from an asynchronous stream.
/// Returns `ProxyError::VarIntOverflow` if the VarLong exceeds 10 bytes.
pub async fn read_varlong<R: AsyncRead + Unpin>(reader: &mut R) -> Result<i64, ProxyError> {
    let mut value = 0u64;
    let mut position = 0;
    let mut bytes_read = 0;

    loop {
        let current_byte = reader.read_u8().await?;
        bytes_read += 1;

        if bytes_read == 10 && (current_byte & 0xFE) != 0 {
            return Err(ProxyError::VarIntOverflow);
        }

        value |= ((current_byte & 0x7F) as u64).wrapping_shl(position);

        if (current_byte & 0x80) == 0 {
            break;
        }

        if bytes_read >= 10 {
            return Err(ProxyError::VarIntOverflow);
        }

        position += 7;
    }

    Ok(value as i64)
}

/// Decodes a Minecraft VarInt from an in-memory buffer.
/// Returns `ProxyError::Io` with `UnexpectedEof` on underflow,
/// or `ProxyError::VarIntOverflow` if greater than 5 bytes.
pub fn decode_varint(buf: &mut impl Buf) -> Result<i32, ProxyError> {
    let mut value = 0u32;
    let mut position = 0;
    let mut bytes_read = 0;

    loop {
        if !buf.has_remaining() {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected end of buffer while decoding VarInt",
            )));
        }

        let current_byte = buf.get_u8();
        bytes_read += 1;

        if bytes_read == 5 && (current_byte & 0xF0) != 0 {
            return Err(ProxyError::VarIntOverflow);
        }

        value |= ((current_byte & 0x7F) as u32).wrapping_shl(position);

        if (current_byte & 0x80) == 0 {
            break;
        }

        if bytes_read >= 5 {
            return Err(ProxyError::VarIntOverflow);
        }

        position += 7;
    }

    Ok(value as i32)
}

/// Decodes a Minecraft VarLong from an in-memory buffer.
/// Returns `ProxyError::Io` with `UnexpectedEof` on underflow,
/// or `ProxyError::VarIntOverflow` if greater than 10 bytes.
pub fn decode_varlong(buf: &mut impl Buf) -> Result<i64, ProxyError> {
    let mut value = 0u64;
    let mut position = 0;
    let mut bytes_read = 0;

    loop {
        if !buf.has_remaining() {
            return Err(ProxyError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected end of buffer while decoding VarLong",
            )));
        }

        let current_byte = buf.get_u8();
        bytes_read += 1;

        if bytes_read == 10 && (current_byte & 0xFE) != 0 {
            return Err(ProxyError::VarIntOverflow);
        }

        value |= ((current_byte & 0x7F) as u64).wrapping_shl(position);

        if (current_byte & 0x80) == 0 {
            break;
        }

        if bytes_read >= 10 {
            return Err(ProxyError::VarIntOverflow);
        }

        position += 7;
    }

    Ok(value as i64)
}

/// Encodes a 32-bit signed integer as a Minecraft VarInt into `buf`.
pub fn encode_varint(value: i32, buf: &mut impl BufMut) {
    let mut temp = value as u32;
    loop {
        if (temp & !0x7F) == 0 {
            buf.put_u8(temp as u8);
            return;
        }
        buf.put_u8(((temp & 0x7F) | 0x80) as u8);
        temp >>= 7;
    }
}

/// Encodes a 64-bit signed integer as a Minecraft VarLong into `buf`.
pub fn encode_varlong(value: i64, buf: &mut impl BufMut) {
    let mut temp = value as u64;
    loop {
        if (temp & !0x7F) == 0 {
            buf.put_u8(temp as u8);
            return;
        }
        buf.put_u8(((temp & 0x7F) | 0x80) as u8);
        temp >>= 7;
    }
}

/// Calculates the wire length in bytes of an encoded VarInt.
pub fn varint_size(value: i32) -> usize {
    match value as u32 {
        0..=0x7F => 1,
        0x80..=0x3FFF => 2,
        0x4000..=0x1F_FFFF => 3,
        0x20_0000..=0x0FFF_FFFF => 4,
        _ => 5,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use bytes::BytesMut;

    #[tokio::test]
    async fn test_wire_specification_vectors() {
        let test_cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (255, &[0xFF, 0x01]),
            (2147483647, &[0xFF, 0xFF, 0xFF, 0xFF, 0x07]),
            (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
            (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];

        for &(value, expected_bytes) in test_cases {
            // 1. varint_size check
            assert_eq!(
                varint_size(value),
                expected_bytes.len(),
                "varint_size mismatch for value {}",
                value
            );

            // 2. encode_varint check
            let mut buf = BytesMut::new();
            encode_varint(value, &mut buf);
            assert_eq!(
                &buf[..],
                expected_bytes,
                "encode_varint mismatch for value {}",
                value
            );

            // 3. decode_varint check
            let mut cursor = expected_bytes;
            let decoded = decode_varint(&mut cursor).expect("Failed to decode VarInt");
            assert_eq!(decoded, value, "decode_varint mismatch for value {}", value);
            assert_eq!(
                cursor.len(),
                0,
                "Buffer not fully consumed for value {}",
                value
            );

            // 4. read_varint async check
            let mut async_cursor = expected_bytes;
            let async_decoded = read_varint(&mut async_cursor)
                .await
                .expect("Failed to read_varint async");
            assert_eq!(
                async_decoded, value,
                "read_varint async mismatch for value {}",
                value
            );
            assert_eq!(
                async_cursor.len(),
                0,
                "Async buffer not fully consumed for value {}",
                value
            );
        }
    }

    #[tokio::test]
    async fn test_buffer_underflow() {
        // Empty buffer
        let mut empty = &[][..];
        match decode_varint(&mut empty) {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("Expected UnexpectedEof, got {:?}", other),
        }

        let mut async_empty = &[][..];
        match read_varint(&mut async_empty).await {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("Expected UnexpectedEof, got {:?}", other),
        }

        // Incomplete multi-byte VarInt (has continuation bit set, but stream ends)
        let mut incomplete = &[0x80, 0x80][..];
        match decode_varint(&mut incomplete) {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("Expected UnexpectedEof, got {:?}", other),
        }

        let mut async_incomplete = &[0x80, 0x80][..];
        match read_varint(&mut async_incomplete).await {
            Err(ProxyError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof),
            other => panic!("Expected UnexpectedEof, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_varint_overflow() {
        // 6-byte VarInt with continuation bits
        let six_bytes = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];

        let mut cursor = &six_bytes[..];
        match decode_varint(&mut cursor) {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!("Expected VarIntOverflow, got {:?}", other),
        }

        let mut async_cursor = &six_bytes[..];
        match read_varint(&mut async_cursor).await {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!("Expected VarIntOverflow, got {:?}", other),
        }

        // 5-byte VarInt with invalid upper 4 bits (e.g. 0x70)
        let five_bytes_overflow = [0x80, 0x80, 0x80, 0x80, 0x70];
        let mut cursor_5 = &five_bytes_overflow[..];
        match decode_varint(&mut cursor_5) {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!("Expected VarIntOverflow on 5th byte, got {:?}", other),
        }

        let mut async_cursor_5 = &five_bytes_overflow[..];
        match read_varint(&mut async_cursor_5).await {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!("Expected VarIntOverflow on 5th byte async, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_varlong_roundtrip_and_overflow() {
        let test_cases: &[(i64, usize)] = &[
            (0, 1),
            (1, 1),
            (127, 1),
            (128, 2),
            (255, 2),
            (2147483647, 5),
            (-1, 10),
            (-2147483648, 10),
            (i64::MAX, 9),
            (i64::MIN, 10),
        ];

        for &(value, expected_len) in test_cases {
            let mut buf = BytesMut::new();
            encode_varlong(value, &mut buf);
            assert_eq!(buf.len(), expected_len);

            let mut cursor = &buf[..];
            let decoded = decode_varlong(&mut cursor).expect("Failed to decode VarLong");
            assert_eq!(decoded, value);
            assert_eq!(cursor.len(), 0);

            let mut async_cursor = &buf[..];
            let async_decoded = read_varlong(&mut async_cursor)
                .await
                .expect("Failed to read_varlong async");
            assert_eq!(async_decoded, value);
            assert_eq!(async_cursor.len(), 0);
        }

        // 11-byte VarLong overflow
        let eleven_bytes = [
            0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01,
        ];
        let mut cursor = &eleven_bytes[..];
        match decode_varlong(&mut cursor) {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!("Expected VarIntOverflow, got {:?}", other),
        }

        let mut async_cursor = &eleven_bytes[..];
        match read_varlong(&mut async_cursor).await {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!("Expected VarIntOverflow, got {:?}", other),
        }

        // 10-byte VarLong with invalid upper 7 bits (e.g. 0x02)
        let ten_bytes_overflow = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02];
        let mut cursor_10 = &ten_bytes_overflow[..];
        match decode_varlong(&mut cursor_10) {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!("Expected VarIntOverflow on 10th byte, got {:?}", other),
        }

        let mut async_cursor_10 = &ten_bytes_overflow[..];
        match read_varlong(&mut async_cursor_10).await {
            Err(ProxyError::VarIntOverflow) => (),
            other => panic!(
                "Expected VarIntOverflow on 10th byte async, got {:?}",
                other
            ),
        }
    }
}
