import { create } from "zustand";
import type { ClientConfig } from "@/generated/ClientConfig";
import type { ConnectionType } from "@/generated/ConnectionType";
import type { Health } from "@/generated/Health";
import type { MqttState } from "@/transports/mqtt";
import type { WsState } from "@/transports/wss";

export type HttpsState = "unknown" | "ok" | "degraded" | "down";

export type LastOp = {
  transport: ConnectionType;
  op: string;
  ms: number;
  error?: string;
};

type ConnectionState = {
  /** The transport used for CRUD requests. */
  selected: ConnectionType;
  https: { state: HttpsState; health?: Health; detail?: string };
  wss: { state: WsState; detail?: string };
  mqtt: { state: MqttState; detail?: string; serverStatus?: string };
  config?: ClientConfig;
  lastOp?: LastOp;

  select: (t: ConnectionType) => void;
  setHttps: (https: ConnectionState["https"]) => void;
  setWss: (state: WsState, detail?: string) => void;
  setMqtt: (state: MqttState, detail?: string) => void;
  setServerStatus: (serverStatus: string) => void;
  setConfig: (config: ClientConfig) => void;
  setLastOp: (lastOp: LastOp) => void;
};

export const useConnectionStore = create<ConnectionState>((set) => ({
  selected: "https",
  https: { state: "unknown" },
  wss: { state: "closed" },
  mqtt: { state: "disconnected" },

  select: (selected) => set({ selected }),
  setHttps: (https) => set({ https }),
  setWss: (state, detail) => set({ wss: { state, detail } }),
  setMqtt: (state, detail) => set((s) => ({ mqtt: { ...s.mqtt, state, detail } })),
  setServerStatus: (serverStatus) => set((s) => ({ mqtt: { ...s.mqtt, serverStatus } })),
  setConfig: (config) => set({ config }),
  setLastOp: (lastOp) => set({ lastOp }),
}));
