#![allow(dead_code)]
use std::io;
use std::mem;
use std::string::FromUtf8Error;

use anyhow::Result;
use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

use crate::command::RexCommand;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RexHeader {
    header_len: usize,
    header_ext_len: usize,
    data_len: usize,
    command: RexCommand,
    source: usize,
    target: usize,
}

impl RexHeader {
    pub fn new(command: RexCommand, source: usize, target: usize) -> Self {
        Self {
            header_len: mem::size_of::<RexHeader>(),
            header_ext_len: 0,
            data_len: 0,
            command,
            source,
            target,
        }
    }

    // Getter方法
    pub fn header_len(&self) -> usize {
        self.header_len
    }
    pub fn header_ext_len(&self) -> usize {
        self.header_ext_len
    }
    pub fn data_len(&self) -> usize {
        self.data_len
    }
    pub fn command(&self) -> RexCommand {
        self.command
    }
    pub fn source(&self) -> usize {
        self.source
    }
    pub fn target(&self) -> usize {
        self.target
    }

    // Setter方法
    pub fn set_header_ext_len(&mut self, len: usize) {
        self.header_ext_len = len;
    }

    pub fn set_data_len(&mut self, len: usize) {
        self.data_len = len;
    }

    // 计算总包大小
    pub fn total_size(&self) -> usize {
        self.header_len + self.header_ext_len + self.data_len
    }
}

#[derive(Debug, Clone)]
struct RexHeaderExt {
    title: String,
}

impl RexHeaderExt {
    pub fn new(title: String) -> Self {
        Self { title }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: String) {
        self.title = title;
    }

    // 序列化扩展头部
    pub fn serialize(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        let title_bytes = self.title.as_bytes();

        // 写入title长度和内容
        buf.put_u32_le(title_bytes.len() as u32);
        buf.put_slice(title_bytes);

        buf
    }

    // 反序列化扩展头部
    pub fn deserialize(buf: &mut BytesMut) -> Result<Self, RexError> {
        if buf.remaining() < 4 {
            return Err(RexError::InsufficientData {
                expected: 4,
                actual: buf.remaining(),
            });
        }

        let title_len = buf.get_u32_le() as usize;

        if buf.remaining() < title_len {
            return Err(RexError::InsufficientData {
                expected: title_len,
                actual: buf.remaining(),
            });
        }

        let title_bytes = buf.split_to(title_len);
        let title = String::from_utf8(title_bytes.to_vec())?; // 使用 From trait

        Ok(Self { title })
    }

    // 计算序列化后的大小
    pub fn serialized_size(&self) -> usize {
        4 + self.title.len() // 4字节长度 + 字符串内容
    }
}

#[derive(Debug, Clone)]
pub struct RexData {
    header: RexHeader,
    header_ext: Option<RexHeaderExt>,
    data: BytesMut,
}

impl RexData {
    // 创建不带扩展头部的数据包
    pub fn new(command: RexCommand, source: usize, target: usize, data: BytesMut) -> Self {
        let mut header = RexHeader::new(command, source, target);
        header.set_data_len(data.len());

        Self {
            header,
            header_ext: None,
            data,
        }
    }

    // 创建带扩展头部的数据包
    pub fn new_with_title(
        command: RexCommand,
        source: usize,
        target: usize,
        title: String,
        data: BytesMut,
    ) -> Self {
        let header_ext = RexHeaderExt::new(title);
        let mut header = RexHeader::new(command, source, target);

        header.set_header_ext_len(header_ext.serialized_size());
        header.set_data_len(data.len());

        Self {
            header,
            header_ext: Some(header_ext),
            data,
        }
    }

    // Getter方法
    pub fn header(&self) -> &RexHeader {
        &self.header
    }
    pub(self) fn header_ext(&self) -> &Option<RexHeaderExt> {
        &self.header_ext
    }
    pub fn data(&self) -> &BytesMut {
        &self.data
    }
    pub fn data_mut(&mut self) -> &mut BytesMut {
        &mut self.data
    }

    // 获取title（如果存在）
    pub fn title(&self) -> Option<&str> {
        self.header_ext.as_ref().map(|ext| ext.title())
    }

    // 设置title
    pub fn set_title(&mut self, title: String) {
        match &mut self.header_ext {
            Some(ext) => {
                ext.set_title(title);
                self.header.set_header_ext_len(ext.serialized_size());
            }
            None => {
                let ext = RexHeaderExt::new(title);
                self.header.set_header_ext_len(ext.serialized_size());
                self.header_ext = Some(ext);
            }
        }
    }

    // 移除扩展头部
    pub fn remove_header_ext(&mut self) {
        self.header_ext = None;
        self.header.set_header_ext_len(0);
    }

    // 设置数据
    pub fn set_data(&mut self, data: BytesMut) {
        self.header.set_data_len(data.len());
        self.data = data;
    }

