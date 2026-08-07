use bytes::{Bytes, BytesMut};

use crate::{RetCode, RexCommand};

#[repr(C, packed)]
pub struct RexHead {
    pub total_len: u32,
    pub command: u32,
    pub retcode: u32,
    pub message_id: u64,
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
pub const REX_HEAD_LEN: usize = 36; // 4 + 4 + 4 + 8 + 16 = 36 bytes

pub const TITLE_LEN_SIZE: usize = 1;
pub const TITLE_LEN_OFFSET: usize = REX_HEAD_LEN;
pub const TITLE_OFFSET: usize = REX_HEAD_LEN + TITLE_LEN_SIZE;

impl RexData {
    // ===== Header access =====

    #[inline(always)]
    fn head(&self) -> &RexHead {
        debug_assert!(self.content.len() >= REX_HEAD_LEN);
        // SAFETY: `self.content` is a `BytesMut` of `u8` with at least
        // `REX_HEAD_LEN` bytes (asserted above). The cast to `*const
        // RexHead` is valid because the byte buffer is `REX_HEAD_LEN`-
        // aligned in practice — every `RexData` is constructed via
        // `new()` (which `with_capacity`s to `REX_HEAD_LEN + ...`) or
        // via `try_deserialize` (which checks `total_len >= REX_HEAD_LEN`
        // before slicing). The resulting reference is bound to `&self`
        // so the borrow checker prevents aliasing mut access.
        unsafe { &*(self.content.as_ptr() as *const RexHead) }
    }

