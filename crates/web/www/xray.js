// The X-ray panel: shows what the emulated hardware is doing.
//
// Everything here only *reads* the emulator (registers, memory, event logs)
// and draws it. The panel is rebuilt from that data on every `render`, so
// it never holds state of its own beyond what is needed to highlight
// changes between two steps.

import { t, translatePage } from "./i18n.js";

const DOCS = "https://gbdev.io/pandocs/";
// The colors used for the four shades in the tile views; replaced when
// the user picks another palette.
let shades = [
  [0xe0, 0xf8, 0xd0],
  [0x88, 0xc0, 0x70],
  [0x34, 0x68, 0x56],
  [0x08, 0x18, 0x20],
];
const NEXT_INSTRUCTIONS = 8;
const LOG_ENTRIES = 8;

const REGISTER_NAMES = ["AF", "BC", "DE", "HL", "SP", "PC"];

// Hardware registers shown in the memory section, in address order.
const IO_REGISTERS = [
  [0xff00, "P1"],
  [0xff01, "SB"],
  [0xff02, "SC"],
  [0xff04, "DIV"],
  [0xff05, "TIMA"],
  [0xff06, "TMA"],
  [0xff07, "TAC"],
  [0xff0f, "IF"],
  [0xff24, "NR50"],
  [0xff25, "NR51"],
  [0xff26, "NR52"],
  [0xff40, "LCDC"],
  [0xff41, "STAT"],
  [0xff42, "SCY"],
  [0xff43, "SCX"],
  [0xff44, "LY"],
  [0xff45, "LYC"],
  [0xff46, "DMA"],
  [0xff47, "BGP"],
  [0xff48, "OBP0"],
  [0xff49, "OBP1"],
  [0xff4a, "WY"],
  [0xff4b, "WX"],
  [0xffff, "IE"],
];
const IO_NAME_BY_ADDRESS = new Map(IO_REGISTERS);

// [first address, last address, translation key]
const MEMORY_REGIONS = [
  [0x0000, 0x3fff, "mem.rom0"],
  [0x4000, 0x7fff, "mem.romN"],
  [0x8000, 0x9fff, "mem.vram"],
  [0xa000, 0xbfff, "mem.sram"],
  [0xc000, 0xdfff, "mem.wram"],
  [0xe000, 0xfdff, "mem.echo"],
  [0xfe00, 0xfe9f, "mem.oam"],
  [0xfea0, 0xfeff, "mem.unusable"],
  [0xff00, 0xff7f, "mem.io"],
  [0xff80, 0xfffe, "mem.hram"],
  [0xffff, 0xffff, "mem.ie"],
];

// [bit, translation key], in priority order.
const INTERRUPTS = [
  [0x01, "int.vblank"],
  [0x02, "int.stat"],
  [0x04, "int.timer"],
  [0x08, "int.serial"],
  [0x10, "int.joypad"],
];
const TIMER_HZ = [4096, 262144, 65536, 16384];

const hex8 = (value) => "$" + value.toString(16).toUpperCase().padStart(2, "0");
const hex16 = (value) => "$" + value.toString(16).toUpperCase().padStart(4, "0");

/** Decodes the instruction at `address`. */
function decode(emulator, address) {
  const [length, operation, operands] = emulator.disassemble(address).split("|");
  const args = operands ? operands.split(",") : [];
  const bytes = Array.from(emulator.read_range(address, Number(length)));
  return {
    address,
    length: Number(length),
    operation,
    args,
    text: args.length ? `${operation} ${args.join(",")}` : operation,
    bytes: bytes.map((byte) => byte.toString(16).toUpperCase().padStart(2, "0")).join(" "),
  };
}

/** A one-sentence description of what an instruction does. */
function explain({ operation, args }) {
  const conditional =
    (operation === "RET" && args.length === 1) ||
    (["JR", "JP", "CALL"].includes(operation) && args.length === 2);
  let text = conditional
    ? t(`op.${operation}.cond`, t(`cond.${args[0]}`), args[1])
    : t(`op.${operation}`, ...args);

  // Point out hardware registers by name: ($FF44) is LY.
  for (const arg of args) {
    const match = /^\(\$(FF[0-9A-F]{2})\)$/.exec(arg);
    const name = match && IO_NAME_BY_ADDRESS.get(parseInt(match[1], 16));
    if (name) text += t("op.registerNote", `$${match[1]}`, `${name} (${t(`io.${name}`)})`);
  }
  return text;
}

