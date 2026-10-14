//! The serial port (link cable).
//!
//! Bytes sent are kept in a buffer: test ROMs print their results this way.
//! With nothing plugged in, incoming bits are all 1. A frontend can plug in
//! a link cable to another emulator (see [`LinkMessage`]).
//!
//! Reference: <https://gbdev.io/pandocs/Serial_Data_Transfer_(Link_Cable).html>

use crate::state::{Reader, StateError, Writer};

/// Serial transfer data.
pub const SB: u16 = 0xFF01;
/// Serial transfer control: bit 7 starts a transfer, bit 0 selects the
/// internal clock.
pub const SC: u16 = 0xFF02;

/// The internal clock shifts one bit every 128 M-cycles (8192 Hz).
const TRANSFER_CYCLES: u16 = 8 * 128;

/// A message for the Game Boy at the other end of the link cable.
///
/// On real hardware the two shift registers swap their contents bit by bit,
/// driven by the clock of whichever side started the transfer. Over a
/// network the swap is split in two messages: the side that drives the
/// clock sends its byte, and the other side answers with its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMessage {
    /// "I clocked a transfer; here is my byte."
    Data(u8),
    /// "Here is my byte in exchange for yours."
    Reply(u8),
}

#[derive(Debug, Clone, Default)]
pub struct Serial {
    sb: u8,
    sc: u8,
    /// M-cycles until the transfer in progress finishes; 0 when idle.
    remaining: u16,
    output: Vec<u8>,
    /// A link cable is plugged in: transfers wait for the other side
    /// instead of reading all ones.
    linked: bool,
    /// Messages for the other side, not yet sent.
    outbox: Vec<LinkMessage>,
    /// A byte the other side clocked out, waiting for this side to be ready
    /// to receive it with the external clock.
    incoming_data: Option<u8>,
    /// The other side's answer to this side's transfer.
    incoming_reply: Option<u8>,
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
                // partner to drive it.
                if self.internal_transfer() {
                    self.output.push(self.sb);
                    self.remaining = TRANSFER_CYCLES;
                    if self.linked {
                        self.outbox.push(LinkMessage::Data(self.sb));
                    }
                }
            }
        }
    }

    fn internal_transfer(&self) -> bool {
        self.sc == 0x81
    }

    fn external_transfer(&self) -> bool {
        self.sc == 0x80
    }

    /// Advances one M-cycle. Returns `true` when a transfer completes, which
    /// requests the serial interrupt.
    pub fn tick(&mut self) -> bool {
        if self.linked && self.external_transfer() {
            // The other side clocked a byte in: swap and answer.
            if let Some(byte) = self.incoming_data.take() {
                self.outbox.push(LinkMessage::Reply(self.sb));
                self.finish(byte);
                return true;
            }
        }
        if !self.internal_transfer() {
            return false;
        }
        if self.remaining > 0 {
            self.remaining -= 1;
            if self.remaining > 0 {
                return false;
            }
        }
        // All eight bits have been clocked out.
        if !self.linked {
            // Nothing is connected, so all ones came in.
            self.finish(0xFF);
            return true;
        }
        // Over a network the answer can take a while; the transfer simply
        // lasts until it arrives. If both sides clocked at once, each one's
        // data doubles as the answer to the other.
        match self
            .incoming_reply
            .take()
            .or_else(|| self.incoming_data.take())
        {
            Some(byte) => {
                self.finish(byte);
                true
            }
            None => false,
        }
    }

    fn finish(&mut self, received: u8) {
        self.sb = received;
        self.sc &= !0x80;
        self.remaining = 0;
    }

    /// Returns the bytes sent since the last call.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// Plugs in or unplugs the link cable. Unplugging drops anything in
    /// flight, so a transfer waiting for an answer ends with all ones.
    pub fn set_linked(&mut self, linked: bool) {
        self.linked = linked;
        if !linked {
            self.outbox.clear();
            self.incoming_data = None;
            self.incoming_reply = None;
        }
    }

    pub fn linked(&self) -> bool {
        self.linked
    }

    /// Returns the messages to send to the other side since the last call.
    pub fn take_link_messages(&mut self) -> Vec<LinkMessage> {
        std::mem::take(&mut self.outbox)
    }

    /// Delivers a message from the other side.
    pub fn receive_link_message(&mut self, message: LinkMessage) {
        if !self.linked {
            return;
        }
        match message {
            LinkMessage::Data(byte) => self.incoming_data = Some(byte),
            LinkMessage::Reply(byte) => self.incoming_reply = Some(byte),
        }
    }
}

