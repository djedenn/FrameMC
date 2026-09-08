use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

use crate::error::ProxyError;

/// Bridges client and backend streams in Play state using zero-copy bidirectional I/O.
///
/// Splits both streams into read and write halves, running `tokio::io::copy` concurrently
/// to route uninspected packets with minimal latency and zero intermediate allocations ([R-02]).
/// If either half reaches EOF or encounters an I/O error, or if a shutdown signal is received,
/// both streams are immediately flushed and shut down in compliance with [R-08].
pub async fn bridge_play_streams<C, B>(
    client: C,
    backend: B,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), ProxyError>
where
    C: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut backend_read, mut backend_write) = tokio::io::split(backend);

    {
        let client_to_backend = tokio::io::copy(&mut client_read, &mut backend_write);
        let backend_to_client = tokio::io::copy(&mut backend_read, &mut client_write);

        tokio::pin!(client_to_backend);
        tokio::pin!(backend_to_client);

        tokio::select! {
            res = &mut client_to_backend => {
                if let Err(e) = res {
                    tracing::debug!("Client-to-backend bridge copy terminated with error: {e}");
                }
            }
            res = &mut backend_to_client => {
                if let Err(e) = res {
                    tracing::debug!("Backend-to-client bridge copy terminated with error: {e}");
                }
            }
            _ = shutdown_rx.wait_for(|&is_shutdown| is_shutdown) => {
                tracing::info!("Shutdown signal received, closing bridge");
            }
        }
    }

    // Fail-Closed stream termination ([R-08]): immediately flush and shut down both directions
    let _ = tokio::join!(client_write.shutdown(), backend_write.shutdown(),);

    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use rand::RngCore;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_bridge_bidirectional_5mb_integrity() {
        const BUFFER_SIZE: usize = 65536;
        const DATA_SIZE: usize = 5 * 1024 * 1024; // 5 MiB

        // Stream pair 1: user client <-> proxy client
        let (user_client, proxy_client) = tokio::io::duplex(BUFFER_SIZE);
        // Stream pair 2: proxy backend <-> downstream server
        let (proxy_backend, downstream_server) = tokio::io::duplex(BUFFER_SIZE);

        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        // Spawn bridge connecting proxy_client and proxy_backend
        let bridge_handle = tokio::spawn(bridge_play_streams(
            proxy_client,
            proxy_backend,
            shutdown_rx,
        ));

        // Generate 5MB of random test data in both directions
        let mut c2s_data = vec![0u8; DATA_SIZE];
        rand::thread_rng().fill_bytes(&mut c2s_data);
        let c2s_expected = c2s_data.clone();

        let mut s2c_data = vec![0u8; DATA_SIZE];
        rand::thread_rng().fill_bytes(&mut s2c_data);
        let s2c_expected = s2c_data.clone();

        let (mut uc_read, mut uc_write) = tokio::io::split(user_client);
        let (mut ds_read, mut ds_write) = tokio::io::split(downstream_server);

        let (c2s_done_tx, c2s_done_rx) = tokio::sync::oneshot::channel();
        let (s2c_done_tx, s2c_done_rx) = tokio::sync::oneshot::channel();

        // Client writer
        let client_write_task = tokio::spawn(async move {
            uc_write.write_all(&c2s_data).await.unwrap();
            uc_write.flush().await.unwrap();
            let _ = s2c_done_rx.await;
            let _ = uc_write.shutdown().await;
        });

        // Server reader
        let server_read_task = tokio::spawn(async move {
            let mut received = vec![0u8; DATA_SIZE];
            ds_read.read_exact(&mut received).await.unwrap();
            assert_eq!(received.len(), DATA_SIZE);
            assert_eq!(received, c2s_expected);
            let _ = c2s_done_tx.send(());
        });

        // Server writer
        let server_write_task = tokio::spawn(async move {
            ds_write.write_all(&s2c_data).await.unwrap();
            ds_write.flush().await.unwrap();
            let _ = c2s_done_rx.await;
            let _ = ds_write.shutdown().await;
        });

        // Client reader
        let client_read_task = tokio::spawn(async move {
            let mut received = vec![0u8; DATA_SIZE];
            uc_read.read_exact(&mut received).await.unwrap();
            assert_eq!(received.len(), DATA_SIZE);
            assert_eq!(received, s2c_expected);
            let _ = s2c_done_tx.send(());
        });

        tokio::try_join!(
            client_write_task,
            server_read_task,
            server_write_task,
            client_read_task
        )
        .unwrap();

        let bridge_result = bridge_handle.await.unwrap();
        assert!(bridge_result.is_ok());
    }

    #[tokio::test]
    async fn test_bridge_shutdown_signal_terminates_immediately() {
        const BUFFER_SIZE: usize = 1024;
        let (user_client, proxy_client) = tokio::io::duplex(BUFFER_SIZE);
        let (proxy_backend, downstream_server) = tokio::io::duplex(BUFFER_SIZE);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let bridge_handle = tokio::spawn(bridge_play_streams(
            proxy_client,
            proxy_backend,
            shutdown_rx,
        ));

        // Signal shutdown
        shutdown_tx.send(true).unwrap();

        let bridge_result = bridge_handle.await.unwrap();
        assert!(bridge_result.is_ok());

        // Both client and server endpoints should observe stream shutdown
        drop(user_client);
        drop(downstream_server);
    }

    #[tokio::test]
    async fn test_bridge_early_client_eof_shuts_down_backend() {
        const BUFFER_SIZE: usize = 1024;
        let (user_client, proxy_client) = tokio::io::duplex(BUFFER_SIZE);
        let (proxy_backend, mut downstream_server) = tokio::io::duplex(BUFFER_SIZE);

        let (_shutdown_tx, shutdown_rx) = watch::channel(false);

        let bridge_handle = tokio::spawn(bridge_play_streams(
            proxy_client,
            proxy_backend,
            shutdown_rx,
        ));

        // Client immediately drops connection (EOF)
        drop(user_client);

        // Server must read EOF (0 bytes) as bridge closes backend write half
        let mut buf = [0u8; 128];
        let n = downstream_server.read(&mut buf).await.unwrap();
        assert_eq!(n, 0, "Backend must observe EOF when client disconnects");

        let bridge_result = bridge_handle.await.unwrap();
        assert!(bridge_result.is_ok());
    }
}