function element(tag, text, className) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  if (className) node.className = className;
  return node;
}

function row(cells, cellTag = "td") {
  const tr = document.createElement("tr");
  for (const cell of cells) {
    tr.append(cell instanceof Node ? cell : element(cellTag, String(cell)));
  }
  return tr;
}

function section(id, titleKey, docsPage, body) {
  return `
    <details class="xray-section" id="xray-${id}" open>
      <summary>
        <span data-i18n="${titleKey}"></span>
        <a href="${DOCS}${docsPage}" target="_blank" rel="noopener" data-i18n="xray.docs"></a>
      </summary>
      ${body}
    </details>`;
}

const TEMPLATE = `
  <h2 data-i18n="xray.title"></h2>
  <p class="note" data-i18n="xray.intro"></p>
  <div id="xray-controls">
    <button type="button" id="xray-pause"></button>
    <button type="button" id="xray-step-instruction" data-i18n="xray.stepInstruction"></button>
    <button type="button" id="xray-step-line" data-i18n="xray.stepLine"></button>
    <button type="button" id="xray-step-frame" data-i18n="xray.stepFrame"></button>
  </div>
  <p class="note" id="xray-state"></p>

  ${section(
    "cpu",
    "cpu.title",
    "CPU_Registers_and_Flags.html",
    `
    <h3 data-i18n="cpu.lastExecuted"></h3>
    <p id="xray-last" class="xray-last"></p>
    <h3 data-i18n="cpu.registers"></h3>
    <div id="xray-registers" class="xray-registers"></div>
    <ul id="xray-flags" class="xray-list"></ul>
    <h3 data-i18n="cpu.next"></h3>
    <table id="xray-disassembly" class="xray-code"></table>
    <p id="xray-next-explanation" class="note"></p>`,
  )}

  ${section(
    "ppu",
    "ppu.title",
    "Rendering.html",
    `
    <table id="xray-ppu-state" class="xray-table"></table>
    <h3 data-i18n="ppu.tilemap"></h3>
    <p class="note" data-i18n="ppu.tilemapHint"></p>
    <canvas id="xray-tilemap" width="256" height="256"></canvas>
    <h3 data-i18n="ppu.tiles"></h3>
    <p class="note" data-i18n="ppu.tilesHint"></p>
    <canvas id="xray-tiles" width="128" height="192"></canvas>
    <h3 data-i18n="ppu.sprites"></h3>
    <div id="xray-sprites"></div>`,
  )}

  ${section(
    "memory",
    "mem.title",
    "Memory_Map.html",
    `
    <h3 data-i18n="mem.map"></h3>
    <table id="xray-regions" class="xray-table"></table>
    <h3 data-i18n="mem.io.title"></h3>
    <table id="xray-io" class="xray-table"></table>
    <h3 data-i18n="mem.writes"></h3>
    <ul id="xray-writes" class="xray-list"></ul>`,
  )}

  ${section(
    "interrupts",
    "int.title",
    "Interrupts.html",
    `
    <p id="xray-ime" class="note"></p>
    <table id="xray-interrupt-table" class="xray-table"></table>
    <h3 data-i18n="int.log"></h3>
    <ul id="xray-interrupt-log" class="xray-list"></ul>
    <h3><span data-i18n="int.timer.title"></span>
      <a href="${DOCS}Timer_and_Divider_Registers.html" target="_blank" rel="noopener" data-i18n="xray.docs"></a></h3>
    <table id="xray-timer" class="xray-table"></table>
    <p id="xray-timer-state" class="note"></p>`,
  )}
`;

