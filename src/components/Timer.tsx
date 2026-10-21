import { useEffect, useRef, useState } from "react";
import { api, type TimerView, type UnstickReply } from "../api";
import { useT } from "../i18n";
import { sound } from "../sound";
import { useApp } from "../store";

/** Overtime past this share of the time limit opens the overtime protocol. */
const PROTOCOL_AT = 1.5;
const EXTEND_OPTIONS = [5, 10, 15];

function elapsedMs(t: TimerView, now: number) {
  const pausedNow = t.pausedSinceMs !== null ? now - t.pausedSinceMs : 0;
  return Math.max(0, now - t.startedMs - t.pausedMs - pausedNow);
}

export function fmtClock(ms: number) {
  const s = Math.floor(Math.abs(ms) / 1000);
  const h = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, "0");
  const ss = String(s % 60).padStart(2, "0");
  return `${ms < 0 ? "+" : ""}${h ? `${h}:` : ""}${mm}:${ss}`;
}

/** Live numbers for the running timer, re-rendered four times a second. */
function useClock(t: TimerView | null) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!t) return;
    const id = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(id);
  }, [t]);
  if (!t) return null;
  const limit = (t.plannedSeconds + t.extendedSeconds) * 1000;
  const elapsed = elapsedMs(t, now);
  return {
    limit,
    elapsed,
    remaining: limit - elapsed,
    frac: elapsed / Math.max(limit, 1),
    overtime: elapsed > limit,
    paused: t.pausedSinceMs !== null,
    pausesLeft: Math.max(0, t.maxPauses - t.pausesUsed),
  };
}

function useTimerActions() {
  const { timer, setTimer, celebrate, fail, refresh, setFocusOpen } = useApp();
  const [busy, setBusy] = useState(false);
  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };
  return {
    busy,
    complete: () =>
      run(async () => {
        // Capture what comes next before the run ends and the timer clears.
        const after = timer
          ? {
              completedTitle: timer.stepTitle,
              questTitle: timer.questTitle,
              xp: 0,
              next: timer.upcoming[0] ?? null,
            }
          : undefined;
        await celebrate(await api.completeStep(), after);
      }),
    pause: () => run(async () => setTimer(await api.pauseTimer())),
    resume: () => run(async () => setTimer(await api.resumeTimer())),
    extend: (m: number) => run(async () => setTimer(await api.extendTimer(m))),
    abort: () =>
      run(async () => {
        await api.abortStep();
        setFocusOpen(false);
        await refresh();
      }),
    timer,
  };
}

/** Compact bar across the top of every page while an objective runs. */
export function TimerDock() {
  const t = useT();
  const { timer, focusOpen, setFocusOpen } = useApp();
  const clock = useClock(timer);
  const act = useTimerActions();
  if (!timer || !clock || focusOpen) return null;

  const state = clock.overtime ? "overtime" : clock.paused ? "paused" : "running";
  return (
    <div className={`dock dock-${state}`}>
      <span className="status-chip mono">{t(`tm.${state}`)}</span>
      <button className="dock-title" onClick={() => setFocusOpen(true)}>
        <span className="label">{timer.questTitle}</span>
        <span>{timer.stepTitle}</span>
      </button>
      <span className="dock-clock mono">{fmtClock(clock.remaining)}</span>
      <div className="row" style={{ gap: 8 }}>
        <button className="btn btn-ghost" onClick={() => setFocusOpen(true)}>{t("tm.focus")}</button>
        <button className="btn btn-primary" disabled={act.busy} onClick={act.complete}>
          {t("tm.complete")}
        </button>
      </div>
      <div className="dock-progress">
        <div style={{ width: `${Math.min(100, clock.frac * 100)}%` }} />
      </div>
    </div>
  );
}

