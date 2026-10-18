/**
 * Development server only (`bun dev`). In production probe-server serves the
 * built files from `dist/`, and this file is not used.
 *
 * - Serves the app at /probe/ with hot module reloading (Bun HTML import).
 * - Proxies the backend routes to probe-server, so the browser sees one
 *   origin exactly like in production (no CORS, same relative URLs).
 */

import { serve, type Server, type ServerWebSocket } from "bun";
import index from "../public/index.html";

const BACKEND = process.env.PROBE_BACKEND_URL ?? "http://127.0.0.1:8080";
const BACKEND_WS = BACKEND.replace(/^http/, "ws");
const PORT = Number(process.env.PORT ?? 3000);

/** Forwards one HTTP request to probe-server and returns its response. */
async function proxy(req: Request): Promise<Response> {
  const url = new URL(req.url);
  const headers = new Headers(req.headers);
  headers.delete("host");
  try {
    return await fetch(new URL(url.pathname + url.search, BACKEND), {
      method: req.method,
      headers,
      body: req.method === "GET" || req.method === "HEAD" ? undefined : await req.arrayBuffer(),
      redirect: "manual",
    });
  } catch {
    return Response.json(
      { code: "internal", message: `probe-server is not reachable at ${BACKEND}` },
      { status: 502 },
    );
  }
}

/** Per-browser-socket state: the matching socket to probe-server. */
type WsData = { upstream?: WebSocket; queue: (string | Uint8Array<ArrayBuffer>)[] };

/** Close codes that may not be sent in a close frame are replaced. */
function sendableCode(code: number): number {
  return code === 1000 || (code >= 3000 && code <= 4999) || (code >= 1001 && code <= 1014 && code !== 1005 && code !== 1006)
    ? code
    : 1011;
}

const server = serve<WsData>({
  port: PORT,
  routes: {
    "/": Response.redirect("/probe/", 302),
    "/probe": Response.redirect("/probe/", 302),
    "/probe/api/*": proxy,
    "/probe/health": proxy,
    "/probe/config.json": proxy,
    "/probe/ws": (req: Request, server: Server<WsData>) => {
      if (server.upgrade(req, { data: { queue: [] } })) return undefined;
      return new Response("WebSocket upgrade expected", { status: 400 });
    },
    // The app itself, for every other path under /probe/.
    "/probe/*": index,
  },

  // Relays WebSocket frames both ways between the browser and probe-server.
  websocket: {
    open(ws: ServerWebSocket<WsData>) {
      const upstream = new WebSocket(`${BACKEND_WS}/probe/ws`);
      ws.data.upstream = upstream;
      upstream.onopen = () => {
        for (const msg of ws.data.queue) upstream.send(msg);
        ws.data.queue = [];
      };
      upstream.onmessage = (e) => ws.send(e.data as string);
      upstream.onclose = (e) => ws.close(sendableCode(e.code), e.reason);
      upstream.onerror = () => ws.close(1011, "probe-server is not reachable");
    },
    message(ws, raw) {
      const msg = typeof raw === "string" ? raw : new Uint8Array(raw);
      const upstream = ws.data.upstream;
      if (upstream?.readyState === WebSocket.OPEN) upstream.send(msg);
      else ws.data.queue.push(msg);
    },
    close(ws) {
      ws.data.upstream?.close();
    },
  },

  development: process.env.NODE_ENV !== "production" && {
    // Enable browser hot reloading in development
    hmr: true,

    // Echo console logs from the browser to the server
    console: true,
  },
});

console.log(`🚀 Probe client dev server at ${server.url}probe/ (backend: ${BACKEND})`);
