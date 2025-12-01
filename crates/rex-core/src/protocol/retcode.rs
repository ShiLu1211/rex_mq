// 定义返回码枚举
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetCode {
    Success = 0,
    // 通用错误码
    InvalidRequest = 1000,
    InvalidParameter = 1001,
    InternalError = 1002,
    Timeout = 1003,
    NetworkError = 1004,
    // 认证相关错误码
    AuthRequired = 2000,
    AuthFailed = 2001,
    PermissionDenied = 2002,
    TokenExpired = 2003,
    // 业务逻辑错误码
    ResourceNotFound = 3000,
    ResourceExists = 3001,
    ResourceLocked = 3002,
    QuotaExceeded = 3003,
    // 数据相关错误码
    DataCorrupted = 4000,
    DataTooLarge = 4001,
    DataFormatError = 4002,
    // 系统相关错误码
    ServiceUnavailable = 5000,
    MaintenanceMode = 5001,
    VersionMismatch = 5002,
    NoTargetAvailable = 5003,
}

impl RetCode {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            0 => Some(RetCode::Success),
            1000 => Some(RetCode::InvalidRequest),
            1001 => Some(RetCode::InvalidParameter),
            1002 => Some(RetCode::InternalError),
            1003 => Some(RetCode::Timeout),
            1004 => Some(RetCode::NetworkError),
            2000 => Some(RetCode::AuthRequired),
            2001 => Some(RetCode::AuthFailed),
            2002 => Some(RetCode::PermissionDenied),
            2003 => Some(RetCode::TokenExpired),
            3000 => Some(RetCode::ResourceNotFound),
            3001 => Some(RetCode::ResourceExists),
            3002 => Some(RetCode::ResourceLocked),
            3003 => Some(RetCode::QuotaExceeded),
            4000 => Some(RetCode::DataCorrupted),
            4001 => Some(RetCode::DataTooLarge),
            4002 => Some(RetCode::DataFormatError),
            5000 => Some(RetCode::ServiceUnavailable),
            5001 => Some(RetCode::MaintenanceMode),
            5002 => Some(RetCode::VersionMismatch),
            5003 => Some(RetCode::NoTargetAvailable),
            _ => None,
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, RetCode::Success)
    }

    pub fn description(&self) -> &'static str {
        match self {
            RetCode::Success => "Success",
            RetCode::InvalidRequest => "Invalid request",
            RetCode::InvalidParameter => "Invalid parameter",
            RetCode::InternalError => "Internal error",
            RetCode::Timeout => "Request timeout",
            RetCode::NetworkError => "Network error",
            RetCode::AuthRequired => "Authentication required",
            RetCode::AuthFailed => "Authentication failed",
            RetCode::PermissionDenied => "Permission denied",
            RetCode::TokenExpired => "Token expired",
            RetCode::ResourceNotFound => "Resource not found",
            RetCode::ResourceExists => "Resource already exists",
            RetCode::ResourceLocked => "Resource is locked",
            RetCode::QuotaExceeded => "Quota exceeded",
            RetCode::DataCorrupted => "Data corrupted",
            RetCode::DataTooLarge => "Data too large",
            RetCode::DataFormatError => "Data format error",
            RetCode::ServiceUnavailable => "Service unavailable",
            RetCode::MaintenanceMode => "Service in maintenance mode",
            RetCode::VersionMismatch => "Version mismatch",
            RetCode::NoTargetAvailable => "No target available",
        }
    }
}
