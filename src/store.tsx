// App-wide state: settings, the running timer, player stats and reward
// feedback. Pages read it through `useApp()`.
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import {
  api,
  errorMessage,
  type CompletionResult,
  type Settings,
  type Stats,
  type TimerView,
} from "./api";
import { useT } from "./i18n";
import { sound } from "./sound";

export interface Toast {
  id: number;
  kind: "xp" | "achievement" | "level" | "error" | "info";
  title: string;
  detail?: string;
  lines?: { label: string; amount: number }[];
  critical?: boolean;
}

interface AppState {
  settings: Settings | null;
  setSettings: (s: Settings) => void;
  timer: TimerView | null;
  setTimer: (t: TimerView | null) => void;
  stats: Stats | null;
  /** Increments whenever progress changes, so pages can reload their data. */
  version: number;
  refresh: () => Promise<void>;
  toast: (t: Omit<Toast, "id">) => void;
  toasts: Toast[];
  dismissToast: (id: number) => void;
  mission: { title: string; bonus: number } | null;
  closeMission: () => void;
  focusOpen: boolean;
  setFocusOpen: (open: boolean) => void;
  /** Starts an objective's timed run and opens focus mode. */
  start: (stepId: number) => Promise<boolean>;
  /** Shows rewards for a completion and refreshes everything. */
  celebrate: (r: CompletionResult) => Promise<void>;
  fail: (e: unknown) => void;
}

const AppContext = createContext<AppState | null>(null);

export function useApp(): AppState {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useApp outside AppProvider");
  return ctx;
}

const TOAST_MS = 5200;

export function AppProvider({
  settings,
  setSettings,
  children,
}: {
  settings: Settings | null;
  setSettings: (s: Settings) => void;
  children: ReactNode;
}) {
  const t = useT();
  const [timer, setTimer] = useState<TimerView | null>(null);
  const [stats, setStats] = useState<Stats | null>(null);
  const [version, setVersion] = useState(0);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [mission, setMission] = useState<{ title: string; bonus: number } | null>(null);
  const [focusOpen, setFocusOpen] = useState(false);
  const nextId = useRef(1);

  const dismissToast = useCallback((id: number) => setToasts((ts) => ts.filter((x) => x.id !== id)), []);

  const toast = useCallback(
    (x: Omit<Toast, "id">) => {
      const id = nextId.current++;
      setToasts((ts) => [...ts.slice(-4), { ...x, id }]);
      setTimeout(() => dismissToast(id), x.kind === "error" ? TOAST_MS * 1.6 : TOAST_MS);
    },
    [dismissToast],
  );

  const fail = useCallback((e: unknown) => toast({ kind: "error", title: errorMessage(e) }), [toast]);

  const refresh = useCallback(async () => {
    const [tm, st] = await Promise.all([api.getTimer(), api.getStats()]);
    setTimer(tm);
    setStats(st);
    setVersion((v) => v + 1);
  }, []);

  useEffect(() => {
    refresh().catch(fail);
  }, [refresh, fail]);

  const start = useCallback(
    async (stepId: number) => {
      try {
        const r = await api.startStep(stepId);
        setTimer(r.timer);
        setFocusOpen(true);
        sound.start();
        if (r.startXp > 0) toast({ kind: "info", title: t("rw.startXp", { n: r.startXp }) });
        await refresh();
        return true;
      } catch (e) {
        fail(e);
        return false;
      }
    },
    [refresh, toast, fail, t],
  );

  const celebrate = useCallback(
    async (r: CompletionResult) => {
      if (r.critical) sound.critical();
      else sound.complete();
      toast({
        kind: "xp",
        title: `+${r.total - (r.questCompleted?.bonus ?? 0)} XP`,
        critical: r.critical,
        lines: r.lines.map((l) => ({ label: t(`xp.${l.reason}`), amount: l.amount })),
      });
      if (r.levelAfter > r.levelBefore) {
        toast({ kind: "level", title: t("rw.levelUp", { n: r.levelAfter }) });
      }
      for (const a of r.achievements) {
        toast({
          kind: "achievement",
          title: t(`ach.${a}` as Parameters<typeof t>[0]),
          detail: t("rw.achievement"),
        });
      }
      if (r.questCompleted) {
        setMission({ title: r.questCompleted.title, bonus: r.questCompleted.bonus });
        setTimeout(() => sound.mission(), 250);
      }
      setFocusOpen(false);
      await refresh();
    },
    [refresh, toast, t],
  );

  return (
    <AppContext.Provider
      value={{
        settings,
        setSettings,
        timer,
        setTimer,
        stats,
        version,
        refresh,
        toast,
        toasts,
        dismissToast,
        mission,
        closeMission: () => setMission(null),
        focusOpen,
        setFocusOpen,
        start,
        celebrate,
        fail,
      }}
    >
      {children}
    </AppContext.Provider>
  );
}
