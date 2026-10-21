import { useEffect, useState } from "react";
import { api, errorMessage, type Breakdown, type Category, type Quest, type QuestPlan, type StepDraft } from "../api";
import BreakdownReview from "../components/BreakdownReview";
import { useT } from "../i18n";
import { useApp } from "../store";
import { AI_CONCURRENCY, runLimited } from "../util";

const EMPTY_FORM = { title: "", notes: "", categoryId: "", deadline: "" };

type ReviewItem = { quest: Quest; breakdown: Breakdown };

/** Drag payload type for moving a deployed operation back to the inbox. */
const DRAG_TYPE = "application/x-questops-quest";
/** Drag payload type for deploying an inbox operation. */
const INBOX_DRAG_TYPE = "application/x-questops-inbox";

export default function InboxPage() {
  const t = useT();
  const { refresh: refreshApp } = useApp();
  const [categories, setCategories] = useState<Category[]>([]);
  const [inbox, setInbox] = useState<Quest[]>([]);
  const [active, setActive] = useState<Quest[]>([]);
  const [form, setForm] = useState(EMPTY_FORM);
  const [planningId, setPlanningId] = useState<number | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [batch, setBatch] = useState<{ done: number; total: number } | null>(null);
  const [queue, setQueue] = useState<ReviewItem[]>([]);
  const [queueIndex, setQueueIndex] = useState(0);
  const [deployedCount, setDeployedCount] = useState(0);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [editing, setEditing] = useState<QuestPlan | null>(null);
  const [dropActive, setDropActive] = useState(false);
  const [deployDropActive, setDeployDropActive] = useState(false);

  const busy = planningId !== null || batch !== null;

  async function refresh() {
    const [i, a] = await Promise.all([api.listQuests("inbox"), api.listQuests("active")]);
    setInbox(i);
    setActive(a);
    // Drop selections for quests that left the inbox.
    setSelected((prev) => new Set(i.filter((q) => prev.has(q.id)).map((q) => q.id)));
  }

  function toggle(id: number) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  }

  function startQueue(items: ReviewItem[]) {
    setQueue(items);
    setQueueIndex(0);
    setDeployedCount(0);
  }

  // Deploying queues priority scoring in the backend; Operations shows results.
  async function finishQueue(deployed: number, skipped = 0) {
    setQueue([]);
    const parts = [t("in.deployedCount", { n: deployed })];
    if (deployed > 0) parts.push(t("in.scoringNote"));
    if (skipped > 0) parts.push(t("in.needAnswers", { n: skipped }));
    setNotice(parts.join(" "));
    await refresh();
    await refreshApp();
  }

  async function advance(deployedNow: boolean) {
    const deployed = deployedCount + (deployedNow ? 1 : 0);
    setDeployedCount(deployed);
    if (queueIndex + 1 < queue.length) setQueueIndex(queueIndex + 1);
    else await finishQueue(deployed);
  }

  async function planSelected() {
    const targets = inbox.filter((q) => selected.has(q.id));
    if (targets.length === 0) return;
    setError("");
    setNotice("");
    setBatch({ done: 0, total: targets.length });

    const results: (ReviewItem | null)[] = new Array(targets.length).fill(null);
    const failures: string[] = [];
    let done = 0;
    await runLimited(targets, AI_CONCURRENCY, async (quest, i) => {
      try {
        // Operations that already have objectives reuse them; only new ones call the AI.
        results[i] = quest.stepCount > 0 ? await savedItem(quest) : { quest, breakdown: await api.breakdownQuest(quest.id) };
      } catch (e) {
        failures.push(`${quest.title}: ${errorMessage(e)}`);
      }
      setBatch({ done: ++done, total: targets.length });
    });

    setBatch(null);
    if (failures.length) {
      setError(t("in.failedPlan", { n: failures.length, detail: failures.join(" | ") }));
    }
    const ready = results.filter((r): r is ReviewItem => r !== null);
    if (ready.length) startQueue(ready);
  }

  /** Deploys the current briefing (with edits) and every later one as drafted. */
  async function deployRest(title: string, steps: StepDraft[]) {
    let deployed = deployedCount;
    let skipped = 0;
    try {
      await api.deployQuest(queue[queueIndex].quest.id, title, steps);
      deployed++;
      for (const { quest, breakdown } of queue.slice(queueIndex + 1)) {
        if (breakdown.steps.length === 0) {
          skipped++;
          continue;
        }
        await api.deployQuest(quest.id, breakdown.questTitle || quest.title, breakdown.steps);
        deployed++;
      }
    } catch (e) {
      setError(errorMessage(e));
    }
    await finishQueue(deployed, skipped);
  }

  useEffect(() => {
    api.listCategories().then(setCategories).catch((e) => setError(errorMessage(e)));
    refresh().catch((e) => setError(errorMessage(e)));
  }, []);

  async function submit(planNow: boolean) {
    setError("");
    setNotice("");
    try {
      const quest = await api.createQuest({
        title: form.title,
        notes: form.notes,
        categoryId: form.categoryId ? Number(form.categoryId) : null,
        deadline: form.deadline || null,
      });
      setForm(EMPTY_FORM);
      await refresh();
      if (planNow) await plan(quest);
      else setNotice(t("in.logged", { title: quest.title }));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function plan(quest: Quest) {
    setPlanningId(quest.id);
    setError("");
    setNotice("");
    try {
      startQueue([{ quest, breakdown: await api.breakdownQuest(quest.id) }]);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPlanningId(null);
    }
  }

  async function remove(quest: Quest) {
    if (!confirm(t("common.confirmDelete", { title: quest.title }))) return;
    try {
      await api.deleteQuest(quest.id);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /** Reopens a deployed operation on the planning screen. */
  async function openEditor(quest: Quest) {
    setError("");
    setNotice("");
    try {
      setEditing(await api.getQuestPlan(quest.id));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /** Sends a deployed operation back to the inbox, keeping its objectives. */
  async function moveBack(quest: Quest) {
    if (!confirm(t("in.moveConfirm", { title: quest.title }))) return;
    setError("");
    try {
      await api.moveToInbox(quest.id);
      setEditing(null);
      setNotice(t("in.moved", { title: quest.title }));
      await refresh();
      await refreshApp();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /** Opens an inbox operation's saved objectives for review and redeploy, without the AI. */
  async function editSavedPlan(quest: Quest) {
    setError("");
    setNotice("");
    try {
      startQueue([await savedItem(quest)]);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /** A review item built from the operation's saved objectives (no AI call). */
  async function savedItem(quest: Quest): Promise<ReviewItem> {
    const plan = await api.getQuestPlan(quest.id);
    const steps = plan.steps
      .filter((s) => s.status !== "done")
      .map(({ title, doneWhen, effort, estMinutes }) => ({ title, doneWhen, effort, estMinutes }));
    return { quest: plan.quest, breakdown: { questTitle: plan.quest.title, clarifyingQuestions: [], steps } };
  }

  /** Puts an inbox operation back into play with its saved objectives. */
  async function redeploy(quest: Quest) {
    setError("");
    setNotice("");
    try {
      const q = await api.redeployQuest(quest.id);
      setNotice(t("in.redeployed", { title: q.title }));
      await refresh();
      await refreshApp();
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  /** Explicitly asks the AI for a fresh plan, replacing the saved one on deploy. */
  function replanWithAI(quest: Quest) {
    if (confirm(t("in.replanConfirm", { title: quest.title }))) plan(quest);
  }

  if (editing) {
    const open = editing.steps.filter((s) => s.status !== "done");
    return (
      <BreakdownReview
        key={`edit-${editing.quest.id}`}
        quest={editing.quest}
        initial={{
          questTitle: editing.quest.title,
          clarifyingQuestions: [],
          steps: open.map(({ id, title, doneWhen, effort, estMinutes }) => ({ id, title, doneWhen, effort, estMinutes })),
        }}
        edit={{
          importance: editing.importance,
          done: editing.steps.filter((s) => s.status === "done"),
          runningId: open.find((s) => s.status === "active")?.id ?? null,
          categories,
          onMoveToInbox: () => moveBack(editing.quest),
        }}
        onCancel={() => setEditing(null)}
        onDeployed={async (q) => {
          setEditing(null);
          setNotice(t("in.saved", { title: q.title }));
          await refresh();
          await refreshApp();
        }}
      />
    );
  }

  if (queue.length > 0) {
    const item = queue[queueIndex];
    const multi = queue.length > 1;
    return (
      <BreakdownReview
        key={item.quest.id}
        quest={item.quest}
        initial={item.breakdown}
        onCancel={() => (multi ? advance(false) : finishQueue(0))}
        onDeployed={() => advance(true)}
        position={
          multi ? { index: queueIndex, total: queue.length, onDeployRest: deployRest } : undefined
        }
      />
    );
  }

  const canSubmit = form.title.trim().length > 0 && !busy;
  const allSelected = inbox.length > 0 && inbox.every((q) => selected.has(q.id));

  return (
    <div className="stack fade-in">
      <section className="panel">
        <div className="panel-head">
          <h2 className="h-display" style={{ fontSize: 24 }}>{t("in.newTask")}</h2>
          <span className="label">{t("in.ctrlEnter")}</span>
        </div>

        <div className="field">
          <label className="label" htmlFor="task-title">{t("in.task")}</label>
          <input
            id="task-title"
            className="input"
            autoFocus
            placeholder={t("in.taskPlaceholder")}
            value={form.title}
            onChange={(e) => setForm({ ...form, title: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.ctrlKey && canSubmit) submit(true);
            }}
          />
        </div>

        <div className="field">
          <label className="label" htmlFor="task-notes">{t("in.notes")}</label>
          <textarea
            id="task-notes"
            className="input"
            rows={3}
            style={{ minHeight: 70 }}
            placeholder={t("in.notesPlaceholder")}
            value={form.notes}
            onChange={(e) => setForm({ ...form, notes: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.ctrlKey && canSubmit) submit(true);
            }}
          />
        </div>

        <div className="row" style={{ marginTop: 16, alignItems: "flex-end" }}>
          <div className="field" style={{ marginTop: 0 }}>
            <label className="label" htmlFor="task-cat">{t("in.category")}</label>
            <select
              id="task-cat"
              className="input"
              style={{ width: 170 }}
              value={form.categoryId}
              onChange={(e) => setForm({ ...form, categoryId: e.target.value })}
            >
              <option value="">{t("in.none")}</option>
              {categories.map((c) => (
                <option key={c.id} value={c.id}>{c.name}</option>
              ))}
            </select>
          </div>
          <div className="field" style={{ marginTop: 0 }}>
            <label className="label" htmlFor="task-deadline">{t("in.deadline")}</label>
            <input
              id="task-deadline"
              className="input"
              type="date"
              style={{ width: 170 }}
              value={form.deadline}
              onChange={(e) => setForm({ ...form, deadline: e.target.value })}
            />
          </div>
          <div className="row" style={{ marginLeft: "auto" }}>
            <button className="btn" disabled={!canSubmit} onClick={() => submit(false)}>
              {t("in.saveInbox")}
            </button>
            <button className="btn btn-primary" disabled={!canSubmit} onClick={() => submit(true)}>
              {planningId !== null ? t("in.planning") : t("in.breakDown")}
            </button>
          </div>
        </div>

        {planningId !== null && <p className="status warn" style={{ marginTop: 16 }}>{t("in.drafting")}</p>}
        {error && <p className="status err" style={{ marginTop: 16 }}>{error}</p>}
        {notice && <p className="status ok" style={{ marginTop: 16 }}>{notice}</p>}
      </section>

      <div className="grid-2">
        <section
          className={`panel ${dropActive ? "drop-target-inbox" : ""}`}
          onDragOver={(e) => {
            if (!e.dataTransfer.types.includes(DRAG_TYPE)) return;
            e.preventDefault();
            e.dataTransfer.dropEffect = "move";
            if (!dropActive) setDropActive(true);
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropActive(false);
          }}
          onDrop={(e) => {
            e.preventDefault();
            setDropActive(false);
            const q = active.find((x) => x.id === Number(e.dataTransfer.getData(DRAG_TYPE)));
            if (q) moveBack(q);
          }}
        >
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 22 }}>{t("in.inbox")}</h2>
            <span className="label mono">{t("in.unplanned", { n: inbox.length })}</span>
          </div>
          {inbox.length === 0 && <p className="empty-note">{t("in.inboxEmpty")}</p>}
          {inbox.length > 0 && (
            <div className="row batch-bar">
              <label className="check">
                <input
                  type="checkbox"
                  checked={allSelected}
                  disabled={busy}
                  onChange={() => setSelected(allSelected ? new Set() : new Set(inbox.map((q) => q.id)))}
                />
                <span className="label">{t("in.selectAll")}</span>
              </label>
              <button
                className="btn btn-primary"
                style={{ marginLeft: "auto" }}
                disabled={busy || selected.size === 0}
                onClick={planSelected}
              >
                {batch
                  ? t("in.planningBatch", { done: batch.done, total: batch.total })
                  : t("in.breakSelected", { n: selected.size })}
              </button>
            </div>
          )}
          <ul className="quest-list">
            {inbox.map((q) => (
              <li
                key={q.id}
                className={`quest-row ${selected.has(q.id) ? "selected" : ""}`}
                draggable={!busy}
                onDragStart={(e) => {
                  e.dataTransfer.setData(INBOX_DRAG_TYPE, String(q.id));
                  e.dataTransfer.effectAllowed = "move";
                }}
              >
                <label className="check" aria-label={t("in.select", { title: q.title })}>
                  <input
                    type="checkbox"
                    checked={selected.has(q.id)}
                    disabled={busy}
                    onChange={() => toggle(q.id)}
                  />
                </label>
                <div className="quest-row-main" onClick={() => !busy && toggle(q.id)}>
                  <span className="quest-row-title">{q.title}</span>
                  <span className="label mono">
                    {q.categoryName ?? t("common.uncategorized")}
                    {q.deadline && ` · ${t("common.due", { date: q.deadline })}`}
                    {q.stepCount > 0 && ` · ${t("in.savedObjectives", { n: q.stepCount })}`}
                  </span>
                </div>
                {q.stepCount > 0 ? (
                  <div className="row quest-row-actions">
                    <button className="btn btn-primary" disabled={busy} onClick={() => redeploy(q)} title={t("in.redeployHint")}>
                      {t("in.redeploy")}
                    </button>
                    <button className="btn" disabled={busy} onClick={() => editSavedPlan(q)} title={t("in.editPlanHint")}>
                      {t("in.editPlan")}
                    </button>
                    <button className="btn btn-ghost btn-sm" disabled={busy} onClick={() => replanWithAI(q)}>
                      {planningId === q.id ? t("in.planning") : t("in.replanAI")}
                    </button>
                  </div>
                ) : (
                  <button className="btn" disabled={busy} onClick={() => plan(q)}>
                    {planningId === q.id ? t("in.planning") : t("in.breakDown")}
                  </button>
                )}
                <button className="icon-btn danger" onClick={() => remove(q)} aria-label={t("common.delete")}>✕</button>
              </li>
            ))}
          </ul>
        </section>

        <section
          className={`panel ${deployDropActive ? "drop-target-inbox" : ""}`}
          onDragOver={(e) => {
            if (!e.dataTransfer.types.includes(INBOX_DRAG_TYPE)) return;
            e.preventDefault();
            e.dataTransfer.dropEffect = "move";
            if (!deployDropActive) setDeployDropActive(true);
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node)) setDeployDropActive(false);
          }}
          onDrop={(e) => {
            e.preventDefault();
            setDeployDropActive(false);
            const q = inbox.find((x) => x.id === Number(e.dataTransfer.getData(INBOX_DRAG_TYPE)));
            if (!q) return;
            // Saved objectives redeploy straight away; a never-planned task goes to the AI.
            if (q.stepCount > 0) redeploy(q);
            else plan(q);
          }}
        >
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 22 }}>{t("in.deployed")}</h2>
            <span className="label mono">{t("in.active", { n: active.length })}</span>
          </div>
          {active.length === 0 && <p className="empty-note">{t("in.noActive")}</p>}
          {active.length > 0 && <p className="field-hint" style={{ marginTop: 0 }}>{t("in.clickToEdit")}</p>}
          <ul className="quest-list">
            {active.map((q) => (
              <li
                key={q.id}
                className="quest-row editable"
                draggable
                onDragStart={(e) => {
                  e.dataTransfer.setData(DRAG_TYPE, String(q.id));
                  e.dataTransfer.effectAllowed = "move";
                }}
              >
                <div
                  className="quest-row-main"
                  role="button"
                  tabIndex={0}
                  onClick={() => openEditor(q)}
                  onKeyDown={(e) => e.key === "Enter" && openEditor(q)}
                >
                  <span className="quest-row-title">{q.title}</span>
                  <span className="label mono">
                    {t("common.objectives", { n: q.stepCount })} · {q.totalMinutes} {t("common.min")}
                    {q.deadline && ` · ${t("common.due", { date: q.deadline })}`}
                  </span>
                </div>
                <button className="icon-btn" onClick={() => moveBack(q)} aria-label={t("in.moveToInbox")} title={t("in.moveToInbox")}>↩</button>
                <button className="icon-btn danger" onClick={() => remove(q)} aria-label={t("common.delete")}>✕</button>
              </li>
            ))}
          </ul>
        </section>
      </div>
    </div>
  );
}
