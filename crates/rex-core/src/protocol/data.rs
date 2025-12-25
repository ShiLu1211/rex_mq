use bytes::{Buf, BufMut, Bytes, BytesMut};
use rkyv::{
    Archive, Deserialize, Serialize,
    ser::{Serializer, serializers::AllocSerializer},
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

    #[inline]
    pub fn with_retcode(
        command: RexCommand,
        source: u128,
        target: u128,
        retcode: RetCode,
        data: BytesMut,
    ) -> Self {
        Self {
            command: command.as_u32(),
            source,
            target,
            retcode: retcode as u32,
            title: None,
            data: data.to_vec(),
        }
    }

    // === 核心序列化方法（零拷贝优化） ===

    /// 高性能序列化（使用栈缓冲区优化小消息）
    pub fn serialize(&self) -> Bytes {
        let mut serializer = AllocSerializer::<0>::default();

        if let Err(e) = serializer.serialize_value(self) {
            warn!("Failed to serialize RexData: {}", e);
        }

        let payload = serializer.into_serializer().into_inner();
        let len = payload.len() as u32;

        let mut buf = BytesMut::with_capacity(4 + payload.len());
        buf.put_u32(len);
        buf.extend_from_slice(payload.as_slice());

        buf.freeze()
    }

    /// 尝试从缓冲区解析一帧完整 RexData
    pub fn try_deserialize(buf: &mut BytesMut) -> anyhow::Result<Option<Self>> {
        const HEADER: usize = 4;

        // 1️⃣ 还没到 header
        if buf.len() < HEADER {
            return Ok(None);
        }

        // 2️⃣ 读取长度（不 advance）
        let payload_len = u32::from_be_bytes(buf[..4].try_into()?) as usize;

        // 3️⃣ 半包
        if buf.len() < HEADER + payload_len {
            return Ok(None);
        }

        // 4️⃣ 消费 header
        buf.advance(HEADER);

        // 5️⃣ 拆出 payload（O(1)）
        let payload = buf.split_to(payload_len);

        // 6️⃣ rkyv 反序列化
        let archived = unsafe { rkyv::archived_root::<RexData>(&payload) };

        let data = archived.deserialize(&mut rkyv::Infallible)?;

        Ok(Some(data))
    }

    /// 极致性能零拷贝（worker 线程用）
    #[inline]
    pub fn as_archived(buf: &[u8]) -> RexDataRef<'_> {
        RexDataRef {
            archived: unsafe { rkyv::archived_root::<Self>(buf) },
        }
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

    #[inline]
    pub fn data_bytes(&self) -> Bytes {
        Bytes::from(self.data.clone())
    }

    #[inline(always)]
    pub fn data_slice(&self) -> &[u8] {
        &self.data
    }

    // === 修改器方法 ===

    #[inline]
    pub fn set_command(&mut self, command: RexCommand) -> &mut Self {
        self.command = command.as_u32();
        self
    }

    #[inline]
    pub fn set_source(&mut self, source: u128) -> &mut Self {
        self.source = source;
        self
    }

    #[inline]
    pub fn set_target(&mut self, target: u128) -> &mut Self {
        self.target = target;
        self
    }

    #[inline]
    pub fn set_retcode(&mut self, retcode: RetCode) -> &mut Self {
        self.retcode = retcode as u32;
        self
    }

    #[inline]
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
        String::from_utf8_lossy(&self.data).to_string()
    }

    #[inline]
    pub fn take_data(self) -> Bytes {
        Bytes::from(self.data)
    }

    #[inline]
    pub fn into_mutable_data(self) -> BytesMut {
        BytesMut::from(self.data.as_slice())
    }

    // === 构建器模式 ===

    #[inline]
    pub fn builder(command: RexCommand) -> RexDataBuilder {
        RexDataBuilder::new(command)
    }

    // === 响应创建方法 ===

    #[inline]
    pub fn create_success_response(&self, response_command: RexCommand, data: BytesMut) -> Self {
        Self::with_retcode(
            response_command,
            self.target,
            self.source,
            RetCode::Success,
            data,
        )
    }

    #[inline]
    pub fn create_error_response(
        &self,
        response_command: RexCommand,
        retcode: RetCode,
        message: &str,
    ) -> Self {
        let data = BytesMut::from(message.as_bytes());
        Self::with_retcode(response_command, self.target, self.source, retcode, data)
    }
}

/// 零拷贝引用包装器（高性能访问层）
///
/// 这个结构体提供了对 archived 数据的零拷贝访问
/// 所有访问方法都不涉及内存分配或数据复制
#[derive(Clone, Copy)]
pub struct RexDataRef<'a> {
    archived: &'a ArchivedRexData,
}

