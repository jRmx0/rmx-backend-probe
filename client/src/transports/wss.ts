import type { ChangeEvent } from "@/generated/ChangeEvent";
import type { Request } from "@/generated/Request";
import type { RequestEnvelope } from "@/generated/RequestEnvelope";
import type { RpcData } from "@/generated/RpcData";
import type { ServerMessage } from "@/generated/ServerMessage";
import { PendingRequests, RpcError } from "@/lib/rpc";

export type WsState = "connecting" | "open" | "closed";

export type WsHandlers = {
  onState: (state: WsState, detail?: string) => void;
  onEvent: (event: ChangeEvent) => void;
};

/**
 * One WebSocket to probe-server, reconnecting with backoff. Carries
 * request/response pairs and the server's change events.
 */
export class WssConnection {
  private socket?: WebSocket;
  private readonly pending = new PendingRequests();
  private retryMs = 1000;
  private retryTimer?: ReturnType<typeof setTimeout>;
  private stopped = false;

  constructor(
    private readonly url: string,
    private readonly handlers: WsHandlers,
  ) {}

  start(): void {
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    clearTimeout(this.retryTimer);
    this.socket?.close(1000, "client stopped");
  }

  send = (request: Request): Promise<RpcData> => {
    if (this.socket?.readyState !== WebSocket.OPEN) {
      return Promise.reject(new RpcError("disconnected", "WebSocket is not connected"));
    }
    const { requestId, promise } = this.pending.start();
    const envelope: RequestEnvelope = { request_id: requestId, request };
    this.socket.send(JSON.stringify(envelope));
    return promise;
  };

  private connect(): void {
    this.handlers.onState("connecting");
    const socket = new WebSocket(this.url);
    this.socket = socket;

    socket.onopen = () => {
      this.retryMs = 1000;
      this.handlers.onState("open");
    };
    socket.onmessage = (e) => {
      const msg = JSON.parse(e.data as string) as ServerMessage;
      if (msg.type === "response") this.pending.settle(msg.response);
      else this.handlers.onEvent(msg.event);
    };
    socket.onclose = (e) => {
      this.pending.failAll("WebSocket closed");
      this.handlers.onState("closed", e.reason || `code ${e.code}`);
      if (this.stopped) return;
      this.retryTimer = setTimeout(() => this.connect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 15_000);
    };
  }
}
