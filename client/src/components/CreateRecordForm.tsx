import { useState, type FormEvent } from "react";
import { createRecord } from "@/app/probe";
import { useConnectionStore } from "@/stores/connectionStore";

export function CreateRecordForm() {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const selected = useConnectionStore((s) => s.selected);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    const submitted = value;
    try {
      await createRecord(submitted === "" ? null : submitted);
      // Keep anything typed while the request was in flight.
      setValue((current) => (current === submitted ? "" : current));
    } catch {
      // The status panel shows the error.
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} className="flex gap-2">
      <input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="Value (empty = null)"
        aria-label="New record value"
        maxLength={1000}
        className="min-w-0 flex-1 rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm outline-none focus:border-teal-500"
      />
      <button
        type="submit"
        disabled={busy}
        className="rounded-md bg-teal-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-teal-500 disabled:opacity-50"
      >
        Create via {selected.toUpperCase()}
      </button>
    </form>
  );
}
