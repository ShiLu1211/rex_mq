use tracing::warn;

/// 协议命令枚举
#[repr(u32)]
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RexCommand {
    Title = 9901,
    TitleReturn = 9902,
    Group = 9903,
    GroupReturn = 9904,
    Cast = 9905,
    CastReturn = 9906,
    Login = 9907,
    LoginReturn = 9908,
    Check = 9909,
    CheckReturn = 9910,
    RegTitle = 9911,
    RegTitleReturn = 9912,
    DelTitle = 9913,
    DelTitleReturn = 9914,
    Ack = 9915,       // ACK from receiver to server
    AckReturn = 9916, // ACK result from server to sender
}

impl RexCommand {
    #[inline]
    pub fn from_u32(value: u32) -> Self {
        match value {
            9901 => Self::Title,
            9902 => Self::TitleReturn,
            9903 => Self::Group,
            9904 => Self::GroupReturn,
            9905 => Self::Cast,
            9906 => Self::CastReturn,
            9907 => Self::Login,
            9908 => Self::LoginReturn,
            9909 => Self::Check,
            9910 => Self::CheckReturn,
            9911 => Self::RegTitle,
            9912 => Self::RegTitleReturn,
            9913 => Self::DelTitle,
            9914 => Self::DelTitleReturn,
            9915 => Self::Ack,
            9916 => Self::AckReturn,
            _ => {
                warn!("Invalid command: {}", value);
                Self::Login
            }
        }
    }

    #[inline]
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    /// Check if this is an ACK related command
    #[inline]
    pub fn is_ack_command(self) -> bool {
        matches!(self, Self::Ack | Self::AckReturn)
    }
}

/// 返回码枚举
#[derive(Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum RetCode {
    Success = 0,
    Error = 1,
    NoTarget = 8802,
    AckTimeout = 8803, // ACK 超时
    AckFailed = 8804,  // ACK 失败（接收方拒绝）
}

impl RetCode {
    #[inline]
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Success,
            1 => Self::Error,
            8802 => Self::NoTarget,
            8803 => Self::AckTimeout,
            8804 => Self::AckFailed,
            _ => {
                warn!("Unknown RetCode: {}", value);
                Self::Error
            }
        }
    }

    #[inline]
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    #[inline]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::Error => "Error",
            Self::NoTarget => "No target available",
            Self::AckTimeout => "ACK timeout",
            Self::AckFailed => "ACK failed",
        }
    }
}
