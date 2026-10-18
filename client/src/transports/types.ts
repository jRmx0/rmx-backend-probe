import type { ConnectionType } from "@/generated/ConnectionType";
import type { RecordRow } from "@/generated/RecordRow";
import { expectDeleted, expectRecord, expectRecords, type RequestSender } from "@/lib/rpc";

/** CRUD on records over one transport. */
export interface RecordsTransport {
  readonly kind: ConnectionType;
  list(): Promise<RecordRow[]>;
  create(value: string | null): Promise<RecordRow>;
  update(id: number, value: string | null): Promise<RecordRow>;
  remove(id: number): Promise<void>;
}

/** Builds a transport from a request/response channel (WSS and MQTT). */
export function rpcTransport(kind: ConnectionType, send: RequestSender): RecordsTransport {
  return {
    kind,
    list: async () => expectRecords(await send({ op: "list" })),
    create: async (value) => expectRecord(await send({ op: "create", value })),
    update: async (id, value) => expectRecord(await send({ op: "update", id, value })),
    remove: async (id) => {
      expectDeleted(await send({ op: "delete", id }));
    },
  };
}
