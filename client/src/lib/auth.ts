import type { ApiError } from "@/generated/ApiError";

/** The message shown when a login attempt fails. */
export function loginErrorMessage(status: number, body: ApiError | null, retryAfter: string | null): string {
  if (status === 401) return "Wrong username or password.";
  if (status === 429) {
    const seconds = Number(retryAfter);
    return Number.isFinite(seconds) && seconds > 0
      ? `Too many failed attempts. Try again in ${formatWait(seconds)}.`
      : "Too many failed attempts. Try again later.";
  }
  if (status === 403) return "This page's address is not allowed to log in (check ALLOWED_ORIGINS).";
  return body?.message ?? `Login failed (HTTP ${status}).`;
}

function formatWait(seconds: number): string {
  if (seconds < 90) return `${Math.ceil(seconds)} s`;
  return `${Math.ceil(seconds / 60)} min`;
}
