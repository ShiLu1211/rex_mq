use crate::types::PyRexData;
use anyhow::Result;
use pyo3::prelude::*;
use rex_client::{RexClientHandlerTrait, RexClientInner};
use rex_core::RexData;
use std::sync::Arc;

/// Python 回调处理器
pub struct PyClientHandler {
    callback: Py<PyAny>,
}

impl PyClientHandler {
    pub fn new(callback: Py<PyAny>) -> Self {
        Self { callback }
    }
}

#[async_trait::async_trait]
impl RexClientHandlerTrait for PyClientHandler {
    async fn login_ok(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        Python::attach(|py| {
            let py_data = PyRexData::from_rex_data(data);

            // 调用 Python 的 on_login 方法
            if let Ok(method) = self.callback.getattr(py, "on_login") {
                let _ = method.call1(py, (py_data,));
            }
        });
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        Python::attach(|py| {
            let py_data = PyRexData::from_rex_data(data);

            // 调用 Python 的 on_message 方法
            if let Ok(method) = self.callback.getattr(py, "on_message") {
                let _ = method.call1(py, (py_data,));
            }
        });
        Ok(())
    }
}
