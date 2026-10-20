import { useEffect, useState } from "react";
import { api, type Today, type TodayStep } from "../api";
import { useT } from "../i18n";
import { useApp } from "../store";
import { dueLabel, QUAD_CLASS } from "./MatrixPage";

const MINUTES_KEY = "questops.availableMinutes";

function readMinutes() {
  try {
    return Number(localStorage.getItem(MINUTES_KEY)) || 120;
  } catch {
    return 120;
  }
}

function StepRow({ s, running, onStart }: { s: TodayStep; running: boolean; onStart: () => void }) {
  const t = useT();
  const due = dueLabel(s.daysLeft, t);
  const done = s.status === "done";
  const active = s.status === "active";
  return (
    <li className={`today-row ${s.quadrant ? QUAD_CLASS[s.quadrant] : ""} ${done ? "done" : ""}`}>
      <div className="today-row-main">
        <span className="today-row-title">{s.title}</span>
        <span className="label">
          {s.questTitle} · {s.estMinutes} {t("common.min")}
        </span>
        {s.note && <span className="field-hint">{s.note}</span>}
      </div>
      {due && <span className={`tag ${due.hot ? "hot" : ""}`}>{due.text}</span>}
      {done ? (
        <span className="tag">{t("td.planDone")}</span>
      ) : (
        <button className="btn btn-primary" disabled={running} onClick={onStart}>
          {active ? t("tm.focus") : t("mx.start")}
        </button>
      )}
    </li>
  );
}

export default function TodayPage({ onNavigate }: { onNavigate: (p: "inbox") => void }) {
  const t = useT();
  const { stats, timer, version, start, fail, setFocusOpen } = useApp();
  const [today, setToday] = useState<Today | null>(null);
  const [minutes, setMinutes] = useState(readMinutes);
  const [busy, setBusy] = useState<"" | "plan" | "debrief">("");

  useEffect(() => {
    api.getToday().then(setToday).catch(fail);
  }, [version, fail]);

  async function run(kind: "plan" | "debrief", fn: () => Promise<Today>) {
    setBusy(kind);
    try {
      setToday(await fn());
    } catch (e) {
      fail(e);
    } finally {
      setBusy("");
    }
  }

  function plan() {
    try {
      localStorage.setItem(MINUTES_KEY, String(minutes));
    } catch {
      /* per-viewer convenience only */
    }
    run("plan", () => api.planToday(minutes));
  }

  const begin = (s: TodayStep) =>
    timer?.stepId === s.stepId ? setFocusOpen(true) : start(s.stepId);

  if (!today) return <p className="label">{t("common.loading")}</p>;

  const usingPlan = today.planned.length > 0;
  const list = usingPlan ? today.planned : today.suggestions;
  const planMinutes = today.planned
    .filter((s) => s.status !== "done")
    .reduce((n, s) => n + s.estMinutes, 0);

  return (
    <div className="stack fade-in">
      <div className="tiles">
        <div className="tile">
          <span className="label">{t("td.xpToday")}</span>
          <span className="tile-value mono">{stats?.todayXp ?? 0}</span>
        </div>
        <div className="tile">
          <span className="label">{t("td.doneToday")}</span>
          <span className="tile-value mono">{stats?.todayObjectives ?? 0}</span>
        </div>
        <div className="tile">
          <span className="label">{t("td.streak")}</span>
          <span className="tile-value mono">{t("td.streakDays", { n: stats?.streak ?? 0 })}</span>
        </div>
      </div>

      <section className="panel">
        <div className="panel-head">
          <h2 className="h-display" style={{ fontSize: 24 }}>
            {t("td.board")} <span className="label">· {usingPlan ? t("td.aiPlan") : t("td.nextUp")}</span>
          </h2>
          <div className="row" style={{ gap: 8 }}>
            <label className="label" htmlFor="avail">{t("td.available")}</label>
            <input
              id="avail"
              className="input"
              type="number"
              min={10}
              max={960}
              step={15}
              style={{ width: 90 }}
              value={minutes}
              onChange={(e) => setMinutes(Number(e.target.value))}
            />
            <span className="label">{t("common.min")}</span>
            <button
              className="btn btn-primary"
              disabled={!!busy || today.suggestions.length === 0}
              onClick={plan}
            >
              {busy === "plan" ? t("td.planning") : t("td.plan")}
            </button>
            {usingPlan && (
              <button
                className="btn btn-ghost"
                disabled={!!busy}
                onClick={() => run("plan", api.clearTodayPlan)}
              >
                {t("td.clearPlan")}
              </button>
            )}
          </div>
        </div>
        {!usingPlan && list.length > 0 && <p className="field-hint" style={{ marginTop: 0 }}>{t("td.nextUpHint")}</p>}
        {usingPlan && (
          <p className="field-hint" style={{ marginTop: 0 }}>
            {t("common.minLeft", { n: planMinutes })}
          </p>
        )}
        {list.length === 0 ? (
          <div className="row">
            <p className="empty-note">{t("td.empty")}</p>
            <button className="btn" onClick={() => onNavigate("inbox")}>{t("nav.inbox")}</button>
          </div>
        ) : (
          <ol className="today-list">
            {list.map((s) => (
              <StepRow
                key={s.stepId}
                s={s}
                running={!!timer && timer.stepId !== s.stepId}
                onStart={() => begin(s)}
              />
            ))}
          </ol>
        )}
      </section>

      <div className="grid-2">
        <section className="panel">
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 22 }}>{t("td.loot")}</h2>
            <span className="label mono">+{today.completed.reduce((n, s) => n + s.xp, 0)} XP</span>
          </div>
          {today.completed.length === 0 ? (
            <p className="empty-note">{t("td.lootEmpty")}</p>
          ) : (
            <ul className="quest-list">
              {today.completed.map((s) => (
                <li key={s.stepId} className="quest-row">
                  <div className="quest-row-main">
                    <span className="quest-row-title">{s.title}</span>
                    <span className="label">{s.questTitle}</span>
                  </div>
                  <span className="mono" style={{ color: "var(--accent-text)" }}>+{s.xp}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="panel">
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 22 }}>{t("td.debrief")}</h2>
            <button className="btn" disabled={!!busy} onClick={() => run("debrief", api.debriefToday)}>
              {busy === "debrief" ? t("td.writing") : t("td.writeDebrief")}
            </button>
          </div>
          {today.debrief && <p className="rationale" style={{ marginTop: 0 }}>{today.debrief}</p>}
        </section>
      </div>
    </div>
  );
}
