//! The APU (audio processing unit): four sound channels and a mixer.
//!
//! | Channel | Makes                      | Extras                         |
//! | ------- | -------------------------- | ------------------------------ |
//! | 1       | Square wave, 4 duty cycles | Volume envelope, pitch sweep   |
//! | 2       | Square wave, 4 duty cycles | Volume envelope                |
//! | 3       | 32 samples from wave RAM   | 4 volume levels                |
//! | 4       | Pseudo-random noise        | Volume envelope                |
//!
//! Each channel has a timer that decides its pitch and a length counter that
//! can switch it off after a while. The slow effects (length, envelope,
//! sweep) are driven by the *frame sequencer*, a 512 Hz clock that steps
//! through eight slots.
//!
//! Channels produce a number from 0 to 15, a DAC turns it into a voltage,
//! and the mixer pans and scales the four voltages into left and right.
//! This module then averages that signal down to the frontend's sample rate.
//!
//! Reference: <https://gbdev.io/pandocs/Audio.html> and
//! <https://gbdev.io/pandocs/Audio_details.html>

use crate::state::{Reader, StateError, Writer};

/// First and last sound register.
pub const NR10: u16 = 0xFF10;
pub const NR52: u16 = 0xFF26;
/// The 16 bytes of wave RAM.
pub const WAVE_RAM_START: u16 = 0xFF30;
pub const WAVE_RAM_END: u16 = 0xFF3F;

const NR11: u16 = 0xFF11;
const NR12: u16 = 0xFF12;
const NR13: u16 = 0xFF13;
const NR14: u16 = 0xFF14;
const NR21: u16 = 0xFF16;
const NR22: u16 = 0xFF17;
const NR23: u16 = 0xFF18;
const NR24: u16 = 0xFF19;
const NR30: u16 = 0xFF1A;
const NR31: u16 = 0xFF1B;
const NR32: u16 = 0xFF1C;
const NR33: u16 = 0xFF1D;
const NR34: u16 = 0xFF1E;
const NR41: u16 = 0xFF20;
const NR42: u16 = 0xFF21;
const NR43: u16 = 0xFF22;
const NR44: u16 = 0xFF23;
const NR50: u16 = 0xFF24;
const NR51: u16 = 0xFF25;

/// Bits that read as 1 in each register from NR10 to NR52: write-only or
/// nonexistent ones.
const READ_MASKS: [u8; 23] = [
    0x80, 0x3F, 0x00, 0xFF, 0xBF, // NR10-NR14
    0xFF, 0x3F, 0x00, 0xFF, 0xBF, // (unused), NR21-NR24
    0x7F, 0xFF, 0x9F, 0xFF, 0xBF, // NR30-NR34
    0xFF, 0xFF, 0x00, 0x00, 0xBF, // (unused), NR41-NR44
    0x00, 0x00, 0x70, // NR50-NR52
];

/// M-cycles per second: the rate at which [`Apu::tick`] is called.
const CYCLES_PER_SECOND: u32 = 1_048_576;
/// The frame sequencer steps at 512 Hz.
const CYCLES_PER_SEQUENCER_STEP: u16 = 2048;

/// The four square wave shapes: 12.5%, 25%, 50% and 75% high.
const DUTY_PATTERNS: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 0, 0, 1],
    [1, 0, 0, 0, 0, 1, 1, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
];

const NOISE_DIVISORS: [i32; 8] = [8, 16, 32, 48, 64, 80, 96, 112];
/// No channel timer ever holds more than the longest noise period.
const MAX_TIMER: i32 = 112 << 13;

/// Switches a channel off when it has played for long enough.
#[derive(Debug, Clone, Copy, Default)]
struct Length {
    enabled: bool,
    counter: u16,
}

impl Length {
    /// Counts down one step. Returns `true` when the channel must stop.
    fn clock(&mut self) -> bool {
        if self.enabled && self.counter > 0 {
            self.counter -= 1;
            return self.counter == 0;
        }
        false
    }

    /// Handles a write to NRx4. Returns `(triggered, expired)`.
    ///
    /// `half_done` means the frame sequencer's next step will not clock
    /// lengths. Enabling the counter at such a moment clocks it once right
    /// away, a hardware quirk that games do not rely on but tests check.
    fn write_control(&mut self, value: u8, max: u16, half_done: bool) -> (bool, bool) {
        let was_enabled = self.enabled;
        self.enabled = value & 0x40 != 0;
        let triggered = value & 0x80 != 0;

        let mut expired = false;
        if !was_enabled && self.enabled && half_done && self.counter > 0 {
            self.counter -= 1;
            expired = self.counter == 0 && !triggered;
        }
        if triggered && self.counter == 0 {
            self.counter = max;
            if self.enabled && half_done {
                self.counter -= 1;
            }
        }
        (triggered, expired)
    }
}

