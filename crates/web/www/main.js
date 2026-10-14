// Glassbit web frontend: canvas, frame loop, keyboard, sound, ROM loading
// and the X-ray panel. All emulation happens in the wasm module; this file
// is only glue.

import init, { Emulator, core_version, screen_height, screen_width } from "./pkg/glassbit_web.js";
import { AudioOutput } from "./audio.js";
import { LANGUAGES, getLanguage, onLanguageChange, setLanguage, t, translatePage } from "./i18n.js";
import { LinkCable } from "./link.js";
import { readSave, readState, writeSave, writeState } from "./saves.js";
import { Xray } from "./xray.js";

// The Game Boy draws 59.73 frames per second (4194304 Hz / 70224 dots).
const FRAME_MS = 1000 / (4194304 / 70224);
// After a long pause (hidden tab, breakpoint) do not try to catch up.
const MAX_CATCH_UP_MS = 100;
// How often to check whether the game wrote to its battery-backed RAM.
const SAVE_CHECK_MS = 1000;
// While the game runs, the X-ray panel is redrawn once every this many
// frames (about 10 times per second): enough to read, cheap to produce.
const XRAY_REFRESH_FRAMES = 6;

// How many times faster the game runs while fast-forwarding.
const FAST_FORWARD = 4;

// Screen colors, as red, green and blue for each shade from lightest to
// darkest. The first one is the default.
const PALETTES = {
  green: [[0xe0, 0xf8, 0xd0], [0x88, 0xc0, 0x70], [0x34, 0x68, 0x56], [0x08, 0x18, 0x20]],
  gray: [[0xff, 0xff, 0xff], [0xaa, 0xaa, 0xaa], [0x55, 0x55, 0x55], [0x00, 0x00, 0x00]],
  pocket: [[0xc4, 0xcf, 0xa1], [0x8b, 0x95, 0x6d], [0x4d, 0x53, 0x3c], [0x1f, 0x1f, 0x1f]],
  amber: [[0xff, 0xe8, 0xb0], [0xf0, 0xa8, 0x50], [0x98, 0x50, 0x20], [0x28, 0x10, 0x08]],
};
const PALETTE_STORAGE_KEY = "glassbit-palette";

// Colors for DMG games on a Game Boy Color (compatibility mode): a palette
// for the background, then one for each sprite palette (OBP0, OBP1), each
// from lightest to darkest. The real boot ROM chooses by game title; these
// are the colors it uses when it does not know the game, followed by some
// it lets the player pick with the buttons at power on.
const COMPAT_PALETTES = {
  standard: [
    [[0xff, 0xff, 0xff], [0x7b, 0xff, 0x31], [0x00, 0x63, 0xc6], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0xff, 0x84, 0x84], [0x94, 0x39, 0x39], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0xff, 0x84, 0x84], [0x94, 0x39, 0x39], [0x00, 0x00, 0x00]],
  ],
  brown: [
    [[0xff, 0xff, 0xff], [0xff, 0xad, 0x63], [0x84, 0x31, 0x00], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0xff, 0xad, 0x63], [0x84, 0x31, 0x00], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0xff, 0xad, 0x63], [0x84, 0x31, 0x00], [0x00, 0x00, 0x00]],
  ],
  blue: [
    [[0xff, 0xff, 0xff], [0x63, 0xa5, 0xff], [0x00, 0x00, 0xff], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0xff, 0x84, 0x84], [0x94, 0x39, 0x39], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0x7b, 0xff, 0x31], [0x00, 0x84, 0x00], [0x00, 0x00, 0x00]],
  ],
  gray: [
    [[0xff, 0xff, 0xff], [0xa5, 0xa5, 0xa5], [0x52, 0x52, 0x52], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0xa5, 0xa5, 0xa5], [0x52, 0x52, 0x52], [0x00, 0x00, 0x00]],
    [[0xff, 0xff, 0xff], [0xa5, 0xa5, 0xa5], [0x52, 0x52, 0x52], [0x00, 0x00, 0x00]],
  ],
  inverted: [
    [[0x00, 0x00, 0x00], [0x00, 0x84, 0x84], [0xff, 0xde, 0x00], [0xff, 0xff, 0xff]],
    [[0x00, 0x00, 0x00], [0x00, 0x84, 0x84], [0xff, 0xde, 0x00], [0xff, 0xff, 0xff]],
    [[0x00, 0x00, 0x00], [0x00, 0x84, 0x84], [0xff, 0xde, 0x00], [0xff, 0xff, 0xff]],
  ],
};
const COMPAT_PALETTE_STORAGE_KEY = "glassbit-compat-palette";

