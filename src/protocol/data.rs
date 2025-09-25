use std::string::FromUtf8Error;

use anyhow::Result;
use bytes::{Buf, BufMut, BytesMut};
use crc32fast::Hasher as Crc32Hasher;

use crate::protocol::{RetCode, RexCommand, RexError};

#[derive(Debug, Clone)]
pub struct RexHeader {
    command: RexCommand,
    source: usize,
    target: usize,
    retcode: RetCode,
    ext_len: usize,
    data_len: usize,
}

impl RexHeader {
    pub fn new(command: RexCommand, source: usize, target: usize) -> Self {
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
    pub fn command(&self) -> RexCommand {
        self.command
    }
    pub fn source(&self) -> usize {
        self.source
    }
    pub fn target(&self) -> usize {
        self.target
    }
    pub fn retcode(&self) -> RetCode {
        self.retcode
    }
    pub fn ext_len(&self) -> usize {
        self.ext_len
    }
    pub fn data_len(&self) -> usize {
        self.data_len
    }

    // Setters
    pub fn set_retcode(&mut self, retcode: RetCode) {
        self.retcode = retcode;
    }
    pub fn set_ext_len(&mut self, len: usize) {
        self.ext_len = len;
    }
    pub fn set_data_len(&mut self, len: usize) {
        self.data_len = len;
    }

    pub fn is_success(&self) -> bool {
        self.retcode.is_success()
    }
    pub fn serialized_len(&self) -> usize {
        FIXED_HEADER_LEN + self.ext_len + self.data_len + 4
    }
}

// 协议常量
const MAGIC: u32 = 0x5245584D; // 'REXM'
const VERSION: u16 = 1;
const FIXED_HEADER_LEN: usize = 40; // 4+2+2+4+4+4+8+8+4

// 计算CRC32
fn compute_crc32(ext: &[u8], data: &[u8]) -> u32 {
    let mut hasher = Crc32Hasher::new();
    hasher.update(ext);
    hasher.update(data);
    hasher.finalize()
}

// 扩展头部（仅支持 title）
#[derive(Debug, Clone)]
struct RexHeaderExt {
    title: String,
}

impl RexHeaderExt {
    fn new(title: String) -> Self {
        Self { title }
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn serialized_len(&self) -> usize {
        4 + self.title.len()
    }

    fn serialize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        let title_bytes = self.title.as_bytes();
        buf.put_u32_le(title_bytes.len() as u32);
        buf.put_slice(title_bytes);
        buf
    }

    fn deserialize(buf: &mut BytesMut) -> Result<Self, RexError> {
        if buf.remaining() < 4 {
            return Err(RexError::InsufficientData);
        }

        let title_len = buf.get_u32_le() as usize;
        if buf.remaining() < title_len {
            return Err(RexError::InsufficientData);
        }

        let title_bytes = buf.split_to(title_len);
        let title = String::from_utf8(title_bytes.to_vec())?;
        Ok(Self { title })
    }
}

// 主要数据结构
#[derive(Debug, Clone)]
pub struct RexData {
    header: RexHeader,
    header_ext: Option<RexHeaderExt>,
    data: BytesMut,
}

impl RexData {
    // === 构造方法 ===

    pub fn new(command: RexCommand, source: usize, target: usize, data: BytesMut) -> Self {
        let mut header = RexHeader::new(command, source, target);
        header.set_data_len(data.len());

        Self {
            header,
            header_ext: None,
            data,
        }
    }

    pub fn with_title(
        command: RexCommand,
        source: usize,
        target: usize,
        title: String,
        data: BytesMut,
    ) -> Self {
        let ext = RexHeaderExt::new(title);
        let mut header = RexHeader::new(command, source, target);
        header.set_ext_len(ext.serialized_len());
        header.set_data_len(data.len());

        Self {
            header,
            header_ext: Some(ext),
            data,
        }
    }

    pub fn with_retcode(
        command: RexCommand,
        source: usize,
        target: usize,
        retcode: RetCode,
        data: BytesMut,
    ) -> Self {
        let mut header = RexHeader::new(command, source, target);
        header.set_retcode(retcode);
        header.set_data_len(data.len());

        Self {
            header,
            header_ext: None,
            data,
        }
    }

    // === 核心序列化/反序列化方法 ===

    pub fn serialize(&self) -> BytesMut {
        let ext_bytes = self
            .header_ext
            .as_ref()
            .map(|ext| ext.serialize())
            .unwrap_or_default();

        let crc = compute_crc32(&ext_bytes, &self.data);
        let total_len = FIXED_HEADER_LEN + ext_bytes.len() + self.data.len() + 4;
        let mut buf = BytesMut::with_capacity(total_len);

        // 写入固定头部
        buf.put_u32_le(MAGIC);
        buf.put_u16_le(VERSION);
        buf.put_u16_le(0); // flags 预留
        buf.put_u32_le(ext_bytes.len() as u32);
        buf.put_u32_le(self.data.len() as u32);
        buf.put_u32_le(self.header.command.as_u32());
        buf.put_u64_le(self.header.source as u64);
        buf.put_u64_le(self.header.target as u64);
        buf.put_u32_le(self.header.retcode as u32);

        // 写入扩展和数据
        if !ext_bytes.is_empty() {
            buf.put_slice(&ext_bytes);
        }
        if !self.data.is_empty() {
            buf.put_slice(&self.data);
        }

        // 写入CRC
        buf.put_u32_le(crc);
        buf
    }

