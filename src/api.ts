// Typed wrappers around the Tauri commands defined in src-tauri/src.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Lang } from "./i18n";

export type Theme = "dark" | "light" | "system";
export type Provider = "openai" | "deepseek" | "custom";

export interface Settings {
  commanderName: string;
  goalsText: string;
  modelName: string;
  smallModelName: string;
  apiBaseUrl: string;
  maxPauses: number;
  urgentWithinDays: number;
  importanceThreshold: number;
  language: Lang;
  theme: Theme;
  provider: Provider;
}

export interface ConnectionCheck {
  ok: boolean;
  model: string;
  message: string;
  /** Model IDs the key can use. */
  available: string[];
}

export interface Category {
  id: number;
  name: string;
}

export type QuestStatus = "inbox" | "active" | "completed" | "archived";

export interface Quest {
  id: number;
  title: string;
  notes: string;
  categoryId: number | null;
  categoryName: string | null;
  status: QuestStatus;
  deadline: string | null;
  createdAt: string;
  stepCount: number;
  totalMinutes: number;
}

export interface NewQuest {
  title: string;
  notes: string;
  categoryId: number | null;
  deadline: string | null;
}

export type Effort = 1 | 2 | 3 | 5;

export interface StepDraft {
  title: string;
  doneWhen: string;
  effort: Effort;
  estMinutes: number;
}

export interface Breakdown {
  questTitle: string;
  clarifyingQuestions: string[];
  steps: StepDraft[];
}

export interface Clarification {
  question: string;
  answer: string;
}

export type StepStatus = "pending" | "active" | "done" | "aborted";

export interface Step {
  id: number;
  title: string;
  doneWhen: string;
  effort: Effort;
  estMinutes: number;
  orderIndex: number;
  status: StepStatus;
}

export type Quadrant = 1 | 2 | 3 | 4;

export interface MatrixQuest {
  id: number;
  title: string;
  notes: string;
  categoryName: string | null;
  deadline: string | null;
  daysLeft: number | null;
  importance: number | null;
  urgency: number | null;
  rationale: string;
  aiQuadrant: Quadrant | null;
  quadrant: Quadrant | null;
  overridden: boolean;
  stepCount: number;
  stepsDone: number;
  minutesLeft: number;
}

export interface TimerView {
  sessionId: number;
  stepId: number;
  stepTitle: string;
  doneWhen: string;
  effort: Effort;
  questId: number;
  questTitle: string;
  plannedSeconds: number;
  extendedSeconds: number;
  startedMs: number;
  pausedMs: number;
  pausedSinceMs: number | null;
  pausesUsed: number;
  maxPauses: number;
  /** The operation's next open objectives, in order. */
  upcoming: UpcomingStep[];
}

export interface UpcomingStep {
  id: number;
  title: string;
  doneWhen: string;
  estMinutes: number;
}

export interface StartResult {
  timer: TimerView;
  startXp: number;
}

export type XpReason = "objective" | "on_time" | "strategic" | "critical";

export interface Unlocked {
  /** Family name for tiered achievements, otherwise the achievement id. */
  id: string;
  hidden: boolean;
  goal: number | null;
  level: number | null;
}

export interface TierProgress {
  family: string;
  value: number;
  level: number;
  prevGoal: number;
  nextGoal: number;
}

export interface CompletionResult {
  outcome: "on_time" | "overtime" | "untimed";
  elapsedSeconds: number | null;
  lines: { reason: XpReason; amount: number }[];
  total: number;
  critical: boolean;
  questCompleted: { id: number; title: string; bonus: number } | null;
  achievements: Unlocked[];
  levelBefore: number;
  levelAfter: number;
}

export interface UnstickReply {
  advice: string;
  step: StepDraft;
}

export interface TodayStep {
  stepId: number;
  title: string;
  doneWhen: string;
  estMinutes: number;
  effort: Effort;
  status: StepStatus;
  questId: number;
  questTitle: string;
  quadrant: Quadrant | null;
  daysLeft: number | null;
  note: string;
  xp: number;
}

export interface Today {
  date: string;
  planned: TodayStep[];
  suggestions: TodayStep[];
  completed: TodayStep[];
  debrief: string | null;
}

export interface Stats {
  totalXp: number;
  level: number;
  levelXp: number;
  levelNeed: number;
  streak: number;
  longestStreak: number;
  todayXp: number;
  todayObjectives: number;
  objectivesDone: number;
  operationsDone: number;
  timedRuns: number;
  onTimeRuns: number;
  daily: { date: string; xp: number; objectives: number }[];
  calibration: { category: string; ratio: number; samples: number }[];
  tiers: TierProgress[];
  /** Earned special achievements only; unearned ones stay invisible. */
  specials: { id: string; unlockedAt: string }[];
}