// Which console runs the games: "auto" lets each game choose (color games
// on a Game Boy Color, the rest on a DMG).
const MODELS = ["auto", "dmg", "cgb"];
const MODEL_STORAGE_KEY = "glassbit-model";

// Indices match `Button::ALL` in the core.
const KEY_TO_BUTTON = {
  ArrowRight: 0,
  ArrowLeft: 1,
  ArrowUp: 2,
  ArrowDown: 3,
  KeyX: 4, // A
  KeyZ: 5, // B
  ShiftLeft: 6, // Select
  ShiftRight: 6,
  Enter: 7, // Start
};

const canvas = document.getElementById("screen");
const statusLine = document.getElementById("status");
const saveLine = document.getElementById("save-status");
const pauseButton = document.getElementById("pause");
const muteButton = document.getElementById("mute");
const xrayButton = document.getElementById("xray-toggle");
const fastButton = document.getElementById("fast");
const exportSaveButton = document.getElementById("export-save");
const linkButton = document.getElementById("link-toggle");
const linkDialog = document.getElementById("link-dialog");
const importSaveLabel = document.getElementById("import-save-label");
const paletteSelect = document.getElementById("palette");
const modelSelect = document.getElementById("model");
const languageSelect = document.getElementById("language");
const screenFrame = document.getElementById("screen-frame");
const dropHint = document.getElementById("drop-hint");

translatePage();
await init();

const context = canvas.getContext("2d");
const image = context.createImageData(screen_width(), screen_height());

const audio = new AudioOutput();
let emulator = null;
let paused = false;
let lastTime = null;
let pendingMs = 0;
let framesSinceXray = 0;
// Fast-forward is on while the key is held or after the button is clicked.
let fastHeld = false;
let fastLocked = false;
let paletteName = "green";
let compatPaletteName = "standard";
let modelName = "auto";
// The ROM of the running game, needed to restart it with an imported save;
// null for the built-in demo.
let currentRom = null;

// Status lines are kept as translation keys so they can follow a language
// change: `[key, ...arguments]`.
let status = ["status.loading"];
let statusIsError = false;
let saveStatus = null;

function showStatus() {
  statusLine.textContent = t(...status);
  statusLine.classList.toggle("error", statusIsError);
  saveLine.textContent = saveStatus ? t(...saveStatus) : "";
}

function setStatus(message, isError = false) {
  status = message;
  statusIsError = isError;
  showStatus();
}

function setSaveStatus(message) {
  saveStatus = message;
  showStatus();
}

// The current time in whole seconds since 1970, for cartridge clocks.
const now = () => Math.floor(Date.now() / 1000);

// Writes the battery save to IndexedDB if the game changed it.
async function storeSave(target = emulator) {
  if (!target?.take_save_dirty()) return;
  try {
    await writeSave(target.save_key(), target.save_data(now()));
    setSaveStatus(["save.stored", new Date().toLocaleTimeString()]);
  } catch (error) {
    setSaveStatus(["save.storeError", error.message ?? String(error)]);
  }
}

// Replaces the running emulator. `keepPreviousSave: false` skips storing
// the old cartridge's save, for when a save was just imported over it.
async function start(newEmulator, message, { keepPreviousSave = true } = {}) {
  const previous = emulator;
  // Stop the old cartridge first, so nothing runs while its save is stored.
  emulator = null;
  if (previous) {
    if (keepPreviousSave) await storeSave(previous);
    // Free the wasm memory of the cartridge being replaced.
    previous.free();
  }

  if (newEmulator.has_battery()) {
    try {
      const saved = await readSave(newEmulator.save_key());
      if (saved) newEmulator.load_save(saved, now());
      setSaveStatus([saved ? "save.restored" : "save.available"]);
    } catch (error) {
      setSaveStatus(["save.unavailable", error.message ?? String(error)]);
    }
  } else {
    setSaveStatus(null);
  }

  newEmulator.set_sample_rate(audio.sampleRate);
  newEmulator.set_palette(new Uint8Array(PALETTES[paletteName].flat()));
  newEmulator.set_compat_palettes(new Uint8Array(COMPAT_PALETTES[compatPaletteName].flat(2)));
  newEmulator.set_xray(xray.open);
  newEmulator.set_breakpoints(...xray.breakpointArrays());
  newEmulator.set_linked(link.connected);
  exportSaveButton.hidden = !newEmulator.has_battery();
  importSaveLabel.hidden = !newEmulator.has_battery();
  xray.setStopReason(null);
  emulator = newEmulator;
  fillPaletteSelect();
  showPalette();
  pendingMs = 0;
  xray.forget();
  setPaused(false);
  setStatus(message);
  canvas.focus();
}

