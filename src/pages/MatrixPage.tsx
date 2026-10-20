import { useCallback, useEffect, useState } from "react";
import { api, events, type MatrixQuest, type Quadrant, type Step } from "../api";
import { useT, type T, type TKey } from "../i18n";
import { useApp } from "../store";

export const QUAD_CLASS: Record<Quadrant, string> = { 1: "q1", 2: "q2", 3: "q3", 4: "q4" };

const QUADRANTS: { id: Quadrant; code: string; title: TKey; hint: TKey }[] = [
  { id: 1, code: "Q1 / IMP+URG", title: "q1.title", hint: "q1.hint" },
  { id: 2, code: "Q2 / IMP", title: "q2.title", hint: "q2.hint" },
  { id: 3, code: "Q3 / URG", title: "q3.title", hint: "q3.hint" },
  { id: 4, code: "Q4 / LOW", title: "q4.title", hint: "q4.hint" },
];

export const opId = (id: number) => `OP-${String(id).padStart(4, "0")}`;

export function dueLabel(daysLeft: number | null, t: T): { text: string; hot: boolean } | null {
  if (daysLeft === null) return null;
  if (daysLeft < 0) return { text: t("common.overdue", { n: -daysLeft }), hot: true };
  if (daysLeft === 0) return { text: t("common.dueToday"), hot: true };
  if (daysLeft === 1) return { text: t("common.dueTomorrow"), hot: true };
  return { text: t("common.dueIn", { n: daysLeft }), hot: daysLeft <= 3 };
}

function Pips({ label, value }: { label: string; value: number | null }) {
  return (
    <span className="pips" title={`${label} ${value ?? "?"}/5`}>
      <span className="label">{label}</span>
      {[1, 2, 3, 4, 5].map((n) => (
        <i key={n} className={value !== null && n <= value ? "on" : ""} />
      ))}
    </span>
  );
}

function QuestCard({ q, scoring, onOpen }: { q: MatrixQuest; scoring: boolean; onOpen: () => void }) {
  const t = useT();
  const due = dueLabel(q.daysLeft, t);
  const pct = q.stepCount ? Math.round((q.stepsDone / q.stepCount) * 100) : 0;
  return (
    <div
      className={`op-card ${scoring ? "scoring" : ""}`}
      draggable
      onDragStart={(e) => {
        e.dataTransfer.setData("text/plain", String(q.id));
        e.dataTransfer.effectAllowed = "move";
      }}
      onClick={onOpen}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => e.key === "Enter" && onOpen()}
    >
      <div className="op-card-top">
        <span className="label mono">{opId(q.id)}</span>
        {q.overridden && <span className="tag">{t("mx.manual")}</span>}
        {scoring && <span className="tag info">{t("mx.scoring")}</span>}
        {due && <span className={`tag ${due.hot ? "hot" : ""}`}>{due.text}</span>}
      </div>
      <div className="op-card-title">{q.title}</div>
      <div className="op-progress">
        <div style={{ width: `${pct}%` }} />
      </div>
      <div className="op-card-foot">
        <span className="label mono">
          {t("mx.objShort", { done: q.stepsDone, total: q.stepCount })} ·{" "}
          {t("common.minLeft", { n: q.minutesLeft })}
        </span>
        <span className="row" style={{ gap: 10 }}>
          <Pips label={t("mx.imp")} value={q.importance} />
          <Pips label={t("mx.urg")} value={q.urgency} />
        </span>
      </div>
    </div>
  );
}

