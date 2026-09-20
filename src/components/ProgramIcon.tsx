import { AppWindow, Package } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { create } from "zustand";
import { api } from "../services/api";

// id → object URL (null = no icon). Icons are fetched lazily, once per program.
const cache = new Map<string, string | null>();
const pending = new Map<string, Promise<string | null>>();
// Bumped when the cache is cleared so mounted icons fetch again.
const useGeneration = create<{ gen: number }>(() => ({ gen: 0 }));

/** Forget cached icons (after the program list is refreshed). */
export function clearProgramIcons() {
  for (const url of cache.values()) if (url) URL.revokeObjectURL(url);
  cache.clear();
  useGeneration.setState((s) => ({ gen: s.gen + 1 }));
}

function load(id: string, fetch: () => Promise<ArrayBuffer> = () => api.programIcon(id)): Promise<string | null> {
  if (cache.has(id)) return Promise.resolve(cache.get(id)!);
  let p = pending.get(id);
  if (!p) {
    p = fetch()
      .then((buf) => (buf.byteLength ? URL.createObjectURL(new Blob([buf], { type: "image/png" })) : null))
      .catch(() => null)
      .then((url) => {
        cache.set(id, url);
        pending.delete(id);
        return url;
      });
    pending.set(id, p);
  }
  return p;
}

function useIcon(key: string | undefined, fetch?: () => Promise<ArrayBuffer>) {
  const gen = useGeneration((s) => s.gen);
  const [url, setUrl] = useState<string | null | undefined>(key ? cache.get(key) : null);
  useEffect(() => {
    let alive = true;
    if (!key) setUrl(null);
    else if (cache.has(key)) setUrl(cache.get(key));
    else {
      setUrl(undefined);
      load(key, fetch).then((u) => alive && setUrl(u));
    }
    return () => {
      alive = false;
    };
  }, [key, gen]); // eslint-disable-line react-hooks/exhaustive-deps
  return url;
}

export function ProgramIcon({ id, size = 18 }: { id: string; size?: number }) {
  const url = useIcon(id);
  if (url) return <img src={url} width={size} height={size} alt="" style={{ flexShrink: 0, objectFit: "contain" }} />;
  return <Package size={size} className="ico" style={{ flexShrink: 0 }} />;
}

/** Icon of an executable (extracted from its resources, never run). */
export function FileIcon({ path, size = 18, fallback }: { path?: string | null; size?: number; fallback?: ReactNode }) {
  const url = useIcon(path ? `file:${path.toLowerCase()}` : undefined, () => api.fileIcon(path!));
  if (url) return <img src={url} width={size} height={size} alt="" style={{ flexShrink: 0, objectFit: "contain" }} />;
  return <>{fallback ?? <AppWindow size={size} className="ico" style={{ flexShrink: 0 }} />}</>;
}
