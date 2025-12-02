use bytes::BytesMut;
use pyo3::{exceptions::PyValueError, prelude::*};
use rex_core::{Protocol, RetCode, RexCommand, RexData};

/// 协议类型
#[pyclass(name = "Protocol")]
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
#[pyclass(name = "ConnectionState")]
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
#[pyclass(name = "RexCommand")]
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
        }
    }
}

/// 返回码
#[pyclass(name = "RetCode")]
#[derive(Clone, Copy)]
pub enum PyRetCode {
    Success,
    InvalidRequest,
    InvalidParameter,
    InternalError,
    AuthFailed,
    NoTargetAvailable,
}

impl From<RetCode> for PyRetCode {
    fn from(code: RetCode) -> Self {
        match code {
            RetCode::Success => PyRetCode::Success,
            RetCode::InvalidRequest => PyRetCode::InvalidRequest,
            RetCode::InvalidParameter => PyRetCode::InvalidParameter,
            RetCode::InternalError => PyRetCode::InternalError,
            RetCode::AuthFailed => PyRetCode::AuthFailed,
            RetCode::NoTargetAvailable => PyRetCode::NoTargetAvailable,
            _ => PyRetCode::InternalError,
        }
    }
}

/// 消息数据
#[pyclass(name = "RexData")]
#[derive(Clone)]
pub struct PyRexData {
    inner: RexData,
}

#[pymethods]
impl PyRexData {
    #[new]
    fn new(command: PyRexCommand, data: Vec<u8>) -> Self {
        let bytes_data = BytesMut::from(&data[..]);
        let inner = RexData::builder(command.into()).data(bytes_data).build();
        Self { inner }
    }

    #[staticmethod]
    fn from_string(command: PyRexCommand, text: &str) -> Self {
        let inner = RexData::builder(command.into())
            .data_from_string(text)
            .build();
        Self { inner }
    }

    #[getter]
    fn command(&self) -> PyRexCommand {
        self.inner.header().command().into()
    }

    #[getter]
    fn data(&self) -> Vec<u8> {
        self.inner.data().to_vec()
    }

    #[getter]
    fn text(&self) -> Option<String> {
        self.inner.data_as_string().ok()
    }

    #[getter]
    fn title(&self) -> Option<String> {
        self.inner.title().map(|s| s.to_string())
    }

    #[getter]
    fn retcode(&self) -> PyRetCode {
        self.inner.retcode().into()
    }

    #[getter]
    fn source(&self) -> u128 {
        self.inner.header().source()
    }

    #[getter]
    fn target(&self) -> u128 {
        self.inner.header().target()
    }

    fn is_success(&self) -> bool {
        self.inner.is_success()
    }

    fn __str__(&self) -> String {
        format!(
            "RexData(command={:?}, source={}, target={}, len={})",
            self.command(),
            self.source(),
            self.target(),
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
