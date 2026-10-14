//! The SM83 CPU.
//!
//! The CPU has no clock of its own. Every memory access it makes goes through
//! a [`CpuBus`], and each call is exactly one M-cycle (4 clock ticks): the bus
//! advances the rest of the hardware while serving it. An instruction's
//! duration is therefore just the number of bus calls it makes.
//!
//! References: <https://gbdev.io/pandocs/CPU_Instruction_Set.html> and
//! <https://gbdev.io/gb-opcodes/optables/>.

mod execute;
mod registers;

use crate::interrupts;
use crate::state::{Reader, StateError, Writer};

pub use registers::{Registers, FLAG_C, FLAG_H, FLAG_N, FLAG_Z};

/// What the CPU is plugged into. Each method call lasts one M-cycle.
pub trait CpuBus {
    /// An M-cycle that reads `addr`.
    fn read(&mut self, addr: u16) -> u8;
    /// An M-cycle that writes `value` to `addr`.
    fn write(&mut self, addr: u16, value: u8);
    /// An M-cycle in which the CPU works internally and touches no memory.
    fn idle(&mut self);

    /// Interrupts that are both requested (IF) and enabled (IE), one bit
    /// each. A bus with no interrupt hardware reports none.
    fn pending_interrupts(&self) -> u8 {
        0
    }

    /// The CPU is serving the interrupt with this bit: clear its request.
    fn acknowledge_interrupt(&mut self, _bit: u8) {}

    /// The CPU executed STOP. On a Game Boy Color this can switch speed.
    fn stop(&mut self) {}
}

/// The CPU state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cpu {
    pub regs: Registers,
    /// Interrupt master enable.
    pub ime: bool,
    /// EI was executed: IME turns on after the *next* instruction.
    pub ime_pending: bool,
    /// HALT was executed: the CPU sleeps until an interrupt is pending.
    pub halted: bool,
    /// HALT was executed with interrupts disabled while one was already
    /// pending: the CPU does not sleep, and it fails to advance PC when
    /// fetching the next opcode, so that byte gets read twice.
    pub halt_bug: bool,
    /// An illegal opcode was executed: the CPU is stuck until power off.
    pub locked: bool,
}

impl Cpu {
    /// A CPU in the state the boot ROM leaves it in.
    pub fn post_boot(header_checksum: u8) -> Self {
        Self {
            regs: Registers::post_boot(header_checksum),
            ..Self::default()
        }
    }

    /// A CPU in the state the Game Boy Color boot ROM leaves it in when it
    /// starts a color game. A = 0x11 is how games know they run on a Color.
    ///
    /// Reference: <https://gbdev.io/pandocs/Power_Up_Sequence.html#cpu-registers>
    pub fn post_boot_cgb() -> Self {
        let mut regs = Registers {
            a: 0x11,
            sp: 0xFFFE,
            pc: 0x0100,
            ..Registers::default()
        };
        regs.set_flags(true, false, false, false);
        regs.set_de(0xFF56);
        regs.set_hl(0x000D);
        Self {
            regs,
            ..Self::default()
        }
    }

    /// The registers a Game Boy Color leaves behind when it starts a game
    /// made for the DMG, in compatibility mode. Values from Mooneye's
    /// `boot_regs-cgb`; with some licensed games the boot ROM leaves other
    /// values in B and HL, which games do not rely on.
    pub fn post_boot_compat() -> Self {
        let mut regs = Registers {
            a: 0x11,
            sp: 0xFFFE,
            pc: 0x0100,
            ..Registers::default()
        };
        regs.set_flags(true, false, false, false);
        regs.set_de(0x0008);
        regs.set_hl(0x007C);
        Self {
            regs,
            ..Self::default()
        }
    }