export class Xray {
  /**
   * @param root the element the panel lives in
   * @param actions `{ togglePause, stepInstruction, stepLine, stepFrame }`
   */
  constructor(root, actions) {
    this.root = root;
    this.open = false;
    // CPU state before the last step, to highlight what the step changed.
    this.previousState = null;
    // The instruction executed by the last single step.
    this.lastExecuted = null;

    root.innerHTML = TEMPLATE;
    translatePage(root);
    const on = (id, action) => root.querySelector(id).addEventListener("click", action);
    on("#xray-pause", actions.togglePause);
    on("#xray-step-instruction", actions.stepInstruction);
    on("#xray-step-line", actions.stepLine);
    on("#xray-step-frame", actions.stepFrame);
    this.tilemap = root.querySelector("#xray-tilemap").getContext("2d");
    this.tiles = root.querySelector("#xray-tiles").getContext("2d");
  }

  setOpen(open) {
    this.open = open;
    this.root.hidden = !open;
    this.forget();
  }

  /** Uses the same four colors as the game screen: `[[r, g, b], ...]`. */
  setShades(colors) {
    shades = colors;
  }

  /** Drops the step history, e.g. when another cartridge is loaded. */
  forget() {
    this.previousState = null;
    this.lastExecuted = null;
  }

  /** Call right before stepping, so the step's effects can be shown. */
  beforeStep(emulator, singleInstruction) {
    this.previousState = Array.from(emulator.cpu_state());
    const halted = this.previousState[7] === 1;
    this.lastExecuted =
      singleInstruction && !halted ? decode(emulator, this.previousState[5]) : null;
  }

  /** Re-applies translations after the language changes. */
  translate() {
    translatePage(this.root);
  }

  render(emulator, paused) {
    if (!this.open || !emulator) return;
    const $ = (id) => this.root.querySelector(id);
    $("#xray-pause").textContent = t(paused ? "button.resume" : "button.pause");
    $("#xray-state").textContent = t(paused ? "xray.paused" : "xray.running");
    // While running, a diff against some old step would be misleading.
    if (!paused) this.forget();

    const state = Array.from(emulator.cpu_state());
    if ($("#xray-cpu").open) this.renderCpu($, emulator, state);
    if ($("#xray-ppu").open) this.renderPpu($, emulator);
    if ($("#xray-memory").open) this.renderMemory($, emulator, state);
    if ($("#xray-interrupts").open) this.renderInterrupts($, emulator, state);
  }

  renderCpu($, emulator, state) {
    const last = $("#xray-last");
    last.replaceChildren();
    if (this.lastExecuted) {
      last.append(element("code", this.lastExecuted.text), " ", explain(this.lastExecuted));
    } else {
      last.textContent = t("cpu.none");
    }

    const registers = $("#xray-registers");
    registers.replaceChildren();
    REGISTER_NAMES.forEach((name, index) => {
      const changed = this.previousState && this.previousState[index] !== state[index];
      const cell = element("div", undefined, changed ? "changed" : "");
      cell.append(element("span", name), element("code", hex16(state[index])));
      registers.append(cell);
    });

    const flags = state[0] & 0xff;
    const previousFlags = this.previousState ? this.previousState[0] & 0xff : flags;
    const list = $("#xray-flags");
    list.replaceChildren();
    for (const [bit, key] of [
      [0x80, "cpu.flag.z"],
      [0x40, "cpu.flag.n"],
      [0x20, "cpu.flag.h"],
      [0x10, "cpu.flag.c"],
    ]) {
      const item = element("li", `${flags & bit ? "1" : "0"}  ${t(key)}`);
      if ((flags ^ previousFlags) & bit) item.className = "changed";
      list.append(item);
    }
    list.append(element("li", t("cpu.ime", t(state[6] ? "cpu.ime.on" : "cpu.ime.off"))));
    if (state[7]) list.append(element("li", t("cpu.halted")));

    const table = $("#xray-disassembly");
    table.replaceChildren();
    let address = state[5];
    let first = null;
    for (let i = 0; i < NEXT_INSTRUCTIONS; i++) {
      const instruction = decode(emulator, address);
      first ??= instruction;
      const line = row([hex16(address), instruction.bytes, instruction.text]);
      if (i === 0) line.className = "current";
      table.append(line);
      address = (address + instruction.length) & 0xffff;
    }
    $("#xray-next-explanation").textContent = `${first.text}: ${explain(first)}`;
  }

