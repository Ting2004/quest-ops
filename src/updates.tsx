// In-app updates through the Tauri updater (Gitee first for users in China,
// then GitHub; see plugins.updater.endpoints): a quiet check shortly after
// launch, a manual check from Config, and one-click download and install.
// Installing always needs the person's click.
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { errorMessage } from "./api";
import { useT } from "./i18n";
import { useApp } from "./store";

/** Delay before the startup check, so it never slows down opening the app. */
const STARTUP_CHECK_MS = 4000;
/** Per-check timeout, so an unreachable mirror falls through to the next one quickly. */
const CHECK_TIMEOUT_MS = 20000;

export type UpdateStatus =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "current" }
  | { kind: "available" }
  | { kind: "downloading"; pct: number | null }
  | { kind: "installing" }
  | { kind: "error"; message: string };

interface UpdateState {
  currentVersion: string;
  update: Update | null;
  status: UpdateStatus;
  checkNow: () => Promise<void>;
  install: () => Promise<void>;
}

const UpdateContext = createContext<UpdateState | null>(null);

export function useUpdates(): UpdateState {
  const ctx = useContext(UpdateContext);
  if (!ctx) throw new Error("useUpdates outside UpdateProvider");
  return ctx;
}

export function UpdateProvider({ children }: { children: ReactNode }) {
  const t = useT();
  const { toast } = useApp();
  const [currentVersion, setCurrentVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [status, setStatus] = useState<UpdateStatus>({ kind: "idle" });
  const announced = useRef<string | null>(null);

  useEffect(() => {
    getVersion().then(setCurrentVersion).catch(() => setCurrentVersion(""));
  }, []);

  const run = useCallback(
    async (quiet: boolean) => {
      if (!quiet) setStatus({ kind: "checking" });
      try {
        const found = await check({ timeout: CHECK_TIMEOUT_MS });
        setUpdate(found);
        setStatus({ kind: found ? "available" : "current" });
        if (found && announced.current !== found.version) {
          announced.current = found.version;
          toast({ kind: "info", title: t("upd.toast", { v: found.version }) });
        }
      } catch (e) {
        // A failed background check (offline, no release yet) stays silent.
        setStatus(quiet ? { kind: "idle" } : { kind: "error", message: errorMessage(e) });
      }
    },
    [toast, t],
  );

  useEffect(() => {
    const id = setTimeout(() => run(true), STARTUP_CHECK_MS);
    return () => clearTimeout(id);
  }, [run]);

  const install = useCallback(async () => {
    if (!update) return;
    let total = 0;
    let received = 0;
    setStatus({ kind: "downloading", pct: null });
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") total = event.data.contentLength ?? 0;
        if (event.event === "Progress") {
          received += event.data.chunkLength;
          setStatus({ kind: "downloading", pct: total ? Math.round((received / total) * 100) : null });
        }
        if (event.event === "Finished") setStatus({ kind: "installing" });
      });
      // On Windows the installer usually closes the app itself; relaunch covers the rest.
      await relaunch();
    } catch (e) {
      setStatus({ kind: "error", message: errorMessage(e) });
    }
  }, [update]);

  return (
    <UpdateContext.Provider
      value={{ currentVersion, update, status, checkNow: () => run(false), install }}
    >
      {children}
    </UpdateContext.Provider>
  );
}

/** Version, manual check and install controls for the Config page. */
export function UpdatePanel() {
  const t = useT();
  const { currentVersion, update, status, checkNow, install } = useUpdates();
  const busy = ["checking", "downloading", "installing"].includes(status.kind);

  let line: { kind: "ok" | "warn" | "err" | "idle"; text: string } | null = null;
  if (status.kind === "checking") line = { kind: "warn", text: t("upd.checking") };
  if (status.kind === "current") line = { kind: "ok", text: t("upd.upToDate") };
  if (status.kind === "available" && update) line = { kind: "warn", text: t("upd.available", { v: update.version }) };
  if (status.kind === "downloading")
    line = { kind: "warn", text: t("upd.downloading", { pct: status.pct ?? "…" }) };
  if (status.kind === "installing") line = { kind: "warn", text: t("upd.installing") };
  if (status.kind === "error") line = { kind: "err", text: t("upd.failed", { detail: status.message }) };

  return (
    <section className="panel">
      <div className="panel-head">
        <h2 className="h-display" style={{ fontSize: 24 }}>{t("upd.title")}</h2>
        <span className="label mono">{t("upd.current", { v: currentVersion || "?" })}</span>
      </div>
      <div className="row">
        <button className="btn" disabled={busy} onClick={checkNow}>{t("upd.check")}</button>
        {update && status.kind !== "current" && (
          <button
            className="btn btn-primary"
            disabled={busy}
            onClick={() => confirm(t("upd.confirm", { v: update.version })) && install()}
          >
            {t("upd.install")}
          </button>
        )}
        {line && <span className={`status ${line.kind}`}>{line.text}</span>}
      </div>
      {update?.body && (
        <div className="update-notes">
          <span className="label">{t("upd.notes")}</span>
          <p>{update.body}</p>
        </div>
      )}
    </section>
  );
}

/** Compact sidebar prompt shown only while an update is waiting. */
export function UpdateBadge() {
  const t = useT();
  const { update, status, install } = useUpdates();
  if (!update || !["available", "error"].includes(status.kind)) {
    if (status.kind === "downloading" || status.kind === "installing") {
      return <span className="update-badge mono">{t("upd.downloading", { pct: status.kind === "downloading" ? status.pct ?? "…" : 100 })}</span>;
    }
    return null;
  }
  return (
    <button
      className="update-badge"
      onClick={() => confirm(t("upd.confirm", { v: update.version })) && install()}
    >
      {t("upd.sidebar", { v: update.version })}
    </button>
  );
}