/** Full-screen focus view for the running objective. */
export function FocusOverlay() {
  const t = useT();
  const { timer, focusOpen, setFocusOpen, start, fail } = useApp();
  const clock = useClock(timer);
  const act = useTimerActions();
  const [protocolDismissedAt, setProtocolDismissedAt] = useState<number | null>(null);
  const [unstickOpen, setUnstickOpen] = useState(false);
  const [splitting, setSplitting] = useState(false);
  const alerted = useRef<number | null>(null);

  // One audible cue when time runs out while the window is open.
  useEffect(() => {
    if (timer && clock?.overtime && alerted.current !== timer.sessionId) {
      alerted.current = timer.sessionId;
      sound.alert();
    }
  }, [timer, clock?.overtime]);

  useEffect(() => {
    if (!focusOpen) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !unstickOpen && setFocusOpen(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [focusOpen, unstickOpen, setFocusOpen]);

  if (!timer || !clock || !focusOpen) return null;

  const showProtocol =
    clock.frac >= PROTOCOL_AT && protocolDismissedAt !== clock.limit && !unstickOpen;

  async function splitRest() {
    if (!timer || !clock) return;
    setSplitting(true);
    try {
      await api.splitStep(timer.stepId, Math.round(clock.elapsed / 60000));
      await start(timer.stepId); // the objective's first, smaller piece
    } catch (e) {
      fail(e);
    } finally {
      setSplitting(false);
    }
  }

  const state = clock.overtime ? "overtime" : clock.paused ? "paused" : "running";
  return (
    <div className={`focus focus-${state}`}>
      <div className="focus-top">
        <span className="status-chip mono">{t(`tm.${state}`)}</span>
        <span className="label">{timer.questTitle}</span>
        <button className="btn btn-ghost" style={{ marginLeft: "auto" }} onClick={() => setFocusOpen(false)}>
          {t("tm.minimize")}
        </button>
      </div>

      <div className="focus-body">
        <h1 className="focus-title h-display">{timer.stepTitle}</h1>
        {timer.doneWhen && (
          <p className="focus-done">
            <span className="label">{t("tm.doneWhen")}</span> {timer.doneWhen}
          </p>
        )}
        <div className="focus-clock mono">{fmtClock(clock.remaining)}</div>
        <div className="focus-progress">
          <div style={{ width: `${Math.min(100, clock.frac * 100)}%` }} />
        </div>
        <div className="row focus-meta">
          {timer.extendedSeconds > 0 && (
            <>
              <span className="tag info">{t("tm.extended", { n: timer.extendedSeconds / 60 })}</span>
              <span className="label">
                {t("tm.onTimeUntil", { clock: fmtClock(timer.plannedSeconds * 1000) })}
              </span>
            </>
          )}
        </div>

        <div className="row focus-actions">
          <button className="btn btn-primary btn-lg" disabled={act.busy} onClick={act.complete}>
            {t("tm.complete")}
          </button>
          {clock.paused ? (
            <button className="btn" disabled={act.busy} onClick={act.resume}>{t("tm.resume")}</button>
          ) : (
            <button className="btn" disabled={act.busy || clock.pausesLeft === 0} onClick={act.pause}>
              {t("tm.pause", { n: clock.pausesLeft })}
            </button>
          )}
          <button className="btn" onClick={() => setUnstickOpen(true)}>{t("tm.stuck")}</button>
          <button
            className="btn btn-ghost danger-text"
            disabled={act.busy}
            onClick={() => confirm(t("tm.abortConfirm")) && act.abort()}
          >
            {t("tm.abort")}
          </button>
        </div>

        {timer.upcoming.length > 0 && (
          <div className="focus-next">
            <span className="label">{t("tm.nextUp")}</span>
            <ol>
              {timer.upcoming.map((s) => (
                <li key={s.id}>
                  <span>{s.title}</span>
                  <span className="label mono">{s.estMinutes} {t("common.min")}</span>
                </li>
              ))}
            </ol>
          </div>
        )}
      </div>

      {showProtocol && (
        <div className="modal-backdrop">
          <div className="modal panel q1">
            <div className="hazard danger" />
            <h2 className="h-display" style={{ fontSize: 28, marginTop: 14 }}>{t("tm.protocol")}</h2>
            <p style={{ color: "var(--text-muted)" }}>{t("tm.protocolBody")}</p>
            <div className="row">
              {EXTEND_OPTIONS.map((m) => (
                <button key={m} className="btn" disabled={act.busy} onClick={() => act.extend(m)}>
                  {t("tm.extend", { n: m })}
                </button>
              ))}
            </div>
            <div className="row" style={{ marginTop: 12 }}>
              <button className="btn btn-primary" disabled={splitting} onClick={splitRest}>
                {splitting ? t("mx.splitting") : t("tm.splitRest")}
              </button>
              <button className="btn btn-ghost" onClick={() => setProtocolDismissedAt(clock.limit)}>
                {t("tm.keepGoing")}
              </button>
              <button
                className="btn btn-ghost danger-text"
                style={{ marginLeft: "auto" }}
                onClick={() => confirm(t("tm.abortConfirm")) && act.abort()}
              >
                {t("tm.abort")}
              </button>
            </div>
          </div>
        </div>
      )}

      {unstickOpen && <Unstick timer={timer} onClose={() => setUnstickOpen(false)} />}
    </div>
  );
}

function Unstick({ timer, onClose }: { timer: TimerView; onClose: () => void }) {
  const t = useT();
  const { start, fail } = useApp();
  const [reason, setReason] = useState("");
  const [reply, setReply] = useState<UnstickReply | null>(null);
  const [busy, setBusy] = useState(false);

  async function ask() {
    setBusy(true);
    try {
      setReply(await api.unstickStep(timer.stepId, reason));
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  }

  async function insertAndSwitch() {
    if (!reply) return;
    setBusy(true);
    try {
      await api.abortStep();
      const step = await api.insertStep(timer.questId, timer.stepId, reply.step);
      onClose();
      await start(step.id);
    } catch (e) {
      fail(e);
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal panel" onClick={(e) => e.stopPropagation()}>
        <div className="panel-head">
          <h2 className="h-display" style={{ fontSize: 24 }}>{t("tm.unstickTitle")}</h2>
          <button className="icon-btn" onClick={onClose} aria-label={t("common.close")}>✕</button>
        </div>
        <div className="field">
          <label className="label" htmlFor="unstick-reason">{t("tm.unstickPrompt")}</label>
          <textarea
            id="unstick-reason"
            className="input"
            rows={3}
            style={{ minHeight: 70 }}
            autoFocus
            placeholder={t("tm.unstickPlaceholder")}
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
        </div>
        {!reply && (
          <button className="btn btn-primary" style={{ marginTop: 14 }} disabled={busy} onClick={ask}>
            {busy ? t("tm.unstickThinking") : t("tm.unstickAsk")}
          </button>
        )}
        {reply && (
          <div className="stack" style={{ gap: 12, marginTop: 16 }}>
            <p className="rationale" style={{ margin: 0 }}>{reply.advice}</p>
            <div className="objective" style={{ gridTemplateColumns: "1fr" }}>
              <div className="objective-body">
                <strong>{reply.step.title}</strong>
                <span className="field-hint">{reply.step.doneWhen}</span>
                <span className="label mono">{reply.step.estMinutes} {t("common.min")}</span>
              </div>
            </div>
            <div className="row">
              <button className="btn btn-primary" disabled={busy} onClick={insertAndSwitch}>
                {t("tm.unstickInsert")}
              </button>
              <button className="btn btn-ghost" disabled={busy} onClick={ask}>{t("br.regenerate")}</button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

/** Shown after a timed run when the operation still has work: stop here or
 *  go straight into the next objective. */
export function AfterRunPanel() {
  const t = useT();
  const { afterRun, closeAfterRun, start, timer } = useApp();

  useEffect(() => {
    if (!afterRun) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeAfterRun();
      if (e.key === "Enter" && afterRun.next) proceed();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (!afterRun) return null;

  async function proceed() {
    if (!afterRun?.next) return;
    const nextId = afterRun.next.id;
    closeAfterRun();
    await start(nextId);
  }

  return (
    <div className="modal-backdrop" onClick={closeAfterRun}>
      <div className="modal panel after-run" onClick={(e) => e.stopPropagation()}>
        <div className="panel-head">
          <span className="label">{afterRun.questTitle}</span>
          <span className="label mono" style={{ color: "var(--accent-text)" }}>+{afterRun.xp} XP</span>
        </div>
        <h2 className="h-display after-run-title">{t("tm.runComplete")}</h2>
        <p className="after-run-done">✓ {afterRun.completedTitle}</p>

        {afterRun.next ? (
          <div className="after-run-next">
            <span className="label">{t("tm.nextUp")}</span>
            <strong>{afterRun.next.title}</strong>
            {afterRun.next.doneWhen && <span className="field-hint">{afterRun.next.doneWhen}</span>}
            <span className="label mono">{afterRun.next.estMinutes} {t("common.min")}</span>
          </div>
        ) : (
          <p className="field-hint">{t("tm.noneQueued")}</p>
        )}

        <div className="hazard" style={{ margin: "18px 0 14px" }} />
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn btn-ghost" onClick={closeAfterRun}>{t("tm.finish")}</button>
          {afterRun.next && (
            <button className="btn btn-primary" autoFocus disabled={!!timer} onClick={proceed}>
              {t("tm.proceed")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
