import { useEffect, useState } from "react";
import { api, errorMessage, type Settings } from "../api";
import { useT, type Lang } from "../i18n";

type Status = { kind: "ok" | "warn" | "err" | "idle"; text: string };

const LANGUAGES: { value: Lang; label: string }[] = [
  { value: "en", label: "English" },
  { value: "zh", label: "简体中文" },
];

export default function SettingsPage({ onSaved }: { onSaved: (s: Settings) => void }) {
  const t = useT();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [hasKey, setHasKey] = useState(false);
  const [keyDraft, setKeyDraft] = useState("");
  const [saveStatus, setSaveStatus] = useState<Status>({ kind: "idle", text: "" });
  const [linkStatus, setLinkStatus] = useState<Status>({ kind: "idle", text: "" });
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    Promise.all([api.getSettings(), api.hasApiKey()])
      .then(([s, k]) => {
        setSettings(s);
        setHasKey(k);
      })
      .catch((e) => setSaveStatus({ kind: "err", text: errorMessage(e) }));
  }, []);

  if (!settings) return <p className="label">{t("common.loading")}</p>;

  const update = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSettings({ ...settings, [key]: value });
    setSaveStatus({ kind: "warn", text: t("set.unsaved") });
  };

  async function changeLanguage(language: Lang) {
    try {
      const saved = await api.setLanguage(language);
      setSettings((s) => (s ? { ...s, language: saved.language } : saved));
      onSaved(saved);
    } catch (e) {
      setSaveStatus({ kind: "err", text: errorMessage(e) });
    }
  }

  async function save() {
    if (!settings) return;
    setBusy(true);
    try {
      const saved = await api.saveSettings(settings);
      setSettings(saved);
      onSaved(saved);
      setSaveStatus({ kind: "ok", text: t("set.saved") });
    } catch (e) {
      setSaveStatus({ kind: "err", text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  async function withBusy(fn: () => Promise<void>) {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      setLinkStatus({ kind: "err", text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  }

  const storeKey = () =>
    withBusy(async () => {
      await api.setApiKey(keyDraft);
      setKeyDraft("");
      setHasKey(await api.hasApiKey());
      setLinkStatus({ kind: "idle", text: t("set.keyUpdated") });
    });

  const removeKey = () =>
    withBusy(async () => {
      await api.clearApiKey();
      setHasKey(false);
      setLinkStatus({ kind: "idle", text: t("set.keyRemoved") });
    });

  const testLink = () =>
    withBusy(async () => {
      setLinkStatus({ kind: "warn", text: t("set.linking") });
      const r = await api.testLlmConnection();
      setLinkStatus({ kind: r.ok ? "ok" : "err", text: r.message });
    });

  const days = settings.urgentWithinDays;

  return (
    <div className="stack fade-in">
      <div className="grid-2">
        <section className="panel">
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 24 }}>{t("set.uplink")}</h2>
            <span className="label">LLM-01 / OpenAI</span>
          </div>

          <div className="field">
            <label className="label" htmlFor="api-key">{t("set.apiKey")}</label>
            <div className="row" style={{ flexWrap: "nowrap" }}>
              <input
                id="api-key"
                className="input"
                type="password"
                autoComplete="off"
                placeholder={hasKey ? t("set.apiKeyStored") : "sk-…"}
                value={keyDraft}
                onChange={(e) => setKeyDraft(e.target.value)}
              />
              <button className="btn" disabled={busy || !keyDraft.trim()} onClick={storeKey}>
                {t("set.store")}
              </button>
            </div>
            <span className="field-hint">{t("set.apiKeyHint")}</span>
          </div>

          <div className="field">
            <label className="label" htmlFor="model">{t("set.model")}</label>
            <input
              id="model"
              className="input"
              placeholder={t("set.modelPlaceholder")}
              value={settings.modelName}
              onChange={(e) => update("modelName", e.target.value)}
            />
            <span className="field-hint">{t("set.modelHint")}</span>
          </div>

          <div className="field">
            <label className="label" htmlFor="small-model">{t("set.fastModel")}</label>
            <input
              id="small-model"
              className="input"
              placeholder={t("set.fastModelPlaceholder")}
              value={settings.smallModelName}
              onChange={(e) => update("smallModelName", e.target.value)}
            />
            <span className="field-hint">{t("set.fastModelHint")}</span>
          </div>

          <div className="field">
            <label className="label" htmlFor="base-url">{t("set.baseUrl")}</label>
            <input
              id="base-url"
              className="input"
              value={settings.apiBaseUrl}
              onChange={(e) => update("apiBaseUrl", e.target.value)}
            />
          </div>

          <div className="row" style={{ marginTop: 20, justifyContent: "space-between" }}>
            <span className={`status ${hasKey ? linkStatus.kind : "idle"}`}>
              {hasKey ? linkStatus.text || t("set.notTested") : t("set.noKey")}
            </span>
            <div className="row">
              {hasKey && (
                <button className="btn btn-ghost" disabled={busy} onClick={removeKey}>
                  {t("set.removeKey")}
                </button>
              )}
              <button
                className="btn"
                disabled={busy || !hasKey || !settings.modelName.trim()}
                onClick={testLink}
                title={t("set.testLinkTitle")}
              >
                {t("set.testLink")}
              </button>
            </div>
          </div>
        </section>

        <section className="panel">
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 24 }}>{t("set.profile")}</h2>
            <span className="label">PRF-01</span>
          </div>

          <div className="row" style={{ alignItems: "flex-start", gap: 16 }}>
            <div className="field" style={{ flex: 1 }}>
              <label className="label" htmlFor="name">{t("set.callsign")}</label>
              <input
                id="name"
                className="input"
                value={settings.commanderName}
                onChange={(e) => update("commanderName", e.target.value)}
              />
            </div>
            <div className="field" style={{ marginTop: 0 }}>
              <span className="label">{t("set.language")}</span>
              <div className="segmented" role="radiogroup" aria-label={t("set.language")}>
                {LANGUAGES.map((l) => (
                  <button
                    key={l.value}
                    role="radio"
                    aria-checked={settings.language === l.value}
                    className={settings.language === l.value ? "on" : ""}
                    onClick={() => changeLanguage(l.value)}
                  >
                    {l.label}
                  </button>
                ))}
              </div>
            </div>
          </div>
          <span className="field-hint">{t("set.languageHint")}</span>

          <div className="field">
            <label className="label" htmlFor="goals">{t("set.goals")}</label>
            <textarea
              id="goals"
              className="input"
              rows={6}
              placeholder={t("set.goalsPlaceholder")}
              value={settings.goalsText}
              onChange={(e) => update("goalsText", e.target.value)}
            />
            <span className="field-hint">{t("set.goalsHint")}</span>
          </div>

          <div className="field">
            <label className="label" htmlFor="pauses">{t("set.pauses")}</label>
            <input
              id="pauses"
              className="input"
              type="number"
              min={0}
              max={10}
              style={{ width: 100 }}
              value={settings.maxPauses}
              onChange={(e) => update("maxPauses", Number(e.target.value))}
            />
          </div>
        </section>
      </div>

      <section className="panel">
        <div className="panel-head">
          <h2 className="h-display" style={{ fontSize: 24 }}>{t("set.rules")}</h2>
          <span className="label">PRI-01</span>
        </div>
        <div className="grid-2">
          <div className="field" style={{ marginTop: 0 }}>
            <label className="label" htmlFor="urgent-days">{t("set.urgentDays")}</label>
            <div className="row">
              <input
                id="urgent-days"
                className="input"
                type="number"
                min={0}
                max={60}
                style={{ width: 90 }}
                value={days}
                onChange={(e) => update("urgentWithinDays", Number(e.target.value))}
              />
              <span className="label">{t("set.days")}</span>
              <span className="tag hot mono">D-{days}</span>
            </div>
            <span className="field-hint">
              {days === 0 ? t("set.urgentHintZero") : t("set.urgentHint", { n: days })}
            </span>
          </div>
          <div className="field" style={{ marginTop: 0 }}>
            <span className="label">{t("set.importance")}</span>
            <div className="segmented" role="radiogroup" aria-label={t("set.importance")}>
              {[2, 3, 4, 5].map((n) => (
                <button
                  key={n}
                  role="radio"
                  aria-checked={settings.importanceThreshold === n}
                  className={`mono ${settings.importanceThreshold === n ? "on" : ""}`}
                  onClick={() => update("importanceThreshold", n)}
                >
                  {n}+
                </button>
              ))}
            </div>
            <span className="field-hint">{t("set.importanceHint")}</span>
          </div>
        </div>
        <p className="field-hint" style={{ marginBottom: 0 }}>{t("set.rulesHint")}</p>
      </section>

      <div className="hazard" />

      <div className="row" style={{ justifyContent: "space-between" }}>
        <span className={`status ${saveStatus.kind}`}>{saveStatus.text || t("set.noChanges")}</span>
        <button className="btn btn-primary" disabled={busy} onClick={save}>
          {t("set.saveAll")}
        </button>
      </div>
    </div>
  );
}
