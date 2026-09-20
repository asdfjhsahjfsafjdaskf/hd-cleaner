import { useCallback, useEffect, useRef, useState, type MouseEvent } from "react";
import { api } from "../services/api";
import { useT } from "../i18n";
import { CATEGORY_ORDER, CATEGORY_RGB, DIR_BLOCK_RGB, shade } from "../utils/colors";
import { formatBytes, formatDate } from "../utils/format";
import type { NodeDetails } from "../types";

const RECT_BYTES = 24;
const KIND_FILE = 0;
const KIND_DIR_BLOCK = 1;
const KIND_FRAME = 2;
const FLAG_HIGHLIGHT = 1;
const CELL = 32;

interface Layout {
  n: number;
  x: Float32Array;
  y: Float32Array;
  w: Float32Array;
  h: Float32Array;
  id: Uint32Array;
  cat: Uint8Array;
  depth: Uint8Array;
  kind: Uint8Array;
  flags: Uint8Array;
  grid: Map<number, number[]>;
  cols: number;
  anyHighlight: boolean;
}

function parse(buf: ArrayBuffer, width: number): Layout {
  const n = Math.floor(buf.byteLength / RECT_BYTES);
  const dv = new DataView(buf);
  const L: Layout = {
    n,
    x: new Float32Array(n), y: new Float32Array(n), w: new Float32Array(n), h: new Float32Array(n),
    id: new Uint32Array(n), cat: new Uint8Array(n), depth: new Uint8Array(n), kind: new Uint8Array(n), flags: new Uint8Array(n),
    grid: new Map(), cols: Math.max(1, Math.ceil(width / CELL)), anyHighlight: false,
  };
  for (let i = 0; i < n; i++) {
    const o = i * RECT_BYTES;
    L.x[i] = dv.getFloat32(o, true);
    L.y[i] = dv.getFloat32(o + 4, true);
    L.w[i] = dv.getFloat32(o + 8, true);
    L.h[i] = dv.getFloat32(o + 12, true);
    L.id[i] = dv.getUint32(o + 16, true);
    L.cat[i] = dv.getUint8(o + 20);
    L.depth[i] = dv.getUint8(o + 21);
    L.kind[i] = dv.getUint8(o + 22);
    L.flags[i] = dv.getUint8(o + 23);
    if (L.flags[i] & FLAG_HIGHLIGHT) L.anyHighlight = true;
    if (L.kind[i] === KIND_FRAME) continue;
    // Spatial index of leaves for fast hit testing.
    const c0 = Math.floor(L.x[i] / CELL), c1 = Math.floor((L.x[i] + L.w[i]) / CELL);
    const r0 = Math.floor(L.y[i] / CELL), r1 = Math.floor((L.y[i] + L.h[i]) / CELL);
    for (let r = r0; r <= r1; r++) {
      for (let c = c0; c <= c1; c++) {
        const k = r * L.cols + c;
        let cell = L.grid.get(k);
        if (!cell) L.grid.set(k, (cell = []));
        cell.push(i);
      }
    }
  }
  return L;
}

function hit(L: Layout, px: number, py: number): number {
  const cell = L.grid.get(Math.floor(py / CELL) * L.cols + Math.floor(px / CELL));
  if (!cell) return -1;
  for (const i of cell) {
    if (px >= L.x[i] && px < L.x[i] + L.w[i] && py >= L.y[i] && py < L.y[i] + L.h[i]) return i;
  }
  return -1;
}

function hitFrame(L: Layout, px: number, py: number): number {
  let best = -1;
  for (let i = 0; i < L.n; i++) {
    if (L.kind[i] !== KIND_FRAME) continue;
    if (px >= L.x[i] && px < L.x[i] + L.w[i] && py >= L.y[i] && py < L.y[i] + L.h[i]) {
      if (best < 0 || L.depth[i] > L.depth[best]) best = i;
    }
  }
  return best;
}