    // 追加数据
    pub fn append_data(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data);
        self.header.set_data_len(self.data.len());
    }

    // 序列化整个数据包
    pub fn serialize(&self) -> BytesMut {
        let mut buf = BytesMut::new();

        // 序列化主头部
        let header_bytes = unsafe {
            std::slice::from_raw_parts(
                &self.header as *const RexHeader as *const u8,
                mem::size_of::<RexHeader>(),
            )
        };
        buf.put_slice(header_bytes);

        // 序列化扩展头部
        if let Some(ref ext) = self.header_ext {
            let ext_bytes = ext.serialize();
            buf.put_slice(&ext_bytes);
        }

        // 序列化数据部分
        buf.put_slice(&self.data);

        buf
    }

    // 反序列化数据包
    pub fn deserialize(mut buf: BytesMut) -> Result<Self, RexError> {
        let header_size = mem::size_of::<RexHeader>();

        // 检查最小长度
        if buf.len() < header_size {
            return Err(RexError::InsufficientData {
                expected: header_size,
                actual: buf.len(),
            });
        }

        // 反序列化主头部
        let header_bytes = buf.split_to(header_size);
        let header = unsafe { std::ptr::read(header_bytes.as_ptr() as *const RexHeader) };

        // 验证命令有效性
        let command_value = header.command.as_u32();
        RexCommand::from_u32(command_value).ok_or(RexError::InvalidCommand { command_value })?;

        // 反序列化扩展头部
        let header_ext = if header.header_ext_len > 0 {
            if buf.len() < header.header_ext_len {
                return Err(RexError::InsufficientData {
                    expected: header.header_ext_len,
                    actual: buf.len(),
                });
            }

            let mut ext_buf = buf.split_to(header.header_ext_len);
            Some(RexHeaderExt::deserialize(&mut ext_buf)?)
        } else {
            None
        };

        // 检查数据长度
        if buf.len() != header.data_len {
            return Err(RexError::DataLengthMismatch {
                declared: header.data_len,
                actual: buf.len(),
            });
        }

        Ok(Self {
            header,
            header_ext,
            data: buf,
        })
    }

    // 获取总大小
    pub fn total_size(&self) -> usize {
        self.header.total_size()
    }

    // 是否有扩展头部
    pub fn has_header_ext(&self) -> bool {
        self.header_ext.is_some()
    }

    // 克隆数据部分
    pub fn clone_data(&self) -> BytesMut {
        self.data.clone()
    }

    // 获取数据的只读切片
    pub fn data_slice(&self) -> &[u8] {
        &self.data
    }
}

impl RexData {
    // 从Quinn的RecvStream中读取数据包（异步版本）
    pub async fn read_from_quinn_stream(stream: &mut quinn::RecvStream) -> Result<Self, RexError> {
        // 先读取主头部
        let mut header_bytes = vec![0u8; mem::size_of::<RexHeader>()];
        stream.read_exact(&mut header_bytes).await?; // 现在可以直接使用 ? 操作符

        let header = unsafe { std::ptr::read(header_bytes.as_ptr() as *const RexHeader) };

        // 验证命令
        let command_value = header.command.as_u32();
        RexCommand::from_u32(command_value).ok_or(RexError::InvalidCommand { command_value })?;

        // 读取扩展头部
        let header_ext = if header.header_ext_len > 0 {
            let mut ext_bytes = vec![0u8; header.header_ext_len];
            stream.read_exact(&mut ext_bytes).await?;

            let mut ext_buf = BytesMut::from(&ext_bytes[..]);
            Some(RexHeaderExt::deserialize(&mut ext_buf)?)
        } else {
            None
        };

        // 读取数据部分
        let mut data_bytes = vec![0u8; header.data_len];
        stream.read_exact(&mut data_bytes).await?;

        Ok(Self {
            header,
            header_ext,
            data: BytesMut::from(&data_bytes[..]),
        })
    }
}

// 错误类型定义
#[derive(Error, Debug)]
pub enum RexError {
    #[error("数据长度不足: 需要 {expected} 字节，但只有 {actual} 字节")]
    InsufficientData { expected: usize, actual: usize },

    #[error("无效的命令值: {command_value}")]
    InvalidCommand { command_value: u32 },

    #[error("字符串编码错误: {source}")]
    InvalidString {
        #[from]
        source: FromUtf8Error,
    },

    #[error("数据长度不匹配: 头部声明 {declared} 字节，实际 {actual} 字节")]
    DataLengthMismatch { declared: usize, actual: usize },

    #[error("数据过大: {size} 字节超过限制 {limit} 字节")]
    DataTooLarge { size: usize, limit: usize },

    #[error("IO错误: {source}")]
    IoError {
        #[from]
        source: io::Error,
    },

    #[error("Quinn读取到末尾错误: {source}")]
    QuinnReadToEndError {
        #[from]
        source: quinn::ReadToEndError,
    },

    #[error("Quinn读取精确字节数错误: {source}")]
    QuinnReadExactError {
        #[from]
        source: quinn::ReadExactError,
    },
}

// RexData的构建器模式
pub struct RexDataBuilder {
    command: RexCommand,
    source: usize,
    target: usize,
    title: Option<String>,
    data: BytesMut,
}

