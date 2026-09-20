import { create } from "zustand";
import { api, toError } from "../services/api";
import type { ErrorPayload, ScanMeta, ScanProgress, SearchSummary, SortKey } from "../types";
import { useApp } from "./app";

export interface ScanJob {
  id: number;
  root: string;
  startedAt: number;
  elevated: boolean;
  progress?: ScanProgress;
}

interface PreservedState {
  root: string;
  expanded: string[];
  selectedPath?: string;
  treemapRootPath?: string;
}

interface AnalyzerState {
  target: string;
  job?: ScanJob;
  scanId?: number;
  meta?: ScanMeta;
  fromSnapshot: boolean;
  error?: ErrorPayload;
  viewId?: number;
  /** Incremented whenever rows must be re-fetched (expand, sort, delete...). */
  version: number;
  total: number;
  selected?: number;
  treemapRoot?: number;
  highlight: number[];
  sort: SortKey;
  desc: boolean;
  query: string;
  results?: SearchSummary;
  resultSort: SortKey;
  resultDesc: boolean;
  /** Program whose folders should be highlighted once the next scan loads. */
  pendingProgramMap?: string;
  /** Search to run once the next scan loads. */
  pendingSearch?: string;
  /** Row the tree view should scroll to (set by reveal from outside the table). */
  scrollRequest?: { index: number; seq: number };
  set: (p: Partial<AnalyzerState>) => void;
  showProgramOnMap: (programId: string) => Promise<void>;
  setTarget: (t: string) => void;
  startScan: (root: string, elevate: boolean) => Promise<void>;
  cancelScan: () => Promise<void>;
  rescan: () => Promise<void>;
  adoptScan: (scanId: number, meta: ScanMeta, fromSnapshot: boolean, preserved?: PreservedState) => Promise<void>;
  bump: (total?: number) => void;
  /** Re-read row count and totals after the tree changed (e.g. deletion). */
  refresh: () => Promise<void>;
  select: (id?: number) => void;
  setTreemapRoot: (id?: number) => void;
  setHighlight: (ids: number[]) => void;
  toggle: (node: number, expanded: boolean) => Promise<void>;
  expandAll: () => Promise<void>;
  collapseAll: () => Promise<void>;
  setSort: (sort: SortKey) => Promise<void>;
  reveal: (node: number) => Promise<number | null>;
  runSearch: (q: string) => Promise<void>;
  clearSearch: () => void;
  setResultSort: (sort: SortKey) => Promise<void>;
}

