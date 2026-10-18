import { useEffect } from "react";
import { CreateRecordForm } from "@/components/CreateRecordForm";
import { MqttLoginForm } from "@/components/MqttLoginForm";
import { RecordsTable } from "@/components/RecordsTable";
import { StatusPanel } from "@/components/StatusPanel";
import { TransportSelector } from "@/components/TransportSelector";
import { startProbe } from "./probe";

export function App() {
  useEffect(() => startProbe(), []);

  return (
    <div className="mx-auto flex min-h-full max-w-6xl flex-col gap-4 p-4">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Backend probe</h1>
          <p className="text-sm text-zinc-400">Blade of Grass · live records over HTTPS, WSS and MQTT</p>
        </div>
        <TransportSelector />
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
