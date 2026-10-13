# Glassbit

A Game Boy (DMG) emulator written in Rust that runs in the browser through
WebAssembly, built to **show how the hardware works while the game runs**.

> Status: early development. Cartridges load and the memory map exists; the CPU comes next.

## Goals

- Reasonable timing accuracy from the start: hardware advances in lockstep, one M-cycle at a time.
- A platform-independent core that can be driven by a browser, a test runner or anything else.
- An "X-ray mode": a side panel that explains the CPU, PPU, memory map and interrupts live.
- Written from documentation ([Pan Docs](https://gbdev.io/pandocs/)), not from other emulators' code.

## Layout

| Crate | What it is |
| --- | --- |
| `crates/core` (`glassbit-core`) | The emulator. No browser, window or audio dependencies. |
| `crates/web` (`glassbit-web`) | WebAssembly frontend (`wasm-bindgen`): canvas, input, audio, saves. |
| `crates/runner` (`glassbit-runner`) | Headless native runner for test ROMs: serial capture and PNG screenshots. |

## Building

You need [Rust](https://rustup.rs); the pinned toolchain in `rust-toolchain.toml`
installs itself, including the wasm target.

```bash
cargo test --workspace
cargo run -p glassbit-runner -- path/to/rom.gb
```

To build the web frontend, install [wasm-pack](https://rustwasm.github.io/wasm-pack/) and run:

```bash
wasm-pack build crates/web --target web
```

## Test ROMs

Test ROMs are not part of this repository. Download them into `test-roms/` (git-ignored) with:

```bash
bash scripts/fetch-test-roms.sh
```

This fetches a pinned release of
[c-sp/game-boy-test-roms](https://github.com/c-sp/game-boy-test-roms) (Blargg,
Mooneye, dmg-acid2) and the
[SingleStepTests/sm83](https://github.com/SingleStepTests/sm83) CPU test
vectors. Pass `--no-sm83` to skip the latter, which is large.

## Roadmap

- [x] **Phase 0: Foundation.** Workspace, CI, licenses, test ROM script.
- [x] **Phase 1: Cartridge and bus.** Header parsing, ROM-only cartridge, full memory map.
- [ ] **Phase 2: CPU.** All opcodes, validated against SingleStepTests and the individual Blargg `cpu_instrs` ROMs; serial port.
- [ ] **Phase 3: Interrupts and timer.** IME, HALT (and its bug), EI delay, DIV/TIMA/TMA/TAC. Blargg `instr_timing`, `mem_timing`, `cpu_instrs` 02 and Mooneye timer tests.
- [ ] **Phase 4: PPU.** Modes, STAT, background, window, sprites, OAM DMA. Pixel-exact `dmg-acid2`.
- [ ] **Phase 5: Playable web frontend.** Canvas, frame loop, keyboard, ROM loading.
- [ ] **Phase 6: Mappers and saves.** MBC1, MBC3, MBC5, battery RAM persisted in the browser.
- [ ] **Phase 7: Audio.** All four APU channels through an AudioWorklet.
- [ ] **Phase 7.5: X-ray mode.** Live CPU, PPU, memory and interrupt panels, in English and Portuguese.
- [ ] **Phase 8: Polish.** GitHub Pages deploy, save states, fast-forward, palettes.

Later: link cable over WebRTC, Game Boy Color.

## Legal

Glassbit contains no Nintendo code. It does not include or need the boot ROM:
the console starts in the documented post-boot state. No commercial ROMs are
distributed with it, and none will be accepted in this repository.

Game Boy is a trademark of Nintendo. This project is not affiliated with or
endorsed by Nintendo.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