export const useAnalyzer = create<AnalyzerState>((set, get) => ({
  target: "",
  fromSnapshot: false,
  version: 0,
  total: 0,
  highlight: [],
  sort: "alloc",
  desc: true,
  query: "",
  resultSort: "size",
  resultDesc: true,

  setTarget: (target) => set({ target }),
  set: (p) => set(p),
  showProgramOnMap: async (programId) => {
    const { scanId } = get();
    if (scanId === undefined) return;
    try {
      const r = await api.programMapNodes(scanId, programId);
      set({ highlight: r.nodes, treemapRoot: undefined, results: undefined, query: "", selected: r.nodes[0] });
      if (r.nodes[0] !== undefined) await get().reveal(r.nodes[0]);
    } catch (e) {
      useApp.getState().toastError(e);
    }
  },

  startScan: async (root, elevate) => {
    const { job } = get();
    if (job) return;
    const app = useApp.getState();
    // Preserve UI state when re-scanning the same location.
    let preserved: PreservedState | undefined;
    const { viewId, meta, scanId, selected, treemapRoot } = get();
    if (viewId && meta && scanId && meta.rootPath.toLowerCase() === root.toLowerCase()) {
      try {
        preserved = {
          root,
          expanded: await api.viewExpandedPaths(viewId),
          selectedPath: selected !== undefined ? (await api.nodeDetails(scanId, selected)).path : undefined,
          treemapRootPath: treemapRoot !== undefined ? (await api.nodeDetails(scanId, treemapRoot)).path : undefined,
        };
      } catch {
        preserved = undefined;
      }
    }
    // The job is shown immediately; a terminal event may even arrive before
    // `invoke` resolves (tiny folders), so `finished` guards the late update.
    let finished = false;
    set({ error: undefined, job: { id: -1, root, startedAt: Date.now(), elevated: elevate } });
    try {
      const id = await api.startScan(root, "auto", elevate, app.settings.followJunctions, (e) => {
        switch (e.event) {
          case "progress":
            set((s) => (s.job ? { job: { ...s.job, progress: e.data } } : {}));
            break;
          case "done":
            finished = true;
            set({ job: undefined });
            get().adoptScan(e.data.scanId, e.data.meta, false, preserved);
            break;
          case "cancelled":
            finished = true;
            set({ job: undefined });
            break;
          case "error":
            finished = true;
            set({ job: undefined, error: e.data });
            break;
        }
      });
      if (!finished) set((s) => (s.job ? { job: { ...s.job, id } } : {}));
    } catch (e) {
      set({ job: undefined, error: toError(e) });
    }
  },

  cancelScan: async () => {
    const job = get().job;
    if (job) await api.cancelJob(job.id);
  },

  rescan: async () => {
    const { meta, job, fromSnapshot } = get();
    if (!meta || job || fromSnapshot) return;
    const elevated = meta.method === "ntfsMft" && meta.notes.includes("elevatedHelper");
    await get().startScan(meta.rootPath, elevated);
  },

  adoptScan: async (scanId, meta, fromSnapshot, preserved) => {
    const old = get().viewId;
    if (old) api.viewClose(old).catch(() => {});
    try {
      const viewId = await api.viewOpen(scanId);
      let total = 1;
      let selected: number | undefined;
      let treemapRoot: number | undefined;
      if (preserved) {
        total = await api.viewRestore(viewId, preserved.expanded);
        if (preserved.selectedPath) selected = (await api.findPath(scanId, preserved.selectedPath)) ?? undefined;
        if (preserved.treemapRootPath) treemapRoot = (await api.findPath(scanId, preserved.treemapRootPath)) ?? undefined;
      } else {
        total = await api.viewRows(viewId, 0, 1).then((p) => p.total);
      }
      set({
        scanId, meta, fromSnapshot, viewId, total, selected, treemapRoot, highlight: [],
        target: meta.rootPath, version: get().version + 1, results: undefined, query: "",
        sort: "alloc", desc: true,
      });
      const pending = get().pendingProgramMap;
      if (pending && !fromSnapshot) {
        set({ pendingProgramMap: undefined });
        await get().showProgramOnMap(pending);
      }
      const search = get().pendingSearch;
      if (search && !fromSnapshot) {
        set({ pendingSearch: undefined });
        await get().runSearch(search);
      }
    } catch (e) {
      set({ error: toError(e) });
    }
  },

  bump: (total) => set((s) => ({ version: s.version + 1, total: total ?? s.total })),
  refresh: async () => {
    const { viewId, scanId, meta } = get();
    if (!viewId || scanId === undefined || !meta) return get().bump();
    try {
      const [page, root] = await Promise.all([api.viewRows(viewId, 0, 1), api.nodeDetails(scanId, 0)]);
      set((s) => ({
        total: page.total,
        version: s.version + 1,
        meta: { ...meta, files: root.files, dirs: root.dirs, totalSize: root.size, totalAlloc: root.alloc },
      }));
    } catch {
      get().bump();
    }
  },
  select: (selected) => set({ selected }),
  setTreemapRoot: (treemapRoot) => set({ treemapRoot }),
  setHighlight: (highlight) => set({ highlight }),

  toggle: async (node, expanded) => {
    const { viewId } = get();
    if (!viewId) return;
    const total = await api.viewSetExpanded(viewId, node, expanded);
    get().bump(total);
  },
  expandAll: async () => {
    const { viewId } = get();
    if (!viewId) return;
    get().bump(await api.viewExpandAll(viewId));
  },
  collapseAll: async () => {
    const { viewId } = get();
    if (!viewId) return;
    get().bump(await api.viewCollapseAll(viewId));
  },
  setSort: async (sort) => {
    const { viewId, sort: cur, desc } = get();
    if (!viewId) return;
    const nextDesc = sort === cur ? !desc : sort !== "name" && sort !== "ext";
    set({ sort, desc: nextDesc });
    get().bump(await api.viewSort(viewId, sort, nextDesc));
  },
  reveal: async (node) => {
    const { viewId } = get();
    if (!viewId) return null;
    const idx = await api.viewReveal(viewId, node);
    const total = await api.viewRows(viewId, 0, 1).then((p) => p.total);
    set((s) => ({ selected: node, scrollRequest: idx === null ? s.scrollRequest : { index: idx, seq: (s.scrollRequest?.seq ?? 0) + 1 } }));
    get().bump(total);
    return idx;
  },

  runSearch: async (q) => {
    const { scanId, resultSort, resultDesc } = get();
    set({ query: q });
    if (!scanId || !q.trim()) {
      set({ results: undefined });
      return;
    }
    try {
      const results = await api.search(scanId, q, { sort: resultSort, desc: resultDesc });
      set({ results, version: get().version + 1 });
    } catch (e) {
      useApp.getState().toastError(e);
    }
  },
  clearSearch: () => set({ query: "", results: undefined }),
  setResultSort: async (sort) => {
    const { results, resultSort, resultDesc } = get();
    const desc = sort === resultSort ? !resultDesc : sort !== "name" && sort !== "ext" && sort !== "path";
    set({ resultSort: sort, resultDesc: desc });
    if (results) {
      await api.resultSort(results.resultId, sort, desc);
      get().bump();
    }
  },
}));