async function loadRom(bytes, name) {
  let loaded;
  try {
    loaded = new Emulator(bytes, modelName);
  } catch (error) {
    // Keep whatever was running; just report the problem.
    setStatus(["status.loadError", name, error.message], true);
    return;
  }
  currentRom = { bytes, name };
  await start(loaded, ["status.rom", loaded.title() || name, name]);
}

async function loadFile(file) {
  if (!file) return;
  await loadRom(new Uint8Array(await file.arrayBuffer()), file.name);
}

function setPaused(value) {
  paused = value;
  pauseButton.textContent = t(paused ? "button.resume" : "button.pause");
  xray.render(emulator, paused);
}

function draw() {
  emulator.copy_frame(image.data);
  context.putImageData(image, 0, 0);
}

// The display refreshes at 60, 120 or 144 Hz, never at the Game Boy's 59.73.
// So instead of one emulated frame per callback, real time is accumulated
// and spent in whole emulated frames.
function onAnimationFrame(now) {
  requestAnimationFrame(onAnimationFrame);
  const elapsed = lastTime === null ? 0 : now - lastTime;
  lastTime = now;
  if (!emulator || paused) return;

  // With sound on, the sound card's clock is the one that matters: the
  // emulator runs up to 2% faster or slower to keep the audio queue steady.
  // Fast-forward simply pretends more time went by, and plays no sound:
  // four times the audio would not fit in the same real time.
  const fast = fastHeld || fastLocked;
  const speed = fast ? FAST_FORWARD : audio.speed();
  pendingMs = Math.min(pendingMs + elapsed * speed, MAX_CATCH_UP_MS * speed);
  let ran = false;
  while (pendingMs >= FRAME_MS) {
    emulator.run_frame();
    link.send(emulator.take_link_messages());
    if (audio.running && !fast) audio.push(emulator.audio_samples());
    pendingMs -= FRAME_MS;
    framesSinceXray++;
    ran = true;
    const hit = emulator.take_breakpoint_hit();
    if (hit) {
      stopAtBreakpoint(hit);
      return;
    }
  }
  if (!ran) return;
  draw();
  if (xray.open && framesSinceXray >= XRAY_REFRESH_FRAMES) {
    framesSinceXray = 0;
    xray.render(emulator, false);
  }
}

// ---- Link cable ------------------------------------------------------

const link = new LinkCable({
  onMessage: (bytes) => emulator?.receive_link_messages(bytes),
  onStateChange: (state) => {
    emulator?.set_linked(state === "connected");
    showLinkState();
  },
});

function showLinkState() {
  const state = document.getElementById("link-state");
  state.textContent = t(`link.state.${link.state}`);
  state.classList.toggle("connected", link.connected);
  linkButton.textContent = t(link.connected ? "link.buttonConnected" : "link.button");
  if (link.state === "closed" || link.state === "idle") {
    document.getElementById("link-host-steps").hidden = true;
    document.getElementById("link-join-steps").hidden = true;
  }
}

// Runs one step of the connection setup, reporting failures in the dialog.
async function linkStep(action) {
  try {
    await action();
  } catch (error) {
    document.getElementById("link-state").textContent = t("link.error", error.message ?? String(error));
  }
}

function copyField(id) {
  const field = document.getElementById(id);
  field.select();
  navigator.clipboard?.writeText(field.value).then(
    () => (document.getElementById("link-state").textContent = t("link.copied")),
    () => {},
  );
}

