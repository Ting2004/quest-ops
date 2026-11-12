import { open } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { useEffect, useState } from "react";
import { api, errorMessage, type StorageInfo } from "../api";
import { useT } from "../i18n";

/** A folder the user picked, waiting for them to choose what happens to the data. */
type Pending = { dir: string; hasData: boolean };

/** Where the database lives: the local app folder or a folder the user picks (e.g. a sync folder). */
export default function StoragePanel() {
  const t = useT();
  const [info, setInfo] = useState<StorageInfo | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.getStorage().then(setInfo).catch((e) => setError(errorMessage(e)));
  }, []);

  async function pick(dir: string) {
    setError(null);
    setPending(null);
    try {
      const check = await api.inspectStorageDir(dir);
      if (check.isCurrent) setError(t("st.alreadyUsed"));
      else setPending({ dir, hasData: check.hasData });
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function chooseFolder() {
    const dir = await open({ directory: true, multiple: false, defaultPath: info?.dir });
    if (typeof dir === "string") await pick(dir);
  }

  async function apply(mode: "move" | "useExisting") {
    if (!pending) return;
    setBusy(true);
    setError(null);
    try {
      await api.setStorageDir(pending.dir, mode); // restarts the app on success
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <section className="panel">
      <div className="panel-head">
        <h2 className="h-display" style={{ fontSize: 24 }}>{t("st.title")}</h2>
        <span className="label">DAT-00</span>
      </div>
      <p className="field-hint" style={{ marginTop: 0 }}>{t("st.hint")}</p>

      {info?.unreachable && (
        <p className="storage-alert" role="alert">
          {t("st.unreachable", { dir: info.unreachable })}
        </p>
      )}

      {info && (
        <div className="storage-current">
          <span className="label">{info.custom ? t("st.custom") : t("st.default")}</span>
          <code className="storage-path">{info.dir}</code>
        </div>
      )}

      <div className="row">
        <button className="btn" disabled={busy} onClick={chooseFolder}>{t("st.change")}</button>
        {info && (info.custom || info.unreachable) && (
          <button className="btn" disabled={busy} onClick={() => pick(info.defaultDir)}>
            {t("st.useDefault")}
          </button>
        )}
        {info?.unreachable && (
          <button className="btn" disabled={busy} onClick={() => relaunch()}>
            {t("st.retry")}
          </button>
        )}
      </div>

      {pending && (
        <div className="storage-choice">
          <p style={{ margin: 0 }}>
            {t(pending.hasData ? "st.hasData" : "st.empty")}
            <br />
            <code className="storage-path">{pending.dir}</code>
          </p>
          <div className="row">
            {pending.hasData && (
              <button className="btn btn-primary" disabled={busy} onClick={() => apply("useExisting")}>
                {t("st.useExisting")}
              </button>
            )}
            <button
              className={pending.hasData ? "btn" : "btn btn-primary"}
              disabled={busy}
              onClick={() => apply("move")}
            >
              {t(pending.hasData ? "st.replace" : "st.move")}
            </button>
            <button className="btn" disabled={busy} onClick={() => setPending(null)}>
              {t("st.cancel")}
            </button>
          </div>
          <p className="field-hint" style={{ margin: 0 }}>
            {t(pending.hasData ? "st.replaceNote" : "st.moveNote")}
          </p>
        </div>
      )}

      {error && <p className="storage-alert" role="alert">{error}</p>}
    </section>
  );
}
