import type { ChangeEvent } from "@/generated/ChangeEvent";
import type { RecordRow } from "@/generated/RecordRow";

/** Newest first, like the server's list. */
export function sortRecords(records: RecordRow[]): RecordRow[] {
  return [...records].sort((a, b) => b.time.localeCompare(a.time) || b.id - a.id);
}

/** Inserts or replaces a record, ignoring copies older than the one we have. */
export function upsertRecord(records: RecordRow[], record: RecordRow): RecordRow[] {
  const current = records.find((r) => r.id === record.id);
  if (current && current.updated_at > record.updated_at) return records;
  return sortRecords([...records.filter((r) => r.id !== record.id), record]);
}

/**
 * Applies a change event from any transport. Safe to apply the same event
 * twice (for example once from the request's own response and once from the
 * broadcast).
 */
export function applyEvent(records: RecordRow[], event: ChangeEvent): RecordRow[] {
  switch (event.op) {
    case "create":
    case "update":
      return upsertRecord(records, event.record);
    case "delete":
      return records.filter((r) => r.id !== event.id);
  }
}
