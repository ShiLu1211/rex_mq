use pyo3::prelude::*;
use rex_client::{ConnectionState, RexClientTrait, open_client};
use rex_core::RexData;
use rex_core::utils::force_set_value;
use std::sync::Arc;

use crate::{PyConnectionState, PyRexData, PyRexError};

/// Rex 客户端
#[pyclass(name = "RexClient")]
pub struct PyRexClient {
    client: Option<Arc<dyn RexClientTrait>>,

    #[cfg(feature = "sync")]
    runtime: Arc<tokio::runtime::Runtime>,
}

#[pymethods]
impl PyRexClient {
    #[new]
    fn new() -> PyResult<Self> {
        #[cfg(feature = "sync")]
        {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| PyRexError::new_err(format!("Failed to create runtime: {}", e)))?;

            Ok(Self {
                client: None,
                runtime: Arc::new(runtime),
            })
        }

        #[cfg(feature = "async")]
        {
            Ok(Self { client: None })
        }
    }

    /// 异步连接到服务器
    ///
    /// Args:
    ///     config: ClientConfig 配置对象
    #[cfg(feature = "async")]
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

    /// 同步连接到服务器
    ///
    /// Args:
    ///     config: ClientConfig 配置对象
    #[cfg(feature = "sync")]
    fn connect(&self, py: Python, config: PyRef<crate::types::PyClientConfig>) -> PyResult<()> {
        let rust_config = config.into_rust_config()?;
        let runtime = self.runtime.clone();

        py.detach(|| {
            runtime.block_on(async {
                let rex_client = open_client(rust_config)
                    .await
                    .map_err(|e| PyRexError::new_err(format!("Connection failed: {}", e)))?;

                force_set_value(&self.client, Some(rex_client));

                Ok(())
            })
        })
    }

    /// 异步发送消息
    ///
    /// Args:
    ///     data: RexData 消息对象
    #[cfg(feature = "async")]
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

    /// 同步发送消息
    ///
    /// Args:
    ///     data: RexData 消息对象
    #[cfg(feature = "sync")]
    fn send(&self, py: Python, mut data: PyRexData) -> PyResult<()> {
        let runtime = self.runtime.clone();

        py.detach(|| {
            runtime.block_on(async {
                self.client
                    .as_ref()
                    .ok_or_else(|| PyRexError::new_err("Client not connected"))?
                    .send_data(data.inner_mut())
                    .await
                    .map_err(|e| PyRexError::new_err(format!("Send failed: {}", e)))?;

                Ok(())
            })
        })
    }

    /// 异步发送文本消息
    ///
    /// Args:
    ///     command: 命令类型
    ///     text: 文本内容
    ///     target: 目标 ID (可选)
    ///     title: 标题 (可选)
    #[cfg(feature = "async")]
    fn send_text<'py>(
        &'py self,
        py: Python<'py>,
        command: crate::types::PyRexCommand,
        text: String,
        title: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_guard = client.read().await;
            let client = client_guard
                .as_ref()
                .ok_or_else(|| PyRexError::new_err("Client not connected"))?;

            let mut builder = RexData::builder(command.into()).data_from_string(text);

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

    /// 同步发送文本消息
    ///
    /// Args:
    ///     command: 命令类型
    ///     text: 文本内容
    ///     target: 目标 ID (可选)
    ///     title: 标题 (可选)
    #[cfg(feature = "sync")]
    fn send_text(
        &self,
        py: Python,
        command: crate::types::PyRexCommand,
        text: String,
        title: String,
    ) -> PyResult<()> {
        let runtime = self.runtime.clone();

        py.detach(|| {
            runtime.block_on(async {
                let mut data = RexData::new(command.into(), title, text.into());

                self.client
                    .as_ref()
                    .ok_or_else(|| PyRexError::new_err("Client not connected"))?
                    .send_data(&mut data)
                    .await
                    .map_err(|e| PyRexError::new_err(format!("Send failed: {}", e)))?;

                Ok(())
            })
        })
    }

    /// 获取连接状态
    #[cfg(feature = "async")]
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

    /// 同步获取连接状态
    #[cfg(feature = "sync")]
    fn get_state(&self, _py: Python) -> PyResult<PyConnectionState> {
        let state = self
            .client
            .as_ref()
            .ok_or_else(|| PyRexError::new_err("Client not initialized"))?
            .get_connection_state();
        Ok(PyConnectionState::from(state))
    }

    /// 关闭连接
    #[cfg(feature = "async")]
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

    /// 同步关闭连接
    #[cfg(feature = "sync")]
    fn close(&self, py: Python) -> PyResult<()> {
        let runtime = self.runtime.clone();

        py.detach(|| {
            runtime.block_on(async {
                self.client
                    .as_ref()
                    .ok_or_else(|| PyRexError::new_err("Client not initialized"))?
                    .close()
                    .await;
                Ok(())
            })
        })
    }

    /// 检查是否已连接
    #[cfg(feature = "async")]
    fn is_connected<'py>(&'py self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let client = self.client.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let client_guard = client.read().await;
            if let Some(client) = client_guard.as_ref() {
                let state = client.get_connection_state().await;
                Ok(matches!(state, ConnectionState::Connected))
            } else {
                Ok(false)
            }
        })
    }

    /// 同步检查是否已连接
    #[cfg(feature = "sync")]
    fn is_connected(&self, _py: Python) -> PyResult<bool> {
        if let Some(client) = self.client.as_ref() {
            let state = client.get_connection_state();
            Ok(matches!(state, ConnectionState::Connected))
        } else {
            Ok(false)
        }
    }
}