    /// Executes one instruction, serves one interrupt, or sleeps for one
    /// M-cycle.
    pub fn step<B: CpuBus>(&mut self, bus: &mut B) {
        if self.locked {
            bus.idle();
            return;
        }
        if self.halted {
            // A pending interrupt wakes the CPU even when IME is off; it is
            // only *served* when IME is on.
            if bus.pending_interrupts() == 0 {
                bus.idle();
                return;
            }
            self.halted = false;
            if self.ime {
                bus.idle();
                self.serve_interrupt(bus);
                return;
            }
        }

        let enable_interrupts = self.ime_pending;
        let opcode = bus.read(self.regs.pc);
        // Interrupts are sampled while the opcode is being fetched. When one
        // is taken, the fetched opcode is thrown away and PC stays put, so
        // the fetch becomes the first of the dispatch's five M-cycles.
        if self.ime && bus.pending_interrupts() != 0 {
            self.serve_interrupt(bus);
            return;
        }
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.regs.pc = self.regs.pc.wrapping_add(1);
        }
        self.execute(opcode, bus);
        // EI is delayed by one instruction; a DI in between cancels it.
        if enable_interrupts && self.ime_pending {
            self.ime = true;
            self.ime_pending = false;
        }
    }

    /// The last four of the five M-cycles of an interrupt dispatch (the
    /// caller spent the first): push PC and jump to the handler.
    fn serve_interrupt<B: CpuBus>(&mut self, bus: &mut B) {
        self.ime = false;
        bus.idle();
        let [lo, hi] = self.regs.pc.to_le_bytes();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, hi);
        // The source is chosen only now. If the push above landed on IE and
        // disabled everything, the dispatch is cancelled and PC ends up at 0.
        let pending = bus.pending_interrupts();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, lo);
        if pending == 0 {
            self.regs.pc = 0x0000;
        } else {
            // The lowest set bit has the highest priority.
            let bit = pending & pending.wrapping_neg();
            bus.acknowledge_interrupt(bit);
            self.regs.pc = interrupts::vector(bit);
        }
        bus.idle();
    }

    /// HALT. Its three outcomes depend on IME and on pending interrupts.
    fn halt<B: CpuBus>(&mut self, bus: &mut B) {
        // An EI just before HALT counts as interrupts enabled.
        if self.ime || self.ime_pending || bus.pending_interrupts() == 0 {
            self.halted = true;
        } else {
            self.halt_bug = true;
        }
    }

    fn fetch8<B: CpuBus>(&mut self, bus: &mut B) -> u8 {
        let value = bus.read(self.regs.pc);
        self.regs.pc = self.regs.pc.wrapping_add(1);
        value
    }

    fn fetch16<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch8(bus);
        let hi = self.fetch8(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// Pushes a word: high byte first, at descending addresses.
    fn push16<B: CpuBus>(&mut self, bus: &mut B, value: u16) {
        let [lo, hi] = value.to_le_bytes();
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, hi);
        self.regs.sp = self.regs.sp.wrapping_sub(1);
        bus.write(self.regs.sp, lo);
    }

    fn pop16<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let lo = bus.read(self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        let hi = bus.read(self.regs.sp);
        self.regs.sp = self.regs.sp.wrapping_add(1);
        u16::from_le_bytes([lo, hi])
    }
}

impl Cpu {
    pub(crate) fn save_state(&self, w: &mut Writer) {
        let regs = &self.regs;
        for value in [
            regs.a, regs.f, regs.b, regs.c, regs.d, regs.e, regs.h, regs.l,
        ] {
            w.u8(value);
        }
        w.u16(regs.sp);
        w.u16(regs.pc);
        for flag in [
            self.ime,
            self.ime_pending,
            self.halted,
            self.halt_bug,
            self.locked,
        ] {
            w.bool(flag);
        }
    }

