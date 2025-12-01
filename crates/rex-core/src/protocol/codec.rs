use bytes::{Buf, BufMut, Bytes, BytesMut};
use crc32fast::Hasher as Crc32Hasher;

use super::{RetCode, RexCommand};
use crate::error::RexError;

// 协议常量
pub const MAGIC: u32 = 0x5245584D; // 'REXM'
pub const VERSION: u16 = 1;
pub const FIXED_HEADER_LEN: usize = 56; // 4+2+2+4+4+4+16+16+4

// 最大限制（防止恶意数据）
const MAX_EXT_LEN: usize = 1024 * 1024; // 1MB
const MAX_DATA_LEN: usize = 100 * 1024 * 1024; // 100MB

/// 原始协议头部（用于序列化/反序列化）
#[derive(Debug, Clone, Copy)]
pub struct RawHeader {
    pub magic: u32,
    pub version: u16,
    pub flags: u16,
    pub ext_len: u32,
    pub data_len: u32,
    pub command: u32,
    pub source: u128,
    pub target: u128,
    pub retcode: u32,
}

impl RawHeader {
    /// 快速验证头部基本字段
    #[inline]
    pub fn validate(&self) -> Result<(), RexError> {
        if self.magic != MAGIC {
            return Err(RexError::DataCorrupted);
        }
        if self.version != VERSION {
            return Err(RexError::VersionMismatch {
                expected: VERSION,
                actual: self.version,
            });
        }
        if self.ext_len as usize > MAX_EXT_LEN {
            return Err(RexError::DataTooLarge {
                field: "ext_len",
                size: self.ext_len as usize,
                max: MAX_EXT_LEN,
            });
        }
        if self.data_len as usize > MAX_DATA_LEN {
            return Err(RexError::DataTooLarge {
                field: "data_len",
                size: self.data_len as usize,
                max: MAX_DATA_LEN,
            });
        }
        Ok(())
    }

    /// 从缓冲区读取头部（零拷贝）
    #[inline]
    pub fn read_from(buf: &[u8]) -> Result<Self, RexError> {
        if buf.len() < FIXED_HEADER_LEN {
            return Err(RexError::InsufficientData);
        }

        // 使用 unsafe 快速读取（避免边界检查）
        let header = unsafe {
            let ptr = buf.as_ptr();
            Self {
                magic: u32::from_le_bytes([*ptr.add(0), *ptr.add(1), *ptr.add(2), *ptr.add(3)]),
                version: u16::from_le_bytes([*ptr.add(4), *ptr.add(5)]),
                flags: u16::from_le_bytes([*ptr.add(6), *ptr.add(7)]),
                ext_len: u32::from_le_bytes([*ptr.add(8), *ptr.add(9), *ptr.add(10), *ptr.add(11)]),
                data_len: u32::from_le_bytes([
                    *ptr.add(12),
                    *ptr.add(13),
                    *ptr.add(14),
                    *ptr.add(15),
                ]),
                command: u32::from_le_bytes([
                    *ptr.add(16),
                    *ptr.add(17),
                    *ptr.add(18),
                    *ptr.add(19),
                ]),
                source: u128::from_le_bytes([
                    *ptr.add(20),
                    *ptr.add(21),
                    *ptr.add(22),
                    *ptr.add(23),
                    *ptr.add(24),
                    *ptr.add(25),
                    *ptr.add(26),
                    *ptr.add(27),
                    *ptr.add(28),
                    *ptr.add(29),
                    *ptr.add(30),
                    *ptr.add(31),
                    *ptr.add(32),
                    *ptr.add(33),
                    *ptr.add(34),
                    *ptr.add(35),
                ]),
                target: u128::from_le_bytes([
                    *ptr.add(36),
                    *ptr.add(37),
                    *ptr.add(38),
                    *ptr.add(39),
                    *ptr.add(40),
                    *ptr.add(41),
                    *ptr.add(42),
                    *ptr.add(43),
                    *ptr.add(44),
                    *ptr.add(45),
                    *ptr.add(46),
                    *ptr.add(47),
                    *ptr.add(48),
                    *ptr.add(49),
                    *ptr.add(50),
                    *ptr.add(51),
                ]),
                retcode: u32::from_le_bytes([
                    *ptr.add(52),
                    *ptr.add(53),
                    *ptr.add(54),
                    *ptr.add(55),
                ]),
            }
        };

        header.validate()?;
        Ok(header)
    }

    /// 写入头部到缓冲区
    #[inline]
    pub fn write_to(&self, buf: &mut BytesMut) {
        buf.reserve(FIXED_HEADER_LEN);
        buf.put_u32_le(self.magic);
        buf.put_u16_le(self.version);
        buf.put_u16_le(self.flags);
        buf.put_u32_le(self.ext_len);
        buf.put_u32_le(self.data_len);
        buf.put_u32_le(self.command);
        buf.put_u128_le(self.source);
        buf.put_u128_le(self.target);
        buf.put_u32_le(self.retcode);
    }
}