linkButton.addEventListener("click", () => {
  showLinkState();
  linkDialog.showModal();
});
document.getElementById("link-close").addEventListener("click", () => {
  linkDialog.close();
  canvas.focus();
});
document.getElementById("link-disconnect").addEventListener("click", () => link.close());
document.getElementById("link-host").addEventListener("click", () =>
  linkStep(async () => {
    document.getElementById("link-state").textContent = t("link.preparing");
    document.getElementById("link-invite").value = await link.host();
    document.getElementById("link-host-steps").hidden = false;
    showLinkState();
  }),
);
document.getElementById("link-connect").addEventListener("click", () =>
  linkStep(() => link.accept(document.getElementById("link-answer-in").value)),
);
document.getElementById("link-join").addEventListener("click", () =>
  linkStep(async () => {
    document.getElementById("link-state").textContent = t("link.preparing");
    const invite = document.getElementById("link-invite-in").value;
    document.getElementById("link-answer").value = await link.join(invite);
    document.getElementById("link-join-steps").hidden = false;
    showLinkState();
  }),
);
document.getElementById("link-copy-invite").addEventListener("click", () => copyField("link-invite"));
document.getElementById("link-copy-answer").addEventListener("click", () => copyField("link-answer"));

// Pauses the game where a breakpoint stopped it and shows why.
function stopAtBreakpoint(report) {
  xray.setStopReason(report);
  draw();
  setPaused(true);
}

// Pauses and advances the emulator by one unit, then shows the result.
function step(advance, singleInstruction = false) {
  if (!emulator) return;
  paused = true;
  xray.setStopReason(null);
  xray.beforeStep(emulator, singleInstruction);
  advance(emulator);
  link.send(emulator.take_link_messages());
  // A line or frame step can also end at a breakpoint.
  xray.setStopReason(emulator.take_breakpoint_hit() ?? null);
  draw();
  setPaused(true);
}

const xray = new Xray(document.getElementById("xray"), {
  togglePause: () => setPausedByUser(!paused),
  stepInstruction: () => step((machine) => machine.step_instruction(), true),
  stepLine: () => step((machine) => machine.step_line()),
  stepFrame: () => step((machine) => machine.run_frame()),
  breakpointsChanged: (execute, write) => emulator?.set_breakpoints(execute, write),
  refresh: () => xray.render(emulator, paused),
});

function setXrayOpen(open) {
  xray.setOpen(open);
  // Recording costs a little, so it only happens while the panel is open.
  emulator?.set_xray(open);
  document.body.classList.toggle("xray-open", open);
  xrayButton.textContent = t(open ? "button.xrayClose" : "button.xray");
  xrayButton.setAttribute("aria-expanded", String(open));
  xray.render(emulator, paused);
}

// Remembers a choice for the next visit.
function remember(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not being able to remember the choice is harmless.
  }
}

// The colors menu lists DMG shades, or the Color's palettes for a DMG game
// in compatibility mode. A color game brings its own colors.
function compatColors() {
  return emulator?.is_compat() ?? false;
}

function setPalette(name) {
  if (compatColors()) {
    if (!(name in COMPAT_PALETTES)) return;
    compatPaletteName = name;
    emulator.set_compat_palettes(new Uint8Array(COMPAT_PALETTES[name].flat(2)));
    remember(COMPAT_PALETTE_STORAGE_KEY, name);
  } else {
    if (!(name in PALETTES)) return;
    paletteName = name;
    emulator?.set_palette(new Uint8Array(PALETTES[name].flat()));
    remember(PALETTE_STORAGE_KEY, name);
  }
  showPalette();
}

// Brings the X-ray tile views and the canvas background in line with the
// colors in use.
function showPalette() {
  const shades = compatColors() ? COMPAT_PALETTES[compatPaletteName][0] : PALETTES[paletteName];
  xray.setShades(shades);
  // The page behind the canvas shows through before the first frame.
  canvas.style.background = `rgb(${shades[0].join(",")})`;
  if (emulator) draw();
  xray.render(emulator, paused);
}

// Powers the current game (or the demo) on again on the chosen console.
async function setModel(name) {
  if (!MODELS.includes(name)) return;
  modelName = name;
  remember(MODEL_STORAGE_KEY, name);
  if (currentRom) {
    await loadRom(currentRom.bytes, currentRom.name);
  } else {
    await start(Emulator.demo(modelName), ["status.demo"]);
  }
}

function showFastButton() {
  fastButton.textContent = t(fastLocked ? "button.fastOff" : "button.fast");
  fastButton.setAttribute("aria-pressed", String(fastLocked));
}

