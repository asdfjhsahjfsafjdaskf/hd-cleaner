import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDown, ArrowUp } from "lucide-react";
import { forwardRef, useEffect, useImperativeHandle, useRef, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { usePagedRows } from "../hooks/usePagedRows";
import { useApp } from "../stores/app";
import type { Page } from "../types";

export interface Column<T> {
  key: string;
  label: string;
  /** Fixed width in rem, or undefined for flexible. */
  width?: number;
  align?: "right";
  sortKey?: string;
  render: (row: T) => ReactNode;
  className?: string;
}

export interface VirtualTableHandle {
  scrollToIndex: (i: number) => void;
  focus: () => void;
}

interface Props<T> {
  columns: Column<T>[];
  total: number;
  version: number | string;
  fetchPage: (offset: number, limit: number) => Promise<Page<T>>;
  rowId: (row: T) => number;
  selected?: number;
  sort?: string;
  desc?: boolean;
  onSort?: (key: string) => void;
  onSelect?: (row: T, index: number) => void;
  onActivate?: (row: T) => void;
  onContextMenu?: (row: T, e: MouseEvent) => void;
  onKeyDown?: (e: KeyboardEvent, row: T | undefined) => void;
  ariaLabel: string;
}

function VirtualTableInner<T>(props: Props<T>, ref: React.Ref<VirtualTableHandle>) {
  const { columns, total, version, fetchPage, rowId, selected, sort, desc, onSort, onSelect, onActivate, onContextMenu, onKeyDown, ariaLabel } = props;
  const scale = useApp((s) => s.settings.fontScale);
  const rowH = Math.round(26 * scale);
  const parentRef = useRef<HTMLDivElement>(null);
  const { get, ensure } = usePagedRows(version, fetchPage);
  const v = useVirtualizer({ count: total, getScrollElement: () => parentRef.current, estimateSize: () => rowH, overscan: 20 });
  const items = v.getVirtualItems();

  useEffect(() => {
    v.measure();
  }, [rowH, v]);

  useEffect(() => {
    if (items.length) ensure(items[0].index, items[items.length - 1].index);
  });

  useImperativeHandle(ref, () => ({
    scrollToIndex: (i: number) => v.scrollToIndex(i, { align: "auto" }),
    focus: () => parentRef.current?.focus(),
  }));

  const selectedIndex = (() => {
    if (selected === undefined) return -1;
    for (const it of items) {
      const r = get(it.index);
      if (r && rowId(r) === selected) return it.index;
    }
    return -1;
  })();

  const handleKey = (e: KeyboardEvent) => {
    const cur = selectedIndex >= 0 ? get(selectedIndex) : undefined;
    if (e.key === "ArrowDown" || e.key === "ArrowUp" || e.key === "PageDown" || e.key === "PageUp" || e.key === "Home" || e.key === "End") {
      e.preventDefault();
      const page = Math.max(1, Math.floor((parentRef.current?.clientHeight ?? 300) / rowH) - 1);
      const base = selectedIndex >= 0 ? selectedIndex : items[0]?.index ?? 0;
      const next =
        e.key === "ArrowDown" ? base + 1 : e.key === "ArrowUp" ? base - 1 : e.key === "PageDown" ? base + page
        : e.key === "PageUp" ? base - page : e.key === "Home" ? 0 : total - 1;
      const idx = Math.max(0, Math.min(total - 1, next));
      v.scrollToIndex(idx, { align: "auto" });
      ensure(idx, idx);
      const r = get(idx);
      if (r) onSelect?.(r, idx);
      else setTimeout(() => { const r2 = get(idx); if (r2) onSelect?.(r2, idx); }, 80);
      return;
    }
    if (e.key === "Enter" && cur) {
      e.preventDefault();
      onActivate?.(cur);
      return;
    }
    onKeyDown?.(e, cur);
  };

  const widthStyle = (c: Column<T>) => (c.width ? { width: `${c.width}rem`, flex: "none" as const } : { flex: 1, minWidth: "9rem" });

  return (
    <div className="vtable" role="grid" aria-label={ariaLabel} aria-rowcount={total}>
      <div className="vtable-head" role="row">
        {columns.map((c) => (
          <div
            key={c.key}
            role="columnheader"
            className={`th ${c.align === "right" ? "num" : ""}`}
            style={widthStyle(c)}
            onClick={() => c.sortKey && onSort?.(c.sortKey)}
            aria-sort={sort === c.sortKey ? (desc ? "descending" : "ascending") : undefined}
            title={c.label}
          >
            {c.label}
            {c.sortKey && sort === c.sortKey && (desc ? <ArrowDown size={12} /> : <ArrowUp size={12} />)}
          </div>
        ))}
      </div>
      <div className="vtable-body" ref={parentRef} tabIndex={0} onKeyDown={handleKey}>
        <div style={{ height: v.getTotalSize(), position: "relative" }}>
          {items.map((it) => {
            const row = get(it.index);
            const id = row ? rowId(row) : -1;
            return (
              <div
                key={it.key}
                role="row"
                aria-rowindex={it.index + 1}
                aria-selected={id === selected}
                className={`vrow ${row && id === selected ? "sel" : ""}`}
                style={{ transform: `translateY(${it.start}px)`, height: rowH }}
                onMouseDown={() => row && onSelect?.(row, it.index)}
                onDoubleClick={() => row && onActivate?.(row)}
                onContextMenu={(e) => {
                  if (!row) return;
                  e.preventDefault();
                  onSelect?.(row, it.index);
                  onContextMenu?.(row, e);
                }}
              >
                {row
                  ? columns.map((c) => (
                      <div key={c.key} role="gridcell" className={`td ${c.align === "right" ? "num" : ""} ${c.className ?? ""}`} style={widthStyle(c)}>
                        {c.render(row)}
                      </div>
                    ))
                  : <div className="td faint">…</div>}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

export const VirtualTable = forwardRef(VirtualTableInner) as <T>(
  props: Props<T> & { ref?: React.Ref<VirtualTableHandle> },
) => ReturnType<typeof VirtualTableInner>;
