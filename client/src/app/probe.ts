/**
 * Wires the transports to the stores: picks the transport for CRUD, applies
 * change events, polls health and times every operation.
 *
 * The page uses HTTPS and WSS only. MQTT is for devices (like the LTE modem
 * probe), which talk to the broker on 8883; their changes reach the page as
 * WSS events like any other.
 */

import { wsUrl } from "@/lib/paths";
import { useConnectionStore, type WebTransport } from "@/stores/connectionStore";
import { useRecordsStore } from "@/stores/recordsStore";
import { checkSession, fetchHealth, httpsTransport } from "@/transports/https";
import { rpcTransport, type RecordsTransport } from "@/transports/types";
import { WssConnection } from "@/transports/wss";

const HEALTH_INTERVAL_MS = 10_000;

const conn = () => useConnectionStore.getState();

const wss = new WssConnection(wsUrl(), {
  onState: (state, detail) => {
    conn().setWss(state, detail);
    // After a (re)connect, events may have been missed: list again.
    if (state === "open") void reload();
    // A browser doesn't tell scripts why a handshake failed (a 401 looks like
    // any other failure), and the server closes the socket with 4001 when the
    // session ends. Either way, ask the server whether we're still logged in.
    if (state === "closed") void checkSession();
  },
  onEvent: (event) => useRecordsStore.getState().apply(event),
});

const transports: Record<WebTransport, RecordsTransport> = {
  https: httpsTransport,
  wss: rpcTransport("wss", wss.send),
};

/** Runs one operation over the selected transport and records its timing. */
async function timed<T>(op: string, fn: (t: RecordsTransport) => Promise<T>): Promise<T> {
  const transport = transports[conn().selected];
  const start = performance.now();
  try {
    const result = await fn(transport);
    conn().setLastOp({ transport: transport.kind, op, ms: performance.now() - start });
    return result;
  } catch (err) {
    const error = err instanceof Error ? err.message : String(err);
    conn().setLastOp({ transport: transport.kind, op, ms: performance.now() - start, error });
    throw err;
  }
}

export async function reload(): Promise<void> {
  const records = useRecordsStore.getState();
  records.setLoading(true);
  try {
    records.setAll(await timed("list", (t) => t.list()));
  } catch {
    // Shown in the status panel via lastOp.
  } finally {
    records.setLoading(false);
  }
}

export async function createRecord(value: string | null) {
  const record = await timed("create", (t) => t.create(value));
  useRecordsStore.getState().upsert(record);
}

export async function updateRecord(id: number, value: string | null) {
  const record = await timed("update", (t) => t.update(id, value));
  useRecordsStore.getState().upsert(record);
}

export async function deleteRecord(id: number) {
  await timed("delete", (t) => t.remove(id));
  useRecordsStore.getState().remove(id);
}

export function selectTransport(t: WebTransport) {
  conn().select(t);
  void reload();
}

async function checkHealth() {
  try {
    const health = await fetchHealth();
    conn().setHttps({ state: health.status === "ok" ? "ok" : "degraded", health });
  } catch (err) {
    conn().setHttps({ state: "down", detail: err instanceof Error ? err.message : String(err) });
  }
}

/**
 * Starts the WebSocket and health polling. Returns a stop function. Runs only
 * while logged in: the app unmounts on logout, which stops everything.
 */
export function startProbe(): () => void {
  wss.start();
  void checkHealth();
  const timer = setInterval(checkHealth, HEALTH_INTERVAL_MS);
  return () => {
    clearInterval(timer);
    wss.stop();
    // Don't leave data on screen (or in memory) for the next login.
    useRecordsStore.getState().setAll([]);
  };
}
