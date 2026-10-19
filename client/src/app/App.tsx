import { useEffect } from "react";
import { CreateRecordForm } from "@/components/CreateRecordForm";
import { LoginForm } from "@/components/LoginForm";
import { MqttLoginForm } from "@/components/MqttLoginForm";
import { RecordsTable } from "@/components/RecordsTable";
import { StatusPanel } from "@/components/StatusPanel";
import { TransportSelector } from "@/components/TransportSelector";
import { UserMenu } from "@/components/UserMenu";
import { useAuthStore } from "@/stores/authStore";
import { checkSession } from "@/transports/https";
import { startProbe } from "./probe";

/** Login form or the probe, depending on whether the server knows our session. */
export function App() {
  const state = useAuthStore((s) => s.state);
  useEffect(() => void checkSession(), []);

  if (state === "checking") return <p className="p-4 text-sm text-zinc-400">Loading…</p>;
  if (state === "logged-out") return <LoginForm />;
  return <Probe />;
}

function Probe() {
  useEffect(() => startProbe(), []);

  return (
    <div className="mx-auto flex min-h-full max-w-6xl flex-col gap-4 p-4">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Backend probe</h1>
          <p className="text-sm text-zinc-400">Blade of Grass · live records over HTTPS, WSS and MQTT</p>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <TransportSelector />
          <UserMenu />
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[1fr_20rem]">
        <main className="flex min-w-0 flex-col gap-3">
          <CreateRecordForm />
          <RecordsTable />
        </main>
        <aside className="flex flex-col gap-4">
          <StatusPanel />
          <MqttLoginForm />
        </aside>
      </div>
    </div>
  );
}

export default App;
