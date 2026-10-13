//! Disassembler: turns the bytes at an address back into a readable
//! instruction, for debuggers and the X-ray mode.
//!
//! The decoding mirrors `cpu::execute`: opcodes are split into the octal
//! fields `x y z`. Syntax follows RGBDS, the usual Game Boy assembler, with
//! `$` marking hexadecimal numbers.

use std::fmt;

/// One decoded instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// How many bytes it occupies (1 to 3).
    pub length: u8,
    /// The operation, e.g. `"LD"` or `"JR"`.
    pub operation: &'static str,
    /// Zero, one or two operands, e.g. `["A", "(HL)"]`.
    pub operands: Vec<String>,
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.operation)?;
        if !self.operands.is_empty() {
            write!(f, " {}", self.operands.join(","))?;
        }
        Ok(())
    }
}

const R8: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const R16: [&str; 4] = ["BC", "DE", "HL", "SP"];
const R16_STACK: [&str; 4] = ["BC", "DE", "HL", "AF"];
const R16_INDIRECT: [&str; 4] = ["(BC)", "(DE)", "(HL+)", "(HL-)"];
const CONDITIONS: [&str; 4] = ["NZ", "Z", "NC", "C"];
const ALU: [&str; 8] = ["ADD", "ADC", "SUB", "SBC", "AND", "XOR", "OR", "CP"];
const ROTATES: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SWAP", "SRL"];
const ACCUMULATOR_OPS: [&str; 8] = ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"];

