import { create } from "zustand";
import { clearProgramIcons } from "../components/ProgramIcon";
import { api, toError } from "../services/api";
import type { AppSize, ErrorPayload, Program } from "../types";
import { useApp } from "./app";

export type SourceFilter = "all" | "win32" | "msi" | "store";
export type ProgramSort = "name" | "publisher" | "version" | "installDate" | "reported" | "real";

interface ProgramsState {
  loaded: boolean;
  loading: boolean;
  programs: Program[];
  appxError?: ErrorPayload | null;
  error?: ErrorPayload;
  /** Real size totals already computed: id → [total, possible]. */
  sizes: Record<string, [number, number]>;
  details: Record<string, AppSize>;
  selected?: string;
  /** Programs checked for batch uninstall. */
  checked: Set<string>;
  toggleChecked: (id: string) => void;
  query: string;
  source: SourceFilter;
  showHidden: boolean;
  sort: ProgramSort;
  desc: boolean;
  job?: { id: number; done: number; total: number };
  load: (refresh?: boolean) => Promise<void>;
  select: (id?: string) => void;
  set: (p: Partial<ProgramsState>) => void;
  computeSize: (id: string, refresh?: boolean) => Promise<AppSize | undefined>;
  computeAll: (ids: string[]) => Promise<void>;
  cancelAll: () => Promise<void>;
  setSort: (s: ProgramSort) => void;
}

export const isHidden = (p: Program) => p.systemComponent || p.isUpdate || p.isFramework;

export const usePrograms = create<ProgramsState>((set, get) => ({
  loaded: false,
  loading: false,
  programs: [],
  sizes: {},
  details: {},
  checked: new Set(),
  toggleChecked: (id) =>
    set((st) => {
      const c = new Set(st.checked);
      if (c.has(id)) c.delete(id);
      else c.add(id);
      return { checked: c };
    }),
  query: "",
  source: "all",
  showHidden: false,
  sort: "name",
  desc: false,

  load: async (refresh = false) => {
    if (get().loading) return;
    set({ loading: true, error: undefined });
    if (refresh) clearProgramIcons();
    try {
      const [list, cached] = await Promise.all([api.listPrograms(refresh), api.programSizesCached()]);
      const sizes: Record<string, [number, number]> = refresh ? {} : { ...get().sizes };
      for (const [id, total, possible] of cached) sizes[id] = [total, possible];
      // Drop checks for programs that no longer exist (e.g. just uninstalled).
      const ids = new Set(list.programs.map((p) => p.id));
      const checked = new Set([...get().checked].filter((id) => ids.has(id)));
      set({ programs: list.programs, appxError: list.appxError, loaded: true, sizes, details: refresh ? {} : get().details, checked });
    } catch (e) {
      set({ error: toError(e) });
    } finally {
      set({ loading: false });
    }
  },
  select: (selected) => set({ selected }),
  set: (p) => set(p),
  computeSize: async (id, refresh = false) => {
    try {
      const s = await api.programSize(id, refresh);
      set((st) => ({ details: { ...st.details, [id]: s }, sizes: { ...st.sizes, [id]: [s.total, s.possibleTotal] } }));
      return s;
    } catch (e) {
      useApp.getState().toastError(e);
      return undefined;
    }
  },
  computeAll: async (ids) => {
    if (get().job) return;
    let finished = false;
    set({ job: { id: -1, done: 0, total: ids.length } });
    try {
      const jobId = await api.programSizesAll(ids, (e) => {
        if (e.event === "progress") {
          set((st) => ({
            job: st.job ? { ...st.job, done: e.data.done, total: e.data.total } : st.job,
            sizes: { ...st.sizes, [e.data.programId]: [e.data.totalSize, st.sizes[e.data.programId]?.[1] ?? 0] },
          }));
        } else {
          finished = true;
          set({ job: undefined });
          // Refresh "possible" totals from the backend cache.
          api.programSizesCached().then((c) => {
            const sizes = { ...get().sizes };
            for (const [id, total, possible] of c) sizes[id] = [total, possible];
            set({ sizes });
          });
        }
      });
      if (!finished) set((st) => (st.job ? { job: { ...st.job, id: jobId } } : {}));
    } catch (e) {
      set({ job: undefined });
      useApp.getState().toastError(e);
    }
  },
  cancelAll: async () => {
    const j = get().job;
    if (j && j.id > 0) await api.cancelJob(j.id);
  },
  setSort: (sort) => {
    const { sort: cur, desc } = get();
    set({ sort, desc: sort === cur ? !desc : sort === "reported" || sort === "real" || sort === "installDate" });
  },
}));
