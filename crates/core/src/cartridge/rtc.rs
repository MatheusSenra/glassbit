//! The real-time clock of MBC3 cartridges.
//!
//! A 32768 Hz crystal on the cartridge drives a counter of seconds, minutes,
//! hours and days, powered by the save battery. The game reads it through
//! five registers, after *latching*: writing 0 then 1 to `0x6000..=0x7FFF`
//! copies the running counter into the registers it can read, so the
//! seconds cannot change between reading the minutes and reading the hours.
//!
//! | Register | Bits used | Meaning                                         |
//! | -------- | --------- | ----------------------------------------------- |
//! | S        | 0-5       | Seconds                                         |
//! | M        | 0-5       | Minutes                                         |
//! | H        | 0-4       | Hours                                           |
//! | DL       | 0-7       | Day counter, low 8 bits                         |
//! | DH       | 0, 6, 7   | Day bit 8, halt (6), day counter overflow (7)   |
//!
//! Here the clock advances with emulated time. A frontend that wants it to
//! follow the real clock while the game is closed stores the time with the
//! save (see [`Rtc::footer`]) and passes the current time when loading.
//!
//! References: <https://gbdev.io/pandocs/MBC3.html> and the rtc3test ROM's
//! description of how out-of-range values count.

use crate::state::{Reader, StateError, Writer};

/// M-cycles per second of real time (the clock is not affected by the
/// Game Boy Color's double speed).
pub const CYCLES_PER_SECOND: u64 = 1_048_576;

/// Writable bits of S, M, H, DL and DH.
const MASKS: [u8; 5] = [0x3F, 0x3F, 0x1F, 0xFF, 0xC1];
const SECONDS: usize = 0;
const DAY_LOW: usize = 3;
const DAY_HIGH: usize = 4;
const HALT: u8 = 0x40;
const DAY_CARRY: u8 = 0x80;

/// Length of the save file footer other emulators (BGB, VBA-M) append for
/// the clock: five current registers and five latched ones as 32-bit
/// numbers, then a 64-bit UNIX timestamp, all little endian.
pub const FOOTER_SIZE: usize = 48;
/// An older variant of the footer with a 32-bit timestamp.
const SHORT_FOOTER_SIZE: usize = 44;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rtc {
    /// The running counter: S, M, H, DL, DH.
    current: [u8; 5],
    /// What the game reads, copied from `current` when it latches.
    latched: [u8; 5],
    /// M-cycles counted towards the next second.
    sub_second: u64,
    /// A 0 was written to the latch register; a 1 now latches.
    latch_armed: bool,
}

impl Rtc {
    pub fn new() -> Self {
        Self::default()
    }

    fn halted(&self) -> bool {
        self.current[DAY_HIGH] & HALT != 0
    }

    /// Lets `cycles` M-cycles of real time pass.
    pub fn advance(&mut self, cycles: u64) {
        if self.halted() {
            return;
        }
        let total = self.sub_second + cycles;
        self.sub_second = total % CYCLES_PER_SECOND;
        for _ in 0..total / CYCLES_PER_SECOND {
            self.tick();
        }
    }

    /// Lets whole seconds pass, e.g. the time the game spent closed.
    pub fn advance_seconds(&mut self, seconds: u64) {
        if self.halted() {
            return;
        }
        // Whole days at once, then the rest second by second, so even years
        // away cost little.
        let days = seconds / 86_400;
        for _ in 0..days.min(1024) {
            self.add_day();
        }
        for _ in 0..seconds % 86_400 {
            self.tick();
        }
    }

    /// One second goes by. Each counter wraps at its bit width without a
    /// carry, but carries into the next one when it reaches 60, 60 or 24:
    /// so a value written out of range counts on until it wraps.
    fn tick(&mut self) {
        let [seconds, minutes, hours, ..] = &mut self.current;
        *seconds = (*seconds + 1) & 0x3F;
        if *seconds != 60 {
            return;
        }
        *seconds = 0;
        *minutes = (*minutes + 1) & 0x3F;
        if *minutes != 60 {
            return;
        }
        *minutes = 0;
        *hours = (*hours + 1) & 0x1F;
        if *hours != 24 {
            return;
        }
        *hours = 0;
        self.add_day();
    }

