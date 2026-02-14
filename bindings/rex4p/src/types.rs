use std::sync::Arc;

use pyo3::{exceptions::PyValueError, prelude::*};
use rex_client::RexClientConfig;
use rex_core::{Protocol, RetCode, RexCommand, RexData};

use crate::PyClientHandler;

/// 协议类型
#[pyclass(name = "Protocol", from_py_object)]
#[derive(Clone)]
pub struct PyProtocol {
    inner: Protocol,
}

#[pymethods]
impl PyProtocol {
    #[new]
    fn new(protocol: &str) -> PyResult<Self> {
        let inner =
            Protocol::from(protocol).ok_or_else(|| PyValueError::new_err("Invalid protocol"))?;
        Ok(Self { inner })
    }

    #[staticmethod]
    fn tcp() -> Self {
        Self {
            inner: Protocol::Tcp,
        }
    }

    #[staticmethod]
    fn quic() -> Self {
        Self {
            inner: Protocol::Quic,
        }
    }

    #[staticmethod]
    fn websocket() -> Self {
        Self {
            inner: Protocol::WebSocket,
        }
    }

    fn __str__(&self) -> String {
        format!("{:?}", self.inner)
    }
}

impl PyProtocol {
    pub fn inner(&self) -> Protocol {
        self.inner
    }
}

/// 连接状态
#[pyclass(name = "ConnectionState", from_py_object)]
#[derive(Clone, Copy)]
pub enum PyConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

impl From<rex_client::ConnectionState> for PyConnectionState {
    fn from(state: rex_client::ConnectionState) -> Self {
        match state {
            rex_client::ConnectionState::Disconnected => PyConnectionState::Disconnected,
            rex_client::ConnectionState::Connecting => PyConnectionState::Connecting,
            rex_client::ConnectionState::Connected => PyConnectionState::Connected,
            rex_client::ConnectionState::Reconnecting => PyConnectionState::Reconnecting,
        }
    }
}

/// 命令类型
#[pyclass(name = "RexCommand", from_py_object)]
#[derive(Debug, Clone, Copy)]
pub enum PyRexCommand {
    Title,
    TitleReturn,
    Group,
    GroupReturn,
    Cast,
    CastReturn,
    Login,
    LoginReturn,
    Check,
    CheckReturn,
    RegTitle,
    RegTitleReturn,
    DelTitle,
    DelTitleReturn,
    Ack,
    AckReturn,
}

impl From<PyRexCommand> for RexCommand {
    fn from(cmd: PyRexCommand) -> Self {
        match cmd {
            PyRexCommand::Title => RexCommand::Title,
            PyRexCommand::TitleReturn => RexCommand::TitleReturn,
            PyRexCommand::Group => RexCommand::Group,
            PyRexCommand::GroupReturn => RexCommand::GroupReturn,
            PyRexCommand::Cast => RexCommand::Cast,
            PyRexCommand::CastReturn => RexCommand::CastReturn,
            PyRexCommand::Login => RexCommand::Login,
            PyRexCommand::LoginReturn => RexCommand::LoginReturn,
            PyRexCommand::Check => RexCommand::Check,
            PyRexCommand::CheckReturn => RexCommand::CheckReturn,
            PyRexCommand::RegTitle => RexCommand::RegTitle,
            PyRexCommand::RegTitleReturn => RexCommand::RegTitleReturn,
            PyRexCommand::DelTitle => RexCommand::DelTitle,
            PyRexCommand::DelTitleReturn => RexCommand::DelTitleReturn,
            PyRexCommand::Ack => RexCommand::Ack,
            PyRexCommand::AckReturn => RexCommand::AckReturn,
        }
    }
}

impl From<RexCommand> for PyRexCommand {
    fn from(cmd: RexCommand) -> Self {
        match cmd {
            RexCommand::Title => PyRexCommand::Title,
            RexCommand::TitleReturn => PyRexCommand::TitleReturn,
            RexCommand::Group => PyRexCommand::Group,
            RexCommand::GroupReturn => PyRexCommand::GroupReturn,
            RexCommand::Cast => PyRexCommand::Cast,
            RexCommand::CastReturn => PyRexCommand::CastReturn,
            RexCommand::Login => PyRexCommand::Login,
            RexCommand::LoginReturn => PyRexCommand::LoginReturn,
            RexCommand::Check => PyRexCommand::Check,
            RexCommand::CheckReturn => PyRexCommand::CheckReturn,
            RexCommand::RegTitle => PyRexCommand::RegTitle,
            RexCommand::RegTitleReturn => PyRexCommand::RegTitleReturn,
            RexCommand::DelTitle => PyRexCommand::DelTitle,
            RexCommand::DelTitleReturn => PyRexCommand::DelTitleReturn,
            RexCommand::Ack => PyRexCommand::Ack,
            RexCommand::AckReturn => PyRexCommand::AckReturn,
        }
    }
}

/// 返回码
#[pyclass(name = "RetCode", skip_from_py_object)]
#[derive(Clone, Copy)]
pub enum PyRetCode {
    Success,
    Error,
    NoTarget,
    AckTimeout,
    AckFailed,
}

impl From<RetCode> for PyRetCode {
    fn from(code: RetCode) -> Self {
        match code {
            RetCode::Success => PyRetCode::Success,
            RetCode::Error => PyRetCode::Error,
            RetCode::NoTarget => PyRetCode::NoTarget,
            RetCode::AckTimeout => PyRetCode::AckTimeout,
            RetCode::AckFailed => PyRetCode::AckFailed,
        }
    }
}

