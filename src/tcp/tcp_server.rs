use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use bytes::{Buf, BytesMut};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream, tcp::OwnedReadHalf},
    sync::{RwLock, broadcast},
};
use tracing::{debug, error, info, warn};

use crate::{
    client::RexClient,
    command::RexCommand,
    common::now_secs,
    data::{RetCode, RexData},
    new_uuid,
    tcp::TcpSender,
};

pub struct TcpServer {
    listener: Arc<TcpListener>,
    clients: RwLock<Vec<Arc<RexClient>>>,
    shutdown_tx: Arc<broadcast::Sender<()>>,
}

impl TcpServer {
    pub async fn open(addr: SocketAddr) -> Result<Arc<Self>> {
        let listener = TcpListener::bind(addr).await?;

        let (shutdown_tx, _) = broadcast::channel(4);
        let server = Arc::new(TcpServer {
            listener: Arc::new(listener),
            clients: RwLock::new(vec![]),
            shutdown_tx: Arc::new(shutdown_tx),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server_ = server.clone();
            let mut shutdown_rx = server_.shutdown_tx.subscribe();
            async move {
                info!("Accepting connections on {}", addr);
                loop {
                    tokio::select! {
                        Ok((stream, peer_addr)) =  server_.listener.accept() => {
                            server_.clone().handle_connection(stream, peer_addr).await;
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Server received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
                info!("Stopped accepting connections");
            }
        });

        // 客户端超时清理任务
        tokio::spawn({
            let server_clone = server.clone();
            let mut shutdown_rx = server_clone.shutdown_tx.subscribe();
            async move {
                let check_interval = Duration::from_secs(15); // 检查频率
                let client_timeout = 45; // 客户端超时时间（秒）

                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(check_interval) => {
                            server_clone.cleanup_inactive_clients(client_timeout).await;
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Cleanup task received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
            }
        });

        Ok(server)
    }

    pub async fn close(&self) {
        // Send shutdown signal to all tasks
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!("Error sending shutdown signal: {}", e);
        }

        self.close_clients().await;
        info!("Shutdown complete");
    }

    // 清理不活跃的客户端
    async fn cleanup_inactive_clients(&self, timeout_secs: u64) {
        let mut clients = self.clients.write().await;
        let now = now_secs();
        let initial_count = clients.len();

        clients.retain(|client| {
            let last_active = client.last_recv();
            if now - last_active > timeout_secs {
                warn!(
                    "Client {} (addr: {}) timed out, removing...",
                    client.id(),
                    client.local_addr()
                );
                false
            } else {
                true
            }
        });

        let removed_count = initial_count - clients.len();
        if removed_count > 0 {
            info!("Cleaned up {} inactive clients", removed_count);
        }
    }
}

impl TcpServer {
    async fn handle_connection(self: Arc<Self>, stream: TcpStream, peer_addr: SocketAddr) {
        info!("New connection from {}", peer_addr);

        if let Err(e) = stream.set_nodelay(true) {
            warn!("Error setting TCP_NODELAY for {}: {}", peer_addr, e);
        }

        let (reader, writer) = stream.into_split();
        let sender = Arc::new(TcpSender::new(writer));
        let peer = Arc::new(RexClient::new(new_uuid(), peer_addr, "", sender));

        // 为每个连接启动处理任务
        tokio::spawn({
            let server_clone = self.clone();
            let peer_clone = peer.clone();
            async move {
                server_clone
                    .handle_connection_inner(peer_clone.clone(), reader)
                    .await;
                // 连接断开时清理客户端
                self.remove_client(peer_clone.id()).await;
                info!("Connection {} closed and cleaned up", peer_addr);
            }
        });
    }

    async fn handle_connection_inner(
        self: Arc<Self>,
        peer: Arc<RexClient>,
        mut reader: OwnedReadHalf,
    ) {
        let peer_addr = peer.local_addr();
        info!("Handling new connection: {}", peer_addr);

        let mut buffer = BytesMut::new();
        let mut temp_buf = vec![0u8; 8192];

        loop {
            // 从 TCP 流中读取数据
            match reader.read(&mut temp_buf).await {
                Ok(0) => {
                    info!("Connection {} closed by client", peer_addr);
                    break;
                }
                Ok(n) => {
                    // 将读取的数据添加到缓冲区
                    buffer.extend_from_slice(&temp_buf[..n]);

                    // 尝试解析完整的数据包
                    while let Some(parse_result) = RexData::try_deserialize(&buffer) {
                        match parse_result {
                            Ok((mut data, consumed_bytes)) => {
                                debug!(
                                    "Received data from {}: command={:?}, consumed {} bytes",
                                    peer_addr,
                                    data.header().command(),
                                    consumed_bytes
                                );

                                // 移除已消耗的字节
                                buffer.advance(consumed_bytes);

                                // 异步处理数据
                                tokio::spawn({
                                    let peer_clone = peer.clone();
                                    let server_clone = self.clone();
                                    async move {
                                        server_clone.handle_data(&mut data, peer_clone).await;
                                    }
                                });
                            }
                            Err(e) => {
                                error!(
                                    "Error parsing data from {}: {}, clearing buffer",
                                    peer_addr, e
                                );
                                buffer.clear();
                                break;
                            }
                        }
                    }

                    // 检查缓冲区大小，防止内存泄漏
                    if buffer.len() > 64 * 1024 {
                        warn!("Buffer too large for connection {}, clearing", peer_addr);
                        buffer.clear();
                    }
                }
                Err(e) => {
                    info!("Connection {} read error: {}", peer_addr, e);
                    break;
                }
            }
        }
    }

    async fn handle_data(&self, data: &mut RexData, peer: Arc<RexClient>) {
        let client_id = data.header().source();
        let source_client = self.find_client_by_id(client_id).await;

        // 更新客户端活跃时间
        if let Some(client) = &source_client {
            client.update_last_recv();
        }

        match data.header().command() {
            RexCommand::Title => {
                self.handle_title_message(data, client_id, &source_client)
                    .await;
            }
            RexCommand::Group => {
                self.handle_group_message(data, client_id, &source_client)
                    .await;
            }
            RexCommand::Cast => {
                self.handle_cast_message(data, client_id, &source_client)
                    .await;
            }
            RexCommand::Login => {
                self.handle_login_message(data, &source_client, peer).await;
            }
            RexCommand::Check => {
                self.handle_check_message(data, &source_client).await;
            }
            RexCommand::RegTitle => {
                self.handle_reg_title_message(data, &source_client).await;
            }
            RexCommand::DelTitle => {
                self.handle_del_title_message(data, &source_client).await;
            }
            _ => {
                debug!("Received unhandled command: {:?}", data.header().command());
            }
        }
    }

    // 处理点对点消息 (Title)
    async fn handle_title_message(
        &self,
        data: &mut RexData,
        client_id: usize,
        source_client: &Option<Arc<RexClient>>,
    ) {
        let title = data.title().unwrap_or_default().to_string();
        debug!("Received title message: {}", title);

        if let Some(target_client) = self.find_target_client(&title, client_id).await {
            data.set_target(target_client.id());

            if let Err(e) = self.send_to_client(&target_client, &data.serialize()).await {
                error!(
                    "Failed to send title message to client {}: {}",
                    target_client.id(),
                    e
                );
                self.send_error_response(
                    source_client,
                    data,
                    RexCommand::TitleReturn,
                    RetCode::NoTargetAvailable,
                )
                .await;
            }
        } else {
            warn!("No client found for title: {}", title);
            self.send_error_response(
                source_client,
                data,
                RexCommand::TitleReturn,
                RetCode::NoTargetAvailable,
            )
            .await;
        }
    }

    // 处理组消息 (Group) - 轮询发送
    async fn handle_group_message(
        &self,
        data: &mut RexData,
        client_id: usize,
        source_client: &Option<Arc<RexClient>>,
    ) {
        let title = data.title().unwrap_or_default().to_string();
        debug!("Received group message: {}", title);

        let matching_clients = self.find_matching_clients(&title, client_id).await;

        if matching_clients.is_empty() {
            warn!("No clients found for group title: {}", title);
            self.send_error_response(
                source_client,
                data,
                RexCommand::GroupReturn,
                RetCode::NoTargetAvailable,
            )
            .await;
            return;
        }

        // 安全的轮询选择
        static GROUP_ROUND_ROBIN_INDEX: AtomicUsize = AtomicUsize::new(0);
        let index =
            GROUP_ROUND_ROBIN_INDEX.fetch_add(1, Ordering::Relaxed) % matching_clients.len();
        let target_client = &matching_clients[index];

        data.set_target(target_client.id());

        if let Err(e) = self.send_to_client(target_client, &data.serialize()).await {
            error!(
                "Failed to send group message to client {}: {}",
                target_client.id(),
                e
            );
            self.send_error_response(
                source_client,
                data,
                RexCommand::GroupReturn,
                RetCode::NoTargetAvailable,
            )
            .await;
        } else {
            debug!("Sent group message to client ID: {}", target_client.id());
        }
    }

    // 处理广播消息 (Cast)
    async fn handle_cast_message(
        &self,
        data: &mut RexData,
        client_id: usize,
        source_client: &Option<Arc<RexClient>>,
    ) {
        let title = data.title().unwrap_or_default().to_string();
        debug!("Received cast message: {}", title);

        let matching_clients = self.find_matching_clients(&title, client_id).await;

        if matching_clients.is_empty() {
            warn!("No clients found for cast title: {}", title);
            self.send_error_response(
                source_client,
                data,
                RexCommand::CastReturn,
                RetCode::NoTargetAvailable,
            )
            .await;
            return;
        }

        let mut success_count = 0;
        let mut failed_clients = Vec::new();

        for client in matching_clients {
            data.set_target(client.id());

            if let Err(e) = self.send_to_client(&client, &data.serialize()).await {
                error!(
                    "Failed to send cast message to client {}: {}",
                    client.id(),
                    e
                );
                failed_clients.push(client.id());
            } else {
                success_count += 1;
            }
        }

        debug!(
            "Cast message sent to {} clients, {} failures",
            success_count,
            failed_clients.len()
        );

        // 清理发送失败的客户端
        for failed_client_id in failed_clients {
            self.remove_client(failed_client_id).await;
        }
    }

    // 处理登录消息
    async fn handle_login_message(
        &self,
        data: &mut RexData,
        source_client: &Option<Arc<RexClient>>,
        peer: Arc<RexClient>,
    ) {
        debug!("Received login message");

        if let Some(client) = source_client {
            // 已存在的客户端，更新发送器
            client.set_sender(peer.sender());
            self.send_response(client, data, RexCommand::LoginReturn)
                .await;
        } else {
            // 新客户端
            peer.set_id(data.header().source());
            peer.insert_title(data.data_as_string_lossy());
            self.add_client(peer.clone()).await;
            self.send_response(&peer, data, RexCommand::LoginReturn)
                .await;
            info!(
                "New client {} logged in with title: {}",
                peer.id(),
                data.data_as_string_lossy()
            );
        }
    }

    // 处理心跳检查
    async fn handle_check_message(
        &self,
        data: &mut RexData,
        source_client: &Option<Arc<RexClient>>,
    ) {
        debug!("Received check message");

        if let Some(client) = source_client {
            self.send_response(client, data, RexCommand::CheckReturn)
                .await;
        } else {
            warn!("Received check from unknown client");
        }
    }

    // 处理标题注册
    async fn handle_reg_title_message(
        &self,
        data: &mut RexData,
        source_client: &Option<Arc<RexClient>>,
    ) {
        let title = data.data_as_string_lossy();
        debug!("Received reg title: {}", title);

        if let Some(client) = source_client {
            client.insert_title(title.clone());
            self.send_response(client, data, RexCommand::RegTitleReturn)
                .await;
            info!("Client {} registered title: {}", client.id(), title);
        } else {
            warn!("Received reg title from unknown client");
        }
    }

    // 处理标题删除
    async fn handle_del_title_message(
        &self,
        data: &mut RexData,
        source_client: &Option<Arc<RexClient>>,
    ) {
        let title = data.data_as_string_lossy();
        debug!("Received del title: {}", title);

        if let Some(client) = source_client {
            client.remove_title(&title);
            self.send_response(client, data, RexCommand::DelTitleReturn)
                .await;
            info!("Client {} removed title: {}", client.id(), title);
        } else {
            warn!("Received del title from unknown client");
        }
    }

    // 辅助方法：查找目标客户端（第一个匹配）
    async fn find_target_client(&self, title: &str, exclude_id: usize) -> Option<Arc<RexClient>> {
        let clients = self.clients.read().await;
        clients
            .iter()
            .find(|client| client.has_title(title) && client.id() != exclude_id)
            .cloned()
    }

    // 辅助方法：查找所有匹配的客户端
    async fn find_matching_clients(&self, title: &str, exclude_id: usize) -> Vec<Arc<RexClient>> {
        let clients = self.clients.read().await;
        clients
            .iter()
            .filter(|client| client.has_title(title) && client.id() != exclude_id)
            .cloned()
            .collect()
    }

    // 辅助方法：发送消息到客户端并处理错误
    async fn send_to_client(&self, client: &Arc<RexClient>, data: &BytesMut) -> Result<()> {
        match client.send_buf(data).await {
            Ok(_) => Ok(()),
            Err(e) => {
                warn!(
                    "Client {} appears disconnected, will be removed: {}",
                    client.id(),
                    e
                );
                // 标记客户端需要被移除（在下次清理时处理）
                Err(e)
            }
        }
    }

    // 辅助方法：发送响应
    async fn send_response(
        &self,
        client: &Arc<RexClient>,
        data: &mut RexData,
        command: RexCommand,
    ) {
        if let Err(e) = client
            .send_buf(&data.set_command(command).serialize())
            .await
        {
            warn!("Error sending response to client {}: {}", client.id(), e);
        }
    }

    // 辅助方法：发送错误响应
    async fn send_error_response(
        &self,
        source_client: &Option<Arc<RexClient>>,
        data: &mut RexData,
        command: RexCommand,
        retcode: RetCode,
    ) {
        if let Some(client) = source_client
            && let Err(e) = client
                .send_buf(&data.set_command(command).set_retcode(retcode).serialize())
                .await
        {
            warn!(
                "Error sending error response to client {}: {}",
                client.id(),
                e
            );
        }
    }

    // 添加客户端
    async fn add_client(&self, client: Arc<RexClient>) {
        let mut clients = self.clients.write().await;
        clients.push(client);
        info!("Total clients: {}", clients.len());
    }

    // 移除客户端
    async fn remove_client(&self, client_id: usize) {
        let mut clients = self.clients.write().await;
        let initial_len = clients.len();
        clients.retain(|client| client.id() != client_id);

        if clients.len() < initial_len {
            info!(
                "Removed client {}, remaining clients: {}",
                client_id,
                clients.len()
            );
        }
    }

    // 关闭所有客户端
    async fn close_clients(&self) {
        let clients = self.clients.read().await;
        let mut close_tasks = Vec::new();

        for client in clients.iter() {
            let client_clone = client.clone();
            close_tasks.push(tokio::spawn(async move {
                if let Err(e) = client_clone.close().await {
                    warn!("Error closing client {}: {}", client_clone.id(), e);
                }
            }));
        }

        // 等待所有客户端关闭完成
        for task in close_tasks {
            if let Err(e) = task.await {
                error!("Error waiting for client close task: {}", e);
            }
        }

        info!("All clients closed");
    }

    // 根据ID查找客户端
    async fn find_client_by_id(&self, id: usize) -> Option<Arc<RexClient>> {
        let clients = self.clients.read().await;
        clients.iter().find(|client| client.id() == id).cloned()
    }
}