function draw(canvas: HTMLCanvasElement, L: Layout, w: number, h: number, bg: string) {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(w * dpr);
  canvas.height = Math.round(h * dpr);
  const ctx = canvas.getContext("2d", { alpha: false })!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  const dim = L.anyHighlight;
  for (let i = 0; i < L.n; i++) {
    const k = L.kind[i];
    if (k === KIND_FRAME) continue;
    const rgb = k === KIND_DIR_BLOCK ? DIR_BLOCK_RGB : CATEGORY_RGB[L.cat[i]] ?? CATEGORY_RGB[0];
    const f = dim && !(L.flags[i] & FLAG_HIGHLIGHT) ? 0.32 : 1;
    const x = L.x[i], y = L.y[i], rw = L.w[i], rh = L.h[i];
    ctx.fillStyle = shade(rgb, 0.92 * f);
    ctx.fillRect(x, y, rw, rh);
    if (rw > 4 && rh > 4) {
      // Light top/left and dark bottom/right edges: a cheap bevel.
      ctx.fillStyle = shade(rgb, 1.18 * f);
      ctx.fillRect(x, y, rw, 1);
      ctx.fillRect(x, y, 1, rh);
      ctx.fillStyle = shade(rgb, 0.62 * f);
      ctx.fillRect(x, y + rh - 1, rw, 1);
      ctx.fillRect(x + rw - 1, y, 1, rh);
    }
  }
  // Directory outlines give the nesting structure.
  ctx.lineWidth = 1;
  for (let i = 0; i < L.n; i++) {
    if (L.kind[i] !== KIND_FRAME || L.w[i] < 6 || L.h[i] < 6) continue;
    ctx.strokeStyle = L.depth[i] <= 1 ? "rgba(0,0,0,0.75)" : "rgba(0,0,0,0.4)";
    ctx.strokeRect(L.x[i] + 0.5, L.y[i] + 0.5, L.w[i] - 1, L.h[i] - 1);
  }
}

interface Props {
  scanId: number;
  root: number;
  version: number;
  metric: "allocated" | "logical";
  maxRects: number;
  highlight: number[];
  selected?: number;
  onSelect: (id: number) => void;
  onZoom: (id: number) => void;
  onActivate: (id: number) => void;
  onContextMenu: (id: number, x: number, y: number) => void;
}

