import type { ReactNode } from "react";
import { reload } from "@/app/probe";
import { useConnectionStore } from "@/stores/connectionStore";

type Tone = "good" | "warn" | "bad" | "idle";

const DOT: Record<Tone, string> = {
  good: "bg-emerald-500",
  warn: "bg-amber-400",
  bad: "bg-red-500",
  idle: "bg-zinc-600",
};

function Line({ label, tone, children }: { label: string; tone: Tone; children: ReactNode }) {
  return (
    <div className="flex items-start gap-2 py-1">
      <span className={`mt-1.5 size-2 shrink-0 rounded-full ${DOT[tone]}`} />
      <span className="w-14 shrink-0 font-medium">{label}</span>
      <span className="min-w-0 break-words text-zinc-400">{children}</span>
    </div>
  );
}

export function StatusPanel() {
  const { https, wss, selected, lastOp } = useConnectionStore();

  const httpsTone: Tone = https.state === "ok" ? "good" : https.state === "degraded" ? "warn" : https.state === "down" ? "bad" : "idle";
  const httpsText =
    https.health !== undefined
      ? `${https.health.status} · db ${https.health.db ? "up" : "down"} · v${https.health.version}`
      : (https.detail ?? https.state);

  const wssTone: Tone = wss.state === "open" ? "good" : wss.state === "connecting" ? "warn" : "bad";

  // The server's own link to the broker (devices reach the server through it).
  const broker = https.health?.mqtt;
  const brokerTone: Tone = broker === undefined ? "idle" : broker ? "good" : "bad";

  return (
    <section className="rounded-md border border-zinc-800 p-3 text-sm">
      <div className="mb-1 flex items-center justify-between">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-zinc-400">Connections</h2>
        <button type="button" onClick={() => void reload()} className="rounded px-2 py-0.5 text-xs text-zinc-300 hover:bg-zinc-800">
          Reload
        </button>
      </div>
      <Line label="HTTPS" tone={httpsTone}>
        {httpsText}
      </Line>
      <Line label="WSS" tone={wssTone}>
        {wss.state}
        {wss.state !== "open" && wss.detail ? ` (${wss.detail})` : ""}
      </Line>
      <Line label="MQTT" tone={brokerTone}>
        {broker === undefined ? "unknown" : `server ↔ broker ${broker ? "up" : "down"}`} · devices only
      </Line>

      <div className="mt-2 border-t border-zinc-800 pt-2 text-xs text-zinc-400">
        <div>
          Requests via <b className="text-zinc-200">{selected.toUpperCase()}</b>, live events via{" "}
          <b className="text-zinc-200">WSS</b>
        </div>
        {lastOp && (
          <div className={lastOp.error ? "text-red-400" : ""}>
            Last: {lastOp.op} via {lastOp.transport.toUpperCase()} in {lastOp.ms.toFixed(0)} ms
            {lastOp.error ? ` — ${lastOp.error}` : ""}
          </div>
        )}
      </div>
    </section>
  );
}