// Stores a snapshot of the whole machine for this cartridge.
async function saveState() {
  if (!emulator) return;
  try {
    await writeState(emulator.save_key(), emulator.save_state());
    setSaveStatus(["state.saved", new Date().toLocaleTimeString()]);
  } catch (error) {
    setSaveStatus(["state.error", error.message ?? String(error)]);
  }
  canvas.focus();
}

// Brings the machine back to the stored snapshot.
async function loadState() {
  if (!emulator) return;
  const target = emulator;
  // If the stored state turns out to be damaged, loading can leave the
  // machine half restored; this snapshot puts it back as it was.
  const fallback = target.save_state();
  try {
    const stored = await readState(target.save_key());
    if (emulator !== target) return;
    if (!stored) {
      setSaveStatus(["state.none"]);
      return;
    }
    target.load_state(stored);
    releaseAllButtons();
    xray.forget();
    draw();
    xray.render(target, paused);
    setSaveStatus(["state.loaded"]);
  } catch (error) {
    if (emulator === target) target.load_state(fallback);
    setSaveStatus(["state.error", error.message ?? String(error)]);
  }
  canvas.focus();
}

// Browsers refuse to play sound until the user interacts with the page.
async function enableAudio() {
  await audio.start();
  emulator?.set_sample_rate(audio.sampleRate);
}

function onKey(event, pressed) {
  const button = KEY_TO_BUTTON[event.code];
  const isFastKey = event.code === "Space";
  if ((button === undefined && !isFastKey) || event.ctrlKey || event.altKey || event.metaKey) {
    return;
  }
  // Leave keys alone while the user is operating a control.
  const target = event.target;
  if (
    target instanceof HTMLButtonElement ||
    target instanceof HTMLInputElement ||
    target instanceof HTMLSelectElement ||
    target instanceof HTMLAnchorElement ||
    target instanceof HTMLElement && target.tagName === "SUMMARY"
  ) {
    return;
  }
  // Otherwise the arrow keys and the space bar would scroll the page.
  event.preventDefault();
  if (isFastKey) {
    fastHeld = pressed;
  } else {
    emulator?.set_button(button, pressed);
  }
}

function releaseAllButtons() {
  fastHeld = false;
  for (let button = 0; button < 8; button++) emulator?.set_button(button, false);
}

for (const gesture of ["keydown", "pointerdown", "drop"]) {
  window.addEventListener(gesture, enableAudio, { capture: true });
}

window.addEventListener("keydown", (event) => onKey(event, true));
window.addEventListener("keyup", (event) => onKey(event, false));
// A key released while the window is not focused never sends keyup.
window.addEventListener("blur", releaseAllButtons);

setInterval(storeSave, SAVE_CHECK_MS);
// Last chance before the tab is closed or put to sleep.
window.addEventListener("pagehide", () => storeSave());
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") storeSave();
});

document.getElementById("rom-input").addEventListener("change", (event) => {
  loadFile(event.target.files[0]);
  // Allow picking the same file again.
  event.target.value = "";
});

function setPausedByUser(value) {
  if (!value) xray.setStopReason(null);
  setPaused(value);
}

pauseButton.addEventListener("click", () => {
  setPausedByUser(!paused);
  canvas.focus();
});

// Downloads the cartridge RAM as a .sav file, the format most emulators use.
function exportSave() {
  const data = emulator?.save_data(now());
  if (!data) {
    setSaveStatus(["save.noBattery"]);
    return;
  }
  const base = (currentRom?.name ?? emulator.title()).replace(/\.[^.]*$/, "");
  const fileName = `${base}.sav`;
  const link = document.createElement("a");
  link.href = URL.createObjectURL(new Blob([data], { type: "application/octet-stream" }));
  link.download = fileName;
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 1000);
  setSaveStatus(["save.exported", fileName]);
  canvas.focus();
}

// Restarts the game with the RAM from a .sav file. A running game would
// not notice its RAM changing under it, so it is powered on again.
async function importSave(file) {
  if (!file) return;
  if (!currentRom) {
    setSaveStatus(["save.needRom"]);
    return;
  }
  try {
    const fresh = new Emulator(currentRom.bytes, modelName);
    if (!fresh.load_save(new Uint8Array(await file.arrayBuffer()), now())) {
      fresh.free();
      setSaveStatus(["save.noBattery"]);
      return;
    }
    await writeSave(fresh.save_key(), fresh.save_data(now()));
    await start(fresh, ["status.rom", fresh.title() || currentRom.name, currentRom.name], {
      keepPreviousSave: false,
    });
    setSaveStatus(["save.imported", file.name]);
  } catch (error) {
    setSaveStatus(["save.importError", file.name, error.message ?? String(error)]);
  }
}

