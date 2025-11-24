use bytes::{Bytes, BytesMut};
use std::string::FromUtf8Error;

use super::codec::{DecodedMessage, MessageCodec};
use super::{RetCode, RexCommand};
use crate::error::RexError;

/// 消息头部（业务层）
#[derive(Debug, Clone)]
pub struct RexHeader {
    command: RexCommand,
    source: u128,
    target: u128,
    retcode: RetCode,
    ext_len: usize,
    data_len: usize,
}

impl RexHeader {
    pub fn new(command: RexCommand, source: u128, target: u128) -> Self {
        Self {
            command,
            source,
            target,
            retcode: RetCode::Success,
            ext_len: 0,
            data_len: 0,
        }
    }

    // Getters
    #[inline]
    pub fn command(&self) -> RexCommand {
        self.command
    }

    #[inline]
    pub fn source(&self) -> u128 {
        self.source
    }

    #[inline]
    pub fn target(&self) -> u128 {
        self.target
    }

    #[inline]
    pub fn retcode(&self) -> RetCode {
        self.retcode
    }

    #[inline]
    pub fn ext_len(&self) -> usize {
        self.ext_len
    }

    #[inline]
    pub fn data_len(&self) -> usize {
        self.data_len
    }

    // Setters
    #[inline]
    pub fn set_retcode(&mut self, retcode: RetCode) {
        self.retcode = retcode;
    }

    #[inline]
    pub fn set_ext_len(&mut self, len: usize) {
        self.ext_len = len;
    }

    #[inline]
    pub fn set_data_len(&mut self, len: usize) {
        self.data_len = len;
    }

    #[inline]
    pub fn is_success(&self) -> bool {
        self.retcode.is_success()
    }

    pub fn total_len(&self) -> usize {
        super::codec::FIXED_HEADER_LEN + self.ext_len + self.data_len + 4
    }
}

/// 消息数据结构（业务层 - 零拷贝版本）
#[derive(Debug, Clone)]
pub struct RexData {
    header: RexHeader,
    title: Option<String>,
    data: Bytes,
}

impl RexData {
    // === 构造方法 ===

    pub fn new(command: RexCommand, source: u128, target: u128, data: BytesMut) -> Self {
        let mut header = RexHeader::new(command, source, target);
        header.set_data_len(data.len());

        Self {
            header,
            title: None,
            data: data.freeze(),
        }
    }

    pub fn with_title(
        command: RexCommand,
        source: u128,
        target: u128,
        title: String,
        data: BytesMut,
    ) -> Self {
        let title_len = 4 + title.len();
        let mut header = RexHeader::new(command, source, target);
        header.set_ext_len(title_len);
        header.set_data_len(data.len());

        Self {
            header,
            title: Some(title),
            data: data.freeze(),
        }
    }

    pub fn with_retcode(
        command: RexCommand,
        source: u128,
        target: u128,
        retcode: RetCode,
        data: BytesMut,
    ) -> Self {
        let mut header = RexHeader::new(command, source, target);
        header.set_retcode(retcode);
        header.set_data_len(data.len());

        Self {
            header,
            title: None,
            data: data.freeze(),
        }
    }

    // === 核心序列化/反序列化方法 ===

    /// 序列化（使用优化的 codec）
    pub fn serialize(&self) -> BytesMut {
        MessageCodec::encode(
            self.header.command,
            self.header.source,
            self.header.target,
            self.header.retcode,
            self.title.as_deref(),
            &self.data,
        )
    }

    /// 零拷贝反序列化（主方法）
    pub fn deserialize(buf: &mut BytesMut) -> Result<Self, RexError> {
        let decoded = MessageCodec::decode(buf)?;
        Ok(Self::from_decoded(decoded))
    }

    /// 尝试从缓冲区解析（用于流处理 - 零拷贝）
    pub fn try_deserialize(buf: &mut BytesMut) -> Result<Option<Self>, RexError> {
        match MessageCodec::decode_stream(buf)? {
            Some(decoded) => Ok(Some(Self::from_decoded(decoded))),
            None => Ok(None),
        }
    }

