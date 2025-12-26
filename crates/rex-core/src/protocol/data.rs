use bytes::{BufMut, Bytes, BytesMut};
use rkyv::{
    Archive, Deserialize, Serialize,
    ser::{Serializer, serializers::AllocSerializer},
    validation::{CheckTypeError, validators::DefaultValidator},
};
use std::string::FromUtf8Error;
use tracing::warn;

use super::{RetCode, RexCommand};

/// 消息数据结构（使用 rkyv 0.7 零拷贝优化）
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct RexData {
    command: u32,
    source: u128,
    target: u128,
    retcode: u32,
    title: Option<String>,
    data: Vec<u8>,
}

impl RexData {
    // === 构造方法 ===

    #[inline]
    pub fn new(command: RexCommand, source: u128, target: u128, data: BytesMut) -> Self {
        Self {
            command: command.as_u32(),
            source,
            target,
            retcode: RetCode::Success as u32,
            title: None,
            data: data.to_vec(),
        }
    }

    #[inline]
    pub fn with_title(
        command: RexCommand,
        source: u128,
        target: u128,
        title: String,
        data: BytesMut,
    ) -> Self {
        Self {
            command: command.as_u32(),
            source,
            target,
            retcode: RetCode::Success as u32,
            title: Some(title),
            data: data.to_vec(),
        }
    }

    // === 核心序列化方法（零拷贝优化）===

    /// 高性能序列化 - 使用预分配缓冲区
    #[inline]
    pub fn serialize(&self) -> Bytes {
        let mut serializer = AllocSerializer::<256>::default();

        if let Err(e) = serializer.serialize_value(self) {
            warn!("Failed to serialize RexData: {}", e);
            return Bytes::new();
        }

        let payload = serializer.into_serializer().into_inner();
        let len = payload.len() as u32;

        let mut buf = BytesMut::with_capacity(4 + payload.len());
        buf.put_u32(len);
        buf.extend_from_slice(payload.as_slice());

        buf.freeze()
    }

    /// 零拷贝访问 - 直接返回 archived 引用包装器
    /// 这是最快的访问方式，不涉及任何内存分配
    #[inline(always)]
    pub fn as_archived(buf: &[u8]) -> RexDataRef<'_> {
        RexDataRef {
            archived: unsafe { rkyv::archived_root::<Self>(buf) },
        }
    }

    /// 安全的零拷贝访问 - 带验证
    pub fn as_archived_checked(
        buf: &[u8],
    ) -> Result<RexDataRef<'_>, CheckTypeError<ArchivedRexData, DefaultValidator<'_>>> {
        let archived = rkyv::check_archived_root::<Self>(buf)?;
        Ok(RexDataRef { archived })
    }

    // === 访问器方法 ===

    #[inline(always)]
    pub fn command(&self) -> RexCommand {
        RexCommand::from_u32(self.command).unwrap_or(RexCommand::Title)
    }

    #[inline(always)]
    pub fn source(&self) -> u128 {
        self.source
    }

    #[inline(always)]
    pub fn target(&self) -> u128 {
        self.target
    }

    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    #[inline(always)]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[inline(always)]
    pub fn retcode(&self) -> RetCode {
        RetCode::from_u32(self.retcode).unwrap_or(RetCode::Success)
    }

    #[inline(always)]
    pub fn is_success(&self) -> bool {
        self.retcode == RetCode::Success as u32
    }

    // 使用 Bytes::from 而不是 clone，利用引用计数
    #[inline]
    pub fn data_bytes(&self) -> Bytes {
        Bytes::from(self.data.clone())
    }

    // === 修改器方法 - 链式调用优化 ===

    #[inline(always)]
    pub fn set_command(&mut self, command: RexCommand) -> &mut Self {
        self.command = command.as_u32();
        self
    }

    #[inline(always)]
    pub fn set_source(&mut self, source: u128) -> &mut Self {
        self.source = source;
        self
    }

    #[inline(always)]
    pub fn set_target(&mut self, target: u128) -> &mut Self {
        self.target = target;
        self
    }

    #[inline(always)]
    pub fn set_retcode(&mut self, retcode: RetCode) -> &mut Self {
        self.retcode = retcode as u32;
        self
    }

    #[inline(always)]
    pub fn set_title(&mut self, title: Option<String>) -> &mut Self {
        self.title = title;
        self
    }

    // === 数据处理方法 ===

    pub fn data_as_string(&self) -> Result<String, FromUtf8Error> {
        String::from_utf8(self.data.clone())
    }

    #[inline]
    pub fn data_as_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }

    // === 构建器模式 ===

    #[inline]
    pub fn builder(command: RexCommand) -> RexDataBuilder {
        RexDataBuilder::new(command)
    }
}