  renderPpu($, emulator) {
    const lcdc = emulator.peek(0xff40);
    const stat = emulator.peek(0xff41);
    const line = emulator.peek(0xff44);
    const scx = emulator.peek(0xff43);
    const scy = emulator.peek(0xff42);
    const lcdOn = (lcdc & 0x80) !== 0;

    const table = $("#xray-ppu-state");
    table.replaceChildren(
      row([t("ppu.mode"), lcdOn ? t(`ppu.mode.${stat & 3}`) : t("ppu.lcdOff")]),
      row([t("ppu.line"), `${line}  (LYC ${emulator.peek(0xff45)})`]),
      row([t("ppu.scroll"), `${scx}, ${scy}`]),
      row([t("ppu.window"), `${emulator.peek(0xff4b)}, ${emulator.peek(0xff4a)}`]),
      row(["LCDC", `${hex8(lcdc)}  ${lcdc.toString(2).padStart(8, "0")}`]),
    );

    const vram = emulator.read_range(0x8000, 0x2000);
    const palette = emulator.peek(0xff47);
    this.drawTiles(vram, palette);
    this.drawTilemap(vram, palette, lcdc, scx, scy);
    this.renderSprites($("#xray-sprites"), emulator, lcdc, line);
  }

  /** The color number (0-3) of one pixel of a tile. */
  static tilePixel(vram, tile, x, y) {
    const low = vram[tile * 16 + y * 2];
    const high = vram[tile * 16 + y * 2 + 1];
    const bit = 7 - x;
    return (((high >> bit) & 1) << 1) | ((low >> bit) & 1);
  }

  static plot(image, x, y, palette, color) {
    const shade = shades[(palette >> (color * 2)) & 3];
    const offset = (y * image.width + x) * 4;
    image.data[offset] = shade[0];
    image.data[offset + 1] = shade[1];
    image.data[offset + 2] = shade[2];
    image.data[offset + 3] = 255;
  }

  drawTiles(vram, palette) {
    const image = this.tiles.createImageData(128, 192);
    for (let tile = 0; tile < 384; tile++) {
      const left = (tile % 16) * 8;
      const top = Math.floor(tile / 16) * 8;
      for (let y = 0; y < 8; y++) {
        for (let x = 0; x < 8; x++) {
          Xray.plot(image, left + x, top + y, palette, Xray.tilePixel(vram, tile, x, y));
        }
      }
    }
    this.tiles.putImageData(image, 0, 0);
  }

  drawTilemap(vram, palette, lcdc, scx, scy) {
    const image = this.tilemap.createImageData(256, 256);
    const map = lcdc & 0x08 ? 0x1c00 : 0x1800;
    for (let mapRow = 0; mapRow < 32; mapRow++) {
      for (let column = 0; column < 32; column++) {
        let tile = vram[map + mapRow * 32 + column];
        // In the signed addressing mode, numbers 0-127 live after 128-255.
        if (!(lcdc & 0x10) && tile < 128) tile += 256;
        for (let y = 0; y < 8; y++) {
          for (let x = 0; x < 8; x++) {
            const color = Xray.tilePixel(vram, tile, x, y);
            Xray.plot(image, column * 8 + x, mapRow * 8 + y, palette, color);
          }
        }
      }
    }
    this.tilemap.putImageData(image, 0, 0);

    // The visible 160x144 area wraps around the edges of the map, so it is
    // drawn up to four times; the canvas clips what falls outside.
    this.tilemap.strokeStyle = "#ff4d6d";
    this.tilemap.lineWidth = 2;
    for (const dx of [0, -256]) {
      for (const dy of [0, -256]) {
        this.tilemap.strokeRect(scx + dx + 1, scy + dy + 1, 158, 142);
      }
    }
  }

