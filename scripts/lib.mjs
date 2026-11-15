// Minimal client for the rchatd WebSocket API. Used by the smoke tests and
// handy for poking the daemon without the UI.
export function connect(base, token) {
  const url = base.replace(/^http/, "ws") + "/ws?token=" + encodeURIComponent(token);
  const ws = new WebSocket(url);
  const pending = new Map();
  const listeners = new Set();
  const history = [];
  let nextId = 1;
  let resolveHello;
  let rejectHello;
  const hello = new Promise((resolve, reject) => {
    resolveHello = resolve;
    rejectHello = reject;
  });

  ws.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    if (msg.type === "hello") {
      resolveHello(msg);
      return;
    }
    if (msg.type === "reply") {
      const waiter = pending.get(msg.id);
      if (waiter) {
        pending.delete(msg.id);
        waiter(msg);
      }
      return;
    }
    if (msg.type === "event") {
      history.push(msg);
      for (const listener of listeners) listener(msg);
    }
  });
  ws.addEventListener("error", () => rejectHello(new Error("websocket error")));

  const api = {
    hello,
    history,
    call(cmd, timeoutMs = 20000) {
      const id = nextId++;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error("timeout: " + JSON.stringify(cmd)));
        }, timeoutMs);
        pending.set(id, (msg) => {
          clearTimeout(timer);
          if (msg.ok) resolve(msg.data);
          else reject(new Error(msg.error));
        });
        ws.send(JSON.stringify({ id, ...cmd }));
      });
    },
    onEvent(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    waitFor(predicate, timeoutMs = 30000) {
      for (const event of history) {
        if (predicate(event)) return Promise.resolve(event);
      }
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          off();
          reject(new Error("timeout waiting for event"));
        }, timeoutMs);
        const off = api.onEvent((event) => {
          if (predicate(event)) {
            clearTimeout(timer);
            off();
            resolve(event);
          }
        });
      });
    },
    close() {
      ws.close();
    },
  };
  return api;
}

export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export async function until(check, { timeoutMs = 30000, intervalMs = 250 } = {}) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await check();
    if (last) return last;
    await sleep(intervalMs);
  }
  throw new Error("condition not met in time; last value: " + JSON.stringify(last));
}
