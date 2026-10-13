// Glassbit web frontend: canvas, frame loop, keyboard, sound and ROM loading.
// All emulation happens in the wasm module; this file is only glue.

import init, { Emulator, core_version, screen_height, screen_width } from "./pkg/glassbit_web.js";
import { AudioOutput } from "./audio.js";
import { readSave, writeSave } from "./saves.js";

// The Game Boy draws 59.73 frames per second (4194304 Hz / 70224 dots).
const FRAME_MS = 1000 / (4194304 / 70224);
// After a long pause (hidden tab, breakpoint) do not try to catch up.
const MAX_CATCH_UP_MS = 100;
// How often to check whether the game wrote to its battery-backed RAM.
const SAVE_CHECK_MS = 1000;

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
const screenFrame = document.getElementById("screen-frame");
const dropHint = document.getElementById("drop-hint");

await init();

const context = canvas.getContext("2d");
const image = context.createImageData(screen_width(), screen_height());

const audio = new AudioOutput();
let emulator = null;
let paused = false;
let lastTime = null;
let pendingMs = 0;

function setStatus(text, isError = false) {
  statusLine.textContent = text;
  statusLine.classList.toggle("error", isError);
}

// Writes the battery save to IndexedDB if the game changed it.
async function storeSave(target = emulator) {
  if (!target?.take_save_dirty()) return;
  try {
    await writeSave(target.save_key(), target.save_data());
    saveLine.textContent = `Game save stored in this browser at ${new Date().toLocaleTimeString()}.`;
  } catch (error) {
    saveLine.textContent = `Could not store the game save: ${error.message ?? error}`;
  }
}

async function start(newEmulator, description) {
  const previous = emulator;
  // Stop the old cartridge first, so nothing runs while its save is stored.
  emulator = null;
  if (previous) {
    await storeSave(previous);
    // Free the wasm memory of the cartridge being replaced.
    previous.free();
  }

  if (newEmulator.has_battery()) {
    try {
      const saved = await readSave(newEmulator.save_key());
      if (saved) newEmulator.load_save(saved);
      saveLine.textContent = saved
        ? "Game save restored from this browser."
        : "This cartridge can save; progress is kept in this browser.";
    } catch (error) {
      saveLine.textContent = `Saves are unavailable: ${error.message ?? error}`;
    }
  } else {
    saveLine.textContent = "";
  }

  newEmulator.set_sample_rate(audio.sampleRate);
  emulator = newEmulator;
  pendingMs = 0;
  setPaused(false);
  setStatus(description);
  canvas.focus();
}

async function loadRom(bytes, name) {
  let loaded;
  try {
    loaded = new Emulator(bytes);
  } catch (error) {
    // Keep whatever was running; just report the problem.
    setStatus(`Could not load ${name}: ${error.message}`, true);
    return;
  }
  await start(loaded, `${loaded.title() || name} (${name})`);
}

async function loadFile(file) {
  if (!file) return;
  await loadRom(new Uint8Array(await file.arrayBuffer()), file.name);
}

function setPaused(value) {
  paused = value;
  pauseButton.textContent = paused ? "Resume" : "Pause";
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
  pendingMs = Math.min(pendingMs + elapsed * audio.speed(), MAX_CATCH_UP_MS);
  let ran = false;
  while (pendingMs >= FRAME_MS) {
    emulator.run_frame();
    if (audio.running) audio.push(emulator.audio_samples());
    pendingMs -= FRAME_MS;
    ran = true;
  }
  if (ran) draw();
}

// Browsers refuse to play sound until the user interacts with the page.
async function enableAudio() {
  await audio.start();
  emulator?.set_sample_rate(audio.sampleRate);
}

function onKey(event, pressed) {
  const button = KEY_TO_BUTTON[event.code];
  if (button === undefined || event.ctrlKey || event.altKey || event.metaKey) return;
  // Leave keys alone while the user is operating a button or the file picker.
  if (event.target instanceof HTMLButtonElement || event.target instanceof HTMLInputElement) return;
  // Otherwise the arrow keys would scroll the page.
  event.preventDefault();
  emulator?.set_button(button, pressed);
}

function releaseAllButtons() {
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

pauseButton.addEventListener("click", () => {
  setPaused(!paused);
  canvas.focus();
});

muteButton.addEventListener("click", () => {
  audio.setMuted(!audio.muted);
  muteButton.textContent = audio.muted ? "Unmute" : "Mute";
  canvas.focus();
});

document.getElementById("demo").addEventListener("click", () => {
  start(Emulator.demo(), "Built-in demo: move the ball with the arrow keys, hold X to invert.");
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

await start(Emulator.demo(), "Built-in demo: move the ball with the arrow keys, hold X to invert.");
document.title = `Glassbit ${core_version()}`;
requestAnimationFrame(onAnimationFrame);

// A handle for poking at the emulator from the browser console.
window.glassbit = {
  get emulator() {
    return emulator;
  },
  audio,
  loadRom,
};
