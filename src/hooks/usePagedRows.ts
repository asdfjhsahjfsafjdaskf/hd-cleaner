import { useCallback, useEffect, useReducer, useRef } from "react";
import type { Page } from "../types";

const PAGE = 200;

/**
 * Lazily fetches fixed-size pages of rows from the backend. Only pages that
 * intersect the visible range are requested; the cache is dropped whenever
 * `version` changes, and late responses from an older version are ignored.
 */
export function usePagedRows<T>(version: number | string, fetchPage: (offset: number, limit: number) => Promise<Page<T>>) {
  const cache = useRef(new Map<number, T[]>());
  const pending = useRef(new Set<number>());
  const ver = useRef(version);
  const fetchRef = useRef(fetchPage);
  fetchRef.current = fetchPage;
  const [, force] = useReducer((x: number) => x + 1, 0);

  useEffect(() => {
    ver.current = version;
    cache.current.clear();
    pending.current.clear();
    force();
  }, [version]);

  const get = useCallback((i: number): T | undefined => cache.current.get(Math.floor(i / PAGE))?.[i % PAGE], []);

  const ensure = useCallback((start: number, end: number) => {
    const first = Math.floor(Math.max(0, start) / PAGE);
    const last = Math.floor(Math.max(0, end) / PAGE);
    for (let p = first; p <= last; p++) {
      if (cache.current.has(p) || pending.current.has(p)) continue;
      pending.current.add(p);
      const v = ver.current;
      fetchRef.current(p * PAGE, PAGE)
        .then((page) => {
          if (ver.current !== v) return;
          cache.current.set(p, page.rows);
          force();
        })
        .catch(() => {
          /* the owner shows errors; a failed page is retried on next scroll */
        })
        .finally(() => {
          if (ver.current === v) pending.current.delete(p);
        });
    }
  }, []);

  return { get, ensure };
}