function QuestDetail({
  q,
  scoring,
  onClose,
  onRescore,
  onReset,
  onDelete,
  onChanged,
}: {
  q: MatrixQuest;
  scoring: boolean;
  onClose: () => void;
  onRescore: () => void;
  onReset: () => void;
  onDelete: () => void;
  onChanged: () => void;
}) {
  const t = useT();
  const { timer, start, celebrate, fail, version } = useApp();
  const [steps, setSteps] = useState<Step[] | null>(null);
  const [splitting, setSplitting] = useState<number | null>(null);
  const quad = QUADRANTS.find((x) => x.id === q.quadrant);
  const aiQuad = QUADRANTS.find((x) => x.id === q.aiQuadrant);
  const due = dueLabel(q.daysLeft, t);

  useEffect(() => {
    api.getSteps(q.id).then(setSteps).catch(fail);
  }, [q.id, version, fail]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  async function markDone(s: Step) {
    try {
      await celebrate(await api.completeStepUntimed(s.id));
      onChanged();
    } catch (e) {
      fail(e);
    }
  }

  async function split(s: Step) {
    setSplitting(s.id);
    try {
      setSteps(await api.splitStep(s.id));
      onChanged();
    } catch (e) {
      fail(e);
    } finally {
      setSplitting(null);
    }
  }

  const nextId = steps?.find((s) => s.status !== "done")?.id;

  return (
    <div className="drawer-backdrop" onClick={onClose}>
      <aside
        className={`drawer panel ${q.quadrant ? QUAD_CLASS[q.quadrant] : ""}`}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="panel-head">
          <span className="label mono">
            {opId(q.id)} · {quad ? t(quad.title) : t("q.unassessed")}
          </span>
          <button className="icon-btn" onClick={onClose} aria-label={t("common.close")}>✕</button>
        </div>

        <h2 className="h-display" style={{ fontSize: 30, lineHeight: 1.05 }}>{q.title}</h2>
        <div className="row" style={{ marginTop: 12 }}>
          <span className="label mono">{q.categoryName ?? t("common.uncategorized")}</span>
          {q.deadline && (
            <span className={`tag ${due?.hot ? "hot" : ""}`}>
              {q.deadline} · {due?.text}
            </span>
          )}
        </div>

        <div className="drawer-section">
          <div className="row" style={{ gap: 18 }}>
            <Pips label={t("mx.importance")} value={q.importance} />
            <Pips label={t("mx.urgency")} value={q.urgency} />
          </div>
          {q.rationale && <p className="rationale">{q.rationale}</p>}
          {q.overridden && (
            <p className="field-hint" style={{ margin: "8px 0 0" }}>
              {t("mx.placedManually", { quad: aiQuad ? t(aiQuad.title) : t("mx.noneYet") })}
            </p>
          )}
        </div>

        <div className="drawer-section">
          <span className="label">{t("mx.objectives")}</span>
          {steps === null && <p className="label">{t("common.loading")}</p>}
          <ol className="drawer-steps">
            {steps?.map((s) => {
              const running = timer?.stepId === s.id;
              return (
                <li key={s.id} className={s.status}>
                  <span className="drawer-step-title">{s.title}</span>
                  <span className="label mono">{s.estMinutes}m</span>
                  {s.doneWhen && <span className="field-hint">{s.doneWhen}</span>}
                  {s.status !== "done" && (
                    <div className="row step-actions">
                      <button
                        className={`btn ${s.id === nextId ? "btn-primary" : ""}`}
                        disabled={!!timer && !running}
                        onClick={() => start(s.id)}
                      >
                        {running ? t("tm.focus") : t("mx.start")}
                      </button>
                      <button
                        className="icon-btn"
                        title={t("mx.markDone")}
                        aria-label={t("mx.markDone")}
                        disabled={running}
                        onClick={() => markDone(s)}
                      >
                        ✓
                      </button>
                      <button
                        className="btn btn-ghost"
                        disabled={splitting !== null || running}
                        onClick={() => split(s)}
                      >
                        {splitting === s.id ? t("mx.splitting") : t("mx.split")}
                      </button>
                    </div>
                  )}
                </li>
              );
            })}
          </ol>
        </div>

        <div className="hazard" style={{ margin: "8px 0 16px" }} />
        <div className="row">
          <button className="btn" disabled={scoring} onClick={onRescore}>
            {scoring ? t("mx.scoring") : t("mx.rescore")}
          </button>
          {q.overridden && q.aiQuadrant !== null && (
            <button className="btn btn-ghost" onClick={onReset}>{t("mx.reset")}</button>
          )}
          <button className="btn btn-ghost danger-text" style={{ marginLeft: "auto" }} onClick={onDelete}>
            {t("common.delete")}
          </button>
        </div>
      </aside>
    </div>
  );
}

export default function MatrixPage() {
  const t = useT();
  const { version, fail, refresh: refreshApp } = useApp();
  const [quests, setQuests] = useState<MatrixQuest[] | null>(null);
  const [scoring, setScoring] = useState<Set<number>>(new Set());
  const [openId, setOpenId] = useState<number | null>(null);
  const [dragOver, setDragOver] = useState<Quadrant | null>(null);

  const refresh = useCallback(() => api.listMatrix().then(setQuests).catch(fail), [fail]);

  const replace = (updated: MatrixQuest) =>
    setQuests((qs) => qs?.map((q) => (q.id === updated.id ? updated : q)) ?? null);

  const markScoring = (ids: number[], on: boolean) =>
    setScoring((prev) => {
      const next = new Set(prev);
      ids.forEach((id) => (on ? next.add(id) : next.delete(id)));
      return next;
    });

  useEffect(() => {
    refresh();
  }, [refresh, version]);

  // Background scoring (from deploys or retries) reports back through events.
  useEffect(() => {
    const subs = [
      events.onQuestScored((q) => {
        replace(q);
        markScoring([q.id], false);
      }),
      events.onScoreFailed((f) => {
        markScoring([f.questId], false);
        fail(t("mx.scoreFailed", { detail: f.message }));
      }),
    ];
    api.scorePending().then((ids) => markScoring(ids, true)).catch(fail);
    return () => subs.forEach((p) => p.then((un) => un()));
  }, [fail, t]);

  async function retryPending() {
    try {
      markScoring(await api.scorePending(), true);
    } catch (e) {
      fail(e);
    }
  }

  async function rescoreAll(count: number) {
    if (!confirm(t("mx.rescoreConfirm", { n: count }))) return;
    try {
      markScoring(await api.rescoreAll(), true);
    } catch (e) {
      fail(e);
    }
  }

  async function rescoreOne(id: number) {
    markScoring([id], true);
    try {
      replace(await api.scoreQuest(id));
    } catch (e) {
      fail(e);
    } finally {
      markScoring([id], false);
    }
  }

  async function drop(quadrant: Quadrant, e: React.DragEvent) {
    e.preventDefault();
    setDragOver(null);
    const id = Number(e.dataTransfer.getData("text/plain"));
    const q = quests?.find((x) => x.id === id);
    if (!q || q.quadrant === quadrant) return;
    replace({ ...q, quadrant, overridden: q.aiQuadrant !== quadrant }); // optimistic
    try {
      replace(await api.setQuadrant(id, quadrant));
    } catch (err) {
      fail(err);
      refresh();
    }
  }

  async function resetOverride(id: number) {
    try {
      replace(await api.clearQuadrantOverride(id));
    } catch (e) {
      fail(e);
    }
  }

  async function remove(q: MatrixQuest) {
    if (!confirm(t("common.confirmDelete", { title: q.title }))) return;
    try {
      await api.deleteQuest(q.id);
      setOpenId(null);
      await refreshApp();
    } catch (e) {
      fail(e);
    }
  }

  if (quests === null) return <p className="label">{t("common.loading")}</p>;

  const open = quests.find((q) => q.id === openId) ?? null;
  const unscored = quests.filter((q) => q.quadrant === null);
  const unscoredIdle = unscored.filter((q) => !scoring.has(q.id));
  const rescorable = quests.filter((q) => !q.overridden);

  return (
    <div className="stack fade-in" style={{ gap: 16 }}>
      {quests.length === 0 && <p className="empty-note">{t("mx.empty")}</p>}

      {quests.length > 0 && (
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button
            className="btn btn-ghost"
            disabled={rescorable.length === 0 || scoring.size > 0}
            onClick={() => rescoreAll(rescorable.length)}
          >
            {scoring.size > 0 ? t("mx.scoring") : t("mx.rescoreAll")}
          </button>
        </div>
      )}

      {unscored.length > 0 && (
        <section className="panel">
          <div className="panel-head" style={{ marginBottom: 12 }}>
            <h2 className="h-display" style={{ fontSize: 22 }}>{t("mx.awaiting")}</h2>
            {unscoredIdle.length > 0 && (
              <button className="btn" onClick={retryPending}>
                {t("mx.retry", { n: unscoredIdle.length })}
              </button>
            )}
          </div>
          <div className="op-strip">
            {unscored.map((q) => (
              <QuestCard key={q.id} q={q} scoring={scoring.has(q.id)} onOpen={() => setOpenId(q.id)} />
            ))}
          </div>
          <p className="field-hint" style={{ margin: "10px 0 0" }}>{t("mx.dragHint")}</p>
        </section>
      )}

      <div className="matrix">
        {QUADRANTS.map((quad) => {
          const items = quests.filter((q) => q.quadrant === quad.id);
          const minutes = items.reduce((s, q) => s + q.minutesLeft, 0);
          return (
            <section
              key={quad.id}
              className={`panel quad ${QUAD_CLASS[quad.id]} ${dragOver === quad.id ? "drop-target" : ""}`}
              onDragOver={(e) => {
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                if (dragOver !== quad.id) setDragOver(quad.id);
              }}
              onDragLeave={(e) => {
                if (!e.currentTarget.contains(e.relatedTarget as Node)) setDragOver(null);
              }}
              onDrop={(e) => drop(quad.id, e)}
            >
              <div className="panel-head">
                <h2 className="h-display">{t(quad.title)}</h2>
                <span className="label mono">
                  {quad.code} · {items.length} · {minutes}m
                </span>
              </div>
              {items.length === 0 ? (
                <div className="quad-empty">{t("mx.noOps", { hint: t(quad.hint) })}</div>
              ) : (
                <div className="quad-list">
                  {items.map((q) => (
                    <QuestCard key={q.id} q={q} scoring={scoring.has(q.id)} onOpen={() => setOpenId(q.id)} />
                  ))}
                </div>
              )}
            </section>
          );
        })}
      </div>

      {open && (
        <QuestDetail
          q={open}
          scoring={scoring.has(open.id)}
          onClose={() => setOpenId(null)}
          onRescore={() => rescoreOne(open.id)}
          onReset={() => resetOverride(open.id)}
          onDelete={() => remove(open)}
          onChanged={refresh}
        />
      )}
    </div>
  );
}
