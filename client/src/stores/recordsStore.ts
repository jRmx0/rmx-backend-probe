import { create } from "zustand";
import type { ChangeEvent } from "@/generated/ChangeEvent";
import type { RecordRow } from "@/generated/RecordRow";
import { applyEvent, sortRecords, upsertRecord } from "@/lib/records";

type RecordsState = {
  records: RecordRow[];
  loading: boolean;
  setAll: (records: RecordRow[]) => void;
  upsert: (record: RecordRow) => void;
  remove: (id: number) => void;
  apply: (event: ChangeEvent) => void;
  setLoading: (loading: boolean) => void;
};

export const useRecordsStore = create<RecordsState>((set) => ({
  records: [],
  loading: false,
  setAll: (records) => set({ records: sortRecords(records) }),
  upsert: (record) => set((s) => ({ records: upsertRecord(s.records, record) })),
  remove: (id) => set((s) => ({ records: s.records.filter((r) => r.id !== id) })),
  apply: (event) => set((s) => ({ records: applyEvent(s.records, event) })),
  setLoading: (loading) => set({ loading }),
}));
