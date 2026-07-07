//! ConnectionDriver — the per-connection read-parse-handle loop, generic
//! over the byte source.
//!
//! Before T1, every server transport carried its own `handle_connection_inner`
//! that did the same thing: subscribe to shutdown, select between
//! "read more bytes" and "shutdown signalled", call
//! `parse_and_handle_buffer`, loop. The framing differed (TCP raw bytes vs
//! WebSocket binary frames) but the state machine was identical.
//!
//! T1 keeps the per-protocol framing adapter (which knows how to pull bytes
//! out of the protocol-specific stream) and pushes the rest of the loop
//! here. Each transport becomes a thin adapter that yields a `ByteSource`.

use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use rex_core::RexClientInner;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::{Services, transport::parse_and_handle_buffer};
use rex_observability::metrics::inc_bytes_in;

/// Abstraction over a source of byte chunks. The driver polls this and
/// accumulates into a `BytesMut` for `parse_and_handle_buffer`.
///
/// Three protocol-specific impls:
/// - `OwnedReadHalf` — tokio's TCP half (via AsyncReadExt)
/// - `WebSocketStream::SplitStream` — tungstenite binary frames
/// - `RecvStream` — quinn's per-stream bytes
pub trait ByteSource: Unpin + Send {
    /// Read the next chunk of bytes. Returns `Ok(None)` on EOF / clean
    /// close; `Ok(Some(bytes))` on data; `Err(_)` on protocol error.
    fn poll_read(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Option<Bytes>, String>> + Send;
}

/// Drives a single connection: reads chunks via `R: ByteSource`, calls
/// `parse_and_handle_buffer`, exits on EOF, server shutdown, or
/// per-client cancel.
pub struct ConnectionDriver<'a> {
    services: &'a Arc<Services>,
    peer: &'a Arc<RexClientInner>,
    peer_label: &'a str,
    /// Lowercased transport name (e.g. "tcp", "quic", "websocket") used
    /// as the label for the `rex_bytes_in_total` counter. Distinct from
    /// `peer_label` which is the display form used in logs.
    transport_label: &'a str,
    max_buffer_size: usize,
    shutdown: broadcast::Receiver<()>,
    client_token: CancellationToken,
}

impl<'a> ConnectionDriver<'a> {
    pub fn new(
        services: &'a Arc<Services>,
        peer: &'a Arc<RexClientInner>,
        peer_label: &'a str,
        transport_label: &'a str,
        max_buffer_size: usize,
    ) -> Self {
        let shutdown = services.shutdown.subscribe();
        let client_token = services.register_client_shutdown(peer.id());
        Self {
            services,
            peer,
            peer_label,
            transport_label,
            max_buffer_size,
            shutdown,
            client_token,
        }
    }

    /// Drive the connection until EOF, server shutdown, or admin-triggered
    /// per-client cancel. Any error from the `ByteSource` ends the loop.
    /// The shutdown signal and per-client token are consulted concurrently
    /// via `tokio::select!`. Either termination path unregisters the
    /// per-client token so the admin map does not leak entries.
    pub async fn drive<R: ByteSource>(mut self, mut source: R) {
        let mut buffer = BytesMut::with_capacity(self.max_buffer_size);

        loop {
            tokio::select! {
                result = source.poll_read() => {
                    match result {
                        Ok(Some(bytes)) => {
                            // Observability: record bytes received on this
                            // transport. Counter is registered lazily on first
                            // call.
                            inc_bytes_in(self.transport_label, bytes.len() as u64);
                            buffer.extend_from_slice(&bytes);
                            debug!(
                                "{} received {} bytes (buf now {})",
                                self.peer_label,
                                bytes.len(),
                                buffer.len()
                            );
                            if let Err(e) = parse_and_handle_buffer(
                                self.services,
                                self.peer,
                                &mut buffer,
                                self.max_buffer_size,
                            ).await {
                                warn!(
                                    "Error processing buffer for {}: {}",
                                    self.peer_label,
                                    e
                                );
                            }
                        }
                        Ok(None) => {
                            debug!("{} closed by peer", self.peer_label);
                            break;
                        }
                        Err(e) => {
                            warn!("{} read error: {}", self.peer_label, e);
                            break;
                        }
                    }
                }
                _ = self.shutdown.recv() => {
                    debug!(
                        "{} shutting down due to server shutdown",
                        self.peer_label
                    );
                    self.services.unregister_client_shutdown(self.peer.id());
                    break;
                }
                _ = self.client_token.cancelled() => {
                    debug!("{} admin-triggered disconnect", self.peer_label);
                    self.services.unregister_client_shutdown(self.peer.id());
                    break;
                }
            }
        }
    }
}

/// tokio `OwnedReadHalf` over TCP. Wraps `AsyncReadExt::read_buf` in a
/// future-friendly shape.
pub struct TcpByteSource<'a> {
    pub reader: &'a mut tokio::net::tcp::OwnedReadHalf,
}

impl<'a> ByteSource for TcpByteSource<'a> {
    async fn poll_read(&mut self) -> Result<Option<Bytes>, String> {
        use tokio::io::AsyncReadExt;
        let mut buf = BytesMut::with_capacity(8 * 1024);
        match self.reader.read_buf(&mut buf).await {
            Ok(0) => Ok(None),
            Ok(_n) => Ok(Some(buf.freeze())),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// tungstenite `SplitStream<WebSocketStream>`. Filters to binary frames,
/// passes through pings with a Pong reply, ignores text/pong/binary-
/// continuation, returns None on close.
pub struct WebSocketByteSource<'a, S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    pub stream: &'a mut futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<S>>,
    pub peer: &'a Arc<RexClientInner>,
}

impl<'a, S> ByteSource for WebSocketByteSource<'a, S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    async fn poll_read(&mut self) -> Result<Option<Bytes>, String> {
        use tokio_tungstenite::tungstenite::Message;
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Binary(data))) => return Ok(Some(data)),
                Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(Message::Ping(data))) => {
                    if let Err(e) = self.peer.send_buf(&Message::Pong(data).into_data()).await {
                        warn!("Failed to send pong: {}", e);
                    }
                    // Continue reading.
                }
                Some(Ok(_)) => {
                    // Ignore text/pong/continuation; loop to next message.
                }
                Some(Err(e)) => return Err(e.to_string()),
                None => return Ok(None),
            }
        }
    }
}
