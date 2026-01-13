use bytes::{Bytes, BytesMut};

use crate::{RetCode, RexCommand};

#[repr(C, packed)]
pub struct RexHead {
    pub total_len: u32,
    pub command: u32,
    pub retcode: u32,
    pub pad: [u8; 4],
    pub source: u128,
}

/*
 * RexData
 *
*/
pub struct RexData {
    pub content: BytesMut,
}

pub const REX_LEN_SIZE: usize = 4;
pub const REX_HEAD_LEN: usize = std::mem::size_of::<RexHead>();

pub const TITLE_LEN_SIZE: usize = 1;
pub const TITLE_LEN_OFFSET: usize = REX_HEAD_LEN;
pub const TITLE_OFFSET: usize = REX_HEAD_LEN + TITLE_LEN_SIZE;

impl RexData {
    // ===== Header access =====

    #[inline(always)]
    fn head(&self) -> &RexHead {
        debug_assert!(self.content.len() >= REX_HEAD_LEN);
        unsafe { &*(self.content.as_ptr() as *const RexHead) }
    }

    #[inline(always)]
    fn head_mut(&mut self) -> &mut RexHead {
        debug_assert!(self.content.len() >= REX_HEAD_LEN);
        unsafe { &mut *(self.content.as_mut_ptr() as *mut RexHead) }
    }

    // ===== Pack / Unpack =====

    #[inline]
    pub fn pack_ref(&self) -> &BytesMut {
        &self.content
    }

    #[inline]
    pub fn pack(self) -> Bytes {
        self.content.freeze()
    }

    #[inline]
    pub fn unpack(buf: BytesMut) -> Self {
        debug_assert!(buf.len() >= REX_HEAD_LEN);
        RexData { content: buf }
    }

    pub fn try_deserialize(buf: &mut BytesMut) -> anyhow::Result<Option<Self>> {
        if buf.len() < REX_LEN_SIZE {
            return Ok(None);
        }

        let total_len = ((buf[0] as u32)
            | ((buf[1] as u32) << 8)
            | ((buf[2] as u32) << 16)
            | ((buf[3] as u32) << 24)) as usize;

        debug_assert!(
            total_len >= REX_HEAD_LEN + TITLE_LEN_SIZE,
            "invalid total_len={}",
            total_len
        );

        if buf.len() < total_len {
            return Ok(None);
        }

        let pack = buf.split_to(total_len);
        Ok(Some(RexData::unpack(pack)))
    }

    // ===== Constructor =====

    pub fn new(command: RexCommand, title: &str, data: &[u8]) -> Self {
        let title_len = title.len();
        let data_len = data.len();

        let total_len = REX_HEAD_LEN + TITLE_LEN_SIZE + title_len + data_len;

        let mut content = BytesMut::with_capacity(total_len);
        unsafe {
            content.set_len(total_len);
        }

        let mut rex_data = RexData { content };

        unsafe {
            let ptr = rex_data.content.as_mut_ptr();
            let mut pos = REX_HEAD_LEN;

            // title length
            *ptr.add(pos) = title_len as u8;
            pos += TITLE_LEN_SIZE;

            // title
            if title_len > 0 {
                std::ptr::copy_nonoverlapping(title.as_bytes().as_ptr(), ptr.add(pos), title_len);
                pos += title_len;
            }

            // data
            if data_len > 0 {
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr.add(pos), data_len);
            }
        }

        let head = rex_data.head_mut();
        *head = RexHead {
            total_len: total_len as u32,
            command: command.as_u32(),
            retcode: RetCode::Success.as_u32(),
            pad: [0; 4],
            source: 0,
        };

        rex_data
    }

    // ===== Accessors =====

    #[inline(always)]
    pub fn command(&self) -> RexCommand {
        RexCommand::from_u32(self.head().command)
    }

    #[inline(always)]
    pub fn source(&self) -> u128 {
        self.head().source
    }

    #[inline(always)]
    pub fn retcode(&self) -> RetCode {
        RetCode::from_u32(self.head().retcode)
    }

    #[inline(always)]
    pub fn is_success(&self) -> bool {
        self.retcode() == RetCode::Success
    }

    #[inline(always)]
    pub fn title_len(&self) -> usize {
        debug_assert!(self.content.len() > TITLE_LEN_OFFSET);
        self.content[TITLE_LEN_OFFSET] as usize
    }

    #[inline]
    pub fn title(&self) -> &str {
        let len = self.title_len();
        let start = TITLE_OFFSET;
        let end = start + len;

        debug_assert!(
            end <= self.content.len(),
            "title out of bounds: start={}, len={}, content_len={}",
            start,
            len,
            self.content.len()
        );

        unsafe { std::str::from_utf8_unchecked(&self.content[start..end]) }
    }

    #[inline(always)]
    fn data_offset(&self) -> usize {
        TITLE_OFFSET + self.title_len()
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        let offset = self.data_offset();

        debug_assert!(
            offset <= self.content.len(),
            "data offset out of bounds: offset={}, content_len={}",
            offset,
            self.content.len()
        );

        &self.content[offset..]
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut [u8] {
        let offset = self.data_offset();

        debug_assert!(
            offset <= self.content.len(),
            "data_mut offset out of bounds: offset={}, content_len={}",
            offset,
            self.content.len()
        );

        &mut self.content[offset..]
    }

    // ===== Mutators =====

    #[inline(always)]
    pub fn set_command(&mut self, command: RexCommand) -> &mut Self {
        self.head_mut().command = command.as_u32();
        self
    }

    #[inline(always)]
    pub fn set_source(&mut self, source: u128) -> &mut Self {
        self.head_mut().source = source;
        self
    }

    #[inline(always)]
    pub fn set_retcode(&mut self, retcode: RetCode) -> &mut Self {
        self.head_mut().retcode = retcode.as_u32();
        self
    }

    pub fn set_data(&mut self, data: &[u8]) -> &mut Self {
        let offset = self.data_offset();
        let data_len = data.len();
        let len = offset + data_len;

        if len > self.content.len() {
            self.content.resize(len, 0);
        } else {
            unsafe {
                self.content.set_len(len);
            }
        }

        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.content[offset..].as_mut_ptr(),
                data_len,
            );
        }

        self.head_mut().total_len = len as u32;
        self
    }

    // ===== Helpers =====

    #[inline]
    pub fn data_as_string_lossy(&self) -> String {
        String::from_utf8_lossy(self.data()).into_owned()
    }
}