impl Serial {
    /// Bytes already sent are output, not state, and are left out, and so
    /// is the link cable: a state restores the console, not the connection.
    pub(crate) fn save_state(&self, w: &mut Writer) {
        w.u8(self.sb);
        w.u8(self.sc);
        w.u16(self.remaining);
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.sb = r.u8()?;
        self.sc = r.u8()?;
        self.remaining = r.u16()?;
        Ok(())
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

    /// Two serial ports joined by a cable that delivers messages instantly.
    fn deliver(from: &mut Serial, to: &mut Serial) {
        for message in from.take_link_messages() {
            to.receive_link_message(message);
        }
    }

    fn linked_pair() -> (Serial, Serial) {
        let mut a = Serial::new();
        let mut b = Serial::new();
        a.set_linked(true);
        b.set_linked(true);
        (a, b)
    }

    #[test]
    fn linked_transfer_swaps_bytes_between_the_two_sides() {
        let (mut master, mut slave) = linked_pair();
        slave.write(SB, 0x99);
        slave.write(SC, 0x80); // ready, waiting for the other clock
        master.write(SB, 0x42);
        master.write(SC, 0x81); // drive the clock

        deliver(&mut master, &mut slave);
        assert!(slave.tick(), "slave completes as soon as the byte arrives");
        assert_eq!(slave.read(SB), 0x42);
        assert_eq!(slave.read(SC) & 0x80, 0);

        deliver(&mut slave, &mut master);
        let interrupts = (0..TRANSFER_CYCLES).filter(|_| master.tick()).count();
        assert_eq!(interrupts, 1);
        assert_eq!(master.read(SB), 0x99);
        assert_eq!(
            master.take_output(),
            vec![0x42],
            "still printed for the runner"
        );
    }

    #[test]
    fn linked_master_waits_for_the_answer() {
        let (mut master, mut slave) = linked_pair();
        master.write(SB, 0x42);
        master.write(SC, 0x81);
        // Far longer than a transfer takes: still waiting, not 0xFF.
        assert!((0..10 * TRANSFER_CYCLES).all(|_| !master.tick()));
        assert_eq!(master.read(SC) & 0x80, 0x80);

        // The other side only becomes ready now.
        deliver(&mut master, &mut slave);
        assert!(!slave.tick(), "not ready yet: the byte waits");
        slave.write(SB, 0x07);
        slave.write(SC, 0x80);
        assert!(slave.tick());
        deliver(&mut slave, &mut master);
        assert!(master.tick());
        assert_eq!(master.read(SB), 0x07);
    }

    #[test]
    fn linked_master_still_takes_at_least_a_transfer_time() {
        let (mut master, mut slave) = linked_pair();
        slave.write(SB, 0x01);
        slave.write(SC, 0x80);
        master.write(SB, 0x02);
        master.write(SC, 0x81);
        deliver(&mut master, &mut slave);
        slave.tick();
        deliver(&mut slave, &mut master);
        let completed_at = (1..=TRANSFER_CYCLES).find(|_| master.tick());
        assert_eq!(completed_at, Some(TRANSFER_CYCLES));
    }

    #[test]
    fn both_sides_driving_the_clock_swap_their_bytes() {
        let (mut a, mut b) = linked_pair();
        a.write(SB, 0xAA);
        a.write(SC, 0x81);
        b.write(SB, 0xBB);
        b.write(SC, 0x81);
        deliver(&mut a, &mut b);
        deliver(&mut b, &mut a);
        assert_eq!((0..TRANSFER_CYCLES).filter(|_| a.tick()).count(), 1);
        assert_eq!((0..TRANSFER_CYCLES).filter(|_| b.tick()).count(), 1);
        assert_eq!((a.read(SB), b.read(SB)), (0xBB, 0xAA));
        assert!(a.take_link_messages().is_empty() && b.take_link_messages().is_empty());
    }

    #[test]
    fn unplugging_ends_a_waiting_transfer_with_all_ones() {
        let (mut master, _) = linked_pair();
        master.write(SB, 0x42);
        master.write(SC, 0x81);
        assert!((0..2 * TRANSFER_CYCLES).all(|_| !master.tick()));
        master.set_linked(false);
        assert!(master.tick());
        assert_eq!(master.read(SB), 0xFF);
        assert!(master.take_link_messages().is_empty());
    }

    #[test]
    fn messages_are_ignored_without_a_cable() {
        let mut serial = Serial::new();
        serial.write(SC, 0x80);
        serial.receive_link_message(LinkMessage::Data(0x12));
        assert!(!serial.tick());
        assert_eq!(serial.read(SB), 0x00);
    }

    #[test]
    fn idle_port_reads_like_post_boot() {
        let serial = Serial::new();
        assert_eq!(serial.read(SB), 0x00);
        assert_eq!(serial.read(SC), 0x7E);
    }
}
