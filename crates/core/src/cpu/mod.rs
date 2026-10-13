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

pub use registers::{Registers, FLAG_C, FLAG_H, FLAG_N, FLAG_Z};

/// What the CPU is plugged into. Each method call lasts one M-cycle.
pub trait CpuBus {
    /// An M-cycle that reads `addr`.
    fn read(&mut self, addr: u16) -> u8;
    /// An M-cycle that writes `value` to `addr`.
    fn write(&mut self, addr: u16, value: u8);
    /// An M-cycle in which the CPU works internally and touches no memory.
    fn idle(&mut self);
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

    /// Executes one instruction (or sleeps for one M-cycle).
    pub fn step<B: CpuBus>(&mut self, bus: &mut B) {
        if self.halted || self.locked {
            bus.idle();
            return;
        }

        let enable_interrupts = self.ime_pending;
        let opcode = self.fetch8(bus);
        self.execute(opcode, bus);
        // EI is delayed by one instruction; a DI in between cancels it.
        if enable_interrupts && self.ime_pending {
            self.ime = true;
            self.ime_pending = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 64 KiB of RAM that counts M-cycles.
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