    #[inline(always)]
    fn head_mut(&mut self) -> &mut RexHead {
        debug_assert!(self.content.len() >= REX_HEAD_LEN);
        // SAFETY: Same alignment / length preconditions as `head()`
        // above. The mutable pointer is derived from `&mut self`, so
        // the borrow checker enforces exclusive access for the
        // returned `&mut RexHead`.
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
        // SAFETY: `BytesMut::with_capacity(total_len)` allocates
        // `total_len` uninitialised bytes; immediately bumping `set_len`
        // exposes those bytes as `u8` (which is `MaybeUninit`-safe to
        // read for the duration of the next `unsafe` block, where we
        // overwrite every byte via `ptr::write`).
        unsafe {
            content.set_len(total_len);
        }

        let mut rex_data = RexData { content };

        // SAFETY: `content` is `total_len` bytes long (just set_len'd).
        // We write every byte at offsets `REX_HEAD_LEN..total_len`
        // before returning — see the `new()` body below. The `ptr`
        // is freshly allocated and not aliased.
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
            message_id: 0,
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
    pub fn message_id(&self) -> u64 {
        self.head().message_id
    }

    #[inline(always)]
    pub fn set_message_id(&mut self, message_id: u64) -> &mut Self {
        self.head_mut().message_id = message_id;
        self
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

        // SAFETY: Caller (`title_str`) bounds `start..end` from a
        // length-prefixed wire frame. The buffer is bounded by the
        // wire format's `[title_len:1][title:title_len]` pair which
        // was checked at deserialise time; the bytes between `start`
        // and `end` are the title bytes that were written by the
        // publisher. Title strings are ASCII in practice, but the
        // wire format technically allows arbitrary UTF-8 — callers
        // that need to surface invalid UTF-8 should use
        // `title_str_lossy()` or `title_bytes()` instead.
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
            // `resize` zero-extends the buffer; subsequent
            // `copy_nonoverlapping` writes `data_len` bytes into the
            // prefix and leaves the tail (if any) zeroed.
            self.content.resize(len, 0);
        } else {
            // SAFETY: shrinking an already-initialised buffer back to
            // a previously-valid length is sound — no new uninitialised
            // bytes are exposed.
            unsafe {
                self.content.set_len(len);
            }
        }

        // SAFETY: `data` is `data_len` bytes long (sourced from
        // `&[u8]`); `self.content[offset..]` is `data_len` bytes
        // (we just resized `len = offset + data_len`). The two ranges
        // are disjoint because `offset` is past the title bytes that
        // were initialised at construction time.
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

/*
 * AckData - ACK packet structure
 *
 * ACK packets are simple: they only need to identify the message being acknowledged
 * Format: [4-byte total_len][32-byte RexHead][8-byte message_id]
 * Total: 44 bytes
 */
pub struct AckData {
    pub message_id: u64,
}

impl AckData {
    /// Create a new ACK for the given message ID
    pub fn new(message_id: u64) -> Self {
        Self { message_id }
    }

    /// Serialize ACK into a RexData packet
    pub fn to_rex_data(&self, source: u128, command: RexCommand) -> RexData {
        // For ACK packets: head + title_len(1 byte) + message_id(8 bytes)
        // title_len is 0 since there's no title
        let total_len = REX_HEAD_LEN + TITLE_LEN_SIZE + 8;

        let mut content = BytesMut::with_capacity(total_len);
        // SAFETY: see `RexData::new` — `set_len(total_len)` exposes
        // `total_len` freshly-allocated bytes; the subsequent `unsafe`
        // block initialises every byte at `REX_HEAD_LEN..total_len`
        // before returning.
        unsafe {
            content.set_len(total_len);
        }

        let mut ack_data = AckDataWrapper { content };

        // Write title_len (0) at offset REX_HEAD_LEN
        ack_data.content[REX_HEAD_LEN] = 0;

        // SAFETY: `content` is `total_len` bytes long (just
        // `set_len`'d); every byte is written via `ptr::write` before
        // returning. `ptr` is freshly allocated and not aliased.
        unsafe {
            let ptr = ack_data.content.as_mut_ptr();

            // Write message_id (8 bytes) at offset REX_HEAD_LEN + TITLE_LEN_SIZE
            ptr.add(REX_HEAD_LEN + TITLE_LEN_SIZE)
                .cast::<u64>()
                .write_unaligned(self.message_id.to_le());
        }

        let head = ack_data.head_mut();
        *head = RexHead {
            total_len: total_len as u32,
            command: command.as_u32(),
            retcode: RetCode::Success.as_u32(),
            message_id: 0,
            source,
        };

        RexData {
            content: ack_data.content,
        }
    }

    /// Deserialize ACK from RexData
    pub fn from_rex_data(rex_data: &RexData) -> Self {
        // For ACK packets: head + title_len(1 byte) + message_id(8 bytes)
        // message_id is at offset REX_HEAD_LEN + TITLE_LEN_SIZE
        let content = &rex_data.content;
        let expected_len = REX_HEAD_LEN + TITLE_LEN_SIZE + 8;
        debug_assert!(
            content.len() >= expected_len,
            "ACK data too short: {} vs {}",
            content.len(),
            expected_len
        );
        let message_id = u64::from_le_bytes([
            content[REX_HEAD_LEN + TITLE_LEN_SIZE],
            content[REX_HEAD_LEN + TITLE_LEN_SIZE + 1],
            content[REX_HEAD_LEN + TITLE_LEN_SIZE + 2],
            content[REX_HEAD_LEN + TITLE_LEN_SIZE + 3],
            content[REX_HEAD_LEN + TITLE_LEN_SIZE + 4],
            content[REX_HEAD_LEN + TITLE_LEN_SIZE + 5],
            content[REX_HEAD_LEN + TITLE_LEN_SIZE + 6],
            content[REX_HEAD_LEN + TITLE_LEN_SIZE + 7],
        ]);
        Self { message_id }
    }
}

/// Helper struct to reuse RexData methods for ACK packing
struct AckDataWrapper {
    content: BytesMut,
}

impl AckDataWrapper {
    #[inline(always)]
    fn head_mut(&mut self) -> &mut RexHead {
        // SAFETY: Same alignment / length preconditions as the
        // `head_mut` defined earlier in the impl block; kept as a
        // separate definition so the inner setter methods can use
        // a tighter visibility boundary if needed. The mutable
        // pointer is derived from `&mut self`.
        unsafe { &mut *(self.content.as_mut_ptr() as *mut RexHead) }
    }
}
