import { File, Folder, FolderSymlink, CloudOff } from "lucide-react";
import type { NodeRow } from "../types";
import { NodeFlags } from "../types";
import { CATEGORY_COLORS } from "../utils/colors";

export function Switch({ checked, onChange, disabled, label }: { checked: boolean; onChange: (v: boolean) => void; disabled?: boolean; label: string }) {
  return (
    <label className="switch" title={label}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} aria-label={label} />
      <span />
    </label>
  );
}

export function NodeIcon({ row }: { row: Pick<NodeRow, "isDir" | "flags" | "category"> }) {
  if (row.isDir) {
    if (row.flags & NodeFlags.NOT_FOLLOWED) return <FolderSymlink size={15} className="ico dir" />;
    return <Folder size={15} className="ico dir" />;
  }
  if (row.flags & NodeFlags.CLOUD) return <CloudOff size={15} className="ico" />;
  return <File size={15} className="ico" style={{ color: CATEGORY_COLORS[row.category] }} />;
}

export function ShareBar({ value }: { value: number }) {
  return (
    <div className="share" aria-hidden>
      <div style={{ width: `${Math.max(0, Math.min(1, value)) * 100}%` }} />
    </div>
  );
}

export function UsageBar({ used, total }: { used: number; total: number }) {
  const pct = total > 0 ? used / total : 0;
  return (
    <div className={`bar ${pct > 0.95 ? "danger" : pct > 0.85 ? "warn" : ""}`} role="meter" aria-valuenow={Math.round(pct * 100)} aria-valuemin={0} aria-valuemax={100}>
      <div style={{ width: `${pct * 100}%` }} />
    </div>
  );
}
