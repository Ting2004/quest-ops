import { useState } from "react";
import { api, errorMessage, type Breakdown, type Effort, type Quest, type Step, type StepDraft } from "../api";
import { useT, type TKey } from "../i18n";

const EFFORTS: Effort[] = [1, 2, 3, 5];
const IMPORTANCE = [1, 2, 3, 4, 5];

interface Props {
  quest: Quest;
  initial: Breakdown;
  onDeployed: (q: Quest) => void;
  onCancel: () => void;
  /** Set when reviewing several breakdowns in a row. */
  position?: { index: number; total: number; onDeployRest: (title: string, steps: StepDraft[]) => void };
  /** Set when re-planning an operation that is already deployed. */
  edit?: { importance: number | null; done: Step[]; runningId: number | null };
}

/** Edit an AI breakdown before committing it, answer its clarifying questions,
 *  or (with `edit`) re-plan an operation that is already deployed. */
export default function BreakdownReview({ quest, initial, onDeployed, onCancel, position, edit }: Props) {
  const t = useT();
  const [title, setTitle] = useState(initial.questTitle || quest.title);
  const [steps, setSteps] = useState<StepDraft[]>(initial.steps);
  const [questions, setQuestions] = useState(initial.clarifyingQuestions);
  const [answers, setAnswers] = useState<string[]>(initial.clarifyingQuestions.map(() => ""));
  const [importance, setImportance] = useState<number | null>(edit?.importance ?? null);
  const [busy, setBusy] = useState<"" | "regen" | "deploy">("");
  const [error, setError] = useState("");

  const totalMinutes = steps.reduce((sum, s) => sum + (s.estMinutes || 0), 0);
  const asking = questions.length > 0 && steps.length === 0;

  const patch = (i: number, p: Partial<StepDraft>) =>
    setSteps(steps.map((s, j) => (j === i ? { ...s, ...p } : s)));
  const move = (i: number, d: -1 | 1) => {
    const next = [...steps];
    [next[i], next[i + d]] = [next[i + d], next[i]];
    setSteps(next);
  };

  async function regenerate() {
    setBusy("regen");
    setError("");
    try {
      const b = await api.breakdownQuest(
        quest.id,
        questions.map((q, i) => ({ question: q, answer: answers[i] ?? "" })),
      );
      setTitle(b.questTitle || title);
      setSteps(b.steps);
      setQuestions(b.clarifyingQuestions);
      setAnswers(b.clarifyingQuestions.map(() => ""));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy("");
    }
  }

  async function deploy() {
    setBusy("deploy");
    setError("");
    try {
      onDeployed(
        edit
          ? await api.updateQuestPlan(quest.id, title, importance, steps)
          : await api.deployQuest(quest.id, title, steps),
      );
    } catch (e) {
      setError(errorMessage(e));
      setBusy("");
    }
  }

  return (
    <section className="panel fade-in">
      <div className="panel-head">
        <h2 className="h-display" style={{ fontSize: 24 }}>
          {edit ? t("br.editTitle") : t("br.title")}
          {position && (
            <span style={{ color: "var(--accent-text)" }}>
              {" "}
              {position.index + 1}/{position.total}
            </span>
          )}
        </h2>
        <span className="label mono">
          OP-{String(quest.id).padStart(4, "0")} · {t("br.summary", { n: steps.length, min: totalMinutes })}
        </span>
      </div>

      <div className="field">
        <label className="label" htmlFor="op-title">{t("br.name")}</label>
        <input id="op-title" className="input" value={title} onChange={(e) => setTitle(e.target.value)} />
      </div>

      {edit && (
        <div className="field">
          <span className="label">{t("br.importance")}</span>
          <div className="segmented" role="radiogroup" aria-label={t("br.importance")}>
            {IMPORTANCE.map((n) => (
              <button
                key={n}
                role="radio"
                aria-checked={importance === n}
                className={`mono ${importance === n ? "on" : ""}`}
                onClick={() => setImportance(n)}
              >
                {n}
              </button>
            ))}
          </div>
          <span className="field-hint">{t("br.importanceHint")}</span>
        </div>
      )}

      {edit && edit.done.length > 0 && (
        <div className="field">
          <span className="label">{t("br.completed")}</span>
          <ul className="done-list">
            {edit.done.map((s) => (
              <li key={s.id}>
                <span>✓ {s.title}</span>
                <span className="label mono">{s.estMinutes} {t("common.min")}</span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {asking && (
        <div className="stack" style={{ marginTop: 20, gap: 14 }}>
          <span className="status warn">{t("br.needIntel")}</span>
          {questions.map((q, i) => (
            <div className="field" key={i} style={{ marginTop: 0 }}>
              <label className="label" htmlFor={`q-${i}`}>{q}</label>
              <input
                id={`q-${i}`}
                className="input"
                value={answers[i]}
                onChange={(e) => setAnswers(answers.map((a, j) => (j === i ? e.target.value : a)))}
              />
            </div>
          ))}
        </div>
      )}

      {!asking && (
        <ol className="objectives">
          {steps.map((s, i) => (
            <li key={s.id ?? `new-${i}`} className="objective">
              <span className="objective-index mono">
                {String(i + 1 + (edit?.done.length ?? 0)).padStart(2, "0")}
                {s.id !== undefined && s.id === edit?.runningId && <span className="tag info">{t("tm.running")}</span>}
              </span>
              <div className="objective-body">
                <input
                  className="input objective-title"
                  value={s.title}
                  placeholder={t("br.objective")}
                  onChange={(e) => patch(i, { title: e.target.value })}
                />
                <input
                  className="input"
                  value={s.doneWhen}
                  placeholder={t("br.doneWhen")}
                  onChange={(e) => patch(i, { doneWhen: e.target.value })}
                />
                <div className="row">
                  <label className="label" htmlFor={`min-${i}`}>{t("br.est")}</label>
                  <input
                    id={`min-${i}`}
                    className="input"
                    type="number"
                    min={1}
                    max={240}
                    style={{ width: 80 }}
                    value={s.estMinutes}
                    onChange={(e) => patch(i, { estMinutes: Number(e.target.value) })}
                  />
                  <span className="label">{t("common.min")}</span>
                  <select
                    className="input"
                    style={{ width: 150 }}
                    value={s.effort}
                    onChange={(e) => patch(i, { effort: Number(e.target.value) as Effort })}
                    aria-label={t("br.effort")}
                  >
                    {EFFORTS.map((ef) => (
                      <option key={ef} value={ef}>{t(`effort.${ef}` as TKey)}</option>
                    ))}
                  </select>
                </div>
              </div>
              <div className="objective-tools">
                <button className="icon-btn" disabled={i === 0} onClick={() => move(i, -1)} aria-label={t("br.up")}>▲</button>
                <button className="icon-btn" disabled={i === steps.length - 1} onClick={() => move(i, 1)} aria-label={t("br.down")}>▼</button>
                <button
                  className="icon-btn danger"
                  disabled={s.id !== undefined && s.id === edit?.runningId}
                  onClick={() => setSteps(steps.filter((_, j) => j !== i))}
                  aria-label={t("br.remove")}
                >
                  ✕
                </button>
              </div>
            </li>
          ))}
        </ol>
      )}

      {!asking && (
        <button
          className="btn btn-ghost"
          style={{ marginTop: 12 }}
          onClick={() => setSteps([...steps, { title: "", doneWhen: "", effort: 2, estMinutes: 15 }])}
        >
          {t("br.add")}
        </button>
      )}

      {error && <p className="status err" style={{ marginTop: 16 }}>{error}</p>}

      <div className="hazard" style={{ margin: "20px 0 16px" }} />

      <div className="row" style={{ justifyContent: "space-between" }}>
        <button className="btn btn-ghost" disabled={!!busy} onClick={onCancel}>
          {position ? t("br.skip") : t("br.back")}
        </button>
        <div className="row">
          {position && position.index < position.total - 1 && (
            <button
              className="btn btn-ghost"
              disabled={!!busy || asking || steps.length === 0}
              onClick={() => position.onDeployRest(title, steps)}
              title={t("br.deployRestTitle")}
            >
              {t("br.deployRest")}
            </button>
          )}
          <button className="btn" disabled={!!busy} onClick={regenerate}>
            {busy === "regen" ? t("br.replanning") : asking ? t("br.submitAnswers") : t("br.regenerate")}
          </button>
          {!asking && (
            <button className="btn btn-primary" disabled={!!busy || steps.length === 0} onClick={deploy}>
              {busy === "deploy" ? t("br.deploying") : edit ? t("br.save") : t("br.deploy")}
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
