export function initials(label = "?") {
  const parts = String(label).trim().split(/\s+/).filter(Boolean);
  if (!parts.length) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

export function hashHue(seed = "") {
  let hash = 0;
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) % 360;
  }
  return hash;
}

export function shortAddress(address) {
  if (!address) return "no address yet";
  return address.slice(0, 6) + "…" + address.slice(-4);
}

/// Whether a peer has announced recently enough to be considered online. The
/// members list and the peer card read the same rule so their dots agree.
export const PEER_ONLINE_MS = 10 * 60 * 1000;

/// Whether a peer can receive a message right now. The engine's per-peer
/// reachability row is authoritative when present: a live path with an old
/// announce is still reachable, and a recent announce with no path is not.
/// The announce rule remains the fallback for clients talking to an older
/// engine that does not publish reachability rows.
export function peerReachable(peer, transport, reachability) {
  if (reachability) {
    return Boolean(reachability.path) && reachability.state === "responsive";
  }
  if (transport !== "ready") return false;
  return Boolean(peer?.last_seen) && Date.now() - peer.last_seen < PEER_ONLINE_MS;
}

export function formatTime(ts) {
  return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function dayLabel(ts) {
  const date = new Date(ts);
  const now = new Date();
  const startOf = (value) =>
    new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const diff = Math.round((startOf(now) - startOf(date)) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  return date.toLocaleDateString([], {
    month: "short",
    day: "numeric",
    year: date.getFullYear() === now.getFullYear() ? undefined : "numeric",
  });
}

export function transportLabel(state) {
  switch (state) {
    case "ready":
      return "Reticulum online";
    case "degraded":
      return "Mesh degraded";
    case "starting":
      return "Connecting mesh";
    default:
      return "Offline";
  }
}

/// Short, copy-friendly form of a hash. Never ambiguous at a glance.
export function shortHash(value, size = 6) {
  if (!value) return "—";
  if (value.length <= size * 2 + 1) return value;
  return value.slice(0, size) + "…" + value.slice(-4);
}

export function formatBitrate(bps) {
  if (!bps) return "—";
  if (bps >= 1_000_000) return (bps / 1_000_000).toFixed(bps % 1_000_000 ? 1 : 0) + " Mbit/s";
  if (bps >= 1_000) return Math.round(bps / 1_000) + " kbit/s";
  return bps + " bit/s";
}

export function formatRate(value) {
  if (!value) return "0";
  if (value >= 1_000_000) return (value / 1_000_000).toFixed(1) + "M";
  if (value >= 1_000) return (value / 1_000).toFixed(1) + "k";
  return Number(value).toFixed(value < 10 ? 1 : 0);
}

/// A file size a person can read: "812 B", "64 KB", "8.0 MB".
export function formatBytes(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return value + " B";
  if (value < 1024 * 1024) return Math.round(value / 1024) + " KB";
  return (value / (1024 * 1024)).toFixed(1) + " MB";
}

/// "just now", "3 min ago", "2 days ago". `ts` is milliseconds.
export function formatRelative(ts) {
  if (!ts) return "never";
  const seconds = Math.max(0, Math.round((Date.now() - ts) / 1000));
  if (seconds < 45) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return minutes + " min ago";
  const hours = Math.round(minutes / 60);
  if (hours < 24) return hours + (hours === 1 ? " hour ago" : " hours ago");
  const days = Math.round(hours / 24);
  return days + (days === 1 ? " day ago" : " days ago");
}

/// How long an interface has been offline, for "offline for 2 min". Returns
/// null when it is online (no `down_since`). Unlike `formatRelative` it reads
/// as a duration, not as a point in the past.
export function formatOutage(downSince) {
  if (!downSince) return null;
  const seconds = Math.max(0, Math.round((Date.now() - downSince) / 1000));
  if (seconds < 60) return "less than a minute";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return minutes + " min";
  const hours = Math.round(minutes / 60);
  if (hours < 24) return hours + (hours === 1 ? " hour" : " hours");
  const days = Math.round(hours / 24);
  return days + (days === 1 ? " day" : " days");
}

export function formatUptime(startedAt) {
  if (!startedAt) return "—";
  const seconds = Math.max(0, Math.round(Date.now() / 1000 - startedAt));
  const minutes = Math.floor(seconds / 60);
  if (minutes < 1) return seconds + "s";
  const hours = Math.floor(minutes / 60);
  if (hours < 1) return minutes + "m";
  const days = Math.floor(hours / 24);
  if (days < 1) return hours + "h " + (minutes % 60) + "m";
  return days + "d " + (hours % 24) + "h";
}

export function absoluteTime(ts) {
  if (!ts) return "";
  return new Date(ts).toLocaleString();
}