/// 扩展头部编解码器
pub struct ExtCodec;

impl ExtCodec {
    /// 编码 title（如果存在）
    #[inline]
    pub fn encode_title(title: Option<&str>) -> Option<BytesMut> {
        title.map(|t| {
            let title_bytes = t.as_bytes();
            let mut buf = BytesMut::with_capacity(4 + title_bytes.len());
            buf.put_u32_le(title_bytes.len() as u32);
            buf.put_slice(title_bytes);
            buf
        })
    }

    /// 解码 title（零拷贝）
    #[inline]
    pub fn decode_title(buf: &[u8]) -> Result<String, RexError> {
        if buf.len() < 4 {
            return Err(RexError::InsufficientData);
        }

        let title_len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

        if buf.len() < 4 + title_len {
            return Err(RexError::InsufficientData);
        }

        String::from_utf8(buf[4..4 + title_len].to_vec()).map_err(RexError::InvalidString)
    }
}

/// CRC32 校验工具
pub struct Crc32;

impl Crc32 {
    /// 计算 CRC32
    #[inline]
    pub fn compute(ext: &[u8], data: &[u8]) -> u32 {
        let mut hasher = Crc32Hasher::new();
        hasher.update(ext);
        hasher.update(data);
        hasher.finalize()
    }

    /// 验证 CRC32
    #[inline]
    pub fn verify(ext: &[u8], data: &[u8], expected: u32) -> bool {
        Self::compute(ext, data) == expected
    }
}

/// 消息编解码器
pub struct MessageCodec;

impl MessageCodec {
    /// 快速检查是否有完整消息（用于流式处理）
    #[inline]
    pub fn peek_message_len(buf: &[u8]) -> Option<usize> {
        if buf.len() < FIXED_HEADER_LEN {
            return None;
        }

        // 快速读取长度字段
        let ext_len = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]) as usize;
        let data_len = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]) as usize;

        // 检查合法性
        if ext_len > MAX_EXT_LEN || data_len > MAX_DATA_LEN {
            return None;
        }

        let total = FIXED_HEADER_LEN + ext_len + data_len + 4;

        if buf.len() >= total {
            Some(total)
        } else {
            None
        }
    }

    /// 编码消息（优化版本）
    pub fn encode(
        command: RexCommand,
        source: u128,
        target: u128,
        retcode: RetCode,
        title: Option<&str>,
        data: &[u8],
    ) -> BytesMut {
        // 编码扩展部分
        let ext_bytes = ExtCodec::encode_title(title);
        let ext_len = ext_bytes.as_ref().map_or(0, |b| b.len());

        // 计算 CRC
        let crc = Crc32::compute(ext_bytes.as_ref().map_or(&[], |b| &b[..]), data);

        // 预分配精确大小
        let total_len = FIXED_HEADER_LEN + ext_len + data.len() + 4;
        let mut buf = BytesMut::with_capacity(total_len);

        // 写入头部
        let header = RawHeader {
            magic: MAGIC,
            version: VERSION,
            flags: 0,
            ext_len: ext_len as u32,
            data_len: data.len() as u32,
            command: command.as_u32(),
            source,
            target,
            retcode: retcode as u32,
        };
        header.write_to(&mut buf);

        // 写入扩展和数据
        if let Some(ext) = ext_bytes {
            buf.put_slice(&ext);
        }
        buf.put_slice(data);

        // 写入 CRC
        buf.put_u32_le(crc);

        buf
    }

    /// 零拷贝解码消息（从 BytesMut）
    pub fn decode(buf: &mut BytesMut) -> Result<DecodedMessage, RexError> {
        // 1. 先验证完整性
        let total_len = Self::peek_message_len(buf).ok_or(RexError::InsufficientData)?;

        // 2. 读取并验证头部
        let header = RawHeader::read_from(buf)?;
        let ext_len = header.ext_len as usize;
        let data_len = header.data_len as usize;

        // 3. 解析枚举
        let command =
            RexCommand::from_u32(header.command).ok_or(RexError::InvalidCommand(header.command))?;
        let retcode =
            RetCode::from_u32(header.retcode).ok_or(RexError::InvalidRetCode(header.retcode))?;

        #[cfg(not(feature = "skip_crc"))]
        {
            // 4. 提前计算 CRC（在消费数据前）
            let ext_start = FIXED_HEADER_LEN;
            let data_start = ext_start + ext_len;
            let crc_start = data_start + data_len;

            let ext_slice = if ext_len > 0 {
                &buf[ext_start..data_start]
            } else {
                &[]
            };
            let data_slice = &buf[data_start..crc_start];
            let crc_read = u32::from_le_bytes([
                buf[crc_start],
                buf[crc_start + 1],
                buf[crc_start + 2],
                buf[crc_start + 3],
            ]);

            // 验证 CRC
            if !Crc32::verify(ext_slice, data_slice, crc_read) {
                return Err(RexError::DataCorrupted);
            }
        }

        // 5. 跳过固定头部
        buf.advance(FIXED_HEADER_LEN);

        // 6. 提取并解析扩展部分
        let title = if ext_len > 0 {
            let ext_slice = &buf[..ext_len];
            let title = ExtCodec::decode_title(ext_slice)?;
            buf.advance(ext_len);
            Some(title)
        } else {
            None
        };

        // 7. 提取数据（零拷贝！split_to + freeze）
        let data_bytes = buf.split_to(data_len).freeze();

        // 8. 跳过 CRC
        buf.advance(4);

        Ok(DecodedMessage {
            command,
            source: header.source,
            target: header.target,
            retcode,
            title,
            data: data_bytes,
            consumed_bytes: total_len,
        })
    }

    /// 流式解码（零拷贝版本）
    pub fn decode_stream(buf: &mut BytesMut) -> Result<Option<DecodedMessage>, RexError> {
        // 快速检查是否有完整消息
        if Self::peek_message_len(buf).is_none() {
            return Ok(None);
        }

        // 零拷贝解码
        Self::decode(buf).map(Some)
    }
}