    fn add_day(&mut self) {
        let day = (self.current[DAY_HIGH] as u16 & 0x01) << 8 | self.current[DAY_LOW] as u16;
        let next = (day + 1) & 0x1FF;
        self.current[DAY_LOW] = next as u8;
        self.current[DAY_HIGH] = self.current[DAY_HIGH] & !0x01 | (next >> 8) as u8;
        if next == 0 {
            // The 9-bit day counter overflowed: the flag stays set until the
            // game clears it.
            self.current[DAY_HIGH] |= DAY_CARRY;
        }
    }

    /// Reads register `index` (0-4: S, M, H, DL, DH) as last latched.
    pub fn read(&self, index: usize) -> u8 {
        self.latched[index]
    }

    /// Writes register `index` of the running counter.
    pub fn write(&mut self, index: usize, value: u8) {
        let value = value & MASKS[index];
        if index == SECONDS {
            // Setting the seconds restarts the current second.
            self.sub_second = 0;
        }
        self.current[index] = value;
        // The written value can be read back without latching again.
        self.latched[index] = value;
    }

    /// Handles a write to `0x6000..=0x7FFF`: 0 then 1 latches.
    pub fn write_latch(&mut self, value: u8) {
        if value == 1 && self.latch_armed {
            self.latched = self.current;
        }
        self.latch_armed = value == 0;
    }

    /// The clock in the save file footer format, stamped with `now`
    /// (seconds since 1970).
    pub fn footer(&self, now: u64) -> [u8; FOOTER_SIZE] {
        let mut footer = [0u8; FOOTER_SIZE];
        for (i, &value) in self.current.iter().chain(&self.latched).enumerate() {
            footer[i * 4] = value;
        }
        footer[40..].copy_from_slice(&now.to_le_bytes());
        footer
    }

    /// Restores the clock from a save file footer and lets the time between
    /// its timestamp and `now` pass. Returns `false` if `footer` has neither
    /// footer length.
    pub fn load_footer(&mut self, footer: &[u8], now: u64) -> bool {
        let stamp = match footer.len() {
            FOOTER_SIZE => u64::from_le_bytes(footer[40..48].try_into().expect("8 bytes")),
            SHORT_FOOTER_SIZE => {
                u32::from_le_bytes(footer[40..44].try_into().expect("4 bytes")) as u64
            }
            _ => return false,
        };
        for i in 0..5 {
            self.current[i] = footer[i * 4] & MASKS[i];
            self.latched[i] = footer[20 + i * 4] & MASKS[i];
        }
        self.sub_second = 0;
        // A missing or future timestamp means no time is known to have passed.
        if stamp != 0 && now > stamp {
            self.advance_seconds(now - stamp);
        }
        true
    }

