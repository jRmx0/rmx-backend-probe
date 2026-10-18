import { selectTransport } from "@/app/probe";
import type { ConnectionType } from "@/generated/ConnectionType";
import { useConnectionStore } from "@/stores/connectionStore";

const OPTIONS: { value: ConnectionType; label: string }[] = [
  { value: "https", label: "HTTPS" },
  { value: "wss", label: "WSS" },
  { value: "mqtt", label: "MQTT" },
];

export function TransportSelector() {
  const selected = useConnectionStore((s) => s.selected);

  return (
    <div role="radiogroup" aria-label="Transport" className="flex rounded-md border border-zinc-700 p-0.5">
      {OPTIONS.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={selected === o.value}
          onClick={() => selectTransport(o.value)}
          className={`rounded px-3 py-1 text-sm font-medium ${
            selected === o.value ? "bg-teal-600 text-white" : "text-zinc-300 hover:bg-zinc-800"
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