    /// 统一的反序列化方法，适用于所有传输协议
    /// 返回 (解析的数据, 消耗的字节数)
    pub fn deserialize(mut buf: BytesMut) -> Result<(Self, usize), RexError> {
        let original_len = buf.len();

        // 检查基本头部
        if buf.len() < FIXED_HEADER_LEN {
            return Err(RexError::InsufficientData);
        }

        // 读取头部字段
        let magic = buf.get_u32_le();
        if magic != MAGIC {
            return Err(RexError::DataCorrupted);
        }

        let version = buf.get_u16_le();
        if version != VERSION {
            return Err(RexError::VersionMismatch);
        }

        let _flags = buf.get_u16_le();
        let ext_len = buf.get_u32_le() as usize;
        let data_len = buf.get_u32_le() as usize;
        let command_value = buf.get_u32_le();
        let source = buf.get_u64_le() as usize;
        let target = buf.get_u64_le() as usize;
        let retcode_value = buf.get_u32_le();

        let command = RexCommand::from_u32(command_value).ok_or(RexError::InvalidCommand)?;
        let retcode = RetCode::from_u32(retcode_value).ok_or(RexError::InvalidRetCode)?;

        // 检查剩余数据长度
        let remaining_needed = ext_len + data_len + 4; // +4 for CRC
        if buf.len() < remaining_needed {
            return Err(RexError::InsufficientData);
        }

        // 读取扩展、数据和CRC
        let ext_data = if ext_len > 0 {
            let mut ext_buf = buf.split_to(ext_len);
            Some(RexHeaderExt::deserialize(&mut ext_buf)?)
        } else {
            None
        };

        let data_bytes = if data_len > 0 {
            buf.split_to(data_len)
        } else {
            BytesMut::new()
        };

        let crc_read = buf.get_u32_le();

        // 验证CRC
        let ext_bytes = ext_data
            .as_ref()
            .map(|ext| ext.serialize())
            .unwrap_or_else(BytesMut::new);
        let crc_calc = compute_crc32(&ext_bytes, &data_bytes);
        if crc_calc != crc_read {
            return Err(RexError::DataCorrupted);
        }

        // 构建结果
        let mut header = RexHeader::new(command, source, target);
        header.set_retcode(retcode);
        header.set_ext_len(ext_len);
        header.set_data_len(data_len);

        let consumed_bytes = original_len - buf.len();

        Ok((
            Self {
                header,
                header_ext: ext_data,
                data: data_bytes,
            },
            consumed_bytes,
        ))
    }

    /// 尝试从缓冲区解析，用于流处理
    /// 返回 None 表示数据不完整
    pub fn try_deserialize(buf: &BytesMut) -> Option<Result<(Self, usize), RexError>> {
        if buf.len() < FIXED_HEADER_LEN {
            return None;
        }

        // 预读包长度信息
        let mut temp_buf = buf.clone();
        temp_buf.advance(8); // skip magic, version, flags
        let ext_len = temp_buf.get_u32_le() as usize;
        let data_len = temp_buf.get_u32_le() as usize;

        let total_needed = FIXED_HEADER_LEN + ext_len + data_len + 4;
        if buf.len() < total_needed {
            return None;
        }

        Some(Self::deserialize(buf.clone()))
    }

    // === 访问器方法 ===

    pub fn header(&self) -> &RexHeader {
        &self.header
    }
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    pub fn title(&self) -> Option<&str> {
        self.header_ext.as_ref().map(|ext| ext.title())
    }
    pub fn retcode(&self) -> RetCode {
        self.header.retcode
    }
    pub fn is_success(&self) -> bool {
        self.header.is_success()
    }
    pub fn serialized_len(&self) -> usize {
        self.header.serialized_len()
    }

    // === 修改器方法 ===

    pub fn set_command(&mut self, command: RexCommand) -> &mut Self {
        self.header.command = command;
        self
    }

    pub fn set_source(&mut self, source: usize) -> &mut Self {
        self.header.source = source;
        self
    }

    pub fn set_target(&mut self, target: usize) -> &mut Self {
        self.header.target = target;
        self
    }

    pub fn set_retcode(&mut self, retcode: RetCode) -> &mut Self {
        self.header.set_retcode(retcode);
        self
    }

    // === 数据处理方法 ===

    pub fn data_as_string(&self) -> Result<String, FromUtf8Error> {
        String::from_utf8(self.data.to_vec())
    }

    pub fn data_as_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.data).to_string()
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

// 构建器
pub struct RexDataBuilder {
    command: RexCommand,
    source: usize,
    target: usize,
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

    pub fn source(mut self, source: usize) -> Self {
        self.source = source;
        self
    }

    pub fn target(mut self, target: usize) -> Self {
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

        let serialized = original.serialize();
        let (deserialized, consumed) = RexData::deserialize(serialized.clone()).unwrap();

        assert_eq!(consumed, serialized.len());
        assert_eq!(deserialized.header().command(), RexCommand::Login);
        assert_eq!(deserialized.header().source(), 100);
        assert_eq!(deserialized.header().target(), 200);
        assert_eq!(deserialized.data_as_string().unwrap(), "test message");
        assert!(deserialized.is_success());
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

        let serialized = original.serialize();
        let (deserialized, _) = RexData::deserialize(serialized).unwrap();

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
        let data = BytesMut::from(&[1, 2, 3, 4][..]); // 明显不完整
        assert!(RexData::try_deserialize(&data).is_none());
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

        assert_eq!(response.header().source(), 200); // 交换了source和target
        assert_eq!(response.header().target(), 100);
        assert_eq!(response.header().command(), RexCommand::CheckReturn);
        assert!(response.is_success());
    }
}
