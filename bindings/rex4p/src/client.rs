use pyo3::{exceptions::PyValueError, prelude::*};
use rex_client::{RexClientConfig, RexClientTrait, open_client};
use rex_core::RexData;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{PyClientHandler, PyConnectionState, PyProtocol, PyRexData, PyRexError};

/// Rex 客户端
#[pyclass(name = "RexClient")]
pub struct PyRexClient {
    client: Arc<RwLock<Option<Arc<dyn RexClientTrait>>>>,
    config: Arc<RwLock<Option<RexClientConfig>>>,
}

#[pymethods]
impl PyRexClient {
    #[new]
    fn new() -> Self {
        Self {
            client: Arc::new(RwLock::new(None)),
            config: Arc::new(RwLock::new(None)),
        }
    }

    /// 连接到服务器
    ///
    /// Args:
    ///     server_addr: 服务器地址，例如 "127.0.0.1:8080"
    ///     protocol: 协议类型 (Protocol.tcp(), Protocol.quic(), Protocol.websocket())
    ///     title: 客户端标题
    ///     handler: 消息处理器对象，需要实现 on_login 和 on_message 方法
    fn connect<'py>(
        &'py self,
        py: Python<'py>,
        server_addr: String,
        protocol: PyProtocol,
        title: String,
        handler: Py<PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();
        let config = self.config.clone();

        let server_addr = server_addr
            .parse()
            .map_err(|e| PyValueError::new_err(format!("Invalid server address: {}", e)))?;

        let py_handler = Arc::new(PyClientHandler::new(handler));

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_config =
                RexClientConfig::new(protocol.inner(), server_addr, title, py_handler);

            *config.write().await = Some(client_config.clone());

            let rex_client = open_client(client_config)
                .await
                .map_err(|e| PyRexError::new_err(format!("Connection failed: {}", e)))?;

            *client.write().await = Some(rex_client);

            Ok(())
        })
    }

    /// 发送消息
    ///
    /// Args:
    ///     data: RexData 消息对象
    fn send<'py>(&'py self, py: Python<'py>, mut data: PyRexData) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_guard = client.read().await;
            let client = client_guard
                .as_ref()
                .ok_or_else(|| PyRexError::new_err("Client not connected"))?;

            client
                .send_data(data.inner_mut())
                .await
                .map_err(|e| PyRexError::new_err(format!("Send failed: {}", e)))?;

            Ok(())
        })
    }

    /// 发送文本消息
    ///
    /// Args:
    ///     command: 命令类型
    ///     text: 文本内容
    ///     target: 目标 ID (可选)
    ///     title: 标题 (可选)
    fn send_text<'py>(
        &'py self,
        py: Python<'py>,
        command: crate::types::PyRexCommand,
        text: String,
        target: Option<u128>,
        title: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_guard = client.read().await;
            let client = client_guard
                .as_ref()
                .ok_or_else(|| PyRexError::new_err("Client not connected"))?;

            let mut builder = RexData::builder(command.into()).data_from_string(text);

            if let Some(t) = target {
                builder = builder.target(t);
            }

            if let Some(ttl) = title {
                builder = builder.title(ttl);
            }

            let mut data = builder.build();

            client
                .send_data(&mut data)
                .await
                .map_err(|e| PyRexError::new_err(format!("Send failed: {}", e)))?;

            Ok(())
        })
    }

    /// 获取连接状态
    fn get_state<'py>(&'py self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_guard = client.read().await;
            let client = client_guard
                .as_ref()
                .ok_or_else(|| PyRexError::new_err("Client not initialized"))?;

            let state = client.get_connection_state().await;
            Ok(PyConnectionState::from(state))
        })
    }

    /// 关闭连接
    fn close<'py>(&'py self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_guard = client.read().await;
            if let Some(client) = client_guard.as_ref() {
                client.close().await;
            }
            Ok(())
        })
    }

    /// 检查是否已连接
    fn is_connected<'py>(&'py self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_guard = client.read().await;
            if let Some(client) = client_guard.as_ref() {
                let state = client.get_connection_state().await;
                Ok(matches!(state, rex_client::ConnectionState::Connected))
            } else {
                Ok(false)
            }
        })
    }
}
