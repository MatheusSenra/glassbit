//! Instruction decoding and execution.
//!
//! Opcodes are decoded by their bit fields rather than with 256 separate
//! cases. Written in octal, an opcode is three digits `x y z` (2 + 3 + 3
//! bits), and the opcode table is regular in them: for example every
//! `LD r,r'` is `1 dst src`, and every `ADD/ADC/SUB/...` on a register is
//! `2 operation src`.

use super::{Cpu, CpuBus, FLAG_C, FLAG_H, FLAG_N, FLAG_Z};

impl Cpu {
    pub(super) fn execute<B: CpuBus>(&mut self, opcode: u8, bus: &mut B) {
        let y = (opcode >> 3) & 7;
        let z = opcode & 7;
        match opcode >> 6 {
            0 => self.execute_block0(opcode, y, z, bus),
            // HALT sits where LD (HL),(HL) would be.
            1 if opcode == 0x76 => self.halted = true,
            1 => {
                let value = self.read_r8(z, bus);
                self.write_r8(y, value, bus);
            }
            2 => {
                let value = self.read_r8(z, bus);
                self.alu(y, value);
            }
            _ => self.execute_block3(opcode, y, z, bus),
        }
    }

    /// Opcodes `0x00..=0x3F`: loads, 16-bit arithmetic, INC/DEC, relative jumps.
    fn execute_block0<B: CpuBus>(&mut self, opcode: u8, y: u8, z: u8, bus: &mut B) {
        let pair = y >> 1;
        match (z, y) {
            // NOP
            (0, 0) => {}
            // LD (a16),SP
            (0, 1) => {
                let addr = self.fetch16(bus);
                let [lo, hi] = self.regs.sp.to_le_bytes();
                bus.write(addr, lo);
                bus.write(addr.wrapping_add(1), hi);
            }
            // STOP. Its real behavior depends on buttons and pending
            // interrupts and no test in scope relies on it; treated as a NOP.
            (0, 2) => {}
            // JR e
            (0, 3) => self.jump_relative(true, bus),
            // JR cc,e
            (0, _) => {
                let taken = self.condition(y - 4);
                self.jump_relative(taken, bus);
            }
            // LD r16,d16
            (1, _) if y & 1 == 0 => {
                let value = self.fetch16(bus);
                self.write_r16(pair, value);
            }
            // ADD HL,r16
            (1, _) => {
                let hl = self.regs.hl();
                let value = self.read_r16(pair);
                let (result, carry) = hl.overflowing_add(value);
                let half = (hl & 0x0FFF) + (value & 0x0FFF) > 0x0FFF;
                let zero = self.regs.flag(FLAG_Z);
                self.regs.set_flags(zero, false, half, carry);
                self.regs.set_hl(result);
                bus.idle();
            }
            // LD (BC),A / LD (DE),A / LD (HL+),A / LD (HL-),A
            (2, _) if y & 1 == 0 => {
                let addr = self.indirect_address(pair);
                bus.write(addr, self.regs.a);
            }
            // LD A,(BC) / LD A,(DE) / LD A,(HL+) / LD A,(HL-)
            (2, _) => {
                let addr = self.indirect_address(pair);
                self.regs.a = bus.read(addr);
            }
            // INC r16 / DEC r16 (no flags)
            (3, _) => {
                let value = self.read_r16(pair);
                let result = if y & 1 == 0 {
                    value.wrapping_add(1)
                } else {
                    value.wrapping_sub(1)
                };
                self.write_r16(pair, result);
                bus.idle();
            }
            // INC r8
            (4, _) => {
                let value = self.read_r8(y, bus);
                let result = value.wrapping_add(1);
                let carry = self.regs.flag(FLAG_C);
                self.regs
                    .set_flags(result == 0, false, value & 0x0F == 0x0F, carry);
                self.write_r8(y, result, bus);
            }
            // DEC r8
            (5, _) => {
                let value = self.read_r8(y, bus);
                let result = value.wrapping_sub(1);
                let carry = self.regs.flag(FLAG_C);
                self.regs
                    .set_flags(result == 0, true, value & 0x0F == 0x00, carry);
                self.write_r8(y, result, bus);
            }
            // LD r8,d8
            (6, _) => {
                let value = self.fetch8(bus);
                self.write_r8(y, value, bus);
            }
            // RLCA / RRCA / RLA / RRA: like the CB rotates, but Z is always clear.
            (7, 0..=3) => {
                self.regs.a = self.rotate_shift(y, self.regs.a);
                self.regs.f &= !FLAG_Z;
            }
            // DAA
            (7, 4) => self.daa(),
            // CPL
            (7, 5) => {
                self.regs.a = !self.regs.a;
                self.regs.f |= FLAG_N | FLAG_H;
            }
            // SCF
            (7, 6) => {
                let zero = self.regs.flag(FLAG_Z);
                self.regs.set_flags(zero, false, false, true);
            }
            // CCF
            (7, 7) => {
                let zero = self.regs.flag(FLAG_Z);
                let carry = self.regs.flag(FLAG_C);
                self.regs.set_flags(zero, false, false, !carry);
            }
            _ => unreachable!("opcode {opcode:#04x} is not in block 0"),
        }
    }

