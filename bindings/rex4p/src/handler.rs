use crate::types::PyRexData;
use anyhow::Result;
use pyo3::prelude::*;
use rex_client::RexClientHandlerTrait;
use rex_core::{RexClientInner, RexData};
use std::sync::Arc;

/// Python 回调处理器
pub struct PyClientHandler {
    tx: kanal::Sender<HandlerMessage>,
}

enum HandlerMessage {
    Login(RexData),
    Message(RexData),
}

impl PyClientHandler {
    pub fn new(callback: Py<PyAny>) -> Self {
        let (tx, rx) = kanal::bounded::<HandlerMessage>(10000);

        std::thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                match msg {
                    HandlerMessage::Login(data) => {
                        let py_data = PyRexData::from_rex_data(data);
                        Python::attach(|py| {
                            Self::handle_login(py, &callback, py_data);
                        });
                    }
                    HandlerMessage::Message(data) => {
                        let py_data = PyRexData::from_rex_data(data);
                        Python::attach(|py| {
                            Self::handle(py, &callback, py_data);
                        });
                    }
                };
            }
        });

        Self { tx }
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
