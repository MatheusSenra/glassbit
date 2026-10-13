//! The serial port (link cable).
//!
//! Nothing is ever plugged into the other end, so incoming bits are all 1.
//! Bytes sent are kept in a buffer: test ROMs print their results this way.
//!
//! Reference: <https://gbdev.io/pandocs/Serial_Data_Transfer_(Link_Cable).html>

/// Serial transfer data.
pub const SB: u16 = 0xFF01;
/// Serial transfer control: bit 7 starts a transfer, bit 0 selects the
/// internal clock.
pub const SC: u16 = 0xFF02;

/// The internal clock shifts one bit every 128 M-cycles (8192 Hz).
const TRANSFER_CYCLES: u16 = 8 * 128;

#[derive(Debug, Clone, Default)]
pub struct Serial {
    sb: u8,
    sc: u8,
    /// M-cycles until the transfer in progress finishes; 0 when idle.
    remaining: u16,
    output: Vec<u8>,
}

impl Serial {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            SB => self.sb,
            // Bits 1-6 do not exist on DMG and read as 1.
            _ => self.sc | 0x7E,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            SB => self.sb = value,
            _ => {
                self.sc = value & 0x81;
                // With the external clock selected the transfer waits for a
                // partner that never comes.
                if self.sc == 0x81 {
                    self.output.push(self.sb);
                    self.remaining = TRANSFER_CYCLES;
                }
            }
        }
    }

    /// Advances one M-cycle. Returns `true` when a transfer completes, which
    /// requests the serial interrupt.
    pub fn tick(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        if self.remaining > 0 {
            return false;
        }
        self.sb = 0xFF;
        self.sc &= !0x80;
        true
    }

    /// Returns the bytes sent since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_with_internal_clock_completes_and_captures_the_byte() {
        let mut serial = Serial::new();
        serial.write(SB, b'A');
        serial.write(SC, 0x81);
        assert_eq!(serial.read(SC), 0xFF, "transfer in progress");

        let interrupts = (0..TRANSFER_CYCLES).filter(|_| serial.tick()).count();
        assert_eq!(interrupts, 1);
        assert_eq!(serial.read(SB), 0xFF, "nothing connected: all ones in");
        assert_eq!(serial.read(SC), 0x7F, "start bit cleared");
        assert_eq!(serial.take_output(), b"A");
        assert!(serial.take_output().is_empty());
    }

    #[test]
    fn transfer_with_external_clock_never_completes() {
        let mut serial = Serial::new();
        serial.write(SB, b'B');
        serial.write(SC, 0x80);
        assert!((0..10_000).all(|_| !serial.tick()));
        assert_eq!(serial.read(SB), b'B');
        assert!(serial.take_output().is_empty());
    }

    #[test]
    fn idle_port_reads_like_post_boot() {
        let serial = Serial::new();
        assert_eq!(serial.read(SB), 0x00);
        assert_eq!(serial.read(SC), 0x7E);
    }
}