    pub(crate) fn save_state(&self, w: &mut Writer) {
        w.bytes(&self.current);
        w.bytes(&self.latched);
        w.u64(self.sub_second);
        w.bool(self.latch_armed);
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        r.bytes(&mut self.current)?;
        r.bytes(&mut self.latched)?;
        for (index, mask) in MASKS.iter().enumerate() {
            self.current[index] &= mask;
            self.latched[index] &= mask;
        }
        self.sub_second = r.u64()? % CYCLES_PER_SECOND;
        self.latch_armed = r.bool()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTES: usize = 1;
    const HOURS: usize = 2;

    fn rtc_at(seconds: u8, minutes: u8, hours: u8, day: u16) -> Rtc {
        let mut rtc = Rtc::new();
        rtc.write(SECONDS, seconds);
        rtc.write(MINUTES, minutes);
        rtc.write(HOURS, hours);
        rtc.write(DAY_LOW, day as u8);
        rtc.write(DAY_HIGH, (day >> 8) as u8);
        rtc
    }

    fn time(rtc: &mut Rtc) -> [u8; 5] {
        rtc.write_latch(0);
        rtc.write_latch(1);
        [0, 1, 2, 3, 4].map(|index| rtc.read(index))
    }

    #[test]
    fn counts_seconds_of_emulated_time() {
        let mut rtc = rtc_at(10, 0, 0, 0);
        rtc.advance(CYCLES_PER_SECOND - 1);
        assert_eq!(time(&mut rtc)[SECONDS], 10);
        rtc.advance(1);
        assert_eq!(time(&mut rtc)[SECONDS], 11);
    }

    #[test]
    fn rolls_over_into_minutes_hours_and_days() {
        let mut rtc = rtc_at(59, 59, 23, 255);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc), [0, 0, 0, 0, 1], "day 256");
    }

    #[test]
    fn day_overflow_sets_a_sticky_flag() {
        let mut rtc = rtc_at(59, 59, 23, 511);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc), [0, 0, 0, 0, DAY_CARRY]);
        rtc.write(SECONDS, 59);
        rtc.write(MINUTES, 59);
        rtc.write(HOURS, 23);
        rtc.write(DAY_LOW, 0xFF);
        rtc.write(DAY_HIGH, DAY_CARRY | 0x01);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc)[DAY_HIGH], DAY_CARRY, "still set");
    }

    #[test]
    fn out_of_range_values_count_on_and_wrap_without_carry() {
        // 28:63:60 ticks to 28:63:61.
        let mut rtc = rtc_at(60, 63, 28, 5);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc), [61, 63, 28, 5, 0]);

        // Seconds wrap from 63 to 0 without touching the minutes.
        let mut rtc = rtc_at(63, 10, 1, 0);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc)[..2], [0, 10]);

        // Hours wrap from 31 to 0 without adding a day.
        let mut rtc = rtc_at(59, 59, 31, 7);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc), [0, 0, 0, 7, 0]);
    }

    #[test]
    fn high_minutes_and_hours_still_increment() {
        let mut rtc = rtc_at(59, 61, 3, 0);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc)[..3], [0, 62, 3]);

        let mut rtc = rtc_at(59, 59, 26, 0);
        rtc.advance(CYCLES_PER_SECOND);
        assert_eq!(time(&mut rtc)[..4], [0, 0, 27, 0]);
    }

    #[test]
    fn only_valid_bits_are_kept() {
        let mut rtc = Rtc::new();
        for (index, mask) in MASKS.iter().enumerate() {
            rtc.write(index, 0xFF);
            assert_eq!(rtc.read(index), *mask);
        }
    }

    #[test]
    fn halting_stops_the_clock_and_keeps_the_sub_second() {
        let mut rtc = rtc_at(0, 0, 0, 0);
        rtc.advance(600_000);
        rtc.write(DAY_HIGH, HALT);
        rtc.advance(CYCLES_PER_SECOND * 5);
        assert_eq!(time(&mut rtc)[SECONDS], 0);
        rtc.write(DAY_HIGH, 0);
        rtc.advance(CYCLES_PER_SECOND - 600_000);
        assert_eq!(time(&mut rtc)[SECONDS], 1, "resumes where it stopped");
    }

    #[test]
    fn writing_seconds_restarts_the_second_but_other_writes_do_not() {
        let mut rtc = rtc_at(0, 0, 0, 0);
        rtc.advance(CYCLES_PER_SECOND / 2);
        rtc.write(MINUTES, 5);
        rtc.advance(CYCLES_PER_SECOND / 2);
        assert_eq!(time(&mut rtc)[SECONDS], 1);

        rtc.advance(CYCLES_PER_SECOND / 2);
        rtc.write(SECONDS, 30);
        rtc.advance(CYCLES_PER_SECOND / 2);
        assert_eq!(time(&mut rtc)[SECONDS], 30);
        rtc.advance(CYCLES_PER_SECOND / 2);
        assert_eq!(time(&mut rtc)[SECONDS], 31);
    }

    #[test]
    fn reads_show_the_latched_time() {
        let mut rtc = rtc_at(5, 0, 0, 0);
        rtc.write_latch(0);
        rtc.write_latch(1);
        rtc.advance(CYCLES_PER_SECOND * 3);
        assert_eq!(rtc.read(SECONDS), 5, "not latched again");
        rtc.write_latch(1);
        assert_eq!(rtc.read(SECONDS), 5, "a 1 alone does not latch");
        rtc.write_latch(0);
        rtc.write_latch(1);
        assert_eq!(rtc.read(SECONDS), 8);
    }

    #[test]
    fn footer_round_trip_adds_the_time_spent_away() {
        let rtc = rtc_at(50, 59, 23, 3);
        let footer = rtc.footer(1_000_000);
        let mut restored = Rtc::new();
        // Closed for one day and 15 seconds.
        assert!(restored.load_footer(&footer, 1_000_000 + 86_400 + 15));
        assert_eq!(time(&mut restored), [5, 0, 0, 5, 0]);

        let mut same_time = Rtc::new();
        assert!(same_time.load_footer(&footer, 1_000_000));
        assert_eq!(time(&mut same_time), [50, 59, 23, 3, 0]);
        assert!(!same_time.load_footer(&footer[..10], 0));
    }
}