impl RexDataBuilder {
    pub fn new(command: RexCommand) -> Self {
        Self {
            command,
            source: 0,
            target: 0,
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

    pub fn title<S: Into<String>>(mut self, title: S) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn data(mut self, data: BytesMut) -> Self {
        self.data = data;
        self
    }

    pub fn data_from_slice(mut self, data: &[u8]) -> Self {
        self.data = BytesMut::from(data);
        self
    }

    pub fn data_from_string<S: AsRef<str>>(mut self, data: S) -> Self {
        self.data = BytesMut::from(data.as_ref().as_bytes());
        self
    }

    pub fn build(self) -> RexData {
        if let Some(title) = self.title {
            RexData::new_with_title(self.command, self.source, self.target, title, self.data)
        } else {
            RexData::new(self.command, self.source, self.target, self.data)
        }
    }
}

// 便捷的工厂方法
impl RexData {
    // 构建器入口
    pub fn builder(command: RexCommand) -> RexDataBuilder {
        RexDataBuilder::new(command)
    }

    // 快速创建文本消息
    pub fn text_message(command: RexCommand, source: usize, target: usize, message: &str) -> Self {
        let data = BytesMut::from(message.as_bytes());
        Self::new(command, source, target, data)
    }

    // 快速创建带标题的文本消息
    pub fn text_message_with_title(
        command: RexCommand,
        source: usize,
        target: usize,
        title: String,
        message: &str,
    ) -> Self {
        let data = BytesMut::from(message.as_bytes());
        Self::new_with_title(command, source, target, title, data)
    }

    // 快速创建二进制数据包
    pub fn binary_data(command: RexCommand, source: usize, target: usize, data: Vec<u8>) -> Self {
        let data = BytesMut::from(&data[..]);
        Self::new(command, source, target, data)
    }

    // 创建空数据包（仅头部）
    pub fn header_only(command: RexCommand, source: usize, target: usize) -> Self {
        Self::new(command, source, target, BytesMut::new())
    }

    // 创建响应数据包
    pub fn create_response(&self, response_command: RexCommand, response_data: BytesMut) -> Self {
        // 交换source和target作为响应
        Self::new(
            response_command,
            self.header.target,
            self.header.source,
            response_data,
        )
    }

    // 从字节数组创建
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RexError> {
        let buf = BytesMut::from(bytes);
        Self::deserialize(buf)
    }

    // 转换为字节数组
    pub fn to_bytes(&self) -> Vec<u8> {
        self.serialize().to_vec()
    }

    // 获取数据为字符串（如果是文本数据）
    pub fn data_as_string(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.data.to_vec())
    }

    // 获取数据为字符串（lossy版本）
    pub fn data_as_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.data).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_message() {
        let msg = RexData::text_message(RexCommand::Cast, 100, 200, "test message");

        assert_eq!(msg.header().command(), RexCommand::Cast);
        assert_eq!(msg.header().source(), 100);
        assert_eq!(msg.header().target(), 200);
        assert_eq!(msg.data_as_string().unwrap(), "test message");
        assert!(msg.header_ext().is_none());
    }

    #[test]
    fn test_titled_message() {
        let msg = RexData::text_message_with_title(
            RexCommand::Title,
            100,
            200,
            "Test Title".to_string(),
            "test content",
        );

        assert_eq!(msg.title(), Some("Test Title"));
        assert_eq!(msg.data_as_string().unwrap(), "test content");
        assert!(msg.header_ext().is_some());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let original = RexData::builder(RexCommand::Group)
            .source(1000)
            .target(2000)
            .title("测试标题")
            .data_from_string("测试数据内容")
            .build();

        let serialized = original.serialize();
        let deserialized = RexData::deserialize(serialized).unwrap();

        assert_eq!(original.header().command(), deserialized.header().command());
        assert_eq!(original.header().source(), deserialized.header().source());
        assert_eq!(original.header().target(), deserialized.header().target());
        assert_eq!(original.title(), deserialized.title());
        assert_eq!(original.data_slice(), deserialized.data_slice());
    }

    #[test]
    fn test_builder_pattern() {
        let msg = RexData::builder(RexCommand::Login)
            .source(123)
            .target(456)
            .title("Login Request")
            .data_from_slice(b"username:password")
            .build();

        assert_eq!(msg.header().command(), RexCommand::Login);
        assert_eq!(msg.header().source(), 123);
        assert_eq!(msg.header().target(), 456);
        assert_eq!(msg.title(), Some("Login Request"));
    }

    #[test]
    fn test_response_creation() {
        let request = RexData::text_message(RexCommand::Check, 100, 200, "ping");
        let response =
            request.create_response(RexCommand::CheckReturn, BytesMut::from("pong".as_bytes()));

        // 检查source和target是否正确交换
        assert_eq!(response.header().source(), 200);
        assert_eq!(response.header().target(), 100);
        assert_eq!(response.header().command(), RexCommand::CheckReturn);
    }
}
