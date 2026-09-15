export type SessionStreamState = "connecting" | "connected" | "disconnected" | "exited" | "error";
export type SessionStreamSocket = Pick<WebSocket, "binaryType" | "readyState" | "onopen" | "onmessage" | "onerror" | "onclose" | "send" | "close">;
export const SESSION_STREAM_TIMEOUT_MS = 8_000;
export const SESSION_STREAM_TIMEOUT = "Connection timed out. The process may still be running; reconnect the view to try again.";
export const SESSION_STREAM_ERROR = "Unable to connect the session view. The process may still be running; reconnect the view to try again.";
export const SESSION_STREAM_CLOSED = "The session view disconnected. The process may still be running; reconnect the view to try again.";
export const SESSION_STREAM_RENDER_ERROR = "Unable to display the session stream. The process may still be running; reconnect the view to try again.";
export const TERMINAL_VIEW_ERROR = "Unable to open the terminal view. The process may still be running; reconnect the view to try again.";

interface SessionStreamOptions {
  createSocket(url: string): SessionStreamSocket;
  onState(state: SessionStreamState, notice: string): void;
  onMessage(data: unknown): void;
  onOpen?(): void;
  timeoutMs?: number;
  setTimer?(callback: () => void, delay: number): unknown;
  clearTimer?(timer: unknown): void;
}

/** View transport only: there are no process APIs, automatic retries or queued/replayed inputs. */
export function createSessionStream(options: SessionStreamOptions) {
  let socket: SessionStreamSocket | undefined, timer: unknown;
  let generation = 0, disposed = false;
  let state: SessionStreamState = "disconnected";
  const schedule = options.setTimer ?? ((callback, delay) => setTimeout(callback, delay));
  const cancel = options.clearTimer ?? (handle => clearTimeout(handle as ReturnType<typeof setTimeout>));
  function clearHandshake() { if (timer !== undefined) { cancel(timer); timer = undefined; } }
  function retire(close = true) {
    generation++; clearHandshake();
    const previous = socket; socket = undefined;
    if (!previous) return;
    previous.onopen = previous.onmessage = previous.onerror = previous.onclose = null;
    if (close) try { previous.close(); } catch { /* A retired view must not affect the replacement. */ }
  }
  function publish(next: SessionStreamState, notice = "") { state = next; options.onState(next, notice); }
  function fail(notice = SESSION_STREAM_ERROR) { if (!disposed) { retire(); publish("error", notice); } }
  function connect(url: string) {
    if (disposed) return;
    retire(); publish("connecting");
    const own = generation;
    try {
      const current = options.createSocket(url); socket = current; current.binaryType = "arraybuffer";
      const isCurrent = () => !disposed && generation === own && socket === current;
      current.onopen = () => {
        if (!isCurrent()) return;
        clearHandshake(); publish("connected");
        try { options.onOpen?.(); } catch { fail(SESSION_STREAM_RENDER_ERROR); }
      };
      current.onmessage = event => {
        if (!isCurrent()) return;
        try { options.onMessage(event.data); } catch { fail(SESSION_STREAM_RENDER_ERROR); }
      };
      current.onerror = () => { if (isCurrent()) fail(); };
      current.onclose = () => { if (isCurrent()) { retire(false); publish("disconnected", SESSION_STREAM_CLOSED); } };
      timer = schedule(() => { if (isCurrent() && state === "connecting") fail(SESSION_STREAM_TIMEOUT); }, options.timeoutMs ?? SESSION_STREAM_TIMEOUT_MS);
    } catch { fail(); }
  }
  function send(data: Parameters<WebSocket["send"]>[0]): boolean {
    if (disposed || !socket || socket.readyState !== 1 || state !== "connected") return false;
    try { socket.send(data); return true; } catch { fail(); return false; }
  }
  function disconnect() { if (!disposed) { retire(); publish("disconnected", SESSION_STREAM_CLOSED); } }
  function finish(notice: string) { if (!disposed) { retire(); publish("exited", notice); } }
  function dispose() { if (!disposed) { disposed = true; retire(); } }
  return { connect, send, disconnect, finish, fail, dispose };
}
