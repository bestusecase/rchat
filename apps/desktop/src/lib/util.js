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
    case "starting":
      return "Connecting mesh";
    default:
      return "Offline";
  }
}
