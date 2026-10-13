//! The timer: DIV, TIMA, TMA and TAC.
//!
//! Everything hangs off one 16-bit counter that goes up on every clock tick.
//! DIV is simply its upper byte. TIMA is not driven by a clock of its own: it
//! watches one bit of that counter (chosen by TAC) and goes up each time the
//! bit falls from 1 to 0. That one fact explains the timer's quirks: anything
//! that makes the watched bit fall, such as resetting DIV or switching the
//! timer off, also counts as a tick.
//!
//! Reference: <https://gbdev.io/pandocs/Timer_Obscure_Behaviour.html>

use crate::state::{Reader, StateError, Writer};

/// Divider: the upper byte of the internal counter. Writing resets it.
pub const DIV: u16 = 0xFF04;
/// Timer counter.
pub const TIMA: u16 = 0xFF05;
/// Timer modulo: the value TIMA restarts from after overflowing.
pub const TMA: u16 = 0xFF06;
/// Timer control: bit 2 enables TIMA, bits 0-1 choose its speed.
pub const TAC: u16 = 0xFF07;

/// The internal counter when the boot ROM finishes (DIV reads `0xAB`).
/// Pan Docs only gives DIV; the low byte was found by running Mooneye's
/// `boot_div-dmgABCmgb`, which passes with this value alone.
const POST_BOOT_COUNTER: u16 = 0xABC8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    /// Counts clock ticks (T-cycles), so it advances by 4 per M-cycle.
    counter: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    /// TIMA overflowed in the previous M-cycle and reads 0; the reload from
    /// TMA and the interrupt happen one M-cycle late.
    overflowed: bool,
    /// TIMA was reloaded from TMA during this M-cycle.
    reloaded: bool,
}

impl Timer {
    /// A timer in the post-boot state.
    pub fn new() -> Self {
        Self {
            counter: POST_BOOT_COUNTER,
            tima: 0x00,
            tma: 0x00,
            tac: 0x00,
            overflowed: false,
            reloaded: false,
        }
    }

    /// Advances one M-cycle. Returns `true` when the timer interrupt fires.
    pub fn tick(&mut self) -> bool {
        self.reloaded = false;
        let interrupt = self.overflowed;
        if self.overflowed {
            self.overflowed = false;
            self.tima = self.tma;
            self.reloaded = true;
        }

        let before = self.watched_bit();
        self.counter = self.counter.wrapping_add(4);
        self.count_falling_edge(before);
        interrupt
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            DIV => (self.counter >> 8) as u8,
            TIMA => self.tima,
            TMA => self.tma,
            // The upper five bits do not exist and read as 1.
            _ => self.tac | 0xF8,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        match addr {
            DIV => {
                let before = self.watched_bit();
                self.counter = 0;
                self.count_falling_edge(before);
            }
            TIMA => {
                // In the M-cycle of the reload the new value loses to TMA.
                if !self.reloaded {
                    self.tima = value;
                    // Writing while TIMA sits at 0 after an overflow cancels
                    // both the reload and the interrupt.
                    self.overflowed = false;
                }
            }
            TMA => {
                self.tma = value;
                // A reload happening right now picks up the new value.
                if self.reloaded {
                    self.tima = value;
                }
            }
            _ => {
                let before = self.watched_bit();
                self.tac = value & 0x07;
                self.count_falling_edge(before);
            }
        }
    }

    /// The signal TIMA counts falling edges of: the selected counter bit,
    /// gated by the enable bit.
    fn watched_bit(&self) -> bool {
        let bit = match self.tac & 0x03 {
            0 => 9, // 4096 Hz
            1 => 3, // 262144 Hz
            2 => 5, // 65536 Hz
            _ => 7, // 16384 Hz
        };
        self.tac & 0x04 != 0 && self.counter & (1 << bit) != 0
    }

