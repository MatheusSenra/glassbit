# Glassbit

Open source Game Boy (DMG) emulator in Rust + WebAssembly with an educational
"X-ray mode". The owner is a retro console enthusiast who wants to understand
how the emulator works: implement the code and explain the important parts.

## Language

- Code, identifiers, comments, commits, README: **English**.
- Conversation and explanations with the user: **Portuguese**.

## Workflow

- Work **one phase at a time** (roadmap in `README.md`). At the end of a phase:
  all tests passing, a short summary, a Portuguese explanation of the 2-3 most
  important hardware concepts of that phase, then **stop and wait for approval**.
- Tests ship with the code. Never move to the next phase with a failing test.
- Small Conventional Commits (`feat:`, `fix:`, `test:`, `docs:`, `chore:`, `ci:`).
- Prefer clear code over clever code. Optimize only after measuring.
- For a relevant design decision, present the options in a few lines and
  recommend one before implementing.
- If a spec is ambiguous, say which interpretation was adopted.

## Commands

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p glassbit-web --target wasm32-unknown-unknown
wasm-pack build crates/web --target web      # also run by CI
bash scripts/fetch-test-roms.sh              # fills test-roms/ (git-ignored)
```

## Architecture

- `crates/core`: the emulator, platform independent. No browser, window or
  audio dependencies. Minimal public API: load a ROM, run a frame, read the
  160x144 framebuffer, read audio samples, set button state.
- `crates/web`: `wasm-bindgen` frontend. Minimal HTML/JS, no framework.
- `crates/runner`: headless native runner for test ROMs; the main
  verification tool.

Core modules: `cpu`, `bus`, `ppu`, `timer`, `interrupts`, `joypad`, `serial`,
`apu`, `cartridge` (trait for MBCs).

### Design decisions

- **Timing:** the CPU advances the rest of the hardware on every bus access
  (each read/write is one M-cycle with the tick built in), not once per
  instruction. Needed for `mem_timing` and Mooneye. The tick happens before
  the access.
- **CPU is generic over the `CpuBus` trait** (`read`, `write`, `idle`; one
  M-cycle each) so SingleStepTests can run it against a flat 64 KB RAM and
  compare per-cycle bus activity. `Bus::peek`/`Bus::poke` access memory
  without advancing time (debugger, tests).
- **Interrupts are sampled during the opcode fetch**, after that cycle's tick.
  If one is taken the fetched opcode is discarded and the fetch counts as the
  first of the five dispatch cycles. Waking from HALT into a handler costs one
  cycle more. Changing this breaks Mooneye `rapid_toggle` or `intr_timing`.
- **Post-boot timer counter is `0xABC8`**, measured with Mooneye
  `boot_div-dmgABCmgb` (it is tied to the tick-before-access convention).
- **Opcodes are decoded by bit fields** (octal `x y z`), not a 256-entry table.
- **STOP is a NOP** and **illegal opcodes lock the CPU** until something needs
  more.
- **Tests that need downloaded ROMs skip themselves** when `test-roms/` is
  missing, and fail instead when `GLASSBIT_REQUIRE_TEST_ROMS` is set (CI).
- **Inspection is zero cost when off:** an `Observer` trait with empty default
  methods, the emulator generic over it, `NullObserver` compiled away. Events:
  instruction executed with registers before/after, PPU mode change and LY,
  interrupt dispatched and its cause, I/O register write, MBC bank switch.
- **State stays plain data** (no closures or pointers inside the core) so save
  states are easy later.
- **Web frame loop** uses a real-time accumulator (displays are 60/120/144 Hz,
  the Game Boy is ~59.73 Hz). Audio will use dynamic rate control.
- **AudioWorklet** gets samples by `postMessage` into a ring buffer;
  `SharedArrayBuffer` needs COOP/COEP headers GitHub Pages does not send.
- **Battery saves** go to IndexedDB.

## Sources of truth

Pan Docs (gbdev.io/pandocs), the gbdev opcode table and SM83 manual,
SingleStepTests/sm83, Blargg (`cpu_instrs`, `instr_timing`, `mem_timing`),
Mooneye acceptance tests, dmg-acid2.

**Do not copy code from other emulators.** Work from documentation.

Test result protocols the runner must detect: Blargg serial output, Blargg
memory signature at `0xA000`, and Mooneye (registers B,C,D,E,H,L =
3,5,8,13,21,34 followed by `LD B,B`).

## Legal rules

- Never commit commercial ROMs or the Nintendo boot ROM.
- Do not use the boot ROM: initialize registers and I/O to the post-boot state
  documented in Pan Docs.
- Test ROMs live in `test-roms/` (git-ignored), fetched by
  `scripts/fetch-test-roms.sh`.
- License: dual MIT / Apache-2.0.

## Out of scope for now

Game Boy Color; link cable over WebRTC (after Phase 8).
