use crate::types::PyRexData;
use anyhow::Result;
use kanal::ReceiveErrorTimeout;
use pyo3::prelude::*;
use rex_client::RexClientHandlerTrait;
use rex_core::{RexClientInner, RexData};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Python 回调处理器
pub struct PyClientHandler {
    tx: kanal::Sender<HandlerMessage>,
    running: Arc<AtomicBool>,
}

enum HandlerMessage {
    Login(RexData),
    Message(RexData),
    Shutdown,
}

impl PyClientHandler {
    pub fn new(callback: Py<PyAny>) -> Self {
        let (tx, rx) = kanal::bounded::<HandlerMessage>(10000);
        let running = Arc::new(AtomicBool::new(true));

        let running_clone = running.clone();
        std::thread::spawn(move || {
            while running_clone.load(Ordering::SeqCst) {
                // 使用超时接收，支持定期检查 running 标志
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(HandlerMessage::Login(data)) => {
                        let py_data = PyRexData::from_rex_data(data);
                        Python::attach(|py| {
                            Self::handle_login(py, &callback, py_data);
                        });
                    }
                    Ok(HandlerMessage::Message(data)) => {
                        let py_data = PyRexData::from_rex_data(data);
                        Python::attach(|py| {
                            Self::handle(py, &callback, py_data);
                        });
                    }
                    Ok(HandlerMessage::Shutdown) => {
                        // 处理完所有待处理消息后退出
                        break;
                    }
                    Err(ReceiveErrorTimeout::Timeout) => {
                        // 超时继续循环检查 running 标志
                        continue;
                    }
                    Err(ReceiveErrorTimeout::Closed | ReceiveErrorTimeout::SendClosed) => {
                        // 发送端已关闭，退出
                        break;
                    }
                }
            }
        });

        Self { tx, running }
    }

    fn handle_login(py: Python, callback: &Py<PyAny>, py_data: PyRexData) {
        if let Ok(method) = callback.getattr(py, "on_login")
            && let Err(e) = method.call1(py, (py_data,))
        {
            eprintln!("Login callback error: {:?}", e);
        }
    }

    fn handle(py: Python, callback: &Py<PyAny>, py_data: PyRexData) {
        if let Ok(method) = callback.getattr(py, "on_message")
            && let Err(e) = method.call1(py, (py_data,))
        {
            eprintln!("handler callback error: {:?}", e);
        }
    }

    /// 优雅关闭处理器
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = self.tx.send(HandlerMessage::Shutdown);
    }
}

#[async_trait::async_trait]
impl RexClientHandlerTrait for PyClientHandler {
    async fn login_ok(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        let _ = self.tx.send(HandlerMessage::Login(data));
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        let _ = self.tx.send(HandlerMessage::Message(data));
        Ok(())
    }
}
