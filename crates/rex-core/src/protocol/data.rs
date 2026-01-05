use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use rkyv::deserialize;
use rkyv::util::AlignedVec;
use rkyv::{Archive, Deserialize, Serialize, rancor::Error};
use rkyv::{api::high::to_bytes_with_alloc, ser::allocator::Arena};
use rkyv::{munge::munge, seal::Seal};
use tracing::{debug, warn};

use crate::{RetCode, RexCommand};

#[derive(Archive, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct RexHeader {
    pub command: u32,
    pub source: u128,
    pub retcode: u32,
}

impl RexHeader {
    pub fn new(command: RexCommand, source: u128) -> Self {
        Self {
            command: command.as_u32(),
            source,
            retcode: RetCode::Success.as_u32(),
        }
    }

    pub fn with_retcode(command: RexCommand, source: u128, retcode: RetCode) -> Self {
        Self {
            command: command.as_u32(),
            source,
            retcode: retcode.as_u32(),
        }
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, PartialEq, Clone)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct RexData {
    pub header: RexHeader,
    pub title: String,
    pub data: Vec<u8>,
}

impl RexData {
    pub fn new(command: RexCommand, source: u128, title: String, data: Vec<u8>) -> Self {
        Self {
            header: RexHeader::new(command, source),
            title,
            data,
        }
    }

    /// 序列化为带长度前缀的字节流（用于网络传输）
    /// 格式: [4字节长度(大端)] + [rkyv数据]
    pub fn serialize(&self) -> Vec<u8> {
        if let Ok(bytes) = self.encode() {
            bytes.to_vec()
        } else {
            warn!("Failed to serialize RexData");
            vec![]
        }
    }

    pub fn try_deserialize(buffer: &mut BytesMut) -> Result<Option<Vec<u8>>> {
        // 检查是否有足够的数据读取长度前缀
        if buffer.len() < 4 {
            return Ok(None);
        }

        // 读取长度（不消耗 buffer）
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&buffer[..4]);
        let payload_len = u32::from_be_bytes(len_bytes) as usize;

        debug!("deserialize payload size: {}", payload_len);

        // 检查长度是否合理（防止恶意数据）
        const MAX_PAYLOAD_SIZE: usize = 10 * 1024 * 1024; // 10MB
        if payload_len > MAX_PAYLOAD_SIZE {
            anyhow::bail!(
                "Payload size too large: {} bytes (max: {} bytes)",
                payload_len,
                MAX_PAYLOAD_SIZE
            );
        }

        // 检查是否有完整的消息
        if buffer.len() < 4 + payload_len {
            return Ok(None);
        }

        // 消耗长度前缀
        buffer.advance(4);

        // 提取并消耗 payload
        let payload = buffer.split_to(payload_len);

        Ok(Some(payload.to_vec()))
    }

    pub fn encode(&self) -> Result<AlignedVec> {
        let mut arena = Arena::new();
        let bytes = to_bytes_with_alloc::<_, Error>(self, arena.acquire())?;
        Ok(bytes)
    }

    pub fn as_archive(bytes: &[u8]) -> &ArchivedRexData {
        unsafe { rkyv::access_unchecked::<ArchivedRexData>(bytes) }
    }

    fn as_archive_mut(bytes: &mut [u8]) -> Seal<'_, ArchivedRexData> {
        unsafe { rkyv::access_unchecked_mut::<ArchivedRexData>(bytes) }
    }

    pub fn update_header(
        data_bytes: &mut [u8],
        command: Option<RexCommand>,
        source: Option<u128>,
        retcode: Option<RetCode>,
    ) {
        let archived = Self::as_archive_mut(data_bytes);
        munge!(let ArchivedRexData {
            header,
            ..
        } = archived);

        munge!(let ArchivedRexHeader{
            command: mut cmd,
            source: mut src,
            retcode: mut rc
        } = header);

        if let Some(v) = command {
            *cmd = v.as_u32().into();
        }
        if let Some(v) = source {
            *src = v.into();
        }
        if let Some(v) = retcode {
            *rc = v.as_u32().into();
        }
    }

    pub fn from_archive(archived: &ArchivedRexData) -> Result<Self> {
        Ok(deserialize::<Self, Error>(archived)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let archived = Self::as_archive(bytes);
        Self::from_archive(archived)
    }

    // === 访问器方法 ===

    #[inline(always)]
    pub fn command(&self) -> RexCommand {
        RexCommand::from_u32(self.header.command)
    }

    #[inline(always)]
    pub fn source(&self) -> u128 {
        self.header.source
    }

    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    #[inline(always)]
    pub fn title(&self) -> &str {
        self.title.as_ref()
    }

    #[inline(always)]
    pub fn retcode(&self) -> RetCode {
        RetCode::from_u32(self.header.retcode)
    }

    #[inline(always)]
    pub fn is_success(&self) -> bool {
        self.retcode() == RetCode::Success
    }

    #[inline]
    pub fn data_bytes(&self) -> Bytes {
        Bytes::from(self.data.clone())
    }

    // === 修改器方法 - 链式调用优化 ===

    #[inline(always)]
    pub fn set_command(&mut self, command: RexCommand) -> &mut Self {
        self.header.command = command.as_u32();
        self
    }

    #[inline(always)]
    pub fn set_source(&mut self, source: u128) -> &mut Self {
        self.header.source = source;
        self
    }

    #[inline(always)]
    pub fn set_retcode(&mut self, retcode: RetCode) -> &mut Self {
        self.header.retcode = retcode.as_u32();
        self
    }

    #[inline(always)]
    pub fn set_title(&mut self, title: String) -> &mut Self {
        self.title = title;
        self
    }

    // === 数据处理方法 ===

    #[inline]
    pub fn data_as_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}
