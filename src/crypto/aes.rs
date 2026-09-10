use aes::Aes128;
use cfb8::cipher::inout::InOutBuf;
use cfb8::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use zeroize::Zeroize;

pub type Aes128Cfb8Enc = cfb8::Encryptor<Aes128>;
pub type Aes128Cfb8Dec = cfb8::Decryptor<Aes128>;

/// Wraps an asynchronous stream with AES-128-CFB8 symmetric encryption and decryption.
///
/// Minecraft Java Edition protocol requires AES-128 in 8-bit CFB mode (`CFB8`),
/// where the 16-byte shared secret negotiated via RSA serves as both the cipher
/// key and initialization vector (IV).
pub struct EncryptedStream<S> {
    inner: Option<S>,
    encryptor: Aes128Cfb8Enc,
    decryptor: Aes128Cfb8Dec,
    write_buf: Vec<u8>,
    write_cursor: usize,
    shared_secret: [u8; 16],
}

impl<S> Drop for EncryptedStream<S> {
    fn drop(&mut self) {
        self.shared_secret.zeroize();
        self.write_buf.zeroize();
    }
}

impl<S> Zeroize for EncryptedStream<S> {
    fn zeroize(&mut self) {
        self.shared_secret.zeroize();
        self.write_buf.zeroize();
    }
}

impl<S> EncryptedStream<S> {
    /// Creates a new `EncryptedStream` using the provided 16-byte shared secret
    /// as both the AES-128 key and CFB8 initialization vector (IV).
    pub fn new(inner: S, shared_secret: &[u8; 16]) -> Self {
        let encryptor = Aes128Cfb8Enc::new(shared_secret.into(), shared_secret.into());
        let decryptor = Aes128Cfb8Dec::new(shared_secret.into(), shared_secret.into());

        Self {
            inner: Some(inner),
            encryptor,
            decryptor,
            write_buf: Vec::new(),
            write_cursor: 0,
            shared_secret: *shared_secret,
        }
    }

    pub fn get_ref(&self) -> Option<&S> {
        self.inner.as_ref()
    }

    pub fn get_mut(&mut self) -> Option<&mut S> {
        self.inner.as_mut()
    }

    /// Consumes the wrapper and unwraps the inner stream, securely wiping the shared secret.
    pub fn into_inner(mut self) -> S {
        self.shared_secret.zeroize();
        self.write_buf.zeroize();
        self.inner.take().unwrap_or_else(|| {
            unreachable!(
                "EncryptedStream inner is guaranteed present until into_inner consumes self"
            )
        })
    }
}

fn flush_pending_write<S: AsyncWrite + Unpin>(
    inner: &mut S,
    write_buf: &mut Vec<u8>,
    write_cursor: &mut usize,
    cx: &mut Context<'_>,
) -> Poll<std::io::Result<()>> {
    while *write_cursor < write_buf.len() {
        let pending = &write_buf[*write_cursor..];
        match Pin::new(&mut *inner).poll_write(cx, pending) {
            Poll::Ready(Ok(0)) => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "failed to write buffered encrypted bytes to inner stream",
                )));
            }
            Poll::Ready(Ok(n)) => {
                *write_cursor += n;
            }
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }
    }
    write_buf.clear();
    *write_cursor = 0;
    Poll::Ready(Ok(()))
}