/// 消息数据
#[pyclass(name = "RexData", from_py_object)]
pub struct PyRexData {
    inner: RexData,
}

impl Clone for PyRexData {
    fn clone(&self) -> Self {
        Self {
            inner: RexData {
                content: self.inner.content.clone(),
            },
        }
    }
}

#[pymethods]
impl PyRexData {
    #[new]
    fn new(command: PyRexCommand, title: String, data: Vec<u8>) -> Self {
        let inner = RexData::new(command.into(), &title, &data);
        Self { inner }
    }

    #[staticmethod]
    fn from_string(command: PyRexCommand, title: String, text: &str) -> Self {
        let inner = RexData::new(command.into(), &title, text.as_bytes());
        Self { inner }
    }

    #[getter]
    fn command(&self) -> PyRexCommand {
        self.inner.command().into()
    }

    #[getter]
    fn data(&self) -> Vec<u8> {
        self.inner.data().to_vec()
    }

    #[getter]
    fn text(&self) -> String {
        self.inner.data_as_string_lossy()
    }

    #[getter]
    fn title(&self) -> &str {
        self.inner.title()
    }

    #[getter]
    fn retcode(&self) -> PyRetCode {
        self.inner.retcode().into()
    }

    #[getter]
    fn source(&self) -> u128 {
        self.inner.source()
    }

    fn is_success(&self) -> bool {
        self.inner.is_success()
    }

    fn __str__(&self) -> String {
        format!(
            "RexData(command={:?}, source={}, len={})",
            self.command(),
            self.source(),
            self.inner.data().len()
        )
    }
}

impl PyRexData {
    pub fn from_rex_data(data: RexData) -> Self {
        Self { inner: data }
    }

    pub fn into_inner(self) -> RexData {
        self.inner
    }

    pub fn inner(&self) -> &RexData {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut RexData {
        &mut self.inner
    }
}

/// 客户端配置
#[pyclass(name = "ClientConfig")]
pub struct PyClientConfig {
    protocol: Protocol,
    server_addr: String,
    title: String,
    handler: Option<Py<PyAny>>,
    idle_timeout: u64,
    pong_wait: u64,
    max_reconnect_attempts: u32,
    max_buffer_size: usize,
}

#[pymethods]
impl PyClientConfig {
    #[new]
    #[pyo3(signature = (server_addr, protocol, title, handler))]
    fn new(server_addr: String, protocol: PyProtocol, title: String, handler: Py<PyAny>) -> Self {
        Self {
            protocol: protocol.inner(),
            server_addr,
            title,
            handler: Some(handler),
            idle_timeout: 10,
            pong_wait: 5,
            max_reconnect_attempts: 5,
            max_buffer_size: 8 * 1024 * 1024,
        }
    }

    /// 设置空闲超时时间（秒）
    fn with_idle_timeout(mut slf: PyRefMut<'_, Self>, timeout: u64) -> PyRefMut<'_, Self> {
        slf.idle_timeout = timeout;
        slf
    }

    /// 设置心跳响应等待时间（秒）
    fn with_pong_wait(mut slf: PyRefMut<'_, Self>, wait: u64) -> PyRefMut<'_, Self> {
        slf.pong_wait = wait;
        slf
    }

    /// 设置最大重连尝试次数
    fn with_max_reconnect_attempts(
        mut slf: PyRefMut<'_, Self>,
        attempts: u32,
    ) -> PyRefMut<'_, Self> {
        slf.max_reconnect_attempts = attempts;
        slf
    }

    /// 设置最大缓冲区大小（字节）
    fn with_max_buffer_size(mut slf: PyRefMut<'_, Self>, size: usize) -> PyRefMut<'_, Self> {
        slf.max_buffer_size = size;
        slf
    }

    #[getter]
    fn server_addr(&self) -> String {
        self.server_addr.clone()
    }

    #[getter]
    fn protocol(&self) -> PyProtocol {
        PyProtocol {
            inner: self.protocol,
        }
    }

    #[getter]
    fn title(&self) -> String {
        self.title.clone()
    }

    #[getter]
    fn idle_timeout(&self) -> u64 {
        self.idle_timeout
    }

    #[getter]
    fn pong_wait(&self) -> u64 {
        self.pong_wait
    }

    #[getter]
    fn max_reconnect_attempts(&self) -> u32 {
        self.max_reconnect_attempts
    }

    fn __str__(&self) -> String {
        format!(
            "ClientConfig(server={}, protocol={:?}, title='{}', idle_timeout={}s)",
            self.server_addr, self.protocol, self.title, self.idle_timeout
        )
    }

    fn __repr__(&self) -> String {
        self.__str__()
    }
}

impl PyClientConfig {
    pub fn into_rust_config(&self) -> PyResult<RexClientConfig> {
        let server_addr = self
            .server_addr
            .parse()
            .map_err(|e| PyValueError::new_err(format!("Invalid server address: {}", e)))?;

        let handler = self
            .handler
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("Handler not set"))?;

        let py_handler = Arc::new(PyClientHandler::new(Python::attach(|py| {
            handler.clone_ref(py)
        })));

        let mut config = RexClientConfig::new(self.protocol, server_addr, &self.title, py_handler);

        config.idle_timeout = self.idle_timeout;
        config.pong_wait = self.pong_wait;
        config.max_reconnect_attempts = self.max_reconnect_attempts;
        config.max_buffer_size = self.max_buffer_size;

        Ok(config)
    }
}
