// Runs on the browser's audio thread. The page posts chunks of interleaved
// stereo samples; this processor queues them in a ring buffer and hands
// them to the sound card 128 frames at a time.
//
// The emulator and the sound card run on different clocks, so the queue can
// run dry or overflow. Both are handled here so that they are heard as a
// brief dip instead of a click:
//  - when it runs dry, the output fades out and playback only resumes once
//    a small reserve has built up again;
//  - when it grows too long, the oldest audio is skipped.

const PREFILL_SECONDS = 0.04;
const MAX_SECONDS = 0.25;
// How much of the previous value is kept per frame while fading out.
const FADE = 0.995;
// Tell the page how full the queue is every this many callbacks (~20 ms).
const REPORT_EVERY = 8;

class GlassbitOutput extends AudioWorkletProcessor {
  constructor() {
    super();
    this.capacity = sampleRate; // one second
    this.left = new Float32Array(this.capacity);
    this.right = new Float32Array(this.capacity);
    this.readIndex = 0;
    this.count = 0;
    this.playing = false;
    this.lastLeft = 0;
    this.lastRight = 0;
    this.underruns = 0;
    this.callbacks = 0;
    this.port.onmessage = (event) => this.enqueue(event.data);
  }

  enqueue(samples) {
    const frames = samples.length / 2;
    let writeIndex = (this.readIndex + this.count) % this.capacity;
    for (let i = 0; i < frames; i++) {
      this.left[writeIndex] = samples[2 * i];
      this.right[writeIndex] = samples[2 * i + 1];
      writeIndex = (writeIndex + 1) % this.capacity;
    }
    this.count += frames;

    const max = Math.floor(MAX_SECONDS * sampleRate);
    if (this.count > max) {
      // Too far behind real time: jump ahead, keeping only the reserve.
      const keep = Math.floor(PREFILL_SECONDS * sampleRate);
      this.readIndex = (this.readIndex + this.count - keep) % this.capacity;
      this.count = keep;
    }
  }

  process(_inputs, outputs) {
    const [left, right] = outputs[0];
    if (!this.playing && this.count >= PREFILL_SECONDS * sampleRate) this.playing = true;

    for (let i = 0; i < left.length; i++) {
      if (this.playing && this.count > 0) {
        this.lastLeft = this.left[this.readIndex];
        this.lastRight = this.right[this.readIndex];
        this.readIndex = (this.readIndex + 1) % this.capacity;
        this.count--;
      } else {
        if (this.playing) this.underruns++;
        this.playing = false;
        this.lastLeft *= FADE;
        this.lastRight *= FADE;
      }
      left[i] = this.lastLeft;
      if (right) right[i] = this.lastRight;
    }

    if (++this.callbacks % REPORT_EVERY === 0) {
      this.port.postMessage({ buffered: this.count, underruns: this.underruns });
    }
    return true;
  }
}

registerProcessor("glassbit-output", GlassbitOutput);
