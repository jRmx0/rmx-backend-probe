/** Everything the probe serves lives under this prefix (Caddy keeps it). */
export const BASE = "/probe";

/** The WebSocket URL on the page's own origin (wss:// behind HTTPS). */
export function wsUrl(loc: Pick<Location, "protocol" | "host"> = location): string {
  const scheme = loc.protocol === "https:" ? "wss" : "ws";
  return `${scheme}://${loc.host}${BASE}/ws`;
}
