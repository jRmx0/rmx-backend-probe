import type { ApiError } from "@/generated/ApiError";
import type { ClientConfig } from "@/generated/ClientConfig";
import type { Health } from "@/generated/Health";
import type { RecordInput } from "@/generated/RecordInput";
import type { RecordRow } from "@/generated/RecordRow";
import { BASE } from "@/lib/paths";
import { RpcError } from "@/lib/rpc";
import type { RecordsTransport } from "./types";

async function call<T>(method: string, path: string, body?: RecordInput): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    const err = (await res.json().catch(() => null)) as ApiError | null;
    throw new RpcError(err?.code ?? "http", err?.message ?? `HTTP ${res.status}`);
  }
  return (res.status === 204 ? undefined : await res.json()) as T;
}

export const httpsTransport: RecordsTransport = {
  kind: "https",
  list: () => call<RecordRow[]>("GET", "/api/records"),
  create: (value) => call<RecordRow>("POST", "/api/records", { value }),
  update: (id, value) => call<RecordRow>("PUT", `/api/records/${id}`, { value }),
  remove: (id) => call<void>("DELETE", `/api/records/${id}`),
};

/** GET /probe/health. Resolves with the body even on 503 (degraded). */
export async function fetchHealth(): Promise<Health> {
  const res = await fetch(`${BASE}/health`, { cache: "no-store" });
  if (res.status !== 200 && res.status !== 503) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as Health;
}

export async function fetchClientConfig(): Promise<ClientConfig> {
  const res = await fetch(`${BASE}/config.json`, { cache: "no-store" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as ClientConfig;
}
