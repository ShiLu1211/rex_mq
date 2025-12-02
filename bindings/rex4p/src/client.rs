use pyo3::prelude::*;
use rex_client::{RexClientTrait, open_client};
use rex_core::RexData;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{PyConnectionState, PyRexData, PyRexError};

/// Rex 客户端
#[pyclass(name = "RexClient")]
pub struct PyRexClient {
    client: Arc<RwLock<Option<Arc<dyn RexClientTrait>>>>,
}

#[pymethods]
impl PyRexClient {
    #[new]
    fn new() -> Self {
        Self {
            client: Arc::new(RwLock::new(None)),
        }
    }

    /// 使用配置连接到服务器
    ///
    /// Args:
    ///     config: ClientConfig 配置对象
    fn connect<'py>(
        &'py self,
        py: Python<'py>,
        config: PyRef<crate::types::PyClientConfig>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();
        let rust_config = config.into_rust_config()?;

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let rex_client = open_client(rust_config)
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