/// 解码后的消息（中间结构 - 零拷贝）
#[derive(Debug, Clone)]
pub struct DecodedMessage {
    pub command: RexCommand,
    pub source: u128,
    pub target: u128,
    pub retcode: RetCode,
    pub title: Option<String>,
    pub data: Bytes,
    #[allow(dead_code)]
    pub consumed_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_header_roundtrip() {
        let header = RawHeader {
            magic: MAGIC,
            version: VERSION,
            flags: 0,
            ext_len: 100,
            data_len: 200,
            command: 9901,
            source: 12345,
            target: 67890,
            retcode: 0,
        };

        let mut buf = BytesMut::new();
        header.write_to(&mut buf);

        let decoded = RawHeader::read_from(&buf).unwrap();
        assert_eq!(header.magic, decoded.magic);
        assert_eq!(header.ext_len, decoded.ext_len);
        assert_eq!(header.data_len, decoded.data_len);
    }

    #[test]
    fn test_peek_message_len() {
        let mut buf = BytesMut::new();

        let header = RawHeader {
            magic: MAGIC,
            version: VERSION,
            flags: 0,
            ext_len: 10,
            data_len: 20,
            command: 9901,
            source: 0,
            target: 0,
            retcode: 0,
        };
        header.write_to(&mut buf);
        buf.resize(FIXED_HEADER_LEN + 10 + 20 + 4, 0);

        let len = MessageCodec::peek_message_len(&buf);
        assert_eq!(len, Some(FIXED_HEADER_LEN + 10 + 20 + 4));
    }

    #[test]
    fn test_codec_zerocopy_roundtrip() {
        let encoded = MessageCodec::encode(
            RexCommand::Login,
            123,
            456,
            RetCode::Success,
            Some("test_title"),
            b"hello world",
        );

        let mut buf = encoded;
        let buf_len = buf.len();
        let decoded = MessageCodec::decode(&mut buf).unwrap();

        assert_eq!(decoded.command, RexCommand::Login);
        assert_eq!(decoded.source, 123);
        assert_eq!(decoded.target, 456);
        assert_eq!(decoded.title.as_deref(), Some("test_title"));
        assert_eq!(&decoded.data[..], b"hello world");
        assert_eq!(decoded.consumed_bytes, buf_len);
        assert_eq!(buf.len(), 0); // 完全消费
    }

    #[test]
    fn test_stream_decode() {
        let mut buf =
            MessageCodec::encode(RexCommand::Title, 1, 2, RetCode::Success, None, b"data");

        let msg = MessageCodec::decode_stream(&mut buf)
            .unwrap()
            .expect("should have message");

        assert_eq!(msg.command, RexCommand::Title);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_zerocopy_clone() {
        let mut buf = MessageCodec::encode(
            RexCommand::Title,
            1,
            2,
            RetCode::Success,
            Some("test"),
            b"hello",
        );

        let msg = MessageCodec::decode(&mut buf).unwrap();

        // 克隆数据（引用计数，零拷贝）
        let data1 = msg.data.clone();
        let data2 = msg.data.clone();

        assert_eq!(data1.len(), data2.len());
        assert_eq!(&data1[..], b"hello");
    }

    #[test]
    fn test_invalid_magic() {
        let mut buf = BytesMut::with_capacity(FIXED_HEADER_LEN);
        buf.put_u32_le(0xDEADBEEF);
        buf.resize(FIXED_HEADER_LEN, 0);

        let result = RawHeader::read_from(&buf);
        assert!(matches!(result, Err(RexError::DataCorrupted)));
    }

    #[test]
    fn test_crc_verification() {
        let data = b"test data";
        let ext = b"";

        let crc = Crc32::compute(ext, data);
        assert!(Crc32::verify(ext, data, crc));
        assert!(!Crc32::verify(ext, data, crc + 1));
    }
}
