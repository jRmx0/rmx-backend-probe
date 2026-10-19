/**
 * Wires the transports to the stores: picks the transport for CRUD, routes
 * change events, polls health and times every operation.
 */

import type { ChangeEvent } from "@/generated/ChangeEvent";
import type { ConnectionType } from "@/generated/ConnectionType";
import { wsUrl } from "@/lib/paths";
import { useConnectionStore } from "@/stores/connectionStore";
import { useRecordsStore } from "@/stores/recordsStore";
import { checkSession, fetchClientConfig, fetchHealth, httpsTransport } from "@/transports/https";
import { MqttConnection, type MqttLogin } from "@/transports/mqtt";
import { rpcTransport, type RecordsTransport } from "@/transports/types";
import { WssConnection } from "@/transports/wss";

const HEALTH_INTERVAL_MS = 10_000;

const conn = () => useConnectionStore.getState();

/**
 * Change events come over MQTT while MQTT is the selected transport and
 * connected, and over the WebSocket otherwise. Each path is then visibly
 * tested on its own.
 */
function eventsViaMqtt(): boolean {
  const s = conn();
  return s.selected === "mqtt" && s.mqtt.state === "connected";
}

function onEvent(source: "wss" | "mqtt", event: ChangeEvent) {
  if ((source === "mqtt") === eventsViaMqtt()) useRecordsStore.getState().apply(event);
}

const wss = new WssConnection(wsUrl(), {
  onState: (state, detail) => {
    conn().setWss(state, detail);
    // After a (re)connect, events may have been missed: list again.
    if (state === "open" && !eventsViaMqtt()) void reload();
    // A browser doesn't tell scripts why a handshake failed (a 401 looks like
    // any other failure), and the server closes the socket with 4001 when the
    // session ends. Either way, ask the server whether we're still logged in.
    if (state === "closed") void checkSession();
  },
  onEvent: (event) => onEvent("wss", event),
});

const mqtt = new MqttConnection({
  onState: (state, detail) => {
    conn().setMqtt(state, detail);
    if (state === "connected" && conn().selected === "mqtt") void reload();
  },
  onEvent: (event) => onEvent("mqtt", event),
  onServerStatus: (status) => conn().setServerStatus(status),
});

export const mqttClientId = mqtt.id;

const transports: Record<ConnectionType, RecordsTransport> = {
  https: httpsTransport,
  wss: rpcTransport("wss", wss.send),
  mqtt: rpcTransport("mqtt", mqtt.send),
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

export function selectTransport(t: ConnectionType) {
  conn().select(t);
  void reload();
}

export function connectMqtt(login: Omit<MqttLogin, "url" | "prefix">) {
  const config = conn().config;
  if (!config?.mqtt_ws_url) {
    conn().setMqtt("error", "the server has no MQTT_WS_URL configured");
    return;
  }
  mqtt.connect({ ...login, url: config.mqtt_ws_url, prefix: config.mqtt_topic_prefix });
}

export function disconnectMqtt() {
  mqtt.disconnect();
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
 * while logged in: the app unmounts on logout, which stops everything
 * (including MQTT, which has its own login).
 */
export function startProbe(): () => void {
  wss.start();
  void checkHealth();
  const timer = setInterval(checkHealth, HEALTH_INTERVAL_MS);
  fetchClientConfig()
    .then((c) => conn().setConfig(c))
    .catch((err: unknown) => console.error("config.json failed", err));
  return () => {
    clearInterval(timer);
    wss.stop();
    mqtt.disconnect();
    // Don't leave data on screen (or in memory) for the next login.
    useRecordsStore.getState().setAll([]);
  };
}