    fn count_falling_edge(&mut self, before: bool) {
        if before && !self.watched_bit() {
            let (tima, overflow) = self.tima.overflowing_add(1);
            self.tima = tima;
            self.overflowed = overflow;
        }
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    pub(crate) fn save_state(&self, w: &mut Writer) {
        w.u16(self.counter);
        w.u8(self.tima);
        w.u8(self.tma);
        w.u8(self.tac);
        w.bool(self.overflowed);
        w.bool(self.reloaded);
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.counter = r.u16()?;
        self.tima = r.u8()?;
        self.tma = r.u8()?;
        self.tac = r.u8()?;
        self.overflowed = r.bool()?;
        self.reloaded = r.bool()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A timer with the counter at zero, for predictable arithmetic.
    fn timer() -> Timer {
        let mut timer = Timer::new();
        timer.write(DIV, 0);
        timer
    }

    fn ticks(timer: &mut Timer, count: u32) -> u32 {
        (0..count).filter(|_| timer.tick()).count() as u32
    }

    #[test]
    fn starts_in_the_post_boot_state() {
        let timer = Timer::new();
        assert_eq!(timer.read(DIV), 0xAB);
        assert_eq!(timer.read(TIMA), 0x00);
        assert_eq!(timer.read(TAC), 0xF8);
    }

    #[test]
    fn div_goes_up_every_64_m_cycles_and_resets_on_write() {
        let mut timer = timer();
        ticks(&mut timer, 63);
        assert_eq!(timer.read(DIV), 0);
        ticks(&mut timer, 1);
        assert_eq!(timer.read(DIV), 1);
        timer.write(DIV, 0x99);
        assert_eq!(timer.read(DIV), 0);
    }

    #[test]
    fn tima_speed_follows_tac() {
        // (TAC value, M-cycles per TIMA increment)
        for (tac, period) in [(0x04, 256), (0x05, 4), (0x06, 16), (0x07, 64)] {
            let mut timer = timer();
            timer.write(TAC, tac);
            ticks(&mut timer, period - 1);
            assert_eq!(timer.read(TIMA), 0, "TAC={tac:#04x}");
            ticks(&mut timer, 1);
            assert_eq!(timer.read(TIMA), 1, "TAC={tac:#04x}");
            ticks(&mut timer, period * 9);
            assert_eq!(timer.read(TIMA), 10, "TAC={tac:#04x}");
        }
    }

    #[test]
    fn tima_stays_still_while_disabled() {
        let mut timer = timer();
        timer.write(TAC, 0x01);
        ticks(&mut timer, 1000);
        assert_eq!(timer.read(TIMA), 0);
    }

    #[test]
    fn overflow_reloads_from_tma_one_cycle_late_and_interrupts() {
        let mut timer = timer();
        timer.write(TMA, 0x80);
        timer.write(TIMA, 0xFF);
        timer.write(TAC, 0x05);

        assert_eq!(ticks(&mut timer, 4), 0);
        assert_eq!(timer.read(TIMA), 0x00, "reads zero during the delay");
        assert_eq!(ticks(&mut timer, 1), 1, "interrupt comes with the reload");
        assert_eq!(timer.read(TIMA), 0x80);
    }

    #[test]
    fn writing_tima_during_the_delay_cancels_the_reload() {
        let mut timer = timer();
        timer.write(TMA, 0x80);
        timer.write(TIMA, 0xFF);
        timer.write(TAC, 0x05);
        ticks(&mut timer, 4);

        timer.write(TIMA, 0x12);
        assert_eq!(ticks(&mut timer, 1), 0);
        assert_eq!(timer.read(TIMA), 0x12);
    }

    #[test]
    fn writes_during_the_reload_cycle_lose_to_tma() {
        let mut timer = timer();
        timer.write(TMA, 0x80);
        timer.write(TIMA, 0xFF);
        timer.write(TAC, 0x05);
        ticks(&mut timer, 5);

        timer.write(TIMA, 0x12);
        assert_eq!(timer.read(TIMA), 0x80);
        timer.write(TMA, 0x40);
        assert_eq!(timer.read(TIMA), 0x40, "the reload sees the new TMA");
    }

    #[test]
    fn resetting_div_can_tick_tima() {
        let mut timer = timer();
        timer.write(TAC, 0x05);
        // Two M-cycles in, the watched bit (bit 3) is high.
        ticks(&mut timer, 2);
        timer.write(DIV, 0);
        assert_eq!(timer.read(TIMA), 1);
    }

    #[test]
    fn disabling_the_timer_can_tick_tima() {
        let mut timer = timer();
        timer.write(TAC, 0x05);
        ticks(&mut timer, 2);
        timer.write(TAC, 0x01);
        assert_eq!(timer.read(TIMA), 1);
    }
}
