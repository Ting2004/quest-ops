import { useEffect, useState } from "react";
import { api, type Settings } from "./api";
import { MissionComplete, Toasts } from "./components/Rewards";
import { FocusOverlay, TimerDock } from "./components/Timer";
import { I18nProvider, useT, type TKey } from "./i18n";
import InboxPage from "./pages/InboxPage";
import MatrixPage from "./pages/MatrixPage";
import SettingsPage from "./pages/SettingsPage";
import StatsPage from "./pages/StatsPage";
import TodayPage from "./pages/TodayPage";
import { AppProvider, useApp } from "./store";

type PageId = "today" | "matrix" | "inbox" | "stats" | "settings";

const PAGES: { id: PageId; code: string; title: TKey; kicker: TKey }[] = [
  { id: "today", code: "00", title: "nav.today", kicker: "nav.today.kicker" },
  { id: "matrix", code: "01", title: "nav.matrix", kicker: "nav.matrix.kicker" },
  { id: "inbox", code: "02", title: "nav.inbox", kicker: "nav.inbox.kicker" },
  { id: "stats", code: "03", title: "nav.stats", kicker: "nav.stats.kicker" },
  { id: "settings", code: "04", title: "nav.settings", kicker: "nav.settings.kicker" },
];

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => setSettings(null));
  }, []);

  const lang = settings?.language ?? "en";
  useEffect(() => {
    document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  }, [lang]);

  return (
    <I18nProvider lang={lang}>
      <AppProvider settings={settings} setSettings={setSettings}>
        <Shell />
      </AppProvider>
    </I18nProvider>
  );
}

function Shell() {
  const t = useT();
  const { settings, setSettings, stats } = useApp();
  const [page, setPage] = useState<PageId>("today");

  // Send first-time users to configuration until a model is set.
  useEffect(() => {
    if (settings && !settings.modelName) setPage("settings");
  }, [settings?.modelName]);

  const current = PAGES.find((p) => p.id === page)!;
  const levelPct = stats ? Math.round((stats.levelXp / stats.levelNeed) * 100) : 0;

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="label">{t("brand.kicker")}</span>
          <h1 className="h-display">
            Quest<span>/</span>Ops
          </h1>
        </div>

        <nav className="nav">
          {PAGES.map((p) => (
            <button
              key={p.id}
              className={`nav-item ${p.id === page ? "active" : ""}`}
              onClick={() => setPage(p.id)}
            >
              <span className="label">{p.code}</span>
              {t(p.title)}
            </button>
          ))}
        </nav>

        <div className="sidebar-foot">
          <span className="label">{t("side.commander")}</span>
          <span className="h-display" style={{ fontSize: 20 }}>{settings?.commanderName ?? "—"}</span>
          {stats && (
            <>
              <span className="label mono" style={{ color: "var(--accent)" }}>
                {t("side.level", { level: String(stats.level).padStart(2, "0") })}
              </span>
              <div
                className="xp-bar"
                role="progressbar"
                aria-valuenow={stats.levelXp}
                aria-valuemax={stats.levelNeed}
              >
                <div style={{ width: `${levelPct}%` }} />
              </div>
              <div className="row" style={{ justifyContent: "space-between" }}>
                <span className="label mono">{t("side.xp", { xp: stats.levelXp, need: stats.levelNeed })}</span>
                {stats.streak > 0 && (
                  <span className="label mono" style={{ color: "var(--accent)" }}>
                    {t("side.streak", { n: stats.streak })}
                  </span>
                )}
              </div>
            </>
          )}
        </div>
      </aside>

      <main className="main">
        <TimerDock />
        <header className="page-head">
          <span className="label">
            {current.code} // {t(current.kicker)}
          </span>
          <h1 className="h-display">{t(current.title)}</h1>
        </header>

        {page === "today" && <TodayPage onNavigate={setPage} />}
        {page === "matrix" && <MatrixPage />}
        {page === "inbox" && <InboxPage />}
        {page === "stats" && <StatsPage />}
        {page === "settings" && <SettingsPage onSaved={setSettings} />}
      </main>

      <FocusOverlay />
      <MissionComplete />
      <Toasts />
    </div>
  );
}