    /// Opcodes `0xC0..=0xFF`: stack, calls, jumps, immediates, high-page loads.
    fn execute_block3<B: CpuBus>(&mut self, opcode: u8, y: u8, z: u8, bus: &mut B) {
        match opcode {
            // RET cc
            0xC0 | 0xC8 | 0xD0 | 0xD8 => {
                bus.idle();
                if self.condition(y) {
                    self.regs.pc = self.pop16(bus);
                    bus.idle();
                }
            }
            // LDH (a8),A
            0xE0 => {
                let offset = self.fetch8(bus);
                bus.write(0xFF00 | offset as u16, self.regs.a);
            }
            // LDH A,(a8)
            0xF0 => {
                let offset = self.fetch8(bus);
                self.regs.a = bus.read(0xFF00 | offset as u16);
            }
            // ADD SP,e
            0xE8 => {
                self.regs.sp = self.sp_plus_offset(bus);
                bus.idle();
                bus.idle();
            }
            // LD HL,SP+e
            0xF8 => {
                let result = self.sp_plus_offset(bus);
                self.regs.set_hl(result);
                bus.idle();
            }
            // POP r16
            0xC1 | 0xD1 | 0xE1 | 0xF1 => {
                let value = self.pop16(bus);
                self.write_r16_stack(y >> 1, value);
            }
            // RET
            0xC9 => {
                self.regs.pc = self.pop16(bus);
                bus.idle();
            }
            // RETI: unlike EI, interrupts are enabled right away.
            0xD9 => {
                self.regs.pc = self.pop16(bus);
                bus.idle();
                self.ime = true;
            }
            // JP HL
            0xE9 => self.regs.pc = self.regs.hl(),
            // LD SP,HL
            0xF9 => {
                self.regs.sp = self.regs.hl();
                bus.idle();
            }
            // JP cc,a16
            0xC2 | 0xCA | 0xD2 | 0xDA => {
                let target = self.fetch16(bus);
                if self.condition(y) {
                    self.regs.pc = target;
                    bus.idle();
                }
            }
            // LD (C),A
            0xE2 => bus.write(0xFF00 | self.regs.c as u16, self.regs.a),
            // LD A,(C)
            0xF2 => self.regs.a = bus.read(0xFF00 | self.regs.c as u16),
            // LD (a16),A
            0xEA => {
                let addr = self.fetch16(bus);
                bus.write(addr, self.regs.a);
            }
            // LD A,(a16)
            0xFA => {
                let addr = self.fetch16(bus);
                self.regs.a = bus.read(addr);
            }
            // JP a16
            0xC3 => {
                self.regs.pc = self.fetch16(bus);
                bus.idle();
            }
            // Prefix: the next byte selects a bit operation.
            0xCB => self.execute_cb(bus),
            // DI
            0xF3 => {
                self.ime = false;
                self.ime_pending = false;
            }
            // EI
            0xFB => self.ime_pending = true,
            // CALL cc,a16
            0xC4 | 0xCC | 0xD4 | 0xDC => {
                let target = self.fetch16(bus);
                if self.condition(y) {
                    self.call(target, bus);
                }
            }
            // PUSH r16
            0xC5 | 0xD5 | 0xE5 | 0xF5 => {
                let value = self.read_r16_stack(y >> 1);
                bus.idle();
                self.push16(bus, value);
            }
            // CALL a16
            0xCD => {
                let target = self.fetch16(bus);
                self.call(target, bus);
            }
            // ADD/ADC/SUB/SBC/AND/XOR/OR/CP A,d8
            _ if z == 6 => {
                let value = self.fetch8(bus);
                self.alu(y, value);
            }
            // RST n: a one-byte CALL to a fixed address.
            _ if z == 7 => self.call(y as u16 * 8, bus),
            // The eleven holes in the opcode table hang the real CPU.
            _ => self.locked = true,
        }
    }

