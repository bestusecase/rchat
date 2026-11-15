// Thin bridge to the Tauri shell. Everything here is optional so the same UI
// still runs as a plain browser client during development.

const tauri = typeof window !== "undefined" ? window.__TAURI__ : null;

export const boot =
  typeof window !== "undefined" ? window.__RCHAT_BOOT__ ?? null : null;

export const isDesktop = Boolean(boot);

export async function invoke(command, args = {}) {
  if (!tauri?.core?.invoke) {
    throw new Error("Desktop integration is unavailable in this window.");
  }
  return tauri.core.invoke(command, args);
}

export function listen(event, handler) {
  if (!tauri?.event?.listen) return () => {};
  let unlisten = null;
  let active = true;
  tauri.event
    .listen(event, (payload) => handler(payload.payload))
    .then((fn) => {
      if (active) unlisten = fn;
      else fn();
    })
    .catch(() => {});
  return () => {
    active = false;
    if (unlisten) unlisten();
  };
}