/// Decodes the instruction at `addr`, reading memory through `read`.
pub fn disassemble(read: impl Fn(u16) -> u8, addr: u16) -> Instruction {
    let opcode = read(addr);
    let byte = || read(addr.wrapping_add(1));
    let word = || u16::from_le_bytes([read(addr.wrapping_add(1)), read(addr.wrapping_add(2))]);
    let hex8 = |value: u8| format!("${value:02X}");
    let hex16 = |value: u16| format!("${value:04X}");
    // Relative jumps are shown by where they land.
    let relative = || hex16(addr.wrapping_add(2).wrapping_add(byte() as i8 as u16));
    let signed = || {
        let offset = byte() as i8;
        if offset < 0 {
            format!("-${:02X}", offset.unsigned_abs())
        } else {
            format!("+${offset:02X}")
        }
    };

    let y = (opcode >> 3 & 7) as usize;
    let z = (opcode & 7) as usize;
    let pair = y >> 1;
    let s = |text: &str| text.to_string();
    let make = |length: u8, operation: &'static str, operands: Vec<String>| Instruction {
        length,
        operation,
        operands,
    };

    match opcode >> 6 {
        0 => match (z, y) {
            (0, 0) => make(1, "NOP", vec![]),
            (0, 1) => make(3, "LD", vec![format!("({})", hex16(word())), s("SP")]),
            (0, 2) => make(1, "STOP", vec![]),
            (0, 3) => make(2, "JR", vec![relative()]),
            (0, _) => make(2, "JR", vec![s(CONDITIONS[y - 4]), relative()]),
            (1, _) if y & 1 == 0 => make(3, "LD", vec![s(R16[pair]), hex16(word())]),
            (1, _) => make(1, "ADD", vec![s("HL"), s(R16[pair])]),
            (2, _) if y & 1 == 0 => make(1, "LD", vec![s(R16_INDIRECT[pair]), s("A")]),
            (2, _) => make(1, "LD", vec![s("A"), s(R16_INDIRECT[pair])]),
            (3, _) if y & 1 == 0 => make(1, "INC", vec![s(R16[pair])]),
            (3, _) => make(1, "DEC", vec![s(R16[pair])]),
            (4, _) => make(1, "INC", vec![s(R8[y])]),
            (5, _) => make(1, "DEC", vec![s(R8[y])]),
            (6, _) => make(2, "LD", vec![s(R8[y]), hex8(byte())]),
            _ => make(1, ACCUMULATOR_OPS[y], vec![]),
        },
        1 if opcode == 0x76 => make(1, "HALT", vec![]),
        1 => make(1, "LD", vec![s(R8[y]), s(R8[z])]),
        2 => make(1, ALU[y], vec![s("A"), s(R8[z])]),
        _ => match opcode {
            0xC0 | 0xC8 | 0xD0 | 0xD8 => make(1, "RET", vec![s(CONDITIONS[y])]),
            0xE0 => make(2, "LDH", vec![format!("($FF{:02X})", byte()), s("A")]),
            0xF0 => make(2, "LDH", vec![s("A"), format!("($FF{:02X})", byte())]),
            0xE8 => make(2, "ADD", vec![s("SP"), signed()]),
            0xF8 => make(2, "LD", vec![s("HL"), format!("SP{}", signed())]),
            0xC1 | 0xD1 | 0xE1 | 0xF1 => make(1, "POP", vec![s(R16_STACK[pair])]),
            0xC9 => make(1, "RET", vec![]),
            0xD9 => make(1, "RETI", vec![]),
            0xE9 => make(1, "JP", vec![s("HL")]),
            0xF9 => make(1, "LD", vec![s("SP"), s("HL")]),
            0xC2 | 0xCA | 0xD2 | 0xDA => make(3, "JP", vec![s(CONDITIONS[y]), hex16(word())]),
            0xE2 => make(1, "LDH", vec![s("(C)"), s("A")]),
            0xF2 => make(1, "LDH", vec![s("A"), s("(C)")]),
            0xEA => make(3, "LD", vec![format!("({})", hex16(word())), s("A")]),
            0xFA => make(3, "LD", vec![s("A"), format!("({})", hex16(word()))]),
            0xC3 => make(3, "JP", vec![hex16(word())]),
            0xCB => disassemble_cb(byte()),
            0xF3 => make(1, "DI", vec![]),
            0xFB => make(1, "EI", vec![]),
            0xC4 | 0xCC | 0xD4 | 0xDC => make(3, "CALL", vec![s(CONDITIONS[y]), hex16(word())]),
            0xC5 | 0xD5 | 0xE5 | 0xF5 => make(1, "PUSH", vec![s(R16_STACK[pair])]),
            0xCD => make(3, "CALL", vec![hex16(word())]),
            _ if z == 6 => make(2, ALU[y], vec![s("A"), hex8(byte())]),
            _ if z == 7 => make(1, "RST", vec![hex8(y as u8 * 8)]),
            // One of the eleven holes in the opcode table.
            _ => make(1, "ILLEGAL", vec![hex8(opcode)]),
        },
    }
}

