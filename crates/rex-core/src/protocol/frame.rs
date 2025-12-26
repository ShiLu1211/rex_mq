use std::sync::Arc;

use anyhow::{Result, bail};
use bytes::{Buf, BytesMut};
use rkyv::AlignedVec;

use crate::RexClientInner;

pub struct RexFrame {
    pub peer: Arc<RexClientInner>,
    pub payload: AlignedVec,
}

pub struct RexFramer {
    max_frame_size: usize,
}

impl RexFramer {
    pub fn new(max_frame_size: usize) -> Self {
        Self { max_frame_size }
    }

    /// 尝试从 buffer 中解析一帧
    ///
    /// 返回：
    /// - Ok(Some(Bytes))：成功解析一帧 payload
    /// - Ok(None)：数据不足
    /// - Err：协议错误
    pub fn try_next_frame(&mut self, buffer: &mut BytesMut) -> Result<Option<AlignedVec>> {
        const HEADER: usize = 4;

        // header 不完整
        if buffer.len() < HEADER {
            return Ok(None);
        }

        let len = u32::from_be_bytes(buffer[..4].try_into()?) as usize;

        if len == 0 || len > self.max_frame_size {
            bail!("invalid frame size: {len}");
        }

        if buffer.len() < HEADER + len {
            return Ok(None);
        }

        // 消费 header
        buffer.advance(HEADER);

        // 将数据拷贝到对齐的缓冲区
        let mut aligned = AlignedVec::with_capacity(len);
        aligned.extend_from_slice(&buffer[..len]);

        // 从 buffer 中移除已处理的数据
        buffer.advance(len);

        Ok(Some(aligned))
    }
}