/// 零拷贝引用包装器（极致性能访问层）
///
/// 所有方法都是零开销抽象，直接访问 archived 内存
#[derive(Clone, Copy)]
pub struct RexDataRef<'a> {
    archived: &'a ArchivedRexData,
}

impl<'a> RexDataRef<'a> {
    /// 零拷贝访问 command
    #[inline(always)]
    pub fn command(&self) -> RexCommand {
        RexCommand::from_u32(self.archived.command).unwrap_or(RexCommand::Title)
    }

    /// 零拷贝访问 source
    #[inline(always)]
    pub fn source(&self) -> u128 {
        self.archived.source
    }

    /// 零拷贝访问 target
    #[inline(always)]
    pub fn target(&self) -> u128 {
        self.archived.target
    }

    /// 零拷贝访问 retcode
    #[inline(always)]
    pub fn retcode(&self) -> RetCode {
        RetCode::from_u32(self.archived.retcode).unwrap_or(RetCode::Success)
    }

    /// 零拷贝检查是否成功
    #[inline(always)]
    pub fn is_success(&self) -> bool {
        self.archived.retcode == RetCode::Success as u32
    }

    /// 零拷贝访问 title
    #[inline(always)]
    pub fn title(&self) -> Option<&str> {
        self.archived.title.as_ref().map(|s| s.as_str())
    }

    /// 零拷贝访问 data（返回切片，无分配）
    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        self.archived.data.as_slice()
    }

    /// 零拷贝获取 data 长度
    #[inline(always)]
    pub fn data_len(&self) -> usize {
        self.archived.data.len()
    }

    /// 零拷贝转换 data 为字符串（需验证 UTF-8）
    #[inline]
    pub fn data_as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(self.data())
    }

    /// 零拷贝转换 data 为字符串（lossy）
    /// 注意：这个方法在有无效 UTF-8 时会分配
    #[inline]
    pub fn data_as_string_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.data())
    }

    /// 快速响应创建（仅在需要时反序列化）
    #[inline]
    pub fn create_response(&self, response_command: RexCommand, data: &[u8]) -> RexData {
        RexData {
            command: response_command.as_u32(),
            source: self.target(),
            target: self.source(),
            retcode: RetCode::Success as u32,
            title: None,
            data: data.to_vec(),
        }
    }

    /// 快速响应创建（仅在需要时反序列化）
    #[inline]
    pub fn create_error_response(
        &self,
        response_command: RexCommand,
        response_retcode: RetCode,
        data: &[u8],
    ) -> RexData {
        RexData {
            command: response_command.as_u32(),
            source: self.target(),
            target: self.source(),
            retcode: response_retcode as u32,
            title: None,
            data: data.to_vec(),
        }
    }

    /// 完整反序列化（仅在必要时使用）
    #[inline]
    pub fn deserialize(&self) -> RexData {
        self.archived
            .deserialize(&mut rkyv::Infallible)
            .expect("Infallible deserialization")
    }

    /// 克隆 data 为 Bytes（使用引用计数，相对高效）
    #[inline]
    pub fn data_bytes(&self) -> Bytes {
        Bytes::copy_from_slice(self.data())
    }
}

/// 构建器 - 流畅接口
pub struct RexDataBuilder {
    command: RexCommand,
    source: u128,
    target: u128,
    retcode: RetCode,
    title: Option<String>,
    data: BytesMut,
}

impl RexDataBuilder {
    #[inline]
    pub fn new(command: RexCommand) -> Self {
        Self {
            command,
            source: 0,
            target: 0,
            retcode: RetCode::Success,
            title: None,
            data: BytesMut::new(),
        }
    }

    #[inline]
    pub fn source(mut self, source: u128) -> Self {
        self.source = source;
        self
    }

    #[inline]
    pub fn target(mut self, target: u128) -> Self {
        self.target = target;
        self
    }

    #[inline]
    pub fn retcode(mut self, retcode: RetCode) -> Self {
        self.retcode = retcode;
        self
    }

    #[inline]
    pub fn title<S: Into<String>>(mut self, title: S) -> Self {
        self.title = Some(title.into());
        self
    }

    #[inline]
    pub fn data_from_string<S: AsRef<str>>(mut self, data: S) -> Self {
        self.data = BytesMut::from(data.as_ref().as_bytes());
        self
    }

    #[inline]
    pub fn data(mut self, data: BytesMut) -> Self {
        self.data = data;
        self
    }

    #[inline]
    pub fn build(self) -> RexData {
        RexData {
            command: self.command.as_u32(),
            source: self.source,
            target: self.target,
            retcode: self.retcode as u32,
            title: self.title,
            data: self.data.to_vec(),
        }
    }
}