export function Treemap(props: Props) {
  const { scanId, root, version, metric, maxRects, highlight, selected, onSelect, onZoom, onActivate, onContextMenu } = props;
  const t = useT();
  const hostRef = useRef<HTMLDivElement>(null);
  const baseRef = useRef<HTMLCanvasElement>(null);
  const overRef = useRef<HTMLCanvasElement>(null);
  const layoutRef = useRef<Layout | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [loading, setLoading] = useState(false);
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);
  const [tip, setTip] = useState<NodeDetails | null>(null);
  const tipCache = useRef(new Map<number, NodeDetails>());

  useEffect(() => {
    const el = hostRef.current;
    if (!el) return;
    let timer: number | undefined;
    const ro = new ResizeObserver(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setSize({ w: el.clientWidth, h: el.clientHeight }), 120);
    });
    ro.observe(el);
    setSize({ w: el.clientWidth, h: el.clientHeight });
    return () => {
      ro.disconnect();
      window.clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    tipCache.current.clear();
  }, [scanId, version]);

  const drawOverlay = useCallback(() => {
    const L = layoutRef.current;
    const c = overRef.current;
    if (!c || !L) return;
    const dpr = window.devicePixelRatio || 1;
    if (c.width !== Math.round(size.w * dpr) || c.height !== Math.round(size.h * dpr)) {
      c.width = Math.round(size.w * dpr);
      c.height = Math.round(size.h * dpr);
    }
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, size.w, size.h);
    const accent = getComputedStyle(document.documentElement).getPropertyValue("--accent").trim() || "#4dd0b4";
    if (L.anyHighlight) {
      ctx.strokeStyle = accent;
      ctx.lineWidth = 1;
      for (let i = 0; i < L.n; i++) {
        if (L.kind[i] === KIND_FRAME && L.flags[i] & FLAG_HIGHLIGHT && L.w[i] > 4 && L.h[i] > 4) {
          ctx.strokeRect(L.x[i] + 0.5, L.y[i] + 0.5, L.w[i] - 1, L.h[i] - 1);
        }
      }
    }
    if (selected !== undefined) {
      for (let i = 0; i < L.n; i++) {
        if (L.id[i] === selected) {
          ctx.strokeStyle = "#ffffff";
          ctx.lineWidth = 2;
          ctx.strokeRect(L.x[i] + 1, L.y[i] + 1, Math.max(0, L.w[i] - 2), Math.max(0, L.h[i] - 2));
          break;
        }
      }
    }
    if (hover && hover.i >= 0) {
      const i = hover.i;
      ctx.fillStyle = "rgba(255,255,255,0.18)";
      ctx.fillRect(L.x[i], L.y[i], L.w[i], L.h[i]);
    }
  }, [size, selected, hover]);

  useEffect(() => {
    if (!size.w || !size.h) return;
    let cancelled = false;
    setLoading(true);
    api
      .treemap(scanId, root, { width: size.w, height: size.h, metric, maxRects, highlight })
      .then((buf) => {
        if (cancelled) return;
        const L = parse(buf, size.w);
        layoutRef.current = L;
        const bg = getComputedStyle(document.documentElement).getPropertyValue("--bg").trim() || "#0f1318";
        if (baseRef.current) draw(baseRef.current, L, size.w, size.h, bg);
        drawOverlay();
      })
      .catch(() => {
        layoutRef.current = null;
      })
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
    // drawOverlay intentionally excluded: the overlay effect below handles it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scanId, root, version, size.w, size.h, metric, maxRects, highlight.join(",")]);

  useEffect(() => {
    drawOverlay();
  }, [drawOverlay]);

  // Tooltip data for the hovered item (small cache, debounced).
  useEffect(() => {
    const L = layoutRef.current;
    if (!hover || hover.i < 0 || !L) {
      setTip(null);
      return;
    }
    const id = L.id[hover.i];
    const cached = tipCache.current.get(id);
    if (cached) {
      setTip(cached);
      return;
    }
    const timer = window.setTimeout(() => {
      api.nodeDetails(scanId, id).then((d) => {
        if (tipCache.current.size > 500) tipCache.current.clear();
        tipCache.current.set(id, d);
        setTip(d);
      }).catch(() => setTip(null));
    }, 60);
    return () => window.clearTimeout(timer);
  }, [hover?.i, scanId]); // eslint-disable-line react-hooks/exhaustive-deps

  const pos = (e: MouseEvent) => {
    const r = hostRef.current!.getBoundingClientRect();
    return { px: e.clientX - r.left, py: e.clientY - r.top };
  };

  const onMove = (e: MouseEvent) => {
    const L = layoutRef.current;
    if (!L) return;
    const { px, py } = pos(e);
    const i = hit(L, px, py);
    setHover((h) => (h && h.i === i ? { ...h, x: e.clientX, y: e.clientY } : { i, x: e.clientX, y: e.clientY }));
  };

  const targetAt = (e: MouseEvent): { id: number; kind: number } | null => {
    const L = layoutRef.current;
    if (!L) return null;
    const { px, py } = pos(e);
    let i = hit(L, px, py);
    if (i < 0) i = hitFrame(L, px, py);
    return i < 0 ? null : { id: L.id[i], kind: L.kind[i] };
  };

  return (
    <div
      className="treemap-canvas-host"
      ref={hostRef}
      onMouseMove={onMove}
      onMouseLeave={() => setHover(null)}
      onClick={(e) => {
        const tg = targetAt(e);
        if (tg) onSelect(tg.id);
      }}
      onDoubleClick={(e) => {
        const tg = targetAt(e);
        if (!tg) return;
        if (tg.kind === KIND_FILE) onActivate(tg.id);
        else onZoom(tg.id);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        const tg = targetAt(e);
        if (tg) onContextMenu(tg.id, e.clientX, e.clientY);
      }}
      role="img"
      aria-label={t("analyzer.treemap")}
    >
      <canvas ref={baseRef} />
      <canvas ref={overRef} />
      {loading && <div style={{ position: "absolute", right: 8, top: 6 }} className="faint">{t("common.loading")}</div>}
      {hover && tip && hover.i >= 0 && (
        <div
          className="tooltip"
          style={{
            left: Math.min(hover.x + 14, window.innerWidth - 420),
            top: Math.min(hover.y + 14, window.innerHeight - 110),
          }}
        >
          <div className="t-name">{tip.name}</div>
          <div>
            {formatBytes(tip.alloc)} <span className="faint">· {t("common.size")} {formatBytes(tip.size)}</span>
            {tip.ext && <span className="faint"> · .{tip.ext}</span>}
            {tip.isDir && <span className="faint"> · {tip.files.toLocaleString()} {t("common.files").toLowerCase()}</span>}
          </div>
          <div className="faint">{formatDate(tip.modified)}</div>
          <div className="t-path">{tip.path}</div>
        </div>
      )}
    </div>
  );
}

export function TreemapLegend() {
  const t = useT();
  const shown = CATEGORY_ORDER.filter((c) => c !== "system");
  return (
    <div className="legend">
      {shown.map((c) => (
        <span key={c}>
          <i style={{ background: `rgb(${CATEGORY_RGB[CATEGORY_ORDER.indexOf(c)].join(",")})` }} />
          {t(`categories.${c}`)}
        </span>
      ))}
    </div>
  );
}