    /// 从解码结果构造 RexData
    fn from_decoded(decoded: DecodedMessage) -> Self {
        let mut header = RexHeader::new(decoded.command, decoded.source, decoded.target);
        header.set_retcode(decoded.retcode);

        if let Some(ref title) = decoded.title {
            header.set_ext_len(4 + title.len());
        }
        header.set_data_len(decoded.data.len());

        Self {
            header,
            title: decoded.title,
            data: decoded.data,
        }
    }

    // === 访问器方法 ===

    #[inline]
    pub fn header(&self) -> &RexHeader {
        &self.header
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    #[inline]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[inline]
    pub fn retcode(&self) -> RetCode {
        self.header.retcode
    }

    #[inline]
    pub fn is_success(&self) -> bool {
        self.header.is_success()
    }

    #[inline]
    pub fn total_len(&self) -> usize {
        self.header.total_len()
    }

    /// 获取 Bytes 克隆（零拷贝 - 引用计数）
    #[inline]
    pub fn data_bytes(&self) -> Bytes {
        self.data.clone()
    }

    /// 获取数据切片
    #[inline]
    pub fn data_slice(&self) -> &[u8] {
        &self.data
    }

    // === 修改器方法 ===

    #[inline]
    pub fn set_command(&mut self, command: RexCommand) -> &mut Self {
        self.header.command = command;
        self
    }

    #[inline]
    pub fn set_source(&mut self, source: u128) -> &mut Self {
        self.header.source = source;
        self
    }

    #[inline]
    pub fn set_target(&mut self, target: u128) -> &mut Self {
        self.header.target = target;
        self
    }

    #[inline]
    pub fn set_retcode(&mut self, retcode: RetCode) -> &mut Self {
        self.header.set_retcode(retcode);
        self
    }

    pub fn set_title(&mut self, title: Option<String>) -> &mut Self {
        self.title = title;
        if let Some(ref t) = self.title {
            self.header.set_ext_len(4 + t.len());
        } else {
            self.header.set_ext_len(0);
        }
        self
    }

    // === 数据处理方法 ===

    pub fn data_as_string(&self) -> Result<String, FromUtf8Error> {
        String::from_utf8(self.data.to_vec())
    }

    pub fn data_as_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.data).to_string()
    }

    /// 取出数据（零拷贝）
    pub fn take_data(self) -> Bytes {
        self.data
    }

    /// 转换为可变数据（会分配新内存）
    pub fn into_mutable_data(self) -> BytesMut {
        BytesMut::from(self.data.as_ref())
    }

    // === 构建器模式 ===

    pub fn builder(command: RexCommand) -> RexDataBuilder {
        RexDataBuilder::new(command)
    }

    // === 响应创建方法 ===

    pub fn create_success_response(&self, response_command: RexCommand, data: BytesMut) -> Self {
        Self::with_retcode(
            response_command,
            self.header.target,
            self.header.source,
            RetCode::Success,
            data,
        )
    }

    pub fn create_error_response(
        &self,
        response_command: RexCommand,
        retcode: RetCode,
        message: &str,
    ) -> Self {
        let data = BytesMut::from(message.as_bytes());
        Self::with_retcode(
            response_command,
            self.header.target,
            self.header.source,
            retcode,
            data,
        )
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

    pub fn source(mut self, source: u128) -> Self {
        self.source = source;
        self
    }

    pub fn target(mut self, target: u128) -> Self {
        self.target = target;
        self
    }

    pub fn retcode(mut self, retcode: RetCode) -> Self {
        self.retcode = retcode;
        self
    }

    pub fn title<S: Into<String>>(mut self, title: S) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn data_from_string<S: AsRef<str>>(mut self, data: S) -> Self {
        self.data = BytesMut::from(data.as_ref().as_bytes());
        self
    }

    pub fn data(mut self, data: BytesMut) -> Self {
        self.data = data;
        self
    }

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

        let mut serialized = original.serialize();
        let deserialized = RexData::deserialize(&mut serialized).unwrap();

        assert_eq!(deserialized.header().command(), RexCommand::Login);
        assert_eq!(deserialized.header().source(), 100);
        assert_eq!(deserialized.header().target(), 200);
        assert_eq!(deserialized.data_as_string().unwrap(), "test message");
        assert!(deserialized.is_success());
        assert_eq!(serialized.len(), 0); // 完全消费
    }

    #[test]
    fn test_with_title_and_retcode() {
        let original = RexData::builder(RexCommand::Group)
            .source(1000)
            .target(2000)
            .retcode(RetCode::AuthFailed)
            .title("Error Message")
            .data_from_string("Authentication failed")
            .build();

        let mut serialized = original.serialize();
        let deserialized = RexData::deserialize(&mut serialized).unwrap();

        assert_eq!(deserialized.title(), Some("Error Message"));
        assert_eq!(deserialized.retcode(), RetCode::AuthFailed);
        assert!(!deserialized.is_success());
        assert_eq!(
            deserialized.data_as_string().unwrap(),
            "Authentication failed"
        );
    }

    #[test]
    fn test_try_deserialize_incomplete() {
        let mut data = BytesMut::from(&[1, 2, 3, 4][..]);
        assert!(RexData::try_deserialize(&mut data).unwrap().is_none());
    }

    #[test]
    fn test_response_creation() {
        let request = RexData::builder(RexCommand::Check)
            .source(100)
            .target(200)
            .data_from_string("ping")
            .build();

        let response = request
            .create_success_response(RexCommand::CheckReturn, BytesMut::from("pong".as_bytes()));

        assert_eq!(response.header().source(), 200);
        assert_eq!(response.header().target(), 100);
        assert_eq!(response.header().command(), RexCommand::CheckReturn);
        assert!(response.is_success());
    }

    #[test]
    fn test_stream_decode() {
        let original = RexData::builder(RexCommand::Title)
            .source(1)
            .target(2)
            .title("test")
            .data_from_string("hello")
            .build();

        let mut buf = original.serialize();

        let result = RexData::try_deserialize(&mut buf).unwrap();
        assert!(result.is_some());

        let decoded = result.unwrap();
        assert_eq!(decoded.header().command(), RexCommand::Title);
        assert_eq!(decoded.title(), Some("test"));
        assert_eq!(decoded.data_as_string().unwrap(), "hello");
        assert_eq!(buf.len(), 0); // 完全消费
    }

    #[test]
    fn test_zerocopy_clone() {
        let msg = RexData::builder(RexCommand::Title)
            .source(1)
            .target(2)
            .data_from_string("x".repeat(1024))
            .build();

        // 零拷贝克隆数据
        let data1 = msg.data_bytes();
        let data2 = msg.data_bytes();
        let data3 = msg.data_bytes();

        assert_eq!(data1.len(), data2.len());
        assert_eq!(data2.len(), data3.len());
        // 所有克隆共享同一份底层数据（引用计数）
    }

    #[test]
    fn test_message_forwarding() {
        let msg = RexData::builder(RexCommand::Title)
            .source(1)
            .target(2)
            .data_from_string("forward me")
            .build();

        // 零拷贝转发
        let data = msg.data_bytes();
        let response =
            msg.create_success_response(RexCommand::TitleReturn, BytesMut::from(&data[..]));

        assert_eq!(response.data_as_string().unwrap(), "forward me");
    }

    #[test]
    fn test_batch_processing() {
        let messages: Vec<_> = (0..100)
            .map(|i| {
                RexData::builder(RexCommand::Title)
                    .source(i)
                    .data_from_string(format!("msg_{}", i))
                    .build()
            })
            .collect();

        // 所有消息都可以零拷贝克隆
        let mut total_len = 0;
        for msg in &messages {
            let data = msg.data_bytes();
            total_len += data.len();
        }

        assert!(total_len > 0);
    }
}
