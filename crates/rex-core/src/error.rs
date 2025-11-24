use std::io;
use std::string::FromUtf8Error;
use thiserror::Error;

/// Rex 协议错误类型
#[derive(Error, Debug)]
pub enum RexError {
    #[error("Insufficient data in buffer")]
    InsufficientData,

    #[error("Invalid command: {0}")]
    InvalidCommand(u32),

    #[error("Invalid return code: {0}")]
    InvalidRetCode(u32),

    #[error("Data corrupted or CRC mismatch")]
    DataCorrupted,

    #[error("Version mismatch: expected {expected}, got {actual}")]
    VersionMismatch { expected: u16, actual: u16 },

    #[error("Data too large: {field} = {size} bytes (max: {max})")]
    DataTooLarge {
        field: &'static str,
        size: usize,
        max: usize,
    },

    #[error("String encoding error: {0}")]
    InvalidString(#[from] FromUtf8Error),

    #[error("IO error: {0}")]
    IoError(#[from] io::Error),

    #[error("Protocol error: {0}")]
    ProtocolError(String),
}

// 为了向后兼容，提供简化的构造函数
impl RexError {
    pub fn invalid_command() -> Self {
        Self::InvalidCommand(0)
    }

    pub fn invalid_retcode() -> Self {
        Self::InvalidRetCode(0)
    }

    pub fn version_mismatch() -> Self {
        Self::VersionMismatch {
            expected: 1,
            actual: 0,
        }
    }

    pub fn data_too_large() -> Self {
        Self::DataTooLarge {
            field: "unknown",
            size: 0,
            max: 0,
        }
    }
}