fn disassemble_cb(opcode: u8) -> Instruction {
    let y = (opcode >> 3 & 7) as usize;
    let register = R8[(opcode & 7) as usize].to_string();
    let (operation, operands) = match opcode >> 6 {
        0 => (ROTATES[y], vec![register]),
        1 => ("BIT", vec![y.to_string(), register]),
        2 => ("RES", vec![y.to_string(), register]),
        _ => ("SET", vec![y.to_string(), register]),
    };
    Instruction {
        length: 2,
        operation,
        operands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{Cpu, CpuBus};
    use std::collections::BTreeSet;

    fn text(bytes: &[u8]) -> String {
        let mut memory = vec![0u8; 0x1_0000];
        memory[0x0150..0x0150 + bytes.len()].copy_from_slice(bytes);
        disassemble(|addr| memory[addr as usize], 0x0150).to_string()
    }

    #[test]
    fn formats_the_common_shapes() {
        assert_eq!(text(&[0x00]), "NOP");
        assert_eq!(text(&[0x3E, 0x42]), "LD A,$42");
        assert_eq!(text(&[0x21, 0x34, 0x12]), "LD HL,$1234");
        assert_eq!(text(&[0x7E]), "LD A,(HL)");
        assert_eq!(text(&[0x22]), "LD (HL+),A");
        assert_eq!(text(&[0xEA, 0x00, 0xC0]), "LD ($C000),A");
        assert_eq!(text(&[0xE0, 0x44]), "LDH ($FF44),A");
        assert_eq!(text(&[0xF2]), "LDH A,(C)");
        assert_eq!(text(&[0x86]), "ADD A,(HL)");
        assert_eq!(text(&[0xFE, 0x90]), "CP A,$90");
        assert_eq!(text(&[0xC3, 0x50, 0x01]), "JP $0150");
        assert_eq!(text(&[0xCC, 0x00, 0x40]), "CALL Z,$4000");
        assert_eq!(text(&[0xC5]), "PUSH BC");
        assert_eq!(text(&[0xF1]), "POP AF");
        assert_eq!(text(&[0xEF]), "RST $28");
        assert_eq!(text(&[0x76]), "HALT");
        assert_eq!(text(&[0xD3]), "ILLEGAL $D3");
    }

    #[test]
    fn relative_jumps_show_their_destination() {
        // At 0x0150, a 2-byte jump by -2 lands on itself.
        assert_eq!(text(&[0x18, 0xFE]), "JR $0150");
        assert_eq!(text(&[0x20, 0x05]), "JR NZ,$0157");
    }

    #[test]
    fn signed_offsets_keep_their_sign() {
        assert_eq!(text(&[0xE8, 0xFE]), "ADD SP,-$02");
        assert_eq!(text(&[0xF8, 0x10]), "LD HL,SP+$10");
    }

    #[test]
    fn cb_prefixed_instructions() {
        assert_eq!(text(&[0xCB, 0x37]), "SWAP A");
        assert_eq!(text(&[0xCB, 0x7E]), "BIT 7,(HL)");
        assert_eq!(text(&[0xCB, 0x80]), "RES 0,B");
        assert_eq!(text(&[0xCB, 0xFF]), "SET 7,A");
    }

    /// Records which addresses the CPU reads.
    struct Recorder {
        memory: Vec<u8>,
        reads: BTreeSet<u16>,
    }

    impl CpuBus for Recorder {
        fn read(&mut self, addr: u16) -> u8 {
            self.reads.insert(addr);
            self.memory[addr as usize]
        }

        fn write(&mut self, _addr: u16, _value: u8) {}

        fn idle(&mut self) {}
    }

    /// The length the disassembler reports must be the number of bytes the
    /// CPU really fetches, for every opcode.
    #[test]
    fn lengths_agree_with_the_cpu_for_every_opcode() {
        const PC: u16 = 0x0150;
        let prefixes: [&[u8]; 2] = [&[], &[0xCB]];
        for prefix in prefixes {
            for opcode in 0..=0xFFu8 {
                if prefix.is_empty() && opcode == 0xCB {
                    continue;
                }
                let mut bus = Recorder {
                    memory: vec![0; 0x1_0000],
                    reads: BTreeSet::new(),
                };
                let mut bytes = prefix.to_vec();
                bytes.push(opcode);
                bus.memory[PC as usize..PC as usize + bytes.len()].copy_from_slice(&bytes);

                // Point every register pair far away from the code, so that
                // only instruction fetches touch it.
                let mut cpu = Cpu::default();
                cpu.regs.pc = PC;
                cpu.regs.sp = 0xD000;
                cpu.regs.set_bc(0xC100);
                cpu.regs.set_de(0xC200);
                cpu.regs.set_hl(0xC300);
                cpu.step(&mut bus);

                let fetched = (PC..PC + 3).filter(|addr| bus.reads.contains(addr)).count();
                let decoded = disassemble(|addr| bus.memory[addr as usize], PC);
                assert_eq!(decoded.length as usize, fetched, "{decoded} ({bytes:02X?})");
            }
        }
    }
}