    /// CB-prefixed opcodes: rotates and shifts, BIT, RES, SET.
    fn execute_cb<B: CpuBus>(&mut self, bus: &mut B) {
        let opcode = self.fetch8(bus);
        let y = (opcode >> 3) & 7;
        let z = opcode & 7;
        let value = self.read_r8(z, bus);
        match opcode >> 6 {
            0 => {
                let result = self.rotate_shift(y, value);
                self.write_r8(z, result, bus);
            }
            // BIT y,r: only tests, never writes back.
            1 => {
                let carry = self.regs.flag(FLAG_C);
                self.regs
                    .set_flags(value & (1 << y) == 0, false, true, carry);
            }
            // RES y,r
            2 => self.write_r8(z, value & !(1 << y), bus),
            // SET y,r
            _ => self.write_r8(z, value | (1 << y), bus),
        }
    }

    /// 8-bit register by its 3-bit code: B, C, D, E, H, L, (HL), A.
    /// Code 6 is the byte HL points at, which costs an M-cycle.
    fn read_r8<B: CpuBus>(&mut self, code: u8, bus: &mut B) -> u8 {
        match code {
            0 => self.regs.b,
            1 => self.regs.c,
            2 => self.regs.d,
            3 => self.regs.e,
            4 => self.regs.h,
            5 => self.regs.l,
            6 => bus.read(self.regs.hl()),
            _ => self.regs.a,
        }
    }

    fn write_r8<B: CpuBus>(&mut self, code: u8, value: u8, bus: &mut B) {
        match code {
            0 => self.regs.b = value,
            1 => self.regs.c = value,
            2 => self.regs.d = value,
            3 => self.regs.e = value,
            4 => self.regs.h = value,
            5 => self.regs.l = value,
            6 => bus.write(self.regs.hl(), value),
            _ => self.regs.a = value,
        }
    }

    /// 16-bit register by its 2-bit code: BC, DE, HL, SP.
    fn read_r16(&self, code: u8) -> u16 {
        match code {
            0 => self.regs.bc(),
            1 => self.regs.de(),
            2 => self.regs.hl(),
            _ => self.regs.sp,
        }
    }

    fn write_r16(&mut self, code: u8, value: u16) {
        match code {
            0 => self.regs.set_bc(value),
            1 => self.regs.set_de(value),
            2 => self.regs.set_hl(value),
            _ => self.regs.sp = value,
        }
    }

    /// PUSH and POP use AF where the other instructions use SP.
    fn read_r16_stack(&self, code: u8) -> u16 {
        match code {
            3 => self.regs.af(),
            _ => self.read_r16(code),
        }
    }

    fn write_r16_stack(&mut self, code: u8, value: u16) {
        match code {
            3 => self.regs.set_af(value),
            _ => self.write_r16(code, value),
        }
    }

    /// Address for `LD A,(rr)` / `LD (rr),A`: BC, DE, HL+ (post-increment),
    /// HL- (post-decrement).
    fn indirect_address(&mut self, code: u8) -> u16 {
        match code {
            0 => self.regs.bc(),
            1 => self.regs.de(),
            2 => {
                let hl = self.regs.hl();
                self.regs.set_hl(hl.wrapping_add(1));
                hl
            }
            _ => {
                let hl = self.regs.hl();
                self.regs.set_hl(hl.wrapping_sub(1));
                hl
            }
        }
    }

    /// Condition by its 2-bit code: NZ, Z, NC, C.
    fn condition(&self, code: u8) -> bool {
        match code {
            0 => !self.regs.flag(FLAG_Z),
            1 => self.regs.flag(FLAG_Z),
            2 => !self.regs.flag(FLAG_C),
            _ => self.regs.flag(FLAG_C),
        }
    }

    fn jump_relative<B: CpuBus>(&mut self, taken: bool, bus: &mut B) {
        let offset = self.fetch8(bus) as i8;
        if taken {
            self.regs.pc = self.regs.pc.wrapping_add(offset as u16);
            bus.idle();
        }
    }

