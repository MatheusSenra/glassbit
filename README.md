<p align="center"><img src="assets/banner.webp" alt="Glassbit: transparent emulation" width="800"></p>

# Glassbit

[![CI](https://github.com/MatheusSenra/glassbit/actions/workflows/ci.yml/badge.svg)](https://github.com/MatheusSenra/glassbit/actions/workflows/ci.yml)

A Game Boy (DMG) emulator written in Rust that runs in the browser through
WebAssembly, built to **show how the hardware works while the game runs**.

> Status: early development. It runs in the browser with video and input, for ROM-only and small MBC1 cartridges. There is no sound and there are no saves yet.

<p align="center"><img src="assets/dmg-acid2.png" alt="The dmg-acid2 test rendered by Glassbit" width="320"><br><em>The dmg-acid2 PPU test, rendered by Glassbit.</em></p>

## Goals

- Reasonable timing accuracy from the start: hardware advances in lockstep, one M-cycle at a time.
- A platform-independent core that can be driven by a browser, a test runner or anything else.
- An "X-ray mode": a side panel that explains the CPU, PPU, memory map and interrupts live.
- Written from documentation ([Pan Docs](https://gbdev.io/pandocs/)), not from other emulators' code.

## Layout

| Crate | What it is |
| --- | --- |
| `crates/core` (`glassbit-core`) | The emulator. No browser, window or audio dependencies. |
| `crates/web` (`glassbit-web`) | WebAssembly frontend (`wasm-bindgen`) and the page in `www/`: canvas, frame loop, keyboard, ROM loading. |
| `crates/runner` (`glassbit-runner`) | Headless native runner: runs a test ROM, reports whether it passed and can save the screen as a PNG. |

## Building

You need [Rust](https://rustup.rs); the pinned toolchain in `rust-toolchain.toml`
installs itself, including the wasm target.

```bash
cargo test --workspace
```

Tests that need the downloaded test ROMs (see below) skip themselves when the
ROMs are missing. To run a single test ROM and see its verdict:

```bash
cargo run --release -p glassbit-runner -- "test-roms/game-boy-test-roms/blargg/cpu_instrs/cpu_instrs.gb"
```

Add `--screenshot out.png` to save the final screen, and `--reference ref.png`
to pass or fail by comparing the screen with an image.

## Running in the browser

Install [wasm-pack](https://rustwasm.github.io/wasm-pack/), build the wasm
package into the page's folder and serve that folder with any static server:

```bash
wasm-pack build crates/web --target web --out-dir www/pkg
```

```bash
python -m http.server 8765 --directory crates/web/www
```

Then open <http://localhost:8765>. The page starts with a built-in demo
cartridge; load your own ROM with the button or by dropping the file on the
page. ROMs stay in your browser and are never uploaded.

| Key | Button |
| --- | --- |
| Arrow keys | Direction pad |
| X | A |
| Z | B |
| Enter | Start |
| Shift | Select |

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

## Accuracy

What passes today, all of it checked by `cargo test` in CI:

| Suite | Result |
| --- | --- |
| SingleStepTests/sm83 | all 500 opcodes, 1000 cases each, including per-cycle bus activity |
| Blargg `cpu_instrs` | all 11 |
| Blargg `instr_timing` | pass |
| Blargg `mem_timing` | all 3 |
| dmg-acid2 | pixel-exact match with the reference image |
| Mooneye `acceptance/timer` | all 13 |
| Mooneye `acceptance`, other | 43, covering instruction, interrupt, HALT, OAM DMA and PPU mode timing and the post-boot state |

Of the Mooneye acceptance tests that do not pass, most target console models
other than the DMG. The rest are five PPU tests that need pixel-level timing
(the PPU draws a whole line at once), two `-GS` timing tests, and two that
depend on I/O registers and a mapper not implemented yet.

## Roadmap

- [x] **Phase 0: Foundation.** Workspace, CI, licenses, test ROM script.
- [x] **Phase 1: Cartridge and bus.** Header parsing, ROM-only cartridge, full memory map.
- [x] **Phase 2: CPU.** All opcodes, validated against SingleStepTests and the individual Blargg `cpu_instrs` ROMs; serial port; ROM banking for MBC1.
- [x] **Phase 3: Interrupts and timer.** IME, HALT (and its bug), EI delay, DIV/TIMA/TMA/TAC. Blargg `instr_timing`, `mem_timing`, all of `cpu_instrs` and Mooneye timer tests.
- [x] **Phase 4: PPU.** Modes, STAT, background, window, sprites, OAM DMA. Pixel-exact `dmg-acid2`.
- [x] **Phase 5: Playable web frontend.** Canvas, frame loop, keyboard, ROM loading, joypad, built-in demo cartridge.
- [ ] **Phase 6: Mappers and saves.** The rest of MBC1 (RAM, large ROMs), MBC3, MBC5, battery RAM persisted in the browser.
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
