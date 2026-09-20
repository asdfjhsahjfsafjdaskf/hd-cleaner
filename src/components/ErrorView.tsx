import { AlertTriangle } from "lucide-react";
import { useT, hasKey } from "../i18n";
import type { ErrorPayload } from "../types";

/** Explains an error: what failed, where, why and what the user can do. */
export function ErrorView({ error, onRetry }: { error: ErrorPayload; onRetry?: () => void }) {
  const t = useT();
  const title = hasKey(`errors.${error.kind}`) ? t(`errors.${error.kind}`) : t("errors.unknown");
  return (
    <div className="banner danger" role="alert">
      <AlertTriangle size={18} color="var(--danger)" />
      <div className="col grow" style={{ gap: "0.25rem" }}>
        <strong>{title}</strong>
        {error.path && (
          <div>
            <span className="muted">{t("errors.location")}: </span>
            <span className="mono selectable">{error.path}</span>
          </div>
        )}
        <div className="muted selectable">
          {t("errors.reason")}: {error.message}
          {error.code ? ` (code ${error.code})` : ""}
        </div>
        {error.suggestion && hasKey(`errors.suggestion_${error.suggestion}`) && <div>{t(`errors.suggestion_${error.suggestion}`)}</div>}
      </div>
      {onRetry && (
        <button className="btn sm" onClick={onRetry}>
          {t("common.retry")}
        </button>
      )}
    </div>
  );
}

export function errorTitle(e: ErrorPayload, t: (k: string) => string) {
  return hasKey(`errors.${e.kind}`) ? t(`errors.${e.kind}`) : t("errors.unknown");
}