/// Fades a channel's volume up or down over time.
#[derive(Debug, Clone, Copy, Default)]
struct Envelope {
    /// NRx2: initial volume (bits 7-4), direction (bit 3), pace (bits 2-0).
    register: u8,
    volume: u8,
    timer: u8,
}

impl Envelope {
    /// The DAC is powered by the top five bits; with all of them clear the
    /// channel cannot play.
    fn dac_on(&self) -> bool {
        self.register & 0xF8 != 0
    }

    fn trigger(&mut self) {
        self.volume = self.register >> 4;
        self.timer = self.register & 0x07;
    }

    fn clock(&mut self) {
        let pace = self.register & 0x07;
        if pace == 0 {
            return;
        }
        self.timer = self.timer.saturating_sub(1);
        if self.timer == 0 {
            self.timer = pace;
            if self.register & 0x08 != 0 {
                self.volume = (self.volume + 1).min(15);
            } else {
                self.volume = self.volume.saturating_sub(1);
            }
        }
    }
}

/// Channels 1 and 2. Only channel 1 has the sweep unit wired up.
#[derive(Debug, Clone, Copy, Default)]
struct Pulse {
    active: bool,
    duty: u8,
    duty_step: u8,
    /// 11-bit value; the tone's frequency is 131072 / (2048 - frequency) Hz.
    frequency: u16,
    /// Clock ticks until the next duty step.
    timer: i32,
    length: Length,
    envelope: Envelope,
    /// NR10: pace (bits 6-4), direction (bit 3), shift (bits 2-0).
    sweep: u8,
    sweep_timer: u8,
    sweep_enabled: bool,
    /// The sweep's private copy of the frequency.
    sweep_shadow: u16,
    /// A downward sweep calculation happened since the last trigger.
    sweep_went_down: bool,
}

impl Pulse {
    fn tick(&mut self) {
        self.timer -= 4;
        while self.timer <= 0 {
            self.timer += (2048 - self.frequency as i32) * 4;
            self.duty_step = (self.duty_step + 1) & 7;
        }
    }

    fn output(&self) -> u8 {
        if self.active && DUTY_PATTERNS[self.duty as usize][self.duty_step as usize] == 1 {
            self.envelope.volume
        } else {
            0
        }
    }

    fn trigger(&mut self) {
        self.active = self.envelope.dac_on();
        self.timer = (2048 - self.frequency as i32) * 4;
        self.envelope.trigger();

        self.sweep_shadow = self.frequency;
        self.sweep_went_down = false;
        let pace = self.sweep >> 4 & 0x07;
        let shift = self.sweep & 0x07;
        self.sweep_timer = if pace == 0 { 8 } else { pace };
        self.sweep_enabled = pace != 0 || shift != 0;
        // With a shift set, the overflow check runs right away.
        if shift != 0 && self.sweep_target() > 2047 {
            self.active = false;
        }
    }

    /// The frequency the sweep would move to next.
    fn sweep_target(&mut self) -> u16 {
        let delta = self.sweep_shadow >> (self.sweep & 0x07);
        if self.sweep & 0x08 != 0 {
            self.sweep_went_down = true;
            self.sweep_shadow - delta
        } else {
            self.sweep_shadow + delta
        }
    }

    fn clock_sweep(&mut self) {
        self.sweep_timer = self.sweep_timer.saturating_sub(1);
        if self.sweep_timer != 0 {
            return;
        }
        let pace = self.sweep >> 4 & 0x07;
        self.sweep_timer = if pace == 0 { 8 } else { pace };
        if !self.sweep_enabled || pace == 0 {
            return;
        }
        let target = self.sweep_target();
        if target > 2047 {
            // Sweeping past the highest pitch silences the channel.
            self.active = false;
        } else if self.sweep & 0x07 != 0 {
            self.frequency = target;
            self.sweep_shadow = target;
            if self.sweep_target() > 2047 {
                self.active = false;
            }
        }
    }

    fn write_sweep(&mut self, value: u8) {
        // Switching from down to up after a downward step kills the channel.
        if self.sweep & 0x08 != 0 && value & 0x08 == 0 && self.sweep_went_down {
            self.active = false;
        }
        self.sweep = value;
    }
}

/// Channel 3: plays back 32 four-bit samples from wave RAM.
#[derive(Debug, Clone, Copy, Default)]
struct Wave {
    active: bool,
    dac_on: bool,
    /// NR32 bits 6-5: 0 = mute, 1 = 100%, 2 = 50%, 3 = 25%.
    volume: u8,
    frequency: u16,
    timer: i32,
    position: u8,
    length: Length,
    ram: [u8; 16],
}

impl Wave {
    fn tick(&mut self) {
        self.timer -= 4;
        while self.timer <= 0 {
            self.timer += (2048 - self.frequency as i32) * 2;
            self.position = (self.position + 1) & 31;
        }
    }

