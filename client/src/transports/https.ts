import type { ApiError } from "@/generated/ApiError";
import type { ClientConfig } from "@/generated/ClientConfig";
import type { Health } from "@/generated/Health";
import type { LoginRequest } from "@/generated/LoginRequest";
import type { Me } from "@/generated/Me";
import type { RecordInput } from "@/generated/RecordInput";
import type { RecordRow } from "@/generated/RecordRow";
import { loginErrorMessage } from "@/lib/auth";
import { BASE } from "@/lib/paths";
import { RpcError } from "@/lib/rpc";
import { useAuthStore } from "@/stores/authStore";
import type { RecordsTransport } from "./types";

export const SESSION_ENDED = "Your session has ended. Please log in again.";

/**
 * One request to probe-server. The browser adds the session cookie by itself
 * (same origin), and the Origin header the server checks. A 401 means the
 * session is gone (logout elsewhere, timeout): back to the login form.
 */
async function call<T>(method: string, path: string, body?: RecordInput): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: body ? { "content-type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
    cache: "no-store",
  });
  if (res.status === 401) useAuthStore.getState().setLoggedOut(SESSION_ENDED);
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

/** GET /probe/health (public). Resolves with the body even on 503 (degraded). */
export async function fetchHealth(): Promise<Health> {
  const res = await fetch(`${BASE}/health`, { cache: "no-store" });
  if (res.status !== 200 && res.status !== 503) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as Health;
}

export function fetchClientConfig(): Promise<ClientConfig> {
  return call<ClientConfig>("GET", "/config.json");
}

// ---- Login -------------------------------------------------------------------

/** Asks the server who is logged in, and updates the auth store. */
export async function checkSession(): Promise<void> {
  try {
    const res = await fetch(`${BASE}/api/auth/me`, { cache: "no-store" });
    if (res.ok) useAuthStore.getState().setLoggedIn(((await res.json()) as Me).username);
    else if (res.status === 401) {
      // Logged out already: keep the notice; on first load there is none.
      const s = useAuthStore.getState();
      if (s.state !== "logged-out") s.setLoggedOut(s.state === "logged-in" ? SESSION_ENDED : undefined);
    }
    // Other errors (server down, 502 while the probe is off): leave the state alone.
  } catch {
    // Network error: same.
  }
}

/** Resolves on success; rejects with a message for the login form. */
export async function login(credentials: LoginRequest): Promise<void> {
  let res: Response;
  try {
    res = await fetch(`${BASE}/api/auth/login`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(credentials),
      cache: "no-store",
    });
  } catch {
    throw new Error("The server is not reachable.");
  }
  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as ApiError | null;
    throw new Error(loginErrorMessage(res.status, body, res.headers.get("retry-after")));
  }
  useAuthStore.getState().setLoggedIn(((await res.json()) as Me).username);
}

/** Ends this session (or every session of the user). */
export async function logout(everywhere = false): Promise<void> {
  let ok = false;
  try {
    const res = await fetch(`${BASE}/api/auth/${everywhere ? "logout-all" : "logout"}`, {
      method: "POST",
      cache: "no-store",
    });
    // 401: the session had already ended, which is just as good.
    ok = res.ok || res.status === 401;
  } catch {
    // Not reachable: see below.
  }
  // Stop showing data either way, but say so if the server may not know.
  useAuthStore
    .getState()
    .setLoggedOut(
      !ok
        ? "Logout didn't reach the server, so the session may still be active there. Log in and out again to be sure."
        : everywhere
          ? "Logged out on all devices."
          : "Logged out.",
    );
}
