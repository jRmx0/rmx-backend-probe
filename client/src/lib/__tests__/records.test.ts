import { describe, expect, test } from "bun:test";
import type { RecordRow } from "@/generated/RecordRow";
import { applyEvent, sortRecords, upsertRecord } from "@/lib/records";

function row(id: number, time: string, value: string | null = null, updated_at = time): RecordRow {
  return { id, time, connection_type: "https", value, updated_at };
}

describe("records", () => {
  test("sorts newest first, then by id", () => {
    const sorted = sortRecords([
      row(1, "2026-10-18T10:00:00Z"),
      row(3, "2026-10-18T11:00:00Z"),
      row(2, "2026-10-18T11:00:00Z"),
    ]);
    expect(sorted.map((r) => r.id)).toEqual([3, 2, 1]);
  });

  test("upsert replaces newer and ignores stale copies", () => {
    const base = [row(1, "2026-10-18T10:00:00Z", "a", "2026-10-18T10:05:00Z")];
    const newer = row(1, "2026-10-18T10:00:00Z", "b", "2026-10-18T10:06:00Z");
    const stale = row(1, "2026-10-18T10:00:00Z", "old", "2026-10-18T10:01:00Z");
    expect(upsertRecord(base, newer)[0]?.value).toBe("b");
    expect(upsertRecord(base, stale)[0]?.value).toBe("a");
  });

  test("applies create, update and delete events idempotently", () => {
    const r = row(5, "2026-10-18T10:00:00Z", "x");
    let records = applyEvent([], { op: "create", record: r });
    records = applyEvent(records, { op: "create", record: r });
    expect(records).toHaveLength(1);

    records = applyEvent(records, {
      op: "update",
      record: { ...r, value: "y", connection_type: "mqtt", updated_at: "2026-10-18T10:01:00Z" },
    });
    expect(records[0]).toMatchObject({ value: "y", connection_type: "mqtt" });

    records = applyEvent(records, { op: "delete", id: 5, connection_type: "wss" });
    records = applyEvent(records, { op: "delete", id: 5, connection_type: "wss" });
    expect(records).toHaveLength(0);
  });
});