    fn output(&self) -> u8 {
        if !self.active {
            return 0;
        }
        let byte = self.ram[self.position as usize / 2];
        // The high nibble plays first.
        let sample = if self.position & 1 == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        };
        match self.volume {
            0 => 0,
            1 => sample,
            2 => sample >> 1,
            _ => sample >> 2,
        }
    }

    fn trigger(&mut self) {
        self.active = self.dac_on;
        self.timer = (2048 - self.frequency as i32) * 2;
        self.position = 0;
    }
}

/// Channel 4: noise from a 15-bit linear-feedback shift register.
#[derive(Debug, Clone, Copy, Default)]
struct Noise {
    active: bool,
    /// NR43: clock shift (bits 7-4), short mode (bit 3), divisor (bits 2-0).
    control: u8,
    timer: i32,
    lfsr: u16,
    length: Length,
    envelope: Envelope,
}

impl Noise {
    fn period(&self) -> i32 {
        NOISE_DIVISORS[(self.control & 0x07) as usize] << (self.control >> 4)
    }

    fn tick(&mut self) {
        // The two highest shift values give the register no clock at all.
        if self.control >> 4 >= 14 {
            return;
        }
        self.timer -= 4;
        while self.timer <= 0 {
            self.timer += self.period();
            let bit = (self.lfsr ^ self.lfsr >> 1) & 1;
            self.lfsr = self.lfsr >> 1 | bit << 14;
            if self.control & 0x08 != 0 {
                // Short mode: a 7-bit register, which sounds metallic.
                self.lfsr = self.lfsr & !0x40 | bit << 6;
            }
        }
    }

    fn output(&self) -> u8 {
        if self.active && self.lfsr & 1 == 0 {
            self.envelope.volume
        } else {
            0
        }
    }

    fn trigger(&mut self) {
        self.active = self.envelope.dac_on();
        self.timer = self.period();
        self.lfsr = 0x7FFF;
        self.envelope.trigger();
    }
}

pub struct Apu {
    /// NR52 bit 7: the master switch.
    powered: bool,
    /// The values last written to NR10..=NR52, for reading back.
    registers: [u8; 23],
    pulse1: Pulse,
    pulse2: Pulse,
    wave: Wave,
    noise: Noise,
    /// M-cycles until the frame sequencer's next step.
    sequencer_timer: u16,
    /// The step the frame sequencer will run next (0-7).
    sequencer_step: u8,

    sample_rate: u32,
    /// Counts up by `sample_rate` per M-cycle; a sample is due every time
    /// it passes [`CYCLES_PER_SECOND`].
    sample_phase: u32,
    /// Sum of the mixer output since the last sample, per side.
    sum: [f32; 2],
    summed_cycles: u32,
    /// State of the output capacitors, which remove the DC offset.
    capacitor: [f32; 2],
    /// Interleaved stereo samples (left, right) not yet taken.
    samples: Vec<f32>,
}

impl Apu {
    /// An APU in the post-boot state, producing 48 kHz audio.
    pub fn new() -> Self {
        let mut apu = Self {
            powered: true,
            registers: [0; 23],
            pulse1: Pulse::default(),
            pulse2: Pulse::default(),
            wave: Wave::default(),
            noise: Noise::default(),
            sequencer_timer: CYCLES_PER_SEQUENCER_STEP,
            sequencer_step: 0,
            sample_rate: 48_000,
            sample_phase: 0,
            sum: [0.0; 2],
            summed_cycles: 0,
            capacitor: [0.0; 2],
            samples: Vec::new(),
        };
        // Register values the boot ROM leaves behind, in address order.
        // Reference: <https://gbdev.io/pandocs/Power_Up_Sequence.html>
        const POST_BOOT: [(u16, u8); 19] = [
            (NR10, 0x80),
            (NR11, 0xBF),
            (NR12, 0xF3),
            (NR13, 0xFF),
            (NR14, 0x3F),
            (NR21, 0x3F),
            (NR22, 0x00),
            (NR23, 0xFF),
            (NR24, 0x3F),
            (NR30, 0x7F),
            (NR31, 0xFF),
            (NR32, 0x9F),
            (NR33, 0xFF),
            (NR34, 0x3F),
            (NR41, 0xFF),
            (NR42, 0x00),
            (NR43, 0x00),
            (NR50, 0x77),
            (NR51, 0xF3),
        ];
        for (addr, value) in POST_BOOT {
            apu.write(addr, value);
        }
        // NR52 reads 0xF1: channel 1 is still flagged as playing the boot
        // chime, which has faded to silence by now.
        apu.pulse1.active = true;
        apu
    }

    /// Sets how many samples per second [`Apu::take_samples`] produces.
    pub fn set_sample_rate(&mut self, hz: u32) {
        self.sample_rate = hz.clamp(8_000, 192_000);
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the samples produced since the last call: interleaved stereo
    /// (left, right) in the range -1.0 to 1.0.
    pub fn take_samples(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.samples)
    }

