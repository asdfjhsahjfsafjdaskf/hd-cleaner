import { ShieldAlert } from "lucide-react";
import { useT } from "../i18n";
import type { Leftover, RemovalResult } from "../types";
import { formatBytes } from "../utils/format";

const CATEGORY_ORDER = ["files", "registry", "shortcuts", "startup", "services", "tasks"] as const;
const LEVEL_BADGE = { safe: "safe", moderate: "review", advanced: "dangerous" } as const;

/** Items that quick mode may remove without asking: unambiguous only. */
export function isUnambiguous(l: Leftover) {
  return l.level === "safe" && l.confidence >= 90 && !l.shared && l.risk !== "dangerous";
}

export function LeftoverList({
  items,
  selected,
  onToggle,
  keyPrefix = "",
}: {
  items: Leftover[];
  selected: Set<string>;
  onToggle: (key: string) => void;
  keyPrefix?: string;
}) {
  const t = useT();
  const groups = CATEGORY_ORDER.map((c) => [c, items.filter((l) => l.category === c)] as const).filter(([, v]) => v.length);
  return (
    <>
      {groups.map(([cat, list]) => (
        <div key={cat}>
          <div className="plan-item" style={{ background: "var(--bg-elev-2)", fontWeight: 600 }}>
            {t(`uninstall.cat_${cat}`)} <span className="faint">({list.length})</span>
          </div>
          {list.map((l) => {
            const key = `${keyPrefix}${l.id}`;
            return (
              <label key={key} className="plan-item" style={{ cursor: "pointer", alignItems: "flex-start" }}>
                <input type="checkbox" checked={selected.has(key)} onChange={() => onToggle(key)} style={{ accentColor: "var(--accent)", marginTop: 3 }} />
                <div className="col grow" style={{ gap: "0.15rem", minWidth: 0 }}>
                  <span className="mono" style={{ fontSize: "0.82rem", wordBreak: "break-all" }}>{l.path}</span>
                  <span className="faint" style={{ fontSize: "0.8rem" }}>
                    {t(`uninstall.reason_${l.reason}`)}{l.detail ? ` → ${l.detail}` : ""}
                  </span>
                </div>
                <div className="col" style={{ alignItems: "flex-end", gap: "0.2rem", flexShrink: 0 }}>
                  <div className="row" style={{ gap: "0.25rem" }}>
                    {l.shared && <span className="badge dangerous">{t("uninstall.shared")}</span>}
                    {l.risk === "dangerous" && <span className="badge dangerous"><ShieldAlert size={11} />{t("risk.dangerous")}</span>}
                    <span className={`badge ${LEVEL_BADGE[l.level]}`}>{t(`uninstall.level_${l.level}`)}</span>
                  </div>
                  <span className="faint" style={{ fontSize: "0.78rem" }}>
                    {t("uninstall.confidence", { n: l.confidence })}{l.size ? ` · ${formatBytes(l.size)}` : ""}
                  </span>
                </div>
              </label>
            );
          })}
        </div>
      ))}
    </>
  );
}

export function RemovalCounts({ results }: { results: RemovalResult[] }) {
  const t = useT();
  const count = (s: string) => results.filter((r) => r.status === s).length;
  return (
    <div className="row wrap" style={{ gap: "0.4rem" }}>
      {count("removed") > 0 && <span className="badge safe">{t("uninstall.removed", { n: count("removed") })}</span>}
      {count("wouldRemove") > 0 && <span className="badge review">{t("uninstall.simulated", { n: count("wouldRemove") })}</span>}
      {count("skipped") > 0 && <span className="badge neutral">{t("uninstall.skipped", { n: count("skipped") })}</span>}
      {count("failed") + count("needsElevation") > 0 && (
        <span className="badge blocked">{t("uninstall.failed", { n: count("failed") + count("needsElevation") })}</span>
      )}
    </div>
  );
}

export function RemovalResultList({ results }: { results: RemovalResult[] }) {
  const t = useT();
  return (
    <>
      {results.map((r) => (
        <div key={`${r.id}-${r.path}`} className="plan-item">
          <span className={`badge ${r.status === "removed" ? "safe" : r.status === "wouldRemove" ? "review" : r.status === "skipped" ? "neutral" : "blocked"}`}>
            {t(`uninstall.status_${r.status}`)}
          </span>
          <span className="grow mono ellipsis" style={{ fontSize: "0.82rem" }} title={r.path}>{r.path}</span>
          {r.error && <span className="faint ellipsis" style={{ maxWidth: "16rem" }} title={r.error.message}>{r.error.message}</span>}
        </div>
      ))}
    </>
  );
}