    fn call<B: CpuBus>(&mut self, target: u16, bus: &mut B) {
        bus.idle();
        self.push16(bus, self.regs.pc);
        self.regs.pc = target;
    }

    /// Shared by `ADD SP,e` and `LD HL,SP+e`. The flags come from adding the
    /// offset to the low byte of SP as unsigned 8-bit values.
    fn sp_plus_offset<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let offset = self.fetch8(bus);
        let sp = self.regs.sp;
        let half = (sp & 0x000F) + (offset as u16 & 0x000F) > 0x000F;
        let carry = (sp & 0x00FF) + offset as u16 > 0x00FF;
        self.regs.set_flags(false, false, half, carry);
        sp.wrapping_add(offset as i8 as u16)
    }

    /// The eight accumulator operations: ADD, ADC, SUB, SBC, AND, XOR, OR, CP.
    fn alu(&mut self, operation: u8, value: u8) {
        let a = self.regs.a;
        let carry_in = self.regs.flag(FLAG_C) as u8;
        match operation {
            0 => self.regs.a = self.add(a, value, 0),
            1 => self.regs.a = self.add(a, value, carry_in),
            2 => self.regs.a = self.sub(a, value, 0),
            3 => self.regs.a = self.sub(a, value, carry_in),
            4 => {
                self.regs.a = a & value;
                self.regs.set_flags(self.regs.a == 0, false, true, false);
            }
            5 => {
                self.regs.a = a ^ value;
                self.regs.set_flags(self.regs.a == 0, false, false, false);
            }
            6 => {
                self.regs.a = a | value;
                self.regs.set_flags(self.regs.a == 0, false, false, false);
            }
            // CP is a SUB that throws the result away.
            _ => {
                self.sub(a, value, 0);
            }
        }
    }

    fn add(&mut self, a: u8, value: u8, carry_in: u8) -> u8 {
        let sum = a as u16 + value as u16 + carry_in as u16;
        let half = (a & 0x0F) + (value & 0x0F) + carry_in > 0x0F;
        let result = sum as u8;
        self.regs.set_flags(result == 0, false, half, sum > 0xFF);
        result
    }

    fn sub(&mut self, a: u8, value: u8, carry_in: u8) -> u8 {
        let result = a.wrapping_sub(value).wrapping_sub(carry_in);
        let half = (a & 0x0F) < (value & 0x0F) + carry_in;
        let borrow = (a as u16) < value as u16 + carry_in as u16;
        self.regs.set_flags(result == 0, true, half, borrow);
        result
    }

    /// The eight rotate/shift operations: RLC, RRC, RL, RR, SLA, SRA, SWAP, SRL.
    /// Sets Z from the result and C from the bit shifted out.
    fn rotate_shift(&mut self, operation: u8, value: u8) -> u8 {
        let carry_in = self.regs.flag(FLAG_C) as u8;
        let (result, carry) = match operation {
            0 => (value.rotate_left(1), value & 0x80 != 0),
            1 => (value.rotate_right(1), value & 0x01 != 0),
            2 => (value << 1 | carry_in, value & 0x80 != 0),
            3 => (value >> 1 | carry_in << 7, value & 0x01 != 0),
            4 => (value << 1, value & 0x80 != 0),
            // SRA keeps the sign bit.
            5 => (value >> 1 | value & 0x80, value & 0x01 != 0),
            6 => (value.rotate_left(4), false),
            _ => (value >> 1, value & 0x01 != 0),
        };
        self.regs.set_flags(result == 0, false, false, carry);
        result
    }

    /// Decimal adjust: fixes A after adding or subtracting two BCD numbers,
    /// using the N and H flags left behind by that operation.
    fn daa(&mut self) {
        let mut a = self.regs.a;
        let mut carry = self.regs.flag(FLAG_C);
        if self.regs.flag(FLAG_N) {
            if self.regs.flag(FLAG_H) {
                a = a.wrapping_sub(0x06);
            }
            if carry {
                a = a.wrapping_sub(0x60);
            }
        } else {
            let mut adjust = 0;
            if self.regs.flag(FLAG_H) || a & 0x0F > 0x09 {
                adjust |= 0x06;
            }
            if carry || a > 0x99 {
                adjust |= 0x60;
                carry = true;
            }
            a = a.wrapping_add(adjust);
        }
        let subtract = self.regs.flag(FLAG_N);
        self.regs.set_flags(a == 0, subtract, false, carry);
        self.regs.a = a;
    }
}