    /// Advances one M-cycle.
    pub fn tick(&mut self) {
        if self.powered {
            self.pulse1.tick();
            self.pulse2.tick();
            self.wave.tick();
            self.noise.tick();

            self.sequencer_timer -= 1;
            if self.sequencer_timer == 0 {
                self.sequencer_timer = CYCLES_PER_SEQUENCER_STEP;
                self.step_sequencer();
            }
        }

        let [left, right] = self.mix();
        self.sum[0] += left;
        self.sum[1] += right;
        self.summed_cycles += 1;

        self.sample_phase += self.sample_rate;
        if self.sample_phase >= CYCLES_PER_SECOND {
            self.sample_phase -= CYCLES_PER_SECOND;
            self.emit_sample();
        }
    }

    fn step_sequencer(&mut self) {
        // Step:     0  1  2  3  4  5  6  7
        // Length:   x     x     x     x
        // Sweep:          x           x
        // Envelope:                      x
        if self.sequencer_step % 2 == 0 {
            self.pulse1.active &= !self.pulse1.length.clock();
            self.pulse2.active &= !self.pulse2.length.clock();
            self.wave.active &= !self.wave.length.clock();
            self.noise.active &= !self.noise.length.clock();
        }
        if self.sequencer_step == 2 || self.sequencer_step == 6 {
            self.pulse1.clock_sweep();
        }
        if self.sequencer_step == 7 {
            self.pulse1.envelope.clock();
            self.pulse2.envelope.clock();
            self.noise.envelope.clock();
        }
        self.sequencer_step = (self.sequencer_step + 1) & 7;
    }

    /// The analog output of the mixer for the current instant.
    fn mix(&self) -> [f32; 2] {
        // Each DAC maps 0-15 to a voltage between -1 and 1; a DAC that is
        // switched off outputs nothing.
        let dac = |on: bool, value: u8| if on { value as f32 / 7.5 - 1.0 } else { 0.0 };
        let channels = [
            dac(self.pulse1.envelope.dac_on(), self.pulse1.output()),
            dac(self.pulse2.envelope.dac_on(), self.pulse2.output()),
            dac(self.wave.dac_on, self.wave.output()),
            dac(self.noise.envelope.dac_on(), self.noise.output()),
        ];
        let panning = self.registers[(NR51 - NR10) as usize];
        let volumes = self.registers[(NR50 - NR10) as usize];

        let side = |pan_shift: u8, volume: u8| {
            let sum: f32 = channels
                .iter()
                .enumerate()
                .filter(|(channel, _)| panning >> (pan_shift + *channel as u8) & 1 != 0)
                .map(|(_, voltage)| voltage)
                .sum();
            // Master volume 0-7 means 1/8 to 8/8; divide by 4 channels.
            sum * (volume as f32 + 1.0) / 32.0
        };
        [side(4, volumes >> 4 & 0x07), side(0, volumes & 0x07)]
    }

