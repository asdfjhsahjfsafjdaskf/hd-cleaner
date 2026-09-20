import type { Category } from "../types";

// Category order matches `Category as u8` in crates/hdcleaner-core/src/category.rs.
export const CATEGORY_ORDER: Category[] = [
  "other", "video", "image", "audio", "executable", "archive", "game",
  "document", "code", "cache", "system", "installer", "diskImage",
];

// Muted, distinguishable palette (works on dark and light backgrounds).
export const CATEGORY_COLORS: Record<Category, string> = {
  other: "#7b8594",
  video: "#e0736b",
  image: "#e3a95b",
  audio: "#c889d9",
  executable: "#5b9be3",
  archive: "#d7c55a",
  game: "#8ccf6b",
  document: "#6fc3d6",
  code: "#9d8ff0",
  cache: "#b7a58c",
  system: "#5d6b7d",
  installer: "#4fb89a",
  diskImage: "#d98bb0",
};

export const DIR_BLOCK_COLOR = "#4a5566";

function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export const CATEGORY_RGB: [number, number, number][] = CATEGORY_ORDER.map((c) => hexToRgb(CATEGORY_COLORS[c]));
export const DIR_BLOCK_RGB = hexToRgb(DIR_BLOCK_COLOR);

export function shade([r, g, b]: [number, number, number], f: number): string {
  const c = (v: number) => Math.max(0, Math.min(255, Math.round(v * f)));
  return `rgb(${c(r)},${c(g)},${c(b)})`;
}
