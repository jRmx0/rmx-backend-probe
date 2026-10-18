import mqtt, { type MqttClient } from "mqtt";
import type { ChangeEvent } from "@/generated/ChangeEvent";
import type { Request } from "@/generated/Request";
import type { RequestEnvelope } from "@/generated/RequestEnvelope";
import type { ResponseEnvelope } from "@/generated/ResponseEnvelope";
import type { RpcData } from "@/generated/RpcData";
import { PendingRequests, RpcError } from "@/lib/rpc";
import { mqttTopics, randomClientId } from "@/lib/topics";

export type MqttState = "disconnected" | "connecting" | "connected" | "error";

export type MqttLogin = {
  url: string;
  username: string;
  password: string;
  prefix: string;
};

export type MqttHandlers = {
  onState: (state: MqttState, detail?: string) => void;
  onEvent: (event: ChangeEvent) => void;
  /** The server's retained online/offline status (last will). */
  onServerStatus: (status: string) => void;
};

/**
 * mqtt.js over WebSockets to the broker. Requests go to `<prefix>/req/<id>`,
 * responses come back on `<prefix>/res/<id>`, and change events on
 * `<prefix>/events/records`.
 */
export class MqttConnection {
  private client?: MqttClient;
  private readonly pending = new PendingRequests();
  private readonly clientId = randomClientId();
  private topics = mqttTopics("probe", this.clientId);

  constructor(private readonly handlers: MqttHandlers) {}

  get id(): string {
    return this.clientId;
  }

  connect(login: MqttLogin): void {
    this.disconnect();
    this.topics = mqttTopics(login.prefix, this.clientId);
    this.handlers.onState("connecting");

    const client = mqtt.connect(login.url, {
      clientId: this.clientId,
      username: login.username,
      password: login.password,
      protocolVersion: 4,
      clean: true,
      reconnectPeriod: 3000,
      connectTimeout: 10_000,
    });
    this.client = client;

    client.on("connect", () => {
      this.handlers.onState("connected");
      const t = this.topics;
      client.subscribe([t.response, t.events, t.serverStatus], { qos: 1 }, (err) => {
        if (err) this.handlers.onState("error", `subscribe failed: ${err.message}`);
      });
    });
    client.on("reconnect", () => this.handlers.onState("connecting"));
    client.on("close", () => {
      this.pending.failAll("MQTT connection closed");
      if (this.client === client) this.handlers.onState("disconnected");
    });
    client.on("error", (err) => {
      this.handlers.onState("error", err.message);
      // A refused login won't fix itself: stop retrying.
      if (/not authori[sz]ed|bad user name or password/i.test(err.message)) client.end(true);
    });
    client.on("message", (topic, payload) => this.onMessage(topic, payload.toString()));
  }

  disconnect(): void {
    const client = this.client;
    this.client = undefined;
    client?.end(true);
    this.pending.failAll("MQTT disconnected");
    this.handlers.onState("disconnected");
  }

  send = (request: Request): Promise<RpcData> => {
    const client = this.client;
    if (!client?.connected) {
      return Promise.reject(new RpcError("disconnected", "MQTT is not connected"));
    }
    const { requestId, promise } = this.pending.start();
    const envelope: RequestEnvelope = { request_id: requestId, request };
    client.publish(this.topics.request, JSON.stringify(envelope), { qos: 1 });
    return promise;
  };

  private onMessage(topic: string, text: string): void {
    const t = this.topics;
    if (topic === t.response) this.pending.settle(JSON.parse(text) as ResponseEnvelope);
    else if (topic === t.events) this.handlers.onEvent(JSON.parse(text) as ChangeEvent);
    else if (topic === t.serverStatus) this.handlers.onServerStatus(text);
  }
}
