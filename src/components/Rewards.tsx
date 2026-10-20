import { useEffect } from "react";
import { useT } from "../i18n";
import { useApp } from "../store";

/** Stacked notifications in the bottom-right corner. */
export function Toasts() {
  const { toasts, dismissToast } = useApp();
  return (
    <div className="toasts" aria-live="polite">
      {toasts.map((x) => (
        <div
          key={x.id}
          className={`toast toast-${x.kind} ${x.critical ? "toast-critical" : ""}`}
          onClick={() => dismissToast(x.id)}
          role="status"
        >
          {x.detail && <span className="label">{x.detail}</span>}
          <span className="toast-title">{x.title}</span>
          {x.lines && (
            <ul className="toast-lines">
              {x.lines.map((l, i) => (
                <li key={i}>
                  <span>{l.label}</span>
                  {l.amount !== null && <span className="mono">+{l.amount}</span>}
                </li>
              ))}
            </ul>
          )}
        </div>
      ))}
    </div>
  );
}

/** Full-screen stamp shown when an operation's last objective is done. */
export function MissionComplete() {
  const t = useT();
  const { mission, closeMission } = useApp();

  useEffect(() => {
    if (!mission) return;
    const onKey = (e: KeyboardEvent) => (e.key === "Escape" || e.key === "Enter") && closeMission();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mission, closeMission]);

  if (!mission) return null;
  return (
    <div className="mission-backdrop" onClick={closeMission}>
      <div className="mission" onClick={(e) => e.stopPropagation()}>
        <div className="hazard" />
        <span className="label">{mission.title}</span>
        <h2 className="mission-stamp h-display">{t("rw.missionComplete")}</h2>
        <span className="mission-bonus mono">{t("rw.bonus", { n: mission.bonus })}</span>
        <button className="btn btn-primary" autoFocus onClick={closeMission}>
          {t("rw.continue")}
        </button>
        <div className="hazard" />
      </div>
    </div>
  );
}
