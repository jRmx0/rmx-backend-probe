import type { RecordRow } from "@/generated/RecordRow";
import type { Request } from "@/generated/Request";
import type { ResponseEnvelope } from "@/generated/ResponseEnvelope";
import type { RpcData } from "@/generated/RpcData";

export class RpcError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "RpcError";
  }
}

type Pending = {
  resolve: (data: RpcData) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

/**
 * Matches responses to requests by `request_id`. Used by both the WSS and the
 * MQTT transport, which share the same envelopes.
 */
export class PendingRequests {
  private readonly pending = new Map<string, Pending>();

  constructor(private readonly timeoutMs = 10_000) {}

  /** Registers a request and returns its id plus a promise for its data. */
  start(): { requestId: string; promise: Promise<RpcData> } {
    const requestId = crypto.randomUUID();
    const promise = new Promise<RpcData>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(requestId);
        reject(new RpcError("timeout", "no response in time"));
      }, this.timeoutMs);
      this.pending.set(requestId, { resolve, reject, timer });
    });
    return { requestId, promise };
  }

  /** Settles the matching request. Returns false for an unknown id. */
  settle(response: ResponseEnvelope): boolean {
    const p = this.pending.get(response.request_id);
    if (!p) return false;
    this.pending.delete(response.request_id);
    clearTimeout(p.timer);
    if (response.result.status === "ok") p.resolve(response.result.data);
    else p.reject(new RpcError(response.result.error.code, response.result.error.message));
    return true;
  }

  /** Fails every open request, e.g. when the connection drops. */
  failAll(reason: string): void {
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.reject(new RpcError("disconnected", reason));
    }
    this.pending.clear();
  }

  get size(): number {
    return this.pending.size;
  }
}

/** The three data shapes, checked so callers get the type they asked for. */
export function expectRecords(data: RpcData): RecordRow[] {
  if (data.kind !== "records") throw new RpcError("internal", `unexpected ${data.kind} response`);
  return data.records;
}

export function expectRecord(data: RpcData): RecordRow {
  if (data.kind !== "record") throw new RpcError("internal", `unexpected ${data.kind} response`);
  return data.record;
}

export function expectDeleted(data: RpcData): number {
  if (data.kind !== "deleted") throw new RpcError("internal", `unexpected ${data.kind} response`);
  return data.id;
}

/** Sends one request over a message channel and resolves with its data. */
export type RequestSender = (request: Request) => Promise<RpcData>;