    pub(crate) fn load_state(&mut self, r: &mut Reader) -> Result<(), StateError> {
        let regs = &mut self.regs;
        for register in [
            &mut regs.a,
            &mut regs.f,
            &mut regs.b,
            &mut regs.c,
            &mut regs.d,
            &mut regs.e,
            &mut regs.h,
            &mut regs.l,
        ] {
            *register = r.u8()?;
        }
        regs.sp = r.u16()?;
        regs.pc = r.u16()?;
        self.ime = r.bool()?;
        self.ime_pending = r.bool()?;
        self.halted = r.bool()?;
        self.halt_bug = r.bool()?;
        self.locked = r.bool()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 64 KiB of RAM that counts M-cycles, with IF and IE at their usual
    /// addresses.
    struct FlatBus {
        memory: Vec<u8>,
        cycles: u32,
    }

    impl CpuBus for FlatBus {
        fn read(&mut self, addr: u16) -> u8 {
            self.cycles += 1;
            self.memory[addr as usize]
        }

        fn write(&mut self, addr: u16, value: u8) {
            self.cycles += 1;
            self.memory[addr as usize] = value;
        }

        fn idle(&mut self) {
            self.cycles += 1;
        }

        fn pending_interrupts(&self) -> u8 {
            self.memory[0xFF0F] & self.memory[0xFFFF] & 0x1F
        }

        fn acknowledge_interrupt(&mut self, bit: u8) {
            self.memory[0xFF0F] &= !bit;
        }
    }

    /// Like `run`, with `request` in IF and everything enabled in IE.
    fn run_with_interrupt(program: &[u8], request: u8, ime: bool, steps: usize) -> (Cpu, FlatBus) {
        let mut bus = FlatBus {
            memory: vec![0; 0x1_0000],
            cycles: 0,
        };
        bus.memory[0x100..0x100 + program.len()].copy_from_slice(program);
        bus.memory[0xFF0F] = request;
        bus.memory[0xFFFF] = 0x1F;
        let mut cpu = Cpu {
            ime,
            ..Cpu::default()
        };
        cpu.regs.pc = 0x0100;
        cpu.regs.sp = 0xD000;
        for _ in 0..steps {
            cpu.step(&mut bus);
        }
        (cpu, bus)
    }

    #[test]
    fn serving_an_interrupt_pushes_pc_and_jumps_to_the_vector() {
        let (cpu, bus) = run_with_interrupt(&[0x00], 0x04, true, 1);
        assert_eq!(cpu.regs.pc, 0x0050);
        assert_eq!(cpu.regs.sp, 0xCFFE);
        assert_eq!(bus.memory[0xCFFF], 0x01);
        assert_eq!(bus.memory[0xCFFE], 0x00);
        assert!(
            !cpu.ime,
            "nested interrupts are off until the handler says so"
        );
        assert_eq!(bus.memory[0xFF0F], 0x00, "the request is cleared");
        assert_eq!(bus.cycles, 5);
    }

    #[test]
    fn lowest_bit_wins_when_several_interrupts_are_pending() {
        let (cpu, bus) = run_with_interrupt(&[0x00], 0x14, true, 1);
        assert_eq!(cpu.regs.pc, 0x0050);
        assert_eq!(bus.memory[0xFF0F], 0x10, "the other one keeps waiting");
    }

    #[test]
    fn nothing_is_served_while_ime_is_off() {
        let (cpu, _) = run_with_interrupt(&[0x00, 0x00], 0x04, false, 2);
        assert_eq!(cpu.regs.pc, 0x0102);
    }

    #[test]
    fn halt_sleeps_until_an_interrupt_is_pending() {
        // HALT ; INC A
        let (mut cpu, mut bus) = run_with_interrupt(&[0x76, 0x3C], 0x00, false, 10);
        assert!(cpu.halted);
        assert_eq!(cpu.regs.a, 0);

        // With IME off the CPU just wakes up and carries on.
        bus.memory[0xFF0F] = 0x01;
        cpu.step(&mut bus);
        assert!(!cpu.halted);
        assert_eq!(cpu.regs.a, 1);
        assert_eq!(cpu.regs.pc, 0x0102);
    }

    #[test]
    fn halt_with_ime_on_wakes_into_the_handler() {
        let (mut cpu, mut bus) = run_with_interrupt(&[0x76, 0x3C], 0x00, true, 3);
        bus.memory[0xFF0F] = 0x01;
        cpu.step(&mut bus);
        assert_eq!(cpu.regs.pc, 0x0040);
        assert_eq!(bus.memory[0xCFFE], 0x01, "returns to just after HALT");
    }

    #[test]
    fn halt_bug_reads_the_next_byte_twice() {
        // HALT ; INC A  with IME off and an interrupt already pending:
        // INC A runs twice because PC does not advance the first time.
        let (cpu, _) = run_with_interrupt(&[0x76, 0x3C, 0x00], 0x01, false, 3);
        assert!(!cpu.halted);
        assert_eq!(cpu.regs.a, 2);
        assert_eq!(cpu.regs.pc, 0x0102);
    }

    /// Runs `program` from address 0 for `steps` instructions.
    fn run(program: &[u8], steps: usize) -> (Cpu, FlatBus) {
        let mut bus = FlatBus {
            memory: vec![0; 0x1_0000],
            cycles: 0,
        };
        bus.memory[..program.len()].copy_from_slice(program);
        let mut cpu = Cpu::default();
        cpu.regs.sp = 0xFFFE;
        for _ in 0..steps {
            cpu.step(&mut bus);
        }
        (cpu, bus)
    }

    #[test]
    fn add_sets_half_carry_and_carry() {
        // LD A,0x3A ; ADD A,0xC6
        let (cpu, bus) = run(&[0x3E, 0x3A, 0xC6, 0xC6], 2);
        assert_eq!(cpu.regs.a, 0x00);
        assert_eq!(cpu.regs.f, FLAG_Z | FLAG_H | FLAG_C);
        assert_eq!(bus.cycles, 4);
    }

    #[test]
    fn daa_corrects_bcd_addition() {
        // LD A,0x45 ; ADD A,0x38 ; DAA  => 45 + 38 = 83 in BCD
        let (cpu, _) = run(&[0x3E, 0x45, 0xC6, 0x38, 0x27], 3);
        assert_eq!(cpu.regs.a, 0x83);
        assert!(!cpu.regs.flag(FLAG_C));
    }

    #[test]
    fn call_and_ret_round_trip_through_the_stack() {
        // 0000: CALL 0x0010 ; 0010: RET
        let mut program = vec![0xCD, 0x10, 0x00];
        program.resize(0x10, 0x00);
        program.push(0xC9);

        let (cpu, bus) = run(&program, 1);
        assert_eq!(cpu.regs.pc, 0x0010);
        assert_eq!(cpu.regs.sp, 0xFFFC);
        assert_eq!(bus.memory[0xFFFD], 0x00);
        assert_eq!(bus.memory[0xFFFC], 0x03);
        assert_eq!(bus.cycles, 6);

        let (cpu, bus) = run(&program, 2);
        assert_eq!(cpu.regs.pc, 0x0003);
        assert_eq!(cpu.regs.sp, 0xFFFE);
        assert_eq!(bus.cycles, 6 + 4);
    }

    #[test]
    fn conditional_jump_costs_more_when_taken() {
        // XOR A (sets Z) ; JR Z,+2
        let (cpu, bus) = run(&[0xAF, 0x28, 0x02], 2);
        assert_eq!(cpu.regs.pc, 0x0005);
        assert_eq!(bus.cycles, 1 + 3);

        // XOR A ; JR NZ,+2
        let (cpu, bus) = run(&[0xAF, 0x20, 0x02], 2);
        assert_eq!(cpu.regs.pc, 0x0003);
        assert_eq!(bus.cycles, 1 + 2);
    }

    #[test]
    fn ei_enables_interrupts_after_the_following_instruction() {
        // EI ; NOP ; NOP
        let (cpu, _) = run(&[0xFB, 0x00, 0x00], 1);
        assert!(!cpu.ime);
        let (cpu, _) = run(&[0xFB, 0x00, 0x00], 2);
        assert!(cpu.ime);
    }

    #[test]
    fn di_right_after_ei_cancels_it() {
        // EI ; DI ; NOP
        let (cpu, _) = run(&[0xFB, 0xF3, 0x00], 3);
        assert!(!cpu.ime);
    }

    #[test]
    fn pop_af_clears_the_low_nibble_of_f() {
        // LD BC,0x12FF ; PUSH BC ; POP AF
        let (cpu, _) = run(&[0x01, 0xFF, 0x12, 0xC5, 0xF1], 3);
        assert_eq!(cpu.regs.af(), 0x12F0);
    }

    #[test]
    fn illegal_opcode_locks_the_cpu() {
        let (cpu, _) = run(&[0xD3, 0x3C], 5);
        assert!(cpu.locked);
        assert_eq!(cpu.regs.pc, 0x0001);
        assert_eq!(cpu.regs.a, 0x00);
    }
}
