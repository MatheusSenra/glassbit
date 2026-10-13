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
wasm-pack build crates/web --target web --out-dir www/pkg   # also run by CI and Pages
python -m http.server 8765 --directory crates/web/www       # then open localhost:8765
python scripts/make-web-images.py            # regenerate www/logo.png and favicon.png
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
`apu`, `cartridge` (trait for MBCs), `observer`, `disasm`, `demo`.

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
- **The PPU renders one whole line when mode 3 starts** (no pixel FIFO). Mode
  timing constants in `ppu.rs` (mode 3 at M-cycle 21 of the line, mode 0 at
  64, line 0's OAM scan one cycle late) were fitted to Mooneye
  `acceptance/ppu`; do not "correct" them to the dot counts in Pan Docs
  without rerunning those tests. Mode 3 only grows with `SCX % 8`.
- **The CPU is locked out of VRAM in mode 3 and of OAM in modes 2-3 and
  during OAM DMA** (`CpuBus` reads give `0xFF`, writes are dropped);
  `peek`/`poke` ignore the lockout.
- **`run_frame` returns when VBlank starts**, or after one frame of time when
  the LCD is off. The framebuffer holds shades 0-3, not colors.
- **The web page is plain HTML/CSS/JS in `crates/web/www`**; the wasm package
  is built into `www/pkg` (git-ignored), so that folder is the deployable
  site. `.claude/launch.json` serves it for previews. The preview pane does
  not fire `requestAnimationFrame` while hidden: to test the loop there,
  replace `window.requestAnimationFrame` with a `setTimeout` shim and
  re-import `main.js` with a cache-busting query.
- **`glassbit_core::demo::rom()` is a hand-assembled demo cartridge** used as
  the page's default and for end-to-end input tests. Web key mapping: arrows,
  X = A, Z = B, Enter = Start, Shift = Select.
- **Mappers:** ROM-only, MBC1 (no multicart), MBC3 (MBC30 inferred from
  sizes; RTC registers are storage only, the clock does not tick because the
  core has no wall-clock time) and MBC5 (rumble ignored). MBC2 is missing.
- **Battery saves are the frontend's job:** the core exposes `save_data`,
  `load_save` and `take_save_dirty`; the page stores raw cartridge RAM in
  IndexedDB (`www/saves.js`), keyed by title plus global checksum.
- **Audio:** the APU ticks once per M-cycle with channel timers in T-cycles,
  averages the mixer output down to the frontend's sample rate and applies
  the capacitor high-pass. The frame sequencer has its own counter (it is not
  tied to DIV). Wave RAM access while channel 3 plays is not modeled.
  `GameBoy::audio_samples()` is interleaved stereo for the last frame.
- **Web audio:** samples go by `postMessage` to `www/audio-worklet.js`, which
  holds a ring buffer, fades on underrun and skips ahead on overflow.
  `www/audio.js` nudges emulation speed by at most 2% to hold about 70 ms
  queued. The `AudioContext` is created on the first user gesture.
- **X-ray mode:** the web `Emulator` holds either `GameBoy<NullObserver>` or
  `GameBoy<Recorder>` and swaps between them with `GameBoy::into_observed`
  when the panel opens or closes. `www/xray.js` only reads the emulator and
  redraws; `www/i18n.js` holds every user-visible string in English and
  Portuguese (static HTML uses `data-i18n`). Instruction explanations are
  templates keyed by operation in `i18n.js`, fed by `disasm`.
- **Large buffers (VRAM, framebuffer, WRAM) are `Box<[u8]>`, not arrays.**
  With inline arrays, moving a `GameBoy` by value made LLVM emit a 720 KB
  function in the wasm and `wasm-opt` hang. Keep big state on the heap.
- **Testing the page:** the browser caches ES modules served by
  `http.server`; after editing a `.js` file, `fetch(file, {cache: "reload"})`
  it (or hard-reload) before trusting what the page runs.
- **Save states** (`state.rs`): every component has `save_state`/`load_state`
  writing its fields in a fixed order. Adding, removing or reordering a field
  means updating both functions and bumping `state::VERSION`. The tests in
  `crates/runner/tests/save_states.rs` catch a forgotten field as a
  divergence. Loading masks or rejects values that could index out of bounds.
- **Web extras:** save states go to a second IndexedDB store, fast-forward
  multiplies elapsed time by 4 and sends no audio, palettes are chosen in JS
  and passed to `Emulator::set_palette`.
- **Deploy:** `.github/workflows/pages.yml` publishes `crates/web/www` to
  GitHub Pages on every push to `main`
  (<https://matheussenra.github.io/glassbit/>).
- **README images:** `assets/demo.gif` is recorded with
  `glassbit-runner --gif ... --press ...`; `assets/xray.jpg` is a browser
  screenshot.
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
