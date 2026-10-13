//! Save states: the whole machine written to bytes and read back.
//!
//! The format is deliberately plain: every component writes its fields in a
//! fixed order as little-endian numbers, with no names or padding. A short
//! header identifies the format, its version and the cartridge, so a state
//! is never loaded into the wrong game. The ROM itself is not stored.
//!
//! Adding or reordering a field anywhere changes the layout, which must be
//! signalled by bumping [`VERSION`].

use std::fmt;

/// Identifies a Glassbit save state.
pub const MAGIC: [u8; 4] = *b"GBST";
/// Bumped whenever the layout changes.
pub const VERSION: u8 = 1;

/// Why a save state could not be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    /// The data does not start like a Glassbit save state.
    NotAState,
    /// The state was written by an incompatible version.
    UnsupportedVersion(u8),
    /// The state belongs to another cartridge.
    WrongCartridge,
    /// The data ends early, has bytes left over or holds impossible values.
    Corrupt,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NotAState => write!(f, "not a Glassbit save state"),
            Self::UnsupportedVersion(version) => {
                write!(f, "save state version {version} is not supported")
            }
            Self::WrongCartridge => write!(f, "save state belongs to another cartridge"),
            Self::Corrupt => write!(f, "save state is damaged"),
        }
    }
}

impl std::error::Error for StateError {}

/// Appends values to a growing byte buffer.
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    pub fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub fn bool(&mut self, value: bool) {
        self.u8(value as u8);
    }

    pub fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes a block whose length the reader already knows.
    pub fn bytes(&mut self, values: &[u8]) {
        self.bytes.extend_from_slice(values);
    }
}

/// Reads values back in the order they were written.
pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], StateError> {
        if self.bytes.len() < N {
            return Err(StateError::Corrupt);
        }
        let (head, rest) = self.bytes.split_at(N);
        self.bytes = rest;
        Ok(head.try_into().expect("split at N"))
    }

    pub fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take::<1>()?[0])
    }

    /// Reads a flag, rejecting anything but 0 and 1.
    pub fn bool(&mut self) -> Result<bool, StateError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StateError::Corrupt),
        }
    }

    pub fn u16(&mut self) -> Result<u16, StateError> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    pub fn u32(&mut self) -> Result<u32, StateError> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    pub fn u64(&mut self) -> Result<u64, StateError> {
        Ok(u64::from_le_bytes(self.take()?))
    }

    pub fn i32(&mut self) -> Result<i32, StateError> {
        Ok(i32::from_le_bytes(self.take()?))
    }

    pub fn f32(&mut self) -> Result<f32, StateError> {
        Ok(f32::from_le_bytes(self.take()?))
    }

    /// Fills `out` with the next `out.len()` bytes.
    pub fn bytes(&mut self, out: &mut [u8]) -> Result<(), StateError> {
        if self.bytes.len() < out.len() {
            return Err(StateError::Corrupt);
        }
        let (head, rest) = self.bytes.split_at(out.len());
        out.copy_from_slice(head);
        self.bytes = rest;
        Ok(())
    }

    /// Succeeds only when every byte has been consumed.
    pub fn finish(self) -> Result<(), StateError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(StateError::Corrupt)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_come_back_in_the_order_written() {
        let mut writer = Writer::new();
        writer.u8(0x12);
        writer.bool(true);
        writer.u16(0x3456);
        writer.u32(0x789A_BCDE);
        writer.u64(0x0123_4567_89AB_CDEF);
        writer.i32(-5);
        writer.f32(0.25);
        writer.bytes(&[1, 2, 3]);
        let bytes = writer.finish();

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u8(), Ok(0x12));
        assert_eq!(reader.bool(), Ok(true));
        assert_eq!(reader.u16(), Ok(0x3456));
        assert_eq!(reader.u32(), Ok(0x789A_BCDE));
        assert_eq!(reader.u64(), Ok(0x0123_4567_89AB_CDEF));
        assert_eq!(reader.i32(), Ok(-5));
        assert_eq!(reader.f32(), Ok(0.25));
        let mut block = [0; 3];
        assert_eq!(reader.bytes(&mut block), Ok(()));
        assert_eq!(block, [1, 2, 3]);
        assert_eq!(reader.finish(), Ok(()));
    }

    #[test]
    fn running_out_of_data_is_an_error() {
        let mut reader = Reader::new(&[1]);
        assert_eq!(reader.u16(), Err(StateError::Corrupt));
        let mut reader = Reader::new(&[1, 2]);
        assert_eq!(reader.bytes(&mut [0; 3]), Err(StateError::Corrupt));
    }

    #[test]
    fn leftovers_and_bad_flags_are_errors() {
        assert_eq!(Reader::new(&[0]).finish(), Err(StateError::Corrupt));
        assert_eq!(Reader::new(&[2]).bool(), Err(StateError::Corrupt));
    }
}
