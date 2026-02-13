use arc_swap::ArcSwapOption;
use pyo3::prelude::*;
use rex_client::{ConnectionState, RexClientTrait, open_client};
use rex_core::RexData;
use std::sync::Arc;

use crate::{PyConnectionState, PyRexData, PyRexError, handler::PyClientHandler};

/// Rex 客户端
#[pyclass(name = "RexClient")]
pub struct PyRexClient {
    client: Arc<ArcSwapOption<Arc<dyn RexClientTrait>>>,
    handler: Option<Arc<PyClientHandler>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

#[pymethods]
impl PyRexClient {
    #[new]
    fn new() -> PyResult<Self> {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| PyRexError::new_err(format!("Failed to create runtime: {}", e)))?;

        Ok(Self {
            client: Arc::new(ArcSwapOption::from(None)),
            handler: None,
            runtime: Arc::new(runtime),
        })
    }

    /// 连接到服务器
    ///
    /// Args:
    ///     config: ClientConfig 配置对象
    fn connect(
        &mut self,
        py: Python,
        config: PyRef<'_, crate::types::PyClientConfig>,
    ) -> PyResult<()> {
        let rust_config = config.into_rust_config()?;
        let runtime = self.runtime.clone();
        let client = Arc::clone(&self.client);

        // 提取 handler 引用以便后续关闭
        // 使用 Arc::into_raw 和 Arc::from_raw 进行类型擦除的转换
        let handler_ptr =
            Arc::into_raw(rust_config.client_handler.clone()) as *const PyClientHandler;
        let handler: Arc<PyClientHandler> = unsafe { Arc::from_raw(handler_ptr) };

        py.detach(|| {
            runtime.block_on(async {
                let rex_client = open_client(rust_config)
                    .await
                    .map_err(|e| PyRexError::new_err(format!("Connection failed: {}", e)))?;

                client.store(Some(Arc::new(rex_client)));

                Ok::<(), PyErr>(())
            })
        })?;

        // 存储 handler 引用用于关闭
        self.handler = Some(handler);
        Ok(())
    }

    /// 发送消息
    ///
    /// Args:
    ///     data: RexData 消息对象
    fn send(&self, py: Python, mut data: PyRexData) -> PyResult<()> {
        let runtime = self.runtime.clone();
        let client = Arc::clone(&self.client);

        py.detach(|| {
            runtime.block_on(async {
                client
                    .load()
                    .as_ref()
                    .ok_or_else(|| PyRexError::new_err("Client not connected"))?
                    .send_data(data.inner_mut())
                    .await
                    .map_err(|e| PyRexError::new_err(format!("Send failed: {}", e)))?;

                Ok(())
            })
        })
    }

    /// 发送文本消息
    ///
    /// Args:
    ///     command: 命令类型
    ///     text: 文本内容
    ///     target: 目标 ID (可选)
    ///     title: 标题 (可选)
    fn send_text(
        &self,
        py: Python,
        command: crate::types::PyRexCommand,
        text: String,
        title: String,
    ) -> PyResult<()> {
        let runtime = self.runtime.clone();
        let client = Arc::clone(&self.client);
        let mut data = RexData::new(command.into(), &title, text.as_bytes());

        py.detach(|| {
            runtime.block_on(async {
                client
                    .load()
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
    fn get_state(&self, _py: Python) -> PyResult<PyConnectionState> {
        let state = self
            .client
            .load()
            .as_ref()
            .ok_or_else(|| PyRexError::new_err("Client not initialized"))?
            .get_connection_state();
        Ok(PyConnectionState::from(state))
    }

    /// 关闭连接
    fn close(&mut self, py: Python) -> PyResult<()> {
        let runtime = self.runtime.clone();

        // 先关闭 handler 线程
        if let Some(handler) = &self.handler {
            handler.shutdown();
        }

        py.detach(|| {
            runtime.block_on(async {
                if let Some(client) = self.client.load().as_ref() {
                    client.close().await;
                }
            });
        });

        // 清空引用
        self.handler = None;
        self.client.store(None);
        Ok(())
    }

    /// 检查是否已连接
    fn is_connected(&self, _py: Python) -> PyResult<bool> {
        if let Some(client) = self.client.load().as_ref() {
            let state = client.get_connection_state();
            Ok(matches!(state, ConnectionState::Connected))
        } else {
            Ok(false)
        }
    }
}
