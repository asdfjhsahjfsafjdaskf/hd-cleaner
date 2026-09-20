import { useEffect, useState } from "react";
import { api, toError } from "../services/api";
import { useT } from "../i18n";
import { useApp } from "../stores/app";
import type { ErrorPayload } from "../types";
import { ErrorView } from "./ErrorView";
import { Modal } from "./Modal";

export function RenameDialog() {
  const t = useT();
  const { renameRequest: req, closeRename } = useApp();
  const [name, setName] = useState("");
  const [error, setError] = useState<ErrorPayload>();
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setName(req?.name ?? "");
    setError(undefined);
  }, [req]);

  if (!req) return null;
  const submit = async () => {
    setBusy(true);
    try {
      const p = await api.renamePath(req.path, name.trim());
      closeRename();
      req.onDone?.(p);
    } catch (e) {
      setError(toError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={t("rename.title")}
      onClose={closeRename}
      footer={
        <>
          <button className="btn" onClick={closeRename}>{t("common.cancel")}</button>
          <button className="btn primary" onClick={submit} disabled={busy || !name.trim() || name.trim() === req.name}>{t("common.confirm")}</button>
        </>
      }
    >
      <div className="mono muted ellipsis" title={req.path}>{req.path}</div>
      <label className="col">
        <span className="muted">{t("rename.newName")}</span>
        <input
          className="input"
          value={name}
          autoFocus
          onFocus={(e) => {
            const dot = e.target.value.lastIndexOf(".");
            e.target.setSelectionRange(0, dot > 0 ? dot : e.target.value.length);
          }}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />
      </label>
      {error && <ErrorView error={error} />}
    </Modal>
  );
}
