//! Runs the CPU against the SingleStepTests/sm83 vectors: 1000 cases for each
//! of the 500 opcodes, each giving the full state before and after one
//! instruction plus what happened on the bus in every M-cycle.
//!
//! The vectors are downloaded by `scripts/fetch-test-roms.sh`. Without them
//! this test is skipped, unless `GLASSBIT_REQUIRE_TEST_ROMS` is set (CI sets
//! it), in which case their absence is a failure.

use std::path::{Path, PathBuf};

use glassbit_core::cpu::{Cpu, CpuBus};
use serde_json::Value;

/// One M-cycle as seen from outside the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cycle {
    Read(u16, u8),
    Write(u16, u8),
    Idle,
}

/// 64 KiB of plain RAM that records every cycle.
struct RecordingBus {
    memory: Vec<u8>,
    cycles: Vec<Cycle>,
}

impl CpuBus for RecordingBus {
    fn read(&mut self, addr: u16) -> u8 {
        let value = self.memory[addr as usize];
        self.cycles.push(Cycle::Read(addr, value));
        value
    }

    fn write(&mut self, addr: u16, value: u8) {
        self.memory[addr as usize] = value;
        self.cycles.push(Cycle::Write(addr, value));
    }

    fn idle(&mut self) {
        self.cycles.push(Cycle::Idle);
    }
}

fn vectors_dir() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-roms/sm83/v1");
    if dir.is_dir() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("GLASSBIT_REQUIRE_TEST_ROMS").is_none(),
        "{} not found; run scripts/fetch-test-roms.sh",
        dir.display()
    );
    eprintln!("skipping: run scripts/fetch-test-roms.sh to enable the sm83 tests");
    None
}

fn field(state: &Value, name: &str) -> u16 {
    state[name]
        .as_u64()
        .unwrap_or_else(|| panic!("missing field {name}")) as u16
}

fn ram(state: &Value) -> impl Iterator<Item = (u16, u8)> + '_ {
    state["ram"]
        .as_array()
        .expect("ram is an array")
        .iter()
        .map(|entry| {
            (
                entry[0].as_u64().expect("address") as u16,
                entry[1].as_u64().expect("value") as u8,
            )
        })
}

fn cpu_from(state: &Value) -> Cpu {
    let mut cpu = Cpu::default();
    cpu.regs.a = field(state, "a") as u8;
    cpu.regs.f = field(state, "f") as u8;
    cpu.regs.b = field(state, "b") as u8;
    cpu.regs.c = field(state, "c") as u8;
    cpu.regs.d = field(state, "d") as u8;
    cpu.regs.e = field(state, "e") as u8;
    cpu.regs.h = field(state, "h") as u8;
    cpu.regs.l = field(state, "l") as u8;
    cpu.regs.sp = field(state, "sp");
    cpu.regs.pc = field(state, "pc");
    cpu.ime = field(state, "ime") != 0;
    cpu
}

fn expected_cycles(test: &Value) -> Vec<Cycle> {
    test["cycles"]
        .as_array()
        .expect("cycles is an array")
        .iter()
        .map(|cycle| {
            let addr = cycle[0].as_u64().expect("address") as u16;
            let value = cycle[1].as_u64().expect("value") as u8;
            match cycle[2].as_str().expect("pins") {
                "r-m" => Cycle::Read(addr, value),
                "-wm" => Cycle::Write(addr, value),
                "---" => Cycle::Idle,
                other => panic!("unknown cycle kind {other}"),
            }
        })
        .collect()
}

/// Runs one test case; returns a description of the first mismatch.
fn run_case(test: &Value, check_cycles: bool, bus: &mut RecordingBus) -> Result<(), String> {
    let initial = &test["initial"];
    let expected = &test["final"];

    let mut cpu = cpu_from(initial);
    for (addr, value) in ram(initial) {
        bus.memory[addr as usize] = value;
    }
    bus.cycles.clear();

    cpu.step(bus);

    let mut want = cpu_from(expected);
    // "ei" is only present when an EI is waiting to take effect.
    want.ime_pending = expected.get("ei").and_then(Value::as_u64) == Some(1);
    want.halted = cpu.halted;

    let mut result = Ok(());
    if cpu != want {
        result = Err(format!("state: got {cpu:?}, want {want:?}"));
    } else if let Some((addr, value)) =
        ram(expected).find(|&(addr, value)| bus.memory[addr as usize] != value)
    {
        result = Err(format!(
            "memory[{addr:#06x}]: got {:#04x}, want {value:#04x}",
            bus.memory[addr as usize]
        ));
    } else if check_cycles {
        let want_cycles = expected_cycles(test);
        if bus.cycles != want_cycles {
            result = Err(format!(
                "cycles: got {:?}, want {want_cycles:?}",
                bus.cycles
            ));
        }
    }

    // Leave the shared memory clean for the next case.
    for (addr, _) in ram(initial).chain(ram(expected)) {
        bus.memory[addr as usize] = 0;
    }
    result
}

/// Runs every case in one file; returns the failures (capped) and their count.
fn run_file(path: &Path) -> (usize, Vec<String>) {
    let name = path.file_stem().unwrap().to_string_lossy().to_string();
    // The vectors model HALT and STOP as taking extra cycles that belong to
    // the sleeping state, not to the instruction. Only their end state is
    // compared.
    let check_cycles = name != "76" && name != "10";

    let text = std::fs::read_to_string(path).expect("readable test file");
    let tests: Value = serde_json::from_str(&text).expect("valid JSON");
    let mut bus = RecordingBus {
        memory: vec![0; 0x1_0000],
        cycles: Vec::new(),
    };

    let mut failures = 0;
    let mut messages = Vec::new();
    for test in tests.as_array().expect("array of tests") {
        if let Err(message) = run_case(test, check_cycles, &mut bus) {
            failures += 1;
            if messages.len() < 2 {
                messages.push(format!("{}: {message}", test["name"]));
            }
        }
    }
    (failures, messages)
}

#[test]
fn all_opcodes_match_the_single_step_vectors() {
    let Some(dir) = vectors_dir() else { return };

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("readable directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 500, "expected one file per opcode");

    // Parsing ~200 MB of JSON dominates the run time; spread it over the cores.
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk_size = files.len().div_ceil(workers);
    let results: Vec<(PathBuf, usize, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|path| {
                            let (failures, messages) = run_file(path);
                            (path.clone(), failures, messages)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("worker panicked"))
            .collect()
    });

    let mut failed_files = 0;
    for (path, failures, messages) in &results {
        if *failures > 0 {
            failed_files += 1;
            eprintln!(
                "{}: {failures} failing",
                path.file_name().unwrap().to_string_lossy()
            );
            for message in messages {
                eprintln!("    {message}");
            }
        }
    }
    assert_eq!(failed_files, 0, "{failed_files} opcode files have failures");
}
