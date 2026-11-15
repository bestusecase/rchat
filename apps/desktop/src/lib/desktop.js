// Thin bridge to the Tauri shell. Everything here is optional so the same UI
// still runs as a plain browser client during development.

const tauri = typeof window !== "undefined" ? window.__TAURI__ : null;

export const boot =
  typeof window !== "undefined" ? window.__RCHAT_BOOT__ ?? null : null;

// The desktop shell injects `__RCHAT_BOOT__` before the page loads, so it is
// known synchronously. The mobile shell cannot: its engine starts in a
// foreground service, so `boot()` is an async command that waits briefly for
// the endpoint. `isDesktop` therefore only ever means "injected at load time".
export const isDesktop = Boolean(boot);

export const hasTauri = Boolean(tauri?.core?.invoke);

export async function invoke(command, args = {}) {
  if (!tauri?.core?.invoke) {
    throw new Error("Desktop integration is unavailable in this window.");
  }
  return tauri.core.invoke(command, args);
}

/// Connection details for the current shell, or null in a plain browser.
///
/// Desktop shells return their injected blob unchanged. Mobile shells answer
/// the `boot` command with `{ role: "mobile", base, token, error }`; the store
/// maps `role` onto the platform it runs on.
export async function loadBoot() {
  if (boot) return boot;
  if (!tauri?.core?.invoke) return null;
  return tauri.core.invoke("boot");
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
