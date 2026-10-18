import { useState } from "react";
import { deleteRecord, updateRecord } from "@/app/probe";
import type { RecordRow } from "@/generated/RecordRow";
import { useRecordsStore } from "@/stores/recordsStore";
import { ConnectionBadge } from "./ConnectionBadge";

const timeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "medium" });

export function RecordsTable() {
  const records = useRecordsStore((s) => s.records);
  const loading = useRecordsStore((s) => s.loading);

  return (
    <div className="overflow-x-auto rounded-md border border-zinc-800">
      <table className="w-full text-left text-sm">
        <thead className="bg-zinc-900 text-xs uppercase text-zinc-400">
          <tr>
            <th className="px-3 py-2 font-medium">Time</th>
            <th className="px-3 py-2 font-medium">Connection type</th>
            <th className="px-3 py-2 font-medium">Value</th>
            <th className="px-3 py-2">
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {records.map((r) => (
            <Row key={r.id} record={r} />
          ))}
          {records.length === 0 && (
            <tr>
              <td colSpan={4} className="px-3 py-6 text-center text-zinc-500">
                {loading ? "Loading…" : "No records yet."}
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

function Row({ record }: { record: RecordRow }) {
  // The text being edited, or null when not editing.
  const [draft, setDraft] = useState<string | null>(null);

  async function save() {
    if (draft === null) return;
    try {
      await updateRecord(record.id, draft === "" ? null : draft);
      setDraft(null);
    } catch {
      // The status panel shows the error; keep the edit open.
    }
  }

  return (
    <tr className="border-t border-zinc-800 hover:bg-zinc-900/50">
      <td className="whitespace-nowrap px-3 py-1.5 tabular-nums text-zinc-300" title={`id ${record.id}`}>
        {timeFormat.format(new Date(record.time))}
      </td>
      <td className="px-3 py-1.5">
        <ConnectionBadge type={record.connection_type} />
      </td>
      <td className="w-full break-all px-3 py-1.5">
        {draft === null ? (
          <span className={record.value === null ? "italic text-zinc-500" : ""}>{record.value ?? "null"}</span>
        ) : (
          <input
            autoFocus
            value={draft}
            maxLength={1000}
            aria-label="Edit value"
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void save();
              if (e.key === "Escape") setDraft(null);
            }}
            className="w-full rounded border border-zinc-600 bg-zinc-900 px-2 py-0.5 outline-none focus:border-teal-500"
          />
        )}
      </td>
      <td className="whitespace-nowrap px-3 py-1.5 text-right">
        {draft === null ? (
          <>
            <RowButton onClick={() => setDraft(record.value ?? "")}>Edit</RowButton>
            <RowButton danger onClick={() => void deleteRecord(record.id).catch(() => {})}>
              Delete
            </RowButton>
          </>
        ) : (
          <>
            <RowButton onClick={() => void save()}>Save</RowButton>
            <RowButton onClick={() => setDraft(null)}>Cancel</RowButton>
          </>
        )}
      </td>
    </tr>
  );
}

function RowButton({ children, onClick, danger }: { children: string; onClick: () => void; danger?: boolean }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`ml-1 rounded px-2 py-0.5 text-xs ${
        danger ? "text-red-400 hover:bg-red-950" : "text-zinc-300 hover:bg-zinc-800"
      }`}
    >
      {children}
    </button>
  );
}
