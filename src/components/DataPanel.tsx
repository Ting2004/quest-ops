import { open, save } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { api, errorMessage, type ClearSummary } from "../api";
import { useT } from "../i18n";
import { useApp } from "../store";

/** Accepted confirmation words for the full reset (compared case-insensitively). */
const RESET_WORDS = ["reset", "重置"];

type Status = { kind: "ok" | "err"; text: string } | null;

/** Task export/import, plus the destructive history actions behind a warning. */
export default function DataPanel() {
  const t = useT();
  const { refresh, toast } = useApp();
  const [dataStatus, setDataStatus] = useState<Status>(null);
  const [dangerStatus, setDangerStatus] = useState<Status>(null);
  const [busy, setBusy] = useState(false);
  const [resetWord, setResetWord] = useState("");

  const filters = [{ name: t("data.fileType"), extensions: ["json"] }];
  const resetArmed = RESET_WORDS.includes(resetWord.trim().toLowerCase());

  /** Runs an action and reports its message (or error) in the given slot. */
  async function run(report: (s: Status) => void, fn: () => Promise<string | null>) {
    setBusy(true);
    report(null);
    try {
      const text = await fn();
      if (text) report({ kind: "ok", text });
    } catch (e) {
      report({ kind: "err", text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  const exportTasks = () =>
    run(setDataStatus, async () => {
      const date = new Date().toISOString().slice(0, 10);
      const path = await save({ defaultPath: `questops-tasks-${date}.json`, filters });
      if (!path) return null;
      const r = await api.exportTasks(path);
      return t("data.exported", { q: r.quests, s: r.steps });
    });

  const importTasks = () =>
    run(setDataStatus, async () => {
      const path = await open({ multiple: false, directory: false, filters });
      if (typeof path !== "string") return null;
      const r = await api.importTasks(path);
      await refresh();
      return t("data.imported", { q: r.quests, s: r.steps, k: r.skipped });
    });

  const describe = (s: ClearSummary) =>
    s.operations + s.objectives + s.runs === 0
      ? t("dz.nothing")
      : t("dz.clearedDetail", { ops: s.operations, obj: s.objectives, runs: s.runs });

  const clearFinished = () =>
    run(setDangerStatus, async () => {
      if (!confirm(t("dz.clearConfirm"))) return null;
      const s = await api.clearFinished();
      await refresh();
      toast({ kind: "info", title: t("dz.cleared") });
      return describe(s);
    });

  const resetProgress = () =>
    run(setDangerStatus, async () => {
      const s = await api.resetProgress();
      setResetWord("");
      await refresh();
      toast({ kind: "info", title: t("dz.resetDone") });
      return [
        t("dz.resetDone"),
        t("dz.resetDeleted", { ops: s.operations, obj: s.objectives, runs: s.runs }),
        t("dz.resetDetail", { xp: s.xp, ach: s.achievements }),
      ].join(" ");
    });

  return (
    <>
      <section className="panel">
        <div className="panel-head">
          <h2 className="h-display" style={{ fontSize: 24 }}>{t("data.title")}</h2>
          <span className="label">DAT-01</span>
        </div>
        <p className="field-hint" style={{ marginTop: 0 }}>{t("data.hint")}</p>
        <div className="row">
          <button className="btn" disabled={busy} onClick={exportTasks}>{t("data.export")}</button>
          <button className="btn" disabled={busy} onClick={importTasks}>{t("data.import")}</button>
          {dataStatus && <span className={`status ${dataStatus.kind}`}>{dataStatus.text}</span>}
        </div>
      </section>

      <details className="danger-zone">
        <summary>
          <span className="h-display">{t("dz.title")}</span>
        </summary>
        <div className="danger-body">
          <div className="hazard danger" />
          <p className="danger-warning">{t("dz.warning")}</p>

          <div className="danger-action">
            <div>
              <strong>{t("dz.clear")}</strong>
              <p className="field-hint">{t("dz.clearDesc")}</p>
            </div>
            <div className="danger-controls">
              <button className="btn btn-danger" disabled={busy} onClick={clearFinished}>
                {t("dz.clear")}
              </button>
            </div>
          </div>

          <div className="danger-action">
            <div>
              <strong>{t("dz.reset")}</strong>
              <p className="field-hint">{t("dz.resetDesc")}</p>
            </div>
            <div className="danger-controls">
              <input
                className="input"
                placeholder={t("dz.resetType")}
                aria-label={t("dz.resetType")}
                value={resetWord}
                onChange={(e) => setResetWord(e.target.value)}
              />
              <button className="btn btn-danger" disabled={busy || !resetArmed} onClick={resetProgress}>
                {t("dz.reset")}
              </button>
            </div>
          </div>

          {dangerStatus && (
            <p className={`danger-result ${dangerStatus.kind}`} role="status">
              {dangerStatus.text}
            </p>
          )}
        </div>
      </details>
    </>
  );
}
