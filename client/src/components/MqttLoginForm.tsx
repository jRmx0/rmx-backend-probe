import { useState, type FormEvent } from "react";
import { connectMqtt, disconnectMqtt, mqttClientId } from "@/app/probe";
import { useConnectionStore } from "@/stores/connectionStore";

/**
 * The MQTT login is typed in by the user and kept only in memory: no
 * username or password is ever built into the client.
 */
export function MqttLoginForm() {
  const [username, setUsername] = useState("probe-web");
  const [password, setPassword] = useState("");
  const state = useConnectionStore((s) => s.mqtt.state);
  const url = useConnectionStore((s) => s.config?.mqtt_ws_url);
  const active = state === "connected" || state === "connecting";

  function submit(e: FormEvent) {
    e.preventDefault();
    if (active) disconnectMqtt();
    else connectMqtt({ username, password });
  }

  const input =
    "w-full rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1 text-sm outline-none focus:border-teal-500 disabled:opacity-60";

  return (
    <form onSubmit={submit} className="flex flex-col gap-2 rounded-md border border-zinc-800 p-3 text-sm">
      <h2 className="text-xs font-semibold uppercase tracking-wide text-zinc-400">MQTT login</h2>
      <p className="break-all text-xs text-zinc-500">
        {url ? url : "Broker URL not configured on the server"} · client {mqttClientId}
      </p>
      <input
        className={input}
        value={username}
        onChange={(e) => setUsername(e.target.value)}
        placeholder="Username"
        aria-label="MQTT username"
        autoComplete="username"
        disabled={active}
      />
      <input
        className={input}
        type="password"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        placeholder="Password"
        aria-label="MQTT password"
        autoComplete="current-password"
        disabled={active}
      />
      <button
        type="submit"
        disabled={!active && (!username || !password)}
        className="rounded-md bg-zinc-800 px-3 py-1.5 font-medium hover:bg-zinc-700 disabled:opacity-50"
      >
        {active ? "Disconnect" : "Connect"}
      </button>
    </form>
  );
}
