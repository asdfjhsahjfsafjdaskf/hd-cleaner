import { Blocks, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { ErrorView } from "../components/ErrorView";
import { Modal } from "../components/Modal";
import { useT } from "../i18n";
import { api, toError } from "../services/api";
import { useApp } from "../stores/app";
import type { BrowserExtension, ErrorPayload } from "../types";
import { formatBytes, formatNumber } from "../utils/format";

export function Extensions() {
  const t = useT();
  const app = useApp();
  const [list, setList] = useState<BrowserExtension[]>();
  const [error, setError] = useState<ErrorPayload>();
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [removing, setRemoving] = useState<BrowserExtension>();

  const load = async () => {
    setBusy(true);
    try {
      setList(await api.extensionsList());
      setError(undefined);
    } catch (e) {
      setError(toError(e));
    }
    setBusy(false);
  };
  useEffect(() => {
    void load();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const shown = (list ?? []).filter((e) => !q || e.name.toLowerCase().includes(q) || e.id.includes(q));
    const by = new Map<string, BrowserExtension[]>();
    for (const e of shown) {
      const key = `${e.browserName} · ${e.profile}`;
      by.set(key, [...(by.get(key) ?? []), e]);
    }
    return [...by.entries()];
  }, [list, query]);

  const remove = async () => {
    if (!removing) return;
    const target = removing;
    setRemoving(undefined);
    setBusy(true);
    try {
      await api.extensionRemove(target.id, target.browser, true);
      app.toast("success", t("extensions.removed", { name: target.name }));
      await load();
    } catch (e) {
      app.toastError(e);
    }
    setBusy(false);
  };

  const total = list?.reduce((s, e) => s + e.bytes, 0) ?? 0;

  return (
    <div className="page fill">
      <div className="toolbar">
        <Blocks size={16} className="muted" />
        <strong>{t("nav.extensions")}</strong>
        <input
          className="input"
          style={{ width: "16rem" }}
          placeholder={t("common.search")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label={t("common.search")}
        />
        <span className="grow" />
        {list && <span className="faint">{t("extensions.count", { n: formatNumber(list.length), size: formatBytes(total) })}</span>}
        <button className="btn sm" disabled={busy} onClick={() => void load()}><RefreshCw size={14} />{t("common.refresh")}</button>
      </div>

      <div className="page-body">
        {error && <ErrorView error={error} onRetry={() => void load()} />}
        <div className="muted" style={{ marginBottom: "0.9rem" }}>{t("extensions.intro")}</div>
        {!list && busy && <div className="empty">{t("common.loading")}</div>}
        {list && list.length === 0 && <div className="empty">{t("extensions.none")}</div>}

        {groups.map(([group, items]) => (
          <div key={group} style={{ marginBottom: "1rem" }}>
            <div className="section-title" style={{ marginTop: 0 }}>
              <span>{group} · {t("extensions.inProfile", { n: items.length })}</span>
              {items[0].browserRunning && <span className="badge review" style={{ marginLeft: "0.5rem" }}>{t("extensions.open")}</span>}
            </div>
            <div className="col" style={{ gap: "0.3rem" }}>
              {items.map((e) => (
                <div key={`${e.browser}-${e.profile}-${e.id}`} className="card" style={{ padding: "0.5rem 0.7rem" }}>
                  <div className="row" style={{ gap: "0.5rem" }}>
                    <span className="grow ellipsis" title={e.id}>{e.name}</span>
                    {e.enabled === false && <span className="badge neutral">{t("startup.disabled")}</span>}
                    <span className="faint" style={{ fontSize: "0.82rem" }}>{e.version}</span>
                    <strong style={{ minWidth: "5rem", textAlign: "right" }}>{formatBytes(e.bytes)}</strong>
                    <button
                      className="btn sm danger"
                      disabled={busy || e.browserRunning}
                      title={e.browserRunning ? t("extensions.closeFirst", { name: e.browserName }) : undefined}
                      onClick={() => setRemoving(e)}
                    >
                      <Trash2 size={13} />{t("common.delete")}
                    </button>
                  </div>
                  <div className="faint mono ellipsis" style={{ fontSize: "0.78rem" }} title={e.path}>{e.id}</div>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>

      {removing && (
        <Modal
          title={t("extensions.removeTitle", { name: removing.name })}
          icon={<Trash2 size={17} color="var(--danger)" />}
          onClose={() => setRemoving(undefined)}
          footer={
            <>
              <button className="btn" onClick={() => setRemoving(undefined)}>{t("common.cancel")}</button>
              <button className="btn danger" onClick={() => void remove()}><Trash2 size={14} />{t("common.delete")}</button>
            </>
          }
        >
          <p style={{ margin: 0 }}>{t("extensions.removeBody", { browser: removing.browserName })}</p>
          <div className="banner warning">{t("extensions.syncWarning")}</div>
          <div className="mono faint" style={{ fontSize: "0.8rem", wordBreak: "break-all" }}>{removing.path}</div>
        </Modal>
      )}
    </div>
  );
}
