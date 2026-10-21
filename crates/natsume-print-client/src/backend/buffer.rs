//! Buffers owned by this process and filled by bounded Win32 query calls.
use std::mem::{align_of, size_of};

use windows::core::PWSTR;

use crate::{NativeBufferSnafu, Result};

pub(super) struct NativeBuffer {
    words: Vec<usize>,
    length: usize,
}

impl NativeBuffer {
    pub(super) fn new(length: u32) -> Result<Self> {
        let length = length as usize;
        if length == 0 || length > 16 * 1024 * 1024 {
            return NativeBufferSnafu.fail();
        }
        Ok(Self {
            words: vec![0; length.div_ceil(size_of::<usize>())],
            length,
        })
    }

    pub(super) fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the zero-initialized allocation is aligned and covers length bytes.
        unsafe { std::slice::from_raw_parts_mut(self.words.as_mut_ptr().cast(), self.length) }
    }

    pub(super) fn pointer<T>(&mut self) -> Result<*mut T> {
        if size_of::<T>() > self.length || align_of::<T>() > align_of::<usize>() {
            return NativeBufferSnafu.fail();
        }
        Ok(self.words.as_mut_ptr().cast())
    }

    /// Caller must request a plain Win32 structure actually written by a successful API call.
    pub(super) unsafe fn read<T: Copy>(&self, index: usize) -> Result<T> {
        let offset = index
            .checked_mul(size_of::<T>())
            .ok_or_else(|| NativeBufferSnafu.build())?;
        if offset
            .checked_add(size_of::<T>())
            .is_none_or(|end| end > self.length)
        {
            return NativeBufferSnafu.fail();
        }
        // SAFETY: bounds are checked above; the caller guarantees the initialized type.
        Ok(unsafe {
            self.words
                .as_ptr()
                .cast::<u8>()
                .add(offset)
                .cast::<T>()
                .read_unaligned()
        })
    }

    pub(super) fn string(&self, pointer: PWSTR) -> Result<String> {
        if pointer.is_null() {
            return Ok(String::new());
        }
        let start = self.words.as_ptr() as usize;
        let address = pointer.0 as usize;
        let Some(offset) = address.checked_sub(start) else {
            return NativeBufferSnafu.fail();
        };
        if offset >= self.length || offset % align_of::<u16>() != 0 {
            return NativeBufferSnafu.fail();
        }
        let capacity = (self.length - offset) / size_of::<u16>();
        // SAFETY: this UTF-16 view is aligned and wholly inside the live allocation.
        let units = unsafe { std::slice::from_raw_parts(pointer.0, capacity) };
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .ok_or_else(|| NativeBufferSnafu.build())?;
        String::from_utf16(&units[..end]).map_err(|_| NativeBufferSnafu.build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_and_utf16_termination_are_checked_before_reading() -> Result<()> {
        assert!(NativeBuffer::new(0).is_err());
        assert!(NativeBuffer::new(16 * 1024 * 1024 + 1).is_err());
        let mut buffer = NativeBuffer::new(8)?;
        buffer.bytes().copy_from_slice(&[65, 0, 66, 0, 0, 0, 0, 0]);
        let start = buffer.words.as_mut_ptr().cast::<u16>();
        assert_eq!(buffer.string(PWSTR(start))?, "AB");
        assert!(buffer.string(PWSTR(start.wrapping_add(10))).is_err());
        // SAFETY: these initialized bytes represent a plain u32; the out-of-bounds index is rejected.
        assert!(unsafe { buffer.read::<u32>(2) }.is_err());
        buffer.bytes().fill(1);
        assert!(buffer.string(PWSTR(start)).is_err());
        Ok(())
    }
}
