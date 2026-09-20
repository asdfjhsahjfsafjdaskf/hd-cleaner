import { currentLocale } from "../i18n";

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export function formatBytes(n: number | undefined | null, digits = 1): string {
  if (n === undefined || n === null || !isFinite(n)) return "—";
  if (n < 1024) return `${n} B`;
  let v = n;
  let u = 0;
  while (v >= 1024 && u < UNITS.length - 1) {
    v /= 1024;
    u++;
  }
  const d = v >= 100 ? 0 : v >= 10 ? digits : digits + 1;
  return `${v.toLocaleString(currentLocale(), { maximumFractionDigits: d, minimumFractionDigits: d })} ${UNITS[u]}`;
}

export function formatSignedBytes(n: number): string {
  if (n === 0) return "0 B";
  return `${n > 0 ? "+" : "−"}${formatBytes(Math.abs(n))}`;
}

export function formatNumber(n: number | undefined | null): string {
  if (n === undefined || n === null) return "—";
  return n.toLocaleString(currentLocale());
}

export function formatPercent(v: number, digits = 1): string {
  if (!isFinite(v)) return "—";
  return `${(v * 100).toLocaleString(currentLocale(), { maximumFractionDigits: digits, minimumFractionDigits: digits })}%`;
}

export function formatDate(ms: number | undefined | null, withTime = true): string {
  if (!ms) return "—";
  const d = new Date(ms);
  return withTime
    ? d.toLocaleString(currentLocale(), { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString(currentLocale());
}

/** Compact, unambiguous "YYYY-MM-DD HH:MM" (local time) for dense tables. */
export function formatCompactDate(ms: number | undefined | null): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  const m = Math.floor(s / 60);
  return `${m} min ${Math.round(s % 60)} s`;
}

export function formatAttributes(a: number): string {
  const map: [number, string][] = [
    [0x1, "R"], [0x2, "H"], [0x4, "S"], [0x10, "D"], [0x20, "A"], [0x400, "L"],
    [0x800, "C"], [0x200, "P"], [0x1000, "O"], [0x4000, "E"],
  ];
  return map.filter(([bit]) => a & bit).map(([, s]) => s).join("") || "—";
}

/** Parse "1.5GB", "500 mb", "10k" into bytes; NaN when invalid. */
export function parseSize(s: string): number {
  const m = /^\s*([\d.]+)\s*([kmgt]?i?b?)\s*$/i.exec(s);
  if (!m) return NaN;
  const v = parseFloat(m[1]);
  const u = m[2].toLowerCase()[0];
  const mul = u === "k" ? 1024 : u === "m" ? 1024 ** 2 : u === "g" ? 1024 ** 3 : u === "t" ? 1024 ** 4 : 1;
  return Math.round(v * mul);
}
