import type { ConnectionType } from "@/generated/ConnectionType";

const COLORS: Record<ConnectionType, string> = {
  https: "bg-sky-900/60 text-sky-300",
  wss: "bg-violet-900/60 text-violet-300",
  mqtt: "bg-amber-900/60 text-amber-300",
};

export function ConnectionBadge({ type }: { type: ConnectionType }) {
  return <span className={`rounded px-1.5 py-0.5 font-mono text-xs uppercase ${COLORS[type]}`}>{type}</span>;
}