impl<S: AsyncRead + Unpin> AsyncRead for EncryptedStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before_filled = buf.filled().len();
        let this = self.get_mut();
        let inner = match this.inner.as_mut() {
            Some(i) => i,
            None => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "EncryptedStream inner stream is uninitialized or closed",
                )));
            }
        };

        match Pin::new(inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let after_filled = buf.filled().len();
                let new_bytes = &mut buf.filled_mut()[before_filled..after_filled];
                if !new_bytes.is_empty() {
                    let inout = InOutBuf::from(&mut new_bytes[..]);
                    let (blocks, _) = inout.into_chunks();
                    this.decryptor.decrypt_blocks_inout_mut(blocks);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for EncryptedStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let inner = match this.inner.as_mut() {
            Some(i) => i,
            None => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "EncryptedStream inner stream is uninitialized or closed",
                )));
            }
        };

        // 1. Drain any previously buffered ciphertext first with backpressure
        match flush_pending_write(inner, &mut this.write_buf, &mut this.write_cursor, cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => return Poll::Pending,
        }

        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }

        // 2. Encrypt plaintext in bounded chunks up to 64 KiB
        let chunk_len = buf.len().min(64 * 1024);
        let mut ciphertext = buf[..chunk_len].to_vec();
        let inout = InOutBuf::from(&mut ciphertext[..]);
        let (blocks, _) = inout.into_chunks();
        this.encryptor.encrypt_blocks_inout_mut(blocks);

        // 3. Write directly to inner stream and record cursor if partial write or pending
        match Pin::new(inner).poll_write(cx, &ciphertext) {
            Poll::Ready(Ok(n)) => {
                if n < ciphertext.len() {
                    this.write_buf = ciphertext;
                    this.write_cursor = n;
                }
                Poll::Ready(Ok(chunk_len))
            }
            Poll::Pending => {
                this.write_buf = ciphertext;
                this.write_cursor = 0;
                Poll::Ready(Ok(chunk_len))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let inner = match this.inner.as_mut() {
            Some(i) => i,
            None => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "EncryptedStream inner stream is uninitialized or closed",
                )));
            }
        };
        match flush_pending_write(inner, &mut this.write_buf, &mut this.write_cursor, cx) {
            Poll::Ready(Ok(())) => Pin::new(inner).poll_flush(cx),
            other => other,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let inner = match this.inner.as_mut() {
            Some(i) => i,
            None => {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "EncryptedStream inner stream is uninitialized or closed",
                )));
            }
        };
        match flush_pending_write(inner, &mut this.write_buf, &mut this.write_cursor, cx) {
            Poll::Ready(Ok(())) => Pin::new(inner).poll_shutdown(cx),
            other => other,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn test_encrypted_stream_bidirectional_roundtrip() {
        let (client_io, server_io) = tokio::io::duplex(1024);
        let shared_secret: [u8; 16] = rand::random();

        let mut client_stream = EncryptedStream::new(client_io, &shared_secret);
        let mut server_stream = EncryptedStream::new(server_io, &shared_secret);

        let client_message = b"Hello, Minecraft server! Encrypted packet test 12345.";

        // Client writes to Server
        client_stream.write_all(client_message).await.unwrap();
        client_stream.flush().await.unwrap();

        let mut server_buf = vec![0u8; client_message.len()];
        server_stream.read_exact(&mut server_buf).await.unwrap();

        assert_eq!(&server_buf[..], client_message);

        // Server replies to Client
        let server_reply = b"Welcome! CFB8 symmetric stream confirmed working bit-for-bit.";
        server_stream.write_all(server_reply).await.unwrap();
        server_stream.flush().await.unwrap();

        let mut client_buf = vec![0u8; server_reply.len()];
        client_stream.read_exact(&mut client_buf).await.unwrap();

        assert_eq!(&client_buf[..], server_reply);
    }

    #[tokio::test]
    async fn test_wire_bytes_are_actually_encrypted() {
        let (client_io, mut raw_server_io) = tokio::io::duplex(1024);
        let shared_secret: [u8; 16] = [0x55; 16];

        let mut client_stream = EncryptedStream::new(client_io, &shared_secret);
        let plaintext = b"Sensitive Minecraft Handshake and Credentials";

        client_stream.write_all(plaintext).await.unwrap();
        client_stream.flush().await.unwrap();

        // Read raw bytes on the wire without decryptor
        let mut raw_wire_buf = vec![0u8; plaintext.len()];
        raw_server_io.read_exact(&mut raw_wire_buf).await.unwrap();

        // Raw bytes on the wire must not match plaintext
        assert_ne!(&raw_wire_buf[..], plaintext);

        // Manually decrypt raw wire bytes with standalone decryptor
        let mut dec = Aes128Cfb8Dec::new_from_slices(&shared_secret, &shared_secret).unwrap();
        let inout = InOutBuf::from(&mut raw_wire_buf[..]);
        let (blocks, _) = inout.into_chunks();
        dec.decrypt_blocks_inout_mut(blocks);

        assert_eq!(&raw_wire_buf[..], plaintext);
    }

    #[tokio::test]
    async fn test_multiple_consecutive_chunks_preserve_cipher_state() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let shared_secret: [u8; 16] = rand::random();

        let mut client_stream = EncryptedStream::new(client_io, &shared_secret);
        let mut server_stream = EncryptedStream::new(server_io, &shared_secret);

        let chunks: Vec<Vec<u8>> = vec![
            b"Chunk 1: Short".to_vec(),
            b"".to_vec(),
            vec![0xFF; 1],
            vec![0xAA; 7],
            vec![0x42; 256],
            b"Final chunk of arbitrary data!".to_vec(),
        ];

        for chunk in &chunks {
            if chunk.is_empty() {
                continue;
            }
            client_stream.write_all(chunk).await.unwrap();
            client_stream.flush().await.unwrap();

            let mut received = vec![0u8; chunk.len()];
            server_stream.read_exact(&mut received).await.unwrap();
            assert_eq!(&received, chunk);
        }
    }
}