    /// Turns the mixer output accumulated since the last sample into one
    /// stereo sample.
    fn emit_sample(&mut self) {
        // The capacitor loses this fraction of its charge per clock tick.
        let decay = 0.999_958_f32.powf(4.0 * CYCLES_PER_SECOND as f32 / self.sample_rate as f32);
        for side in 0..2 {
            // Averaging is a simple low-pass filter: it keeps tones above
            // the sample rate from folding back as noise.
            let input = self.sum[side] / self.summed_cycles as f32;
            // High-pass filter: removes the constant offset, like the
            // capacitor between the real mixer and the headphones.
            let output = input - self.capacitor[side];
            self.capacitor[side] = input - output * decay;
            self.samples.push(output);
        }
        // Nobody is listening (a headless run, a paused frontend): keep at
        // most the latest second instead of growing forever.
        if self.samples.len() > 2 * self.sample_rate as usize {
            self.samples.clear();
        }
        self.sum = [0.0; 2];
        self.summed_cycles = 0;
    }

    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            NR52 => {
                let status = self.pulse1.active as u8
                    | (self.pulse2.active as u8) << 1
                    | (self.wave.active as u8) << 2
                    | (self.noise.active as u8) << 3;
                (self.powered as u8) << 7 | 0x70 | status
            }
            NR10..=NR51 => {
                let index = (addr - NR10) as usize;
                self.registers[index] | READ_MASKS[index]
            }
            WAVE_RAM_START..=WAVE_RAM_END => self.wave.ram[(addr - WAVE_RAM_START) as usize],
            _ => 0xFF,
        }
    }

    pub fn write(&mut self, addr: u16, value: u8) {
        if let WAVE_RAM_START..=WAVE_RAM_END = addr {
            self.wave.ram[(addr - WAVE_RAM_START) as usize] = value;
            return;
        }
        if addr == NR52 {
            self.set_power(value & 0x80 != 0);
            return;
        }
        if !(NR10..NR52).contains(&addr) {
            return;
        }
        let mut value = value;
        if !self.powered {
            // With the power off only the length counters can be written
            // (on DMG), not the duty bits that share their registers.
            match addr {
                NR11 | NR21 => value &= 0x3F,
                NR31 | NR41 => {}
                _ => return,
            }
        }
        self.registers[(addr - NR10) as usize] = value;

        // The next sequencer step is odd, so it will not clock lengths.
        let half_done = self.sequencer_step % 2 == 1;
        match addr {
            NR10 => self.pulse1.write_sweep(value),
            NR11 | NR21 => {
                let pulse = self.pulse(addr);
                pulse.duty = value >> 6;
                pulse.length.counter = 64 - (value & 0x3F) as u16;
            }
            NR12 | NR22 => {
                let pulse = self.pulse(addr);
                pulse.envelope.register = value;
                pulse.active &= pulse.envelope.dac_on();
            }
            NR13 | NR23 => {
                let pulse = self.pulse(addr);
                pulse.frequency = pulse.frequency & 0x700 | value as u16;
            }
            NR14 | NR24 => {
                let pulse = self.pulse(addr);
                pulse.frequency = pulse.frequency & 0x0FF | (value as u16 & 0x07) << 8;
                let (triggered, expired) = pulse.length.write_control(value, 64, half_done);
                pulse.active &= !expired;
                if triggered {
                    pulse.trigger();
                }
            }
            NR30 => {
                self.wave.dac_on = value & 0x80 != 0;
                self.wave.active &= self.wave.dac_on;
            }
            NR31 => self.wave.length.counter = 256 - value as u16,
            NR32 => self.wave.volume = value >> 5 & 0x03,
            NR33 => self.wave.frequency = self.wave.frequency & 0x700 | value as u16,
            NR34 => {
                self.wave.frequency = self.wave.frequency & 0x0FF | (value as u16 & 0x07) << 8;
                let (triggered, expired) = self.wave.length.write_control(value, 256, half_done);
                self.wave.active &= !expired;
                if triggered {
                    self.wave.trigger();
                }
            }
            NR41 => self.noise.length.counter = 64 - (value & 0x3F) as u16,
            NR42 => {
                self.noise.envelope.register = value;
                self.noise.active &= self.noise.envelope.dac_on();
            }
            NR43 => self.noise.control = value,
            NR44 => {
                let (triggered, expired) = self.noise.length.write_control(value, 64, half_done);
                self.noise.active &= !expired;
                if triggered {
                    self.noise.trigger();
                }
            }
            // NR50 and NR51 are read straight from `registers` by the mixer.
            _ => {}
        }
    }

    /// Channel 1 or 2, from the address of one of its registers.
    fn pulse(&mut self, addr: u16) -> &mut Pulse {
        if addr <= NR14 {
            &mut self.pulse1
        } else {
            &mut self.pulse2
        }
    }

    fn set_power(&mut self, on: bool) {
        if self.powered && !on {
            // Power off clears every register and silences every channel.
            // Length counters and wave RAM survive (on DMG).
            self.registers = [0; 23];
            let lengths = [
                self.pulse1.length.counter,
                self.pulse2.length.counter,
                self.wave.length.counter,
                self.noise.length.counter,
            ];
            let ram = self.wave.ram;
            self.pulse1 = Pulse::default();
            self.pulse2 = Pulse::default();
            self.wave = Wave::default();
            self.noise = Noise::default();
            self.pulse1.length.counter = lengths[0];
            self.pulse2.length.counter = lengths[1];
            self.wave.length.counter = lengths[2];
            self.noise.length.counter = lengths[3];
            self.wave.ram = ram;
        } else if !self.powered && on {
            self.sequencer_step = 0;
            self.sequencer_timer = CYCLES_PER_SEQUENCER_STEP;
        }
        self.powered = on;
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Length {
    fn save_state(&self, w: &mut Writer) {
        w.bool(self.enabled);
        w.u16(self.counter);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.enabled = r.bool()?;
        self.counter = r.u16()?;
        Ok(())
    }
}

impl Envelope {
    fn save_state(&self, w: &mut Writer) {
        w.u8(self.register);
        w.u8(self.volume);
        w.u8(self.timer);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.register = r.u8()?;
        self.volume = r.u8()? & 0x0F;
        self.timer = r.u8()?;
        Ok(())
    }
}

impl Pulse {
    fn save_state(&self, w: &mut Writer) {
        w.bool(self.active);
        w.u8(self.duty);
        w.u8(self.duty_step);
        w.u16(self.frequency);
        w.i32(self.timer);
        self.length.save_state(w);
        self.envelope.save_state(w);
        w.u8(self.sweep);
        w.u8(self.sweep_timer);
        w.bool(self.sweep_enabled);
        w.u16(self.sweep_shadow);
        w.bool(self.sweep_went_down);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.active = r.bool()?;
        // Masked to the widths the hardware has, so that damaged data
        // cannot index outside the duty table or underflow the timer.
        self.duty = r.u8()? & 0x03;
        self.duty_step = r.u8()? & 0x07;
        self.frequency = r.u16()? & 0x07FF;
        self.timer = r.i32()?.clamp(0, MAX_TIMER);
        self.length.load_state(r)?;
        self.envelope.load_state(r)?;
        self.sweep = r.u8()?;
        self.sweep_timer = r.u8()?;
        self.sweep_enabled = r.bool()?;
        self.sweep_shadow = r.u16()? & 0x07FF;
        self.sweep_went_down = r.bool()?;
        Ok(())
    }
}

impl Wave {
    fn save_state(&self, w: &mut Writer) {
        w.bool(self.active);
        w.bool(self.dac_on);
        w.u8(self.volume);
        w.u16(self.frequency);
        w.i32(self.timer);
        w.u8(self.position);
        self.length.save_state(w);
        w.bytes(&self.ram);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.active = r.bool()?;
        self.dac_on = r.bool()?;
        self.volume = r.u8()? & 0x03;
        self.frequency = r.u16()? & 0x07FF;
        self.timer = r.i32()?.clamp(0, MAX_TIMER);
        self.position = r.u8()? & 31;
        self.length.load_state(r)?;
        r.bytes(&mut self.ram)
    }
}

impl Noise {
    fn save_state(&self, w: &mut Writer) {
        w.bool(self.active);
        w.u8(self.control);
        w.i32(self.timer);
        w.u16(self.lfsr);
        self.length.save_state(w);
        self.envelope.save_state(w);
    }

    fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.active = r.bool()?;
        self.control = r.u8()?;
        self.timer = r.i32()?.clamp(0, MAX_TIMER);
        self.lfsr = r.u16()? & 0x7FFF;
        self.length.load_state(r)?;
        self.envelope.load_state(r)
    }
}

