use std::io;
use std::string::FromUtf8Error;

use anyhow::Result;
use bytes::{Buf, BufMut, BytesMut};
use crc32fast::Hasher as Crc32Hasher;
use thiserror::Error;

use crate::command::RexCommand;

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
    pub fn as_u32(self) -> u32 {
        self as u32
    }

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

    pub fn is_error(self) -> bool {
        !self.is_success()
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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RexHeader {
    header_len: usize,
    header_ext_len: usize,
    data_len: usize,
    command: RexCommand,
    source: usize,
    target: usize,
    retcode: RetCode, // 新增返回码字段
}

impl RexHeader {
    pub fn new(command: RexCommand, source: usize, target: usize) -> Self {
        Self {
            header_len: FIXED_HEADER_LEN,
            header_ext_len: 0,
            data_len: 0,
            command,
            source,
            target,
            retcode: RetCode::Success, // 默认为成功
        }
    }

    pub fn new_with_retcode(
        command: RexCommand,
        source: usize,
        target: usize,
        retcode: RetCode,
    ) -> Self {
        Self {
            header_len: FIXED_HEADER_LEN,
            header_ext_len: 0,
            data_len: 0,
            command,
            source,
            target,
            retcode,
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
    pub fn retcode(&self) -> RetCode {
        self.retcode
    }

    // Setter方法
    pub fn set_header_ext_len(&mut self, len: usize) {
        self.header_ext_len = len;
    }

    pub fn set_data_len(&mut self, len: usize) {
        self.data_len = len;
    }

    pub fn set_retcode(&mut self, retcode: RetCode) {
        self.retcode = retcode;
    }

    // 便捷方法
    pub fn is_success(&self) -> bool {
        self.retcode.is_success()
    }

    pub fn is_error(&self) -> bool {
        self.retcode.is_error()
    }

    // 计算总包大小
    pub fn total_size(&self) -> usize {
        self.header_len + self.header_ext_len + self.data_len
    }
}

// ===== 新的显式编码帧格式 =====
// 小端序，字段顺序固定：
// magic(u32) | version(u16) | flags(u16) |
// ext_len(u32) | data_len(u32) |
// command(u32) | source(u64) | target(u64) | retcode(u32) |
// [ext bytes] | [data bytes] | crc32(u32)
const MAGIC: u32 = 0x5245584D; // 'REXM'
const VERSION: u16 = 1;
const FIXED_HEADER_LEN: usize = 4 + 2 + 2 + 4 + 4 + 4 + 8 + 8 + 4; // 不含crc
const CRC_LEN: usize = 4;

fn compute_crc32(ext: &[u8], data: &[u8]) -> u32 {
    let mut hasher = Crc32Hasher::new();
    hasher.update(ext);
    hasher.update(data);
    hasher.finalize()
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
        let title = String::from_utf8(title_bytes.to_vec())?;

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

    // 创建带返回码的数据包
    pub fn new_with_retcode(
        command: RexCommand,
        source: usize,
        target: usize,
        retcode: RetCode,
        data: BytesMut,
    ) -> Self {
        let mut header = RexHeader::new_with_retcode(command, source, target, retcode);
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

    // 创建带扩展头部和返回码的数据包
    pub fn new_with_title_and_retcode(
        command: RexCommand,
        source: usize,
        target: usize,
        title: String,
        retcode: RetCode,
        data: BytesMut,
    ) -> Self {
        let header_ext = RexHeaderExt::new(title);
        let mut header = RexHeader::new_with_retcode(command, source, target, retcode);

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
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    pub fn data_mut(&mut self) -> &mut BytesMut {
        &mut self.data
    }

    // 获取title（如果存在）
    pub fn title(&self) -> Option<&str> {
        self.header_ext.as_ref().map(|ext| ext.title())
    }

    // 获取返回码相关方法
    pub fn retcode(&self) -> RetCode {
        self.header.retcode()
    }

    pub fn is_success(&self) -> bool {
        self.header.is_success()
    }

    pub fn is_error(&self) -> bool {
        self.header.is_error()
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

    // 设置返回码
    pub fn set_retcode(&mut self, retcode: RetCode) -> &mut Self {
        self.header.set_retcode(retcode);
        self
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

    // 序列化整个数据包（显式编码 + CRC）
    pub fn serialize(&self) -> BytesMut {
        let ext_bytes = if let Some(ref ext) = self.header_ext {
            ext.serialize()
        } else {
            BytesMut::new()
        };
        let ext_len = ext_bytes.len() as u32;
        let data_len = self.data.len() as u32;

        let crc = compute_crc32(&ext_bytes, &self.data);

        let mut buf =
            BytesMut::with_capacity(FIXED_HEADER_LEN + ext_bytes.len() + self.data.len() + CRC_LEN);

        // 固定头
        buf.put_u32_le(MAGIC);
        buf.put_u16_le(VERSION);
        buf.put_u16_le(0); // flags 预留
        buf.put_u32_le(ext_len);
        buf.put_u32_le(data_len);
        buf.put_u32_le(self.header.command.as_u32());
        buf.put_u64_le(self.header.source as u64);
        buf.put_u64_le(self.header.target as u64);
        buf.put_u32_le(self.header.retcode.as_u32());

        // 扩展与数据
        if !ext_bytes.is_empty() {
            buf.put_slice(&ext_bytes);
        }
        if !self.data.is_empty() {
            buf.put_slice(&self.data);
        }

        // CRC32
        buf.put_u32_le(crc);

        buf
    }

    // 设置命令
    pub fn set_command(&mut self, command: RexCommand) -> &mut Self {
        self.header.command = command;
        self
    }

    pub fn set_source(&mut self, source: usize) -> &Self {
        self.header.source = source;
        self
    }

    pub fn set_target(&mut self, target: usize) -> &Self {
        self.header.target = target;
        self
    }

    // 反序列化数据包（显式编码 + CRC 校验）
    pub fn deserialize(mut buf: BytesMut) -> Result<Self, RexError> {
        if buf.len() < FIXED_HEADER_LEN + CRC_LEN {
            return Err(RexError::InsufficientData {
                expected: FIXED_HEADER_LEN + CRC_LEN,
                actual: buf.len(),
            });
        }

        // 固定头
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
        RexCommand::from_u32(command_value).ok_or(RexError::InvalidCommand { command_value })?;
        let source = buf.get_u64_le() as usize;
        let target = buf.get_u64_le() as usize;
        let retcode_value = buf.get_u32_le();
        RetCode::from_u32(retcode_value).ok_or(RexError::InvalidRetCode { retcode_value })?;

        // 剩余应为 ext + data + crc
        if buf.len() != ext_len + data_len + CRC_LEN {
            return Err(RexError::DataLengthMismatch {
                declared: ext_len + data_len + CRC_LEN,
                actual: buf.len(),
            });
        }

        let mut ext_buf = if ext_len > 0 {
            buf.split_to(ext_len)
        } else {
            BytesMut::new()
        };
        let data_buf = if data_len > 0 {
            buf.split_to(data_len)
        } else {
            BytesMut::new()
        };
        let crc_read = buf.get_u32_le();

        let crc_calc = compute_crc32(&ext_buf, &data_buf);
        if crc_calc != crc_read {
            return Err(RexError::DataCorrupted);
        }

        let header_ext = if ext_len > 0 {
            Some(RexHeaderExt::deserialize(&mut ext_buf)?)
        } else {
            None
        };

        let mut header =
            RexHeader::new(RexCommand::from_u32(command_value).unwrap(), source, target);
        header.set_header_ext_len(ext_len);
        header.set_data_len(data_len);
        header.set_retcode(RetCode::from_u32(retcode_value).unwrap());

        Ok(Self {
            header,
            header_ext,
            data: data_buf,
        })
    }

    // 从Quinn的RecvStream中读取数据包（异步版本，显式编码 + CRC）
    pub async fn read_from_quinn_stream(stream: &mut quinn::RecvStream) -> Result<Self, RexError> {
        // 读取固定头
        let mut head = vec![0u8; FIXED_HEADER_LEN];
        stream.read_exact(&mut head).await?;
        let mut head_buf = BytesMut::from(&head[..]);

        let magic = head_buf.get_u32_le();
        if magic != MAGIC {
            return Err(RexError::DataCorrupted);
        }
        let version = head_buf.get_u16_le();
        if version != VERSION {
            return Err(RexError::VersionMismatch);
        }
        let _flags = head_buf.get_u16_le();
        let ext_len = head_buf.get_u32_le() as usize;
        let data_len = head_buf.get_u32_le() as usize;
        let command_value = head_buf.get_u32_le();
        RexCommand::from_u32(command_value).ok_or(RexError::InvalidCommand { command_value })?;
        let source = head_buf.get_u64_le() as usize;
        let target = head_buf.get_u64_le() as usize;
        let retcode_value = head_buf.get_u32_le();
        RetCode::from_u32(retcode_value).ok_or(RexError::InvalidRetCode { retcode_value })?;

        // 读取扩展与数据
        let mut ext_bytes = vec![0u8; ext_len];
        if ext_len > 0 {
            stream.read_exact(&mut ext_bytes).await?;
        }
        let mut data_bytes = vec![0u8; data_len];
        if data_len > 0 {
            stream.read_exact(&mut data_bytes).await?;
        }

        // 读取CRC
        let mut crc_buf = [0u8; 4];
        stream.read_exact(&mut crc_buf).await?;
        let mut crc_reader = BytesMut::from(&crc_buf[..]);
        let crc_read = crc_reader.get_u32_le();

        let crc_calc = compute_crc32(&ext_bytes, &data_bytes);
        if crc_calc != crc_read {
            return Err(RexError::DataCorrupted);
        }

        let mut header =
            RexHeader::new(RexCommand::from_u32(command_value).unwrap(), source, target);
        header.set_header_ext_len(ext_len);
        header.set_data_len(data_len);
        header.set_retcode(RetCode::from_u32(retcode_value).unwrap());

        let header_ext = if ext_len > 0 {
            let mut ext_buf = BytesMut::from(&ext_bytes[..]);
            Some(RexHeaderExt::deserialize(&mut ext_buf)?)
        } else {
            None
        };

        Ok(Self {
            header,
            header_ext,
            data: BytesMut::from(&data_bytes[..]),
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
    // 创建成功响应
    pub fn create_success_response(
        &self,
        response_command: RexCommand,
        response_data: BytesMut,
    ) -> Self {
        Self::new_with_retcode(
            response_command,
            self.header.target,
            self.header.source,
            RetCode::Success,
            response_data,
        )
    }

    // 创建错误响应
    pub fn create_error_response(
        &self,
        response_command: RexCommand,
        retcode: RetCode,
        error_msg: &str,
    ) -> Self {
        let error_data = BytesMut::from(error_msg.as_bytes());
        Self::new_with_retcode(
            response_command,
            self.header.target,
            self.header.source,
            retcode,
            error_data,
        )
    }

    // 创建响应数据包（保留原有方法但添加返回码支持）
    pub fn create_response(&self, response_command: RexCommand, response_data: BytesMut) -> Self {
        Self::create_success_response(self, response_command, response_data)
    }

    // 创建带返回码的响应
    pub fn create_response_with_retcode(
        &self,
        response_command: RexCommand,
        retcode: RetCode,
        response_data: BytesMut,
    ) -> Self {
        Self::new_with_retcode(
            response_command,
            self.header.target,
            self.header.source,
            retcode,
            response_data,
        )
    }
}

// 错误类型定义
#[derive(Error, Debug)]
pub enum RexError {
    #[error("数据长度不足: 需要 {expected} 字节，但只有 {actual} 字节")]
    InsufficientData { expected: usize, actual: usize },

    #[error("无效的命令值: {command_value}")]
    InvalidCommand { command_value: u32 },

    #[error("无效的返回码: {retcode_value}")]
    InvalidRetCode { retcode_value: u32 },

    #[error("字符串编码错误: {source}")]
    InvalidString {
        #[from]
        source: FromUtf8Error,
    },

    #[error("数据长度不匹配: 头部声明 {declared} 字节，实际 {actual} 字节")]
    DataLengthMismatch { declared: usize, actual: usize },

    #[error("数据过大: {size} 字节超过限制 {limit} 字节")]
    DataTooLarge { size: usize, limit: usize },

    #[error("数据损坏或CRC校验失败")]
    DataCorrupted,

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

    #[error("协议版本不匹配")]
    VersionMismatch,
}

// RexData的构建器模式
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
            RexData::new_with_title_and_retcode(
                self.command,
                self.source,
                self.target,
                title,
                self.retcode,
                self.data,
            )
        } else {
            RexData::new_with_retcode(
                self.command,
                self.source,
                self.target,
                self.retcode,
                self.data,
            )
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

    // 快速创建带返回码的文本消息
    pub fn text_message_with_retcode(
        command: RexCommand,
        source: usize,
        target: usize,
        retcode: RetCode,
        message: &str,
    ) -> Self {
        let data = BytesMut::from(message.as_bytes());
        Self::new_with_retcode(command, source, target, retcode, data)
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

    // 创建带返回码的空数据包
    pub fn header_only_with_retcode(
        command: RexCommand,
        source: usize,
        target: usize,
        retcode: RetCode,
    ) -> Self {
        Self::new_with_retcode(command, source, target, retcode, BytesMut::new())
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
    fn test_retcode_functionality() {
        // 测试成功响应
        let success_msg = RexData::text_message_with_retcode(
            RexCommand::Cast,
            100,
            200,
            RetCode::Success,
            "operation successful",
        );

        assert!(success_msg.is_success());
        assert!(!success_msg.is_error());
        assert_eq!(success_msg.retcode(), RetCode::Success);

        // 测试错误响应
        let error_msg = RexData::text_message_with_retcode(
            RexCommand::Cast,
            100,
            200,
            RetCode::AuthFailed,
            "authentication failed",
        );

        assert!(!error_msg.is_success());
        assert!(error_msg.is_error());
        assert_eq!(error_msg.retcode(), RetCode::AuthFailed);
    }

    #[test]
    fn test_error_response_creation() {
        let request = RexData::text_message(RexCommand::Login, 100, 200, "login request");
        let error_response = request.create_error_response(
            RexCommand::LoginReturn,
            RetCode::AuthFailed,
            "Invalid credentials",
        );

        assert_eq!(error_response.retcode(), RetCode::AuthFailed);
        assert!(error_response.is_error());
        assert_eq!(
            error_response.data_as_string().unwrap(),
            "Invalid credentials"
        );
    }

    #[test]
    fn test_builder_with_retcode() {
        let msg = RexData::builder(RexCommand::Login)
            .source(123)
            .target(456)
            .retcode(RetCode::InvalidParameter)
            .title("Error Response")
            .data_from_string("Missing required field")
            .build();

        assert_eq!(msg.retcode(), RetCode::InvalidParameter);
        assert!(msg.is_error());
        assert_eq!(msg.title(), Some("Error Response"));
    }

    #[test]
    fn test_retcode_serialization() {
        let original = RexData::builder(RexCommand::Group)
            .source(1000)
            .target(2000)
            .retcode(RetCode::ResourceNotFound)
            .title("错误消息")
            .data_from_string("资源未找到")
            .build();

        let serialized = original.serialize();
        let deserialized = RexData::deserialize(serialized).unwrap();

        assert_eq!(original.retcode(), deserialized.retcode());
        assert_eq!(deserialized.retcode(), RetCode::ResourceNotFound);
        assert!(deserialized.is_error());
    }

    #[test]
    fn test_retcode_descriptions() {
        assert_eq!(RetCode::Success.description(), "Success");
        assert_eq!(RetCode::AuthFailed.description(), "Authentication failed");
        assert_eq!(
            RetCode::ResourceNotFound.description(),
            "Resource not found"
        );
    }

    #[test]
    fn test_response_creation_methods() {
        let request = RexData::text_message(RexCommand::Check, 100, 200, "ping");

        // 测试成功响应
        let success_response = request
            .create_success_response(RexCommand::CheckReturn, BytesMut::from("pong".as_bytes()));

        assert!(success_response.is_success());
        assert_eq!(success_response.retcode(), RetCode::Success);
        assert_eq!(success_response.header().source(), 200);
        assert_eq!(success_response.header().target(), 100);

        // 测试带返回码的响应
        let custom_response = request.create_response_with_retcode(
            RexCommand::CheckReturn,
            RetCode::Timeout,
            BytesMut::from("timeout occurred".as_bytes()),
        );

        assert!(custom_response.is_error());
        assert_eq!(custom_response.retcode(), RetCode::Timeout);
    }

    #[test]
    fn test_retcode_from_u32() {
        assert_eq!(RetCode::from_u32(0), Some(RetCode::Success));
        assert_eq!(RetCode::from_u32(1000), Some(RetCode::InvalidRequest));
        assert_eq!(RetCode::from_u32(2001), Some(RetCode::AuthFailed));
        assert_eq!(RetCode::from_u32(99999), None); // 无效的返回码
    }

    #[test]
    fn test_simple_message_with_default_success() {
        let msg = RexData::text_message(RexCommand::Cast, 100, 200, "test message");

        assert_eq!(msg.header().command(), RexCommand::Cast);
        assert_eq!(msg.header().source(), 100);
        assert_eq!(msg.header().target(), 200);
        assert_eq!(msg.data_as_string().unwrap(), "test message");
        assert!(msg.header_ext().is_none());
        assert!(msg.is_success()); // 默认应该是成功状态
        assert_eq!(msg.retcode(), RetCode::Success);
    }

    #[test]
    fn test_titled_message_with_retcode() {
        let msg = RexData::new_with_title_and_retcode(
            RexCommand::Title,
            100,
            200,
            "Error Title".to_string(),
            RetCode::DataFormatError,
            BytesMut::from("invalid data format".as_bytes()),
        );

        assert_eq!(msg.title(), Some("Error Title"));
        assert_eq!(msg.data_as_string().unwrap(), "invalid data format");
        assert!(msg.header_ext().is_some());
        assert_eq!(msg.retcode(), RetCode::DataFormatError);
        assert!(msg.is_error());
    }
}