exportSaveButton.addEventListener("click", exportSave);
document.getElementById("import-save").addEventListener("change", (event) => {
  importSave(event.target.files[0]);
  event.target.value = "";
});

muteButton.addEventListener("click", () => {
  audio.setMuted(!audio.muted);
  muteButton.textContent = t(audio.muted ? "button.unmute" : "button.mute");
  canvas.focus();
});

xrayButton.addEventListener("click", () => setXrayOpen(!xray.open));

document.getElementById("save-state").addEventListener("click", saveState);
document.getElementById("load-state").addEventListener("click", loadState);

fastButton.addEventListener("click", () => {
  fastLocked = !fastLocked;
  showFastButton();
  canvas.focus();
});

function fillPaletteSelect() {
  paletteSelect.replaceChildren();
  const compat = compatColors();
  const [list, current, prefix] = compat
    ? [COMPAT_PALETTES, compatPaletteName, "compatPalette"]
    : [PALETTES, paletteName, "palette"];
  for (const name of Object.keys(list)) {
    paletteSelect.append(new Option(t(`${prefix}.${name}`), name, false, name === current));
  }
  paletteSelect.disabled = (emulator?.is_cgb() ?? false) && !compat;
}
paletteSelect.addEventListener("change", () => {
  setPalette(paletteSelect.value);
  canvas.focus();
});

function fillModelSelect() {
  modelSelect.replaceChildren();
  for (const name of MODELS) {
    modelSelect.append(new Option(t(`model.${name}`), name, false, name === modelName));
  }
}
modelSelect.addEventListener("change", () => setModel(modelSelect.value));

document.getElementById("demo").addEventListener("click", () => {
  currentRom = null;
  start(Emulator.demo(modelName), ["status.demo"]);
});

for (const code of LANGUAGES) {
  languageSelect.append(new Option(code.toUpperCase(), code, false, code === getLanguage()));
}
languageSelect.addEventListener("change", () => setLanguage(languageSelect.value));
onLanguageChange(() => {
  translatePage();
  xray.translate();
  showStatus();
  pauseButton.textContent = t(paused ? "button.resume" : "button.pause");
  muteButton.textContent = t(audio.muted ? "button.unmute" : "button.mute");
  xrayButton.textContent = t(xray.open ? "button.xrayClose" : "button.xray");
  showFastButton();
  fillPaletteSelect();
  fillModelSelect();
  showLinkState();
  xray.render(emulator, paused);
});

// Drag and drop anywhere on the page.
let dragDepth = 0;
function setDragging(active) {
  screenFrame.classList.toggle("dragging", active);
  dropHint.hidden = !active;
}
window.addEventListener("dragenter", (event) => {
  event.preventDefault();
  dragDepth++;
  setDragging(true);
});
window.addEventListener("dragleave", () => {
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) setDragging(false);
});
window.addEventListener("dragover", (event) => event.preventDefault());
window.addEventListener("drop", (event) => {
  event.preventDefault();
  dragDepth = 0;
  setDragging(false);
  loadFile(event.dataTransfer.files[0]);
});

try {
  const stored = localStorage.getItem(PALETTE_STORAGE_KEY);
  if (stored in PALETTES) paletteName = stored;
  const storedCompat = localStorage.getItem(COMPAT_PALETTE_STORAGE_KEY);
  if (storedCompat in COMPAT_PALETTES) compatPaletteName = storedCompat;
  const storedModel = localStorage.getItem(MODEL_STORAGE_KEY);
  if (MODELS.includes(storedModel)) modelName = storedModel;
} catch {
  // Storage can be blocked; the defaults are fine.
}
fillPaletteSelect();
fillModelSelect();
showPalette();

await start(Emulator.demo(modelName), ["status.demo"]);
document.title = `Glassbit ${core_version()}`;
requestAnimationFrame(onAnimationFrame);

// A handle for poking at the emulator from the browser console.
window.glassbit = {
  get emulator() {
    return emulator;
  },
  audio,
  xray,
  loadRom,
  setXrayOpen,
  saveState,
  loadState,
  setPalette,
  setModel,
  exportSave,
  importSave,
  link,
  stopAtBreakpoint,
};
