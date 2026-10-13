// Sound output through Web Audio.
//
// Samples produced by the emulator are posted to an AudioWorklet (see
// audio-worklet.js), which queues and plays them. This module also watches
// how full that queue is and tells the frame loop to run a hair faster or
// slower, so the emulator keeps pace with the sound card's clock.

// How much audio to keep queued. Less means lower latency but more risk of
// running dry when a frame arrives late.
const TARGET_SECONDS = 0.07;
// The emulation speed never changes by more than this fraction: far too
// little to see or hear, enough to cancel the drift between the two clocks.
const MAX_CORRECTION = 0.02;

export class AudioOutput {
  constructor() {
    this.context = null;
    this.node = null;
    this.gain = null;
    this.muted = false;
    // Frames queued in the worklet, as last reported plus what was sent since.
    this.buffered = 0;
    this.underruns = 0;
    this.starting = null;
  }

  /** Whether samples pushed now would be played. */
  get running() {
    return this.node !== null && this.context.state === "running";
  }

  get sampleRate() {
    return this.context?.sampleRate ?? 48000;
  }

  /**
   * Creates the audio pipeline. Browsers only allow sound after the user
   * interacts with the page, so call this from a click or key handler.
   */
  start() {
    this.starting ??= this.create().catch((error) => {
      // Sound is optional: the emulator runs fine without it.
      console.warn("Glassbit: audio is unavailable:", error);
      this.starting = null;
    });
    // A context can also be suspended later, e.g. by the browser.
    if (this.context?.state === "suspended") this.context.resume();
    return this.starting;
  }

  async create() {
    const context = new AudioContext({ latencyHint: "interactive" });
    await context.audioWorklet.addModule("audio-worklet.js");
    const node = new AudioWorkletNode(context, "glassbit-output", {
      numberOfInputs: 0,
      outputChannelCount: [2],
    });
    node.port.onmessage = (event) => {
      this.buffered = event.data.buffered;
      this.underruns = event.data.underruns;
    };
    const gain = context.createGain();
    gain.gain.value = this.muted ? 0 : 1;
    node.connect(gain).connect(context.destination);
    if (context.state === "suspended") await context.resume();

    this.context = context;
    this.gain = gain;
    this.node = node;
  }

  /** Queues interleaved stereo samples (a Float32Array) for playback. */
  push(samples) {
    if (!this.running || samples.length === 0) return;
    this.buffered += samples.length / 2;
    // Transferring the buffer avoids copying it to the audio thread.
    this.node.port.postMessage(samples, [samples.buffer]);
  }

  /**
   * The factor to multiply elapsed time by: above 1 when the queue is
   * running low (emulate a little more), below 1 when it is filling up.
   */
  speed() {
    if (!this.running) return 1;
    const error = TARGET_SECONDS - this.buffered / this.sampleRate;
    return 1 + Math.max(-MAX_CORRECTION, Math.min(MAX_CORRECTION, error * 0.5));
  }

  setMuted(muted) {
    this.muted = muted;
    if (this.gain) this.gain.gain.value = muted ? 0 : 1;
  }
}