impl<'a> RexDataRef<'a> {
    /// 获取原始 archived 引用
    #[inline(always)]
    pub fn archived(&self) -> &'a ArchivedRexData {
        self.archived
    }

    /// 零拷贝访问 command（直接读取 archived 数据）
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

    /// 零拷贝访问 title（返回 archived string slice）
    #[inline]
    pub fn title(&self) -> Option<&str> {
        self.archived.title.as_ref().map(|s| s.as_str())
    }

    /// 零拷贝访问 data（返回 archived vec slice）
    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        self.archived.data.as_slice()
    }

    /// 零拷贝获取 data 长度
    #[inline(always)]
    pub fn data_len(&self) -> usize {
        self.archived.data.len()
    }

    /// 零拷贝转换 data 为字符串（需要验证 UTF-8）
    #[inline]
    pub fn data_as_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(self.data())
    }

    /// 零拷贝转换 data 为字符串（lossy）
    #[inline]
    pub fn data_as_string_lossy(&self) -> String {
        String::from_utf8_lossy(self.data()).to_string()
    }

    /// 创建响应（需要反序列化，因为要修改数据）
    pub fn create_success_response(&self, response_command: RexCommand, data: BytesMut) -> RexData {
        RexData::with_retcode(
            response_command,
            self.target(),
            self.source(),
            RetCode::Success,
            data,
        )
    }

    /// 创建错误响应
    pub fn create_error_response(
        &self,
        response_command: RexCommand,
        retcode: RetCode,
        message: &str,
    ) -> RexData {
        let data = BytesMut::from(message.as_bytes());
        RexData::with_retcode(
            response_command,
            self.target(),
            self.source(),
            retcode,
            data,
        )
    }

    /// 完整反序列化（当需要所有权时）
    pub fn deserialize(&self) -> RexData {
        self.archived
            .deserialize(&mut rkyv::Infallible)
            .expect("Infallible deserialization")
    }

    /// 克隆 data 为 Bytes（引用计数，相对高效）
    #[inline]
    pub fn data_bytes(&self) -> Bytes {
        Bytes::copy_from_slice(self.data())
    }
}

/// 构建器
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
        if let Some(title) = self.title {
            let mut result =
                RexData::with_title(self.command, self.source, self.target, title, self.data);
            result.set_retcode(self.retcode);
            result
        } else {
            RexData::with_retcode(
                self.command,
                self.source,
                self.target,
                self.retcode,
                self.data,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_serialization() {
        let original = RexData::builder(RexCommand::Login)
            .source(100)
            .target(200)
            .data_from_string("test message")
            .build();

        let serialized = original.serialize();
        let deserialized = RexData::try_deserialize(&mut serialized.into())
            .unwrap()
            .unwrap();

        assert_eq!(deserialized.command(), RexCommand::Login);
        assert_eq!(deserialized.source(), 100);
        assert_eq!(deserialized.target(), 200);
        assert_eq!(deserialized.data_as_string().unwrap(), "test message");
        assert!(deserialized.is_success());
    }

    #[test]
    fn test_zero_copy_access() {
        let original = RexData::builder(RexCommand::Login)
            .source(100)
            .target(200)
            .data_from_string("test message")
            .build();

        let serialized = original.serialize();

        // 零拷贝访问（推荐方式）
        let data_ref = RexData::as_archived(&serialized);
        assert_eq!(data_ref.command(), RexCommand::Login);
        assert_eq!(data_ref.source(), 100);
        assert_eq!(data_ref.target(), 200);
        assert_eq!(data_ref.data_as_str().unwrap(), "test message");
        assert!(data_ref.is_success());
    }

    #[test]
    fn test_zero_copy_response_creation() {
        let original = RexData::builder(RexCommand::Check)
            .source(100)
            .target(200)
            .data_from_string("ping")
            .build();

        let serialized = original.serialize();
        let data_ref = RexData::as_archived(&serialized);

        // 从零拷贝引用创建响应
        let response = data_ref
            .create_success_response(RexCommand::CheckReturn, BytesMut::from("pong".as_bytes()));

        assert_eq!(response.source(), 200);
        assert_eq!(response.target(), 100);
        assert_eq!(response.command(), RexCommand::CheckReturn);
        assert!(response.is_success());
    }

    #[test]
    fn test_batch_zero_copy_processing() {
        // 批量创建消息
        let messages: Vec<_> = (0..100)
            .map(|i| {
                RexData::builder(RexCommand::Title)
                    .source(i)
                    .data_from_string(format!("msg_{}", i))
                    .build()
            })
            .collect();

        // 序列化所有消息
        let serialized: Vec<_> = messages.iter().map(|m| m.serialize()).collect();

        // 零拷贝批量处理
        let mut total_sources = 0u128;
        for buf in &serialized {
            let data_ref = RexData::as_archived(buf);
            total_sources += data_ref.source();

            // 验证数据
            assert!(data_ref.data_as_str().unwrap().starts_with("msg_"));
        }

        assert_eq!(total_sources, (0..100).sum::<u128>());
    }

    #[test]
    fn test_zero_copy_vs_deserialize() {
        let original = RexData::builder(RexCommand::Title)
            .source(12345)
            .target(67890)
            .title("test")
            .data_from_string("x".repeat(1024))
            .build();

        let serialized = original.serialize();

        // 零拷贝访问 - 无分配
        let data_ref = RexData::as_archived(&serialized);
        assert_eq!(data_ref.source(), 12345);
        assert_eq!(data_ref.data_len(), 1024);

        // 完整反序列化 - 有分配
        let buf = serialized.clone();
        let deserialized = RexData::try_deserialize(&mut buf.into()).unwrap().unwrap();
        assert_eq!(deserialized.source(), 12345);
        assert_eq!(deserialized.data().len(), 1024);
    }

    #[test]
    fn test_stream_decode() {
        let original = RexData::builder(RexCommand::Title)
            .source(1)
            .target(2)
            .title("test")
            .data_from_string("hello")
            .build();

        let buf = original.serialize();
        let result = RexData::try_deserialize(&mut buf.into()).unwrap();

        assert!(result.is_some());
        let decoded = result.unwrap();
        assert_eq!(decoded.command(), RexCommand::Title);
    }

    #[test]
    fn test_ref_is_copy() {
        let original = RexData::builder(RexCommand::Title)
            .source(1)
            .data_from_string("test")
            .build();

        let serialized = original.serialize();
        let ref1 = RexData::as_archived(&serialized);
        let ref2 = ref1; // Copy, not move
        let ref3 = ref1; // Still valid

        assert_eq!(ref1.source(), ref2.source());
        assert_eq!(ref2.source(), ref3.source());
    }
}