  renderSprites(container, emulator, lcdc, line) {
    const oam = emulator.read_range(0xfe00, 160);
    const height = lcdc & 0x04 ? 16 : 8;
    const onLine = [];
    for (let slot = 0; slot < 40; slot++) {
      const y = oam[slot * 4];
      if (line + 16 >= y && line + 16 < y + height) onLine.push(slot);
    }
    container.replaceChildren();
    if (onLine.length === 0) {
      container.append(element("p", t("ppu.spritesNone", line), "note"));
      return;
    }
    const table = element("table", undefined, "xray-table");
    table.append(
      row([t("ppu.sprite.slot"), "X", "Y", t("ppu.sprite.tile"), t("ppu.sprite.flags")], "th"),
    );
    onLine.forEach((slot, index) => {
      const [y, x, tile, attributes] = oam.subarray(slot * 4, slot * 4 + 4);
      const flags = [
        attributes & 0x80 ? "behind" : "",
        attributes & 0x40 ? "flip Y" : "",
        attributes & 0x20 ? "flip X" : "",
        attributes & 0x10 ? "OBP1" : "OBP0",
      ].filter(Boolean);
      const tr = row([slot, x - 8, y - 16, hex8(tile), flags.join(", ")]);
      // Sprites past the tenth are on the line but never drawn.
      if (index >= 10) tr.className = "dropped";
      table.append(tr);
    });
    container.append(table);
    if (onLine.length > 10) container.append(element("p", t("ppu.spritesLimit"), "note"));
  }

  renderMemory($, emulator, state) {
    const pointers = REGISTER_NAMES.slice(1).map((name, index) => [name, state[index + 1]]);
    const regions = $("#xray-regions");
    regions.replaceChildren(row([t("mem.range"), t("mem.region"), t("mem.pointers")], "th"));
    for (const [first, last, key] of MEMORY_REGIONS) {
      let name = t(key);
      if (key === "mem.romN") name += ` (${t("mem.romBank", emulator.rom_bank())})`;
      const here = pointers.filter(([, value]) => value >= first && value <= last);
      regions.append(
        row([
          first === last ? hex16(first) : `${hex16(first)}-${hex16(last)}`,
          name,
          here.map(([pointer]) => pointer).join(" "),
        ]),
      );
    }

    const io = $("#xray-io");
    io.replaceChildren(row(["", t("mem.io.name"), t("mem.io.value"), t("mem.io.meaning")], "th"));
    for (const [address, name] of IO_REGISTERS) {
      io.append(row([hex16(address), name, hex8(emulator.peek(address)), t(`io.${name}`)]));
    }

    const writes = $("#xray-writes");
    writes.replaceChildren();
    const log = emulator.io_write_log();
    for (let i = log.length - 3; i >= 0 && writes.children.length < LOG_ENTRIES; i -= 3) {
      const name = IO_NAME_BY_ADDRESS.get(log[i]);
      const label = name ? `${hex16(log[i])} ${name}` : hex16(log[i]);
      writes.append(element("li", `${label}: ${hex8(log[i + 1])} → ${hex8(log[i + 2])}`));
    }
    if (writes.children.length === 0) writes.append(element("li", t("mem.writesNone")));
  }

  renderInterrupts($, emulator, state) {
    const enabled = emulator.peek(0xffff);
    const requested = emulator.peek(0xff0f);
    $("#xray-ime").textContent = t("cpu.ime", t(state[6] ? "cpu.ime.on" : "cpu.ime.off"));

    const table = $("#xray-interrupt-table");
    table.replaceChildren(
      row([t("int.source"), t("int.vector"), t("int.enabled"), t("int.requested")], "th"),
    );
    INTERRUPTS.forEach(([bit, key], index) => {
      const mark = (set) => element("td", set ? "●" : "○", set ? "on" : "off");
      table.append(row([t(key), hex16(0x40 + index * 8), mark(enabled & bit), mark(requested & bit)]));
    });

    const list = $("#xray-interrupt-log");
    list.replaceChildren();
    const log = emulator.interrupt_log();
    for (let i = log.length - 3; i >= 0 && list.children.length < LOG_ENTRIES; i -= 3) {
      const key = INTERRUPTS.find(([bit]) => bit === log[i])?.[1] ?? "int.source";
      list.append(element("li", t("int.logEntry", t(key), log[i + 1], log[i + 2])));
    }
    if (list.children.length === 0) list.append(element("li", t("int.logNone")));

    const timer = $("#xray-timer");
    timer.replaceChildren();
    for (const [address, name] of IO_REGISTERS.slice(3, 7)) {
      timer.append(row([name, hex8(emulator.peek(address)), t(`io.${name}`)]));
    }
    const tac = emulator.peek(0xff07);
    $("#xray-timer-state").textContent =
      tac & 0x04 ? t("int.timer.state", TIMER_HZ[tac & 3]) : t("int.timer.off");
  }
}
