#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RexCommand {
    Title = 9901,
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

impl TryFrom<u32> for RexCommand {
    type Error = anyhow::Error;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        RexCommand::from_u32(value).ok_or_else(|| anyhow::anyhow!("Invalid RexCommand {}", value))
    }
}

impl RexCommand {
    pub fn from_u32(value: u32) -> Option<Self> {
        match value {
            9901 => Some(RexCommand::Title),
            9902 => Some(RexCommand::TitleReturn),
            9903 => Some(RexCommand::Group),
            9904 => Some(RexCommand::GroupReturn),
            9905 => Some(RexCommand::Cast),
            9906 => Some(RexCommand::CastReturn),
            9907 => Some(RexCommand::Login),
            9908 => Some(RexCommand::LoginReturn),
            9909 => Some(RexCommand::Check),
            9910 => Some(RexCommand::CheckReturn),
            9911 => Some(RexCommand::RegTitle),
            9912 => Some(RexCommand::RegTitleReturn),
            9913 => Some(RexCommand::DelTitle),
            9914 => Some(RexCommand::DelTitleReturn),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> u32 {
        *self as u32
    }
}