/** What a danger-zone action removed. */
export interface ClearSummary {
  operations: number;
  objectives: number;
  runs: number;
  /** Reset only: XP total before the reset. */
  xp: number;
  /** Reset only: achievements removed. */
  achievements: number;
}

export interface TransferResult {
  quests: number;
  steps: number;
  /** Import only: operations skipped as duplicates or finished work. */
  skipped: number;
}

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  setLanguage: (language: Lang) => invoke<Settings>("set_language", { language }),
  setTheme: (theme: Theme) => invoke<Settings>("set_theme", { theme }),
  setApiKey: (provider: Provider, key: string) => invoke<void>("set_api_key", { provider, key }),
  clearApiKey: (provider: Provider) => invoke<void>("clear_api_key", { provider }),
  hasApiKey: (provider: Provider) => invoke<boolean>("has_api_key", { provider }),
  testLlmConnection: () => invoke<ConnectionCheck>("test_llm_connection"),

  listCategories: () => invoke<Category[]>("list_categories"),
  listQuests: (status: QuestStatus) => invoke<Quest[]>("list_quests", { status }),
  createQuest: (quest: NewQuest) => invoke<Quest>("create_quest", { quest }),
  deleteQuest: (id: number) => invoke<void>("delete_quest", { id }),
  breakdownQuest: (questId: number, answers: Clarification[] = []) =>
    invoke<Breakdown>("breakdown_quest", { questId, answers }),
  deployQuest: (questId: number, title: string, steps: StepDraft[]) =>
    invoke<Quest>("deploy_quest", { questId, title, steps }),
  getSteps: (questId: number) => invoke<Step[]>("get_steps", { questId }),

  listMatrix: () => invoke<MatrixQuest[]>("list_matrix"),
  scoreQuest: (questId: number) => invoke<MatrixQuest>("score_quest", { questId }),
  scorePending: () => invoke<number[]>("score_pending"),
  rescoreAll: () => invoke<number[]>("rescore_all"),
  setQuadrant: (questId: number, quadrant: Quadrant) =>
    invoke<MatrixQuest>("set_quadrant", { questId, quadrant }),
  clearQuadrantOverride: (questId: number) =>
    invoke<MatrixQuest>("clear_quadrant_override", { questId }),

  getTimer: () => invoke<TimerView | null>("get_timer"),
  startStep: (stepId: number) => invoke<StartResult>("start_step", { stepId }),
  pauseTimer: () => invoke<TimerView>("pause_timer"),
  resumeTimer: () => invoke<TimerView>("resume_timer"),
  extendTimer: (minutes: number) => invoke<TimerView>("extend_timer", { minutes }),
  abortStep: () => invoke<void>("abort_step"),
  completeStep: () => invoke<CompletionResult>("complete_step"),
  completeStepUntimed: (stepId: number) =>
    invoke<CompletionResult>("complete_step_untimed", { stepId }),

  splitStep: (stepId: number, workedMinutes: number | null = null) =>
    invoke<Step[]>("split_step", { stepId, workedMinutes }),
  unstickStep: (stepId: number, reason: string) =>
    invoke<UnstickReply>("unstick_step", { stepId, reason }),
  insertStep: (questId: number, beforeStepId: number | null, draft: StepDraft) =>
    invoke<Step>("insert_step", { questId, beforeStepId, draft }),

  getToday: () => invoke<Today>("get_today"),
  planToday: (minutes: number) => invoke<Today>("plan_today", { minutes }),
  clearTodayPlan: () => invoke<Today>("clear_today_plan"),
  debriefToday: () => invoke<Today>("debrief_today"),
  getStats: () => invoke<Stats>("get_stats"),
  exportTasks: (path: string) => invoke<TransferResult>("export_tasks", { path }),
  importTasks: (path: string) => invoke<TransferResult>("import_tasks", { path }),
  clearFinished: () => invoke<ClearSummary>("clear_finished"),
  resetProgress: () => invoke<ClearSummary>("reset_progress"),
};

export const events = {
  onQuestScored: (fn: (q: MatrixQuest) => void): Promise<UnlistenFn> =>
    listen<MatrixQuest>("quest-scored", (e) => fn(e.payload)),
  onScoreFailed: (fn: (f: { questId: number; message: string }) => void): Promise<UnlistenFn> =>
    listen<{ questId: number; message: string }>("quest-score-failed", (e) => fn(e.payload)),
};

/** Tauri rejects with the serialized AppError string. */
export function errorMessage(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