impl Apu {
    /// The sample rate and any samples not yet taken belong to the
    /// frontend and are left out. The half-built sample is kept, so that
    /// audio continues without a seam.
    pub(crate) fn save_state(&self, w: &mut Writer) {
        w.bool(self.powered);
        w.bytes(&self.registers);
        self.pulse1.save_state(w);
        self.pulse2.save_state(w);
        self.wave.save_state(w);
        self.noise.save_state(w);
        w.u16(self.sequencer_timer);
        w.u8(self.sequencer_step);
        w.f32(self.capacitor[0]);
        w.f32(self.capacitor[1]);
        w.u32(self.sample_phase);
        w.f32(self.sum[0]);
        w.f32(self.sum[1]);
        w.u32(self.summed_cycles);
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        self.powered = r.bool()?;
        r.bytes(&mut self.registers)?;
        self.pulse1.load_state(r)?;
        self.pulse2.load_state(r)?;
        self.wave.load_state(r)?;
        self.noise.load_state(r)?;
        self.sequencer_timer = r.u16()?;
        self.sequencer_step = r.u8()? & 7;
        self.capacitor = [r.f32()?, r.f32()?];
        if self.sequencer_timer == 0 || self.sequencer_timer > CYCLES_PER_SEQUENCER_STEP {
            return Err(StateError::Corrupt);
        }
        self.sample_phase = r.u32()?;
        self.sum = [r.f32()?, r.f32()?];
        self.summed_cycles = r.u32()?;
        let finite = self
            .capacitor
            .iter()
            .chain(&self.sum)
            .all(|value| value.is_finite());
        if !finite || self.sample_phase >= CYCLES_PER_SECOND {
            return Err(StateError::Corrupt);
        }
        self.samples.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A powered APU with every register cleared.
    fn apu() -> Apu {
        let mut apu = Apu::new();
        apu.write(NR52, 0x00);
        apu.write(NR52, 0x80);
        apu.take_samples();
        apu
    }

    fn run(apu: &mut Apu, cycles: u32) {
        for _ in 0..cycles {
            apu.tick();
        }
    }

    /// Runs for `cycles` and counts rising edges on a channel's output.
    fn count_rises(apu: &mut Apu, cycles: u32, output: impl Fn(&Apu) -> u8) -> u32 {
        let mut rises = 0;
        let mut previous = output(apu);
        for _ in 0..cycles {
            apu.tick();
            let now = output(apu);
            if previous == 0 && now != 0 {
                rises += 1;
            }
            previous = now;
        }
        rises
    }

    #[test]
    fn registers_read_back_with_their_unused_bits_set() {
        let apu = Apu::new();
        assert_eq!(apu.read(NR10), 0x80);
        assert_eq!(apu.read(NR11), 0xBF);
        assert_eq!(apu.read(NR12), 0xF3);
        assert_eq!(apu.read(NR13), 0xFF, "write-only");
        assert_eq!(apu.read(NR14), 0xBF);
        assert_eq!(apu.read(NR50), 0x77);
        assert_eq!(apu.read(NR51), 0xF3);
        assert_eq!(apu.read(NR52), 0xF1);
        assert_eq!(apu.read(0xFF15), 0xFF, "unused");
        assert_eq!(apu.read(0xFF27), 0xFF, "unused");
    }

    #[test]
    fn pulse_plays_at_the_programmed_frequency() {
        let mut apu = apu();
        apu.write(NR12, 0xF0);
        // 2048 - 1798 = 250, so 131072 / 250 = 524.288 Hz (about C5).
        apu.write(NR13, (1798 & 0xFF) as u8);
        apu.write(NR14, 0x80 | (1798 >> 8) as u8);
        assert_eq!(apu.read(NR52) & 0x01, 0x01);
        let rises = count_rises(&mut apu, CYCLES_PER_SECOND, |apu| apu.pulse1.output());
        assert!((523..=525).contains(&rises), "{rises} Hz");
    }

    #[test]
    fn duty_sets_the_fraction_of_time_spent_high() {
        for (duty, expected_high) in [(0u8, 1), (1, 2), (2, 4), (3, 6)] {
            let mut apu = apu();
            apu.write(NR21, duty << 6);
            apu.write(NR22, 0xF0);
            apu.write(NR24, 0x80);
            // Frequency 0: each duty step lasts 2048 M-cycles.
            let mut high = 0;
            for _ in 0..8 {
                run(&mut apu, 2048);
                high += (apu.pulse2.output() != 0) as u32;
            }
            assert_eq!(high, expected_high, "duty {duty}");
        }
    }

    #[test]
    fn length_counter_stops_the_channel() {
        let mut apu = apu();
        apu.write(NR21, 0x3E); // length 64 - 62 = 2 steps of 1/256 s
        apu.write(NR22, 0xF0);
        apu.write(NR24, 0xC0); // trigger with length enabled
        assert_eq!(apu.read(NR52) & 0x02, 0x02);
        run(&mut apu, 2 * 4096 - 2048 - 1);
        assert_eq!(apu.read(NR52) & 0x02, 0x02, "still playing");
        run(&mut apu, 1);
        assert_eq!(apu.read(NR52) & 0x02, 0x00, "stopped by its length");
    }

    #[test]
    fn length_is_ignored_unless_enabled() {
        let mut apu = apu();
        apu.write(NR21, 0x3F);
        apu.write(NR22, 0xF0);
        apu.write(NR24, 0x80);
        run(&mut apu, CYCLES_PER_SECOND);
        assert_eq!(apu.read(NR52) & 0x02, 0x02);
    }

    #[test]
    fn envelope_fades_the_volume_out() {
        let mut apu = apu();
        apu.write(NR22, 0xF1); // start at 15, down, one step per 1/64 s
        apu.write(NR24, 0x80);
        assert_eq!(apu.pulse2.envelope.volume, 15);
        run(&mut apu, 8 * 2048);
        assert_eq!(apu.pulse2.envelope.volume, 14);
        run(&mut apu, 20 * 8 * 2048);
        assert_eq!(apu.pulse2.envelope.volume, 0, "stops at silence");
    }

    #[test]
    fn envelope_can_fade_in() {
        let mut apu = apu();
        apu.write(NR22, 0x09); // start at 0, up
        apu.write(NR24, 0x80);
        run(&mut apu, 3 * 8 * 2048);
        assert_eq!(apu.pulse2.envelope.volume, 3);
    }

    #[test]
    fn a_channel_with_its_dac_off_cannot_play() {
        let mut apu = apu();
        apu.write(NR22, 0x00);
        apu.write(NR24, 0x80);
        assert_eq!(apu.read(NR52) & 0x02, 0x00);

        apu.write(NR22, 0xF0);
        apu.write(NR24, 0x80);
        assert_eq!(apu.read(NR52) & 0x02, 0x02);
        apu.write(NR22, 0x00);
        assert_eq!(
            apu.read(NR52) & 0x02,
            0x00,
            "switching the DAC off stops it"
        );
    }

    #[test]
    fn sweep_raises_the_pitch_until_it_overflows() {
        let mut apu = apu();
        apu.write(NR10, 0x11); // every 1/128 s, upward, shift 1
        apu.write(NR12, 0xF0);
        apu.write(NR13, 0x00);
        apu.write(NR14, 0x84); // frequency 0x400
        run(&mut apu, 3 * 2048);
        assert_eq!(apu.pulse1.frequency, 0x600, "0x400 + 0x400 / 2");
        assert_eq!(apu.read(NR52) & 0x01, 0x00, "the next step would overflow");
    }

    #[test]
    fn sweep_can_lower_the_pitch() {
        let mut apu = apu();
        apu.write(NR10, 0x1A); // every 1/128 s, downward, shift 2
        apu.write(NR12, 0xF0);
        apu.write(NR13, 0x00);
        apu.write(NR14, 0x84);
        run(&mut apu, 3 * 2048);
        assert_eq!(apu.pulse1.frequency, 0x300);
        assert_eq!(apu.read(NR52) & 0x01, 0x01);
    }

    #[test]
    fn wave_channel_plays_wave_ram_high_nibble_first() {
        let mut apu = apu();
        apu.write(WAVE_RAM_START, 0x1F);
        apu.write(WAVE_RAM_START + 1, 0xA5);
        apu.write(NR30, 0x80);
        apu.write(NR32, 0x20); // full volume
        apu.write(NR33, 0x00);
        apu.write(NR34, 0x80); // frequency 0: 1024 M-cycles per sample
        assert_eq!(apu.wave.output(), 0x1);
        run(&mut apu, 1024);
        assert_eq!(apu.wave.output(), 0xF);
        run(&mut apu, 1024);
        assert_eq!(apu.wave.output(), 0xA);
        apu.write(NR32, 0x40); // half volume
        assert_eq!(apu.wave.output(), 0x5);
        apu.write(NR32, 0x00); // mute
        assert_eq!(apu.wave.output(), 0x0);
    }

    #[test]
    fn noise_is_long_in_normal_mode_and_short_in_short_mode() {
        // Collect the output bit over time and measure when it repeats.
        fn period(control: u8) -> usize {
            let mut apu = apu();
            apu.write(NR42, 0xF0);
            apu.write(NR43, control);
            apu.write(NR44, 0x80);
            let bits: Vec<u8> = (0..400)
                .map(|_| {
                    run(&mut apu, 2);
                    apu.noise.output().min(1)
                })
                .collect();
            (1..200)
                .find(|&p| (0..200).all(|i| bits[i] == bits[i + p]))
                .unwrap_or(usize::MAX)
        }
        assert_eq!(period(0x08), 127, "7-bit register");
        assert_eq!(period(0x00), usize::MAX, "15-bit register: no short cycle");
    }

    #[test]
    fn power_off_clears_registers_and_blocks_writes() {
        let mut apu = Apu::new();
        apu.write(NR52, 0x00);
        assert_eq!(apu.read(NR52), 0x70);
        assert_eq!(apu.read(NR50), 0x00);
        assert_eq!(apu.read(NR12), 0x00);

        apu.write(NR50, 0x77);
        apu.write(NR12, 0xF0);
        assert_eq!(apu.read(NR50), 0x00, "ignored while off");
        assert_eq!(apu.read(NR12), 0x00);

        apu.write(WAVE_RAM_START, 0x42);
        assert_eq!(
            apu.read(WAVE_RAM_START),
            0x42,
            "wave RAM is always writable"
        );

        apu.write(NR52, 0x80);
        apu.write(NR50, 0x77);
        assert_eq!(apu.read(NR50), 0x77);
    }

    #[test]
    fn produces_the_requested_number_of_samples() {
        let mut apu = apu();
        apu.set_sample_rate(44_100);
        run(&mut apu, CYCLES_PER_SECOND);
        // Interleaved stereo: two values per sample.
        assert_eq!(apu.take_samples().len(), 2 * 44_100);
        assert!(apu.take_samples().is_empty());
    }

    #[test]
    fn unclaimed_samples_do_not_pile_up() {
        let mut apu = apu();
        apu.set_sample_rate(8_000);
        run(&mut apu, 10 * CYCLES_PER_SECOND);
        assert!(apu.take_samples().len() <= 2 * 8_000);
    }

    #[test]
    fn silence_is_silent_and_a_tone_is_not() {
        let mut apu = apu();
        run(&mut apu, 20_000);
        assert!(apu.take_samples().iter().all(|&s| s == 0.0));

        apu.write(NR50, 0x77);
        apu.write(NR51, 0xFF);
        apu.write(NR12, 0xF0);
        apu.write(NR13, 0x00);
        apu.write(NR14, 0x87);
        run(&mut apu, 20_000);
        let samples = apu.take_samples();
        let loudest = samples.iter().fold(0.0f32, |max, &s| max.max(s.abs()));
        assert!(loudest > 0.05, "peak {loudest}");
        assert!(samples.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn panning_routes_a_channel_to_one_side() {
        let mut apu = apu();
        apu.write(NR50, 0x77);
        apu.write(NR51, 0x10); // channel 1 on the left only
        apu.write(NR12, 0xF0);
        apu.write(NR14, 0x87);
        run(&mut apu, 20_000);
        let samples = apu.take_samples();
        let left = samples.iter().step_by(2).any(|&s| s != 0.0);
        let right = samples.iter().skip(1).step_by(2).any(|&s| s != 0.0);
        assert!(left && !right);
    }

    #[test]
    fn dc_offset_fades_away() {
        let mut apu = apu();
        apu.write(NR50, 0x77);
        apu.write(NR51, 0xFF);
        // DAC on but channel never triggered: a constant voltage.
        apu.write(NR12, 0xF0);
        run(&mut apu, CYCLES_PER_SECOND);
        let samples = apu.take_samples();
        assert!(samples[0].abs() > 0.1, "starts with an offset");
        assert!(samples[samples.len() - 1].abs() < 0.001, "which decays");
    }
}
