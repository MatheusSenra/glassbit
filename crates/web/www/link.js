// The link cable, over the internet: two browsers connected with WebRTC.
//
// WebRTC needs the two sides to exchange a description of how to reach each
// other before they can talk. Usually a server relays that; here the two
// players relay it themselves, by copying an invite code to the other
// person and an answer code back. So no server of ours is involved, only a
// public STUN server that tells each browser its own public address.
//
// Once connected, every message is a few bytes: the serial transfers the
// emulator asks for (see `take_link_messages` in the wasm bindings).

const ICE_SERVERS = [{ urls: "stun:stun.l.google.com:19302" }];
// Stop gathering addresses after this long, and use what was found.
const GATHER_TIMEOUT_MS = 4000;
const CODE_PREFIX = "GB1:";

/** Compresses and base64-encodes a session description for copying. */
async function encode(description) {
  const json = JSON.stringify({ type: description.type, sdp: description.sdp });
  let bytes = new TextEncoder().encode(json);
  if (typeof CompressionStream === "function") {
    const stream = new Blob([bytes]).stream().pipeThrough(new CompressionStream("deflate-raw"));
    bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  }
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return CODE_PREFIX + btoa(binary);
}

/** Reverses `encode`; throws on anything that is not a code. */
async function decode(code) {
  const trimmed = code.trim();
  if (!trimmed.startsWith(CODE_PREFIX)) throw new Error("not a Glassbit link code");
  const binary = atob(trimmed.slice(CODE_PREFIX.length));
  let bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
  if (typeof DecompressionStream === "function") {
    const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate-raw"));
    bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  }
  const description = JSON.parse(new TextDecoder().decode(bytes));
  if (!["offer", "answer"].includes(description.type)) throw new Error("not a link code");
  return description;
}

/** Waits until the connection has found its addresses (or gives up waiting). */
function gathered(connection) {
  if (connection.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      connection.removeEventListener("icegatheringstatechange", check);
      resolve();
    };
    const check = () => {
      if (connection.iceGatheringState === "complete") done();
    };
    const timer = setTimeout(done, GATHER_TIMEOUT_MS);
    connection.addEventListener("icegatheringstatechange", check);
  });
}

export class LinkCable {
  /**
   * @param handlers `{ onMessage(bytes), onStateChange(state) }`, where state
   *   is "idle", "waiting", "connected" or "closed".
   */
  constructor(handlers) {
    this.handlers = handlers;
    this.connection = null;
    this.channel = null;
    this.state = "idle";
  }

  get connected() {
    return this.state === "connected";
  }

  setState(state) {
    this.state = state;
    this.handlers.onStateChange(state);
  }

  newConnection() {
    this.close();
    this.connection = new RTCPeerConnection({ iceServers: ICE_SERVERS });
    this.connection.addEventListener("connectionstatechange", () => {
      const state = this.connection?.connectionState;
      if (state === "failed" || state === "closed" || state === "disconnected") this.close();
    });
    this.setState("waiting");
    return this.connection;
  }

  attach(channel) {
    channel.binaryType = "arraybuffer";
    channel.addEventListener("open", () => this.setState("connected"));
    channel.addEventListener("close", () => this.close());
    channel.addEventListener("message", (event) => {
      this.handlers.onMessage(new Uint8Array(event.data));
    });
    this.channel = channel;
  }

  /** Starts a session as the host. Returns the invite code to send. */
  async host() {
    const connection = this.newConnection();
    // Ordered and reliable: serial bytes must arrive, and in order.
    this.attach(connection.createDataChannel("glassbit-link"));
    await connection.setLocalDescription(await connection.createOffer());
    await gathered(connection);
    return encode(connection.localDescription);
  }

  /** Joins with the host's invite code. Returns the answer code to send back. */
  async join(inviteCode) {
    const offer = await decode(inviteCode);
    if (offer.type !== "offer") throw new Error("this is an answer code, not an invite");
    const connection = this.newConnection();
    connection.addEventListener("datachannel", (event) => this.attach(event.channel));
    await connection.setRemoteDescription(offer);
    await connection.setLocalDescription(await connection.createAnswer());
    await gathered(connection);
    return encode(connection.localDescription);
  }

  /** Completes the host's side with the answer code from the other player. */
  async accept(answerCode) {
    const answer = await decode(answerCode);
    if (answer.type !== "answer") throw new Error("this is an invite code, not an answer");
    if (!this.connection) throw new Error("host a game first");
    await this.connection.setRemoteDescription(answer);
  }

  send(bytes) {
    if (this.connected && bytes.length > 0) this.channel.send(bytes);
  }

  close() {
    if (!this.connection && !this.channel) return;
    // Clear first: closing fires events that would call this again.
    const { channel, connection } = this;
    this.channel = null;
    this.connection = null;
    channel?.close();
    connection?.close();
    this.setState("closed");
  }
}
