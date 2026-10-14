# Glassbit

Open source Game Boy (DMG) and Game Boy Color emulator in Rust + WebAssembly with an educational
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
- **Mode 3 is a dot-by-dot pixel FIFO** (`ppu.rs`): 6 startup dots, a
  fetcher (tile number, low and high byte, 6 dots) that pushes 8 pixels
  into the background FIFO only when it is empty, one pixel out per dot,
  then one dot to mix (reads LCDC) and one to apply palettes (reads BGP/OBP,
  with the DMG `old | new` BGP quirk). Sprites pause the output by Pan
  Docs' penalty (6 dots, plus up to 5 for the first sprite on a tile); the
  window restarts the fetcher; WX below 7 is compared during the first
  fetch. Fitted constants: mode 3 starts at line M-cycle 21 with 3 of its
  dots in that cycle (Mooneye `intr_2_mode0_timing_sprites`), mode 0 starts
  with the last pixel, the fetcher reads on its dots 3/5/6 (Mealybug SCX/SCY
  tests), line 0's OAM scan is one cycle late. Do not "correct" them to Pan
  Docs without rerunning Mooneye `acceptance/ppu` and the Mealybug sweep
  (`cargo test -p glassbit-runner --test mealybug`, plus comparing all
  `*_dmg_blob.png` references with the runner's `--reference`). A
  16-pixel FIFO (push at <= 8) was tried and made Mealybug much worse.
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
- **Mappers:** ROM-only, MBC1 (no multicart), MBC2 (512x4-bit RAM; address
  bit 8 picks RAMG or ROMB), MBC3 (MBC30 inferred from sizes) and MBC5
  (rumble ignored).
- **MBC3 clock** (`cartridge/rtc.rs`): ticks with emulated time (1,048,576
  M-cycles per second of real time, so half-rate during double speed); the
  bus hands it cycles lazily (`sync_clock`) before cartridge accesses and at
  the end of `run_frame`. The core has no wall clock: `save_data_at(now)`
  appends the BGB/VBA 48-byte footer with a timestamp, and
  `load_save_at(data, now)` lets the clock catch up. The page passes
  `Date.now()` seconds.
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
  and passed to `Emulator::set_palette` (DMG shades) or
  `Emulator::set_compat_palettes` (compatibility mode). The Console menu
  passes `"auto"`, `"dmg"` or `"cgb"` to `new Emulator(rom, model)` and
  powers the game on again when changed.
- **Unused I/O registers read `0xFF` and ignore writes**; FF72-FF77 exist
  only on Color hardware (FF74 only in color mode).
- **Deploy:** `.github/workflows/pages.yml` publishes `crates/web/www` to
  GitHub Pages on every push to `main`
  (<https://matheussenra.github.io/glassbit/>).
- **README images:** `assets/demo.gif` is recorded with
  `glassbit-runner --gif ... --press ...`; `assets/xray.jpg` is a browser
  screenshot.
- **Breakpoints** live in the web `Recorder` observer (only while X-ray is
  open): `Observer::memory_write` sees every CPU write and
  `Observer::should_stop` makes `run_frame`/`step_line` return early. Execute
  breakpoints stop *before* the instruction at that address. OAM DMA copies
  are not CPU writes and do not trigger write breakpoints.
- **`.sav` import** restarts the game from the ROM bytes kept in JS
  (`currentRom`), because a running game would not notice its RAM changing.
- **Link cable:** `Serial` turns each transfer into `LinkMessage`s. The side
  that drives the clock sends `Data` and its transfer lasts until the
  `Reply` arrives (never less than the normal 1024 M-cycles); the other side
  holds incoming `Data` until its game arms an external-clock transfer, then
  answers. Both driving at once is resolved by each side's `Data` serving as
  the other's reply. `www/link.js` carries the messages over an
  `RTCDataChannel`; signaling is manual (copy-paste invite and answer codes,
  deflated SDP in base64), so there is no server of ours. Link state is not
  part of save states.
- **Game Boy Color:** `Model::for_header` picks the console: carts that
  declare Color support (0x80/0xC0) run in color mode, the rest on a DMG.
  `GameBoy::new_as(rom, Model::Cgb)` with a DMG cart gives compatibility
  mode (`bus::Hardware::CgbCompat`): the bus acts as a DMG (Color registers
  locked; VBK/BCPS/OCPS read what the boot ROM left), the PPU draws as a
  DMG and maps each shade through BG palette 0 / OBJ palettes 0-1 into the
  color framebuffer. Without the boot ROM there is no per-title palette
  hash: `ppu::DEFAULT_COMPAT_PALETTES` (the unknown-title colors) until
  `set_compat_palettes`. Post-boot CPU regs, I/O and the timer counter
  (`0x2674`) come from Mooneye `misc/` tests; PPU timing stays DMG timing
  (`vblank_stat_intr-C` and Mealybug `_cgb_c` references differ). Color state: VRAM 2x8 KiB (`Ppu`),
  WRAM 8x4 KiB, palette RAM, VBK/SVBK/KEY1/HDMA (`Bus::ColorState`). The
  color screen is a separate RGB555 framebuffer; `rgb_framebuffer()` gives
  RGB for either model (`x << 3 | x >> 2`, as the test references expect).
  Double speed ticks the PPU and APU every other CPU M-cycle. HBlank DMA
  copies a block when a visible line enters HBlank, without stalling the
  CPU; general DMA stalls it 8 M-cycles per block (16 at double speed).
  `cgb-acid-hell` is 2 pixels off (mid-scanline effects).
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
Mooneye acceptance tests, dmg-acid2, cgb-acid2, Mealybug Tearoom
(mid-scanline, `*_dmg_blob.png`; most failures draw the boot ROM's (R)
sprite, which is blank here), rtc3test.

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

## Not done yet

Color-specific PPU timing, window re-activation and mid-line window
re-enable quirks, per-title compatibility palettes, MBC1 multicart, wave RAM
access while channel 3 plays.
