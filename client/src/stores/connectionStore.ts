import { create } from "zustand";
import type { ConnectionType } from "@/generated/ConnectionType";
import type { Health } from "@/generated/Health";
import type { WsState } from "@/transports/wss";

/** The transports a browser uses. MQTT is for devices only. */
export type WebTransport = Exclude<ConnectionType, "mqtt">;

export type HttpsState = "unknown" | "ok" | "degraded" | "down";

export type LastOp = {
  transport: ConnectionType;
  op: string;
  ms: number;
  error?: string;
};

type ConnectionState = {
  /** The transport used for CRUD requests. */
  selected: WebTransport;
  https: { state: HttpsState; health?: Health; detail?: string };
  wss: { state: WsState; detail?: string };
  lastOp?: LastOp;

  select: (t: WebTransport) => void;
  setHttps: (https: ConnectionState["https"]) => void;
  setWss: (state: WsState, detail?: string) => void;
  setLastOp: (lastOp: LastOp) => void;
};

export const useConnectionStore = create<ConnectionState>((set) => ({
  selected: "https",
  https: { state: "unknown" },
  wss: { state: "closed" },

  select: (selected) => set({ selected }),
  setHttps: (https) => set({ https }),
  setWss: (state, detail) => set({ wss: { state, detail } }),
  setLastOp: (lastOp) => set({ lastOp }),
}));
