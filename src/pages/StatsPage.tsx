import { useEffect, useState } from "react";
import { api, type Stats } from "../api";
import { useI18n, type TKey } from "../i18n";
import { useApp } from "../store";

const W = 640;
const H = 200;
const PAD = { top: 20, right: 8, bottom: 26, left: 36 };
const RADIUS = 4;

/** Upper bound rounded to a readable step, so gridlines land on round numbers. */
function niceMax(v: number) {
  if (v <= 0) return 50;
  const step = 10 ** Math.floor(Math.log10(v));
  return Math.ceil(v / step / 2) * step * 2;
}

/** Bar path with rounded top corners and a square base on the baseline. */
function barPath(x: number, y: number, w: number, h: number) {
  const r = Math.min(RADIUS, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

function DailyChart({ daily }: { daily: Stats["daily"] }) {
  const { t, lang } = useI18n();
  const [hover, setHover] = useState<number | null>(null);
  const max = niceMax(Math.max(...daily.map((d) => d.xp)));
  const innerW = W - PAD.left - PAD.right;
  const innerH = H - PAD.top - PAD.bottom;
  const slot = innerW / daily.length;
  const barW = Math.max(4, slot - 2 - slot * 0.35);
  const y = (v: number) => PAD.top + innerH - (v / max) * innerH;
  const grid = [0, max / 2, max];
  const fmtDay = (iso: string) =>
    new Date(`${iso}T00:00:00`).toLocaleDateString(lang === "zh" ? "zh-CN" : "en", {
      month: "numeric",
      day: "numeric",
    });
  const last = daily.length - 1;

  return (
    <div className="chart" onMouseLeave={() => setHover(null)}>
      <svg viewBox={`0 0 ${W} ${H}`} role="img" aria-label={t("st.daily")}>
        {grid.map((g) => (
          <g key={g}>
            <line x1={PAD.left} x2={W - PAD.right} y1={y(g)} y2={y(g)} className="chart-grid" />
            <text x={PAD.left - 6} y={y(g) + 4} className="chart-axis" textAnchor="end">{g}</text>
          </g>
        ))}
        {daily.map((d, i) => {
          const x = PAD.left + i * slot + (slot - barW) / 2;
          const h = Math.max(0, y(0) - y(d.xp));
          return (
            <g key={d.date}>
              {h > 0 && (
                <path
                  d={barPath(x, y(d.xp), barW, h)}
                  className={`chart-bar ${hover === i ? "hover" : ""} ${hover !== null && hover !== i ? "dim" : ""}`}
                />
              )}
              {(i % 2 === last % 2) && (
                <text x={x + barW / 2} y={H - 8} className="chart-axis" textAnchor="middle">
                  {i === last ? t("st.today") : fmtDay(d.date)}
                </text>
              )}
              {i === last && d.xp > 0 && (
                <text x={x + barW / 2} y={y(d.xp) - 6} className="chart-label" textAnchor="middle">
                  {d.xp}
                </text>
              )}
              <rect
                x={PAD.left + i * slot}
                y={PAD.top}
                width={slot}
                height={innerH}
                fill="transparent"
                onMouseEnter={() => setHover(i)}
              />
            </g>
          );
        })}
      </svg>
      {hover !== null && (
        <div
          className="chart-tip"
          style={{ left: `${((PAD.left + hover * slot + slot / 2) / W) * 100}%`, top: 0 }}
        >
          <span className="label">{fmtDay(daily[hover].date)}</span>
          <span className="mono">{daily[hover].xp} XP</span>
          <span className="label">{t("common.objectives", { n: daily[hover].objectives })}</span>
        </div>
      )}
    </div>
  );
}

export default function StatsPage() {
  const { t } = useI18n();
  const { version, fail } = useApp();
  const [stats, setStats] = useState<Stats | null>(null);
  const [asTable, setAsTable] = useState(false);

  useEffect(() => {
    api.getStats().then(setStats).catch(fail);
  }, [version, fail]);

  if (!stats) return <p className="label">{t("common.loading")}</p>;

  const onTime = stats.timedRuns ? Math.round((stats.onTimeRuns / stats.timedRuns) * 100) : null;
  const tiles: { label: TKey; value: string; detail?: string }[] = [
    { label: "st.level", value: String(stats.level) },
    { label: "st.totalXp", value: String(stats.totalXp) },
    { label: "st.objectives", value: String(stats.objectivesDone) },
    { label: "st.operations", value: String(stats.operationsDone) },
    { label: "st.longest", value: t("td.streakDays", { n: stats.longestStreak }) },
    {
      label: "st.onTime",
      value: onTime === null ? "—" : `${onTime}%`,
      detail: stats.timedRuns ? t("st.onTimeDetail", { ok: stats.onTimeRuns, n: stats.timedRuns }) : undefined,
    },
  ];

  return (
    <div className="stack fade-in">
      <div className="tiles">
        {tiles.map((x) => (
          <div className="tile" key={x.label}>
            <span className="label">{t(x.label)}</span>
            <span className="tile-value mono">{x.value}</span>
            {x.detail && <span className="label">{x.detail}</span>}
          </div>
        ))}
      </div>

      <section className="panel">
        <div className="panel-head">
          <h2 className="h-display" style={{ fontSize: 22 }}>{t("st.daily")}</h2>
          <button className="btn btn-ghost" onClick={() => setAsTable(!asTable)}>
            {asTable ? t("st.chartView") : t("st.tableView")}
          </button>
        </div>
        {asTable ? (
          <table className="data-table">
            <thead>
              <tr>
                <th>{t("st.date")}</th>
                <th className="num">{t("st.xp")}</th>
                <th className="num">{t("st.objectives")}</th>
              </tr>
            </thead>
            <tbody>
              {[...stats.daily].reverse().map((d) => (
                <tr key={d.date}>
                  <td className="mono">{d.date}</td>
                  <td className="num mono">{d.xp}</td>
                  <td className="num mono">{d.objectives}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <DailyChart daily={stats.daily} />
        )}
      </section>

      <div className="grid-2">
        <section className="panel">
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 22 }}>{t("st.calibration")}</h2>
          </div>
          <p className="field-hint" style={{ marginTop: 0 }}>{t("st.calibrationHint")}</p>
          {stats.calibration.length === 0 ? (
            <p className="empty-note">{t("st.calibrationEmpty")}</p>
          ) : (
            <table className="data-table">
              <thead>
                <tr>
                  <th>{t("st.category")}</th>
                  <th className="num">{t("st.ratio")}</th>
                  <th className="num">{t("st.samples")}</th>
                </tr>
              </thead>
              <tbody>
                {stats.calibration.map((c) => (
                  <tr key={c.category}>
                    <td>{c.category}</td>
                    <td className={`num mono ${c.ratio > 1.15 ? "warn-text" : c.ratio < 0.85 ? "info-text" : ""}`}>
                      {c.ratio.toFixed(2)}×
                    </td>
                    <td className="num mono">{c.samples}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>

        <section className="panel ach-panel">
          <div className="panel-head">
            <h2 className="h-display" style={{ fontSize: 22 }}>{t("st.achievements")}</h2>
            <span className="label mono">LV.{stats.tiers.reduce((n, f) => n + f.level, 0)}</span>
          </div>
          <ul className="tier-list">
            {stats.tiers.map((f) => {
              const pct = Math.min(
                100,
                Math.round(((f.value - f.prevGoal) / Math.max(1, f.nextGoal - f.prevGoal)) * 100),
              );
              return (
                <li key={f.family} className={`tier ${f.level > 0 ? "earned" : ""}`}>
                  <div className="tier-badge" aria-label={`LV.${f.level}`}>
                    <span className="label">LV</span>
                    <span className="mono">{f.level}</span>
                  </div>
                  <div className="tier-body">
                    <strong>{t(`fam.${f.family}` as TKey)}</strong>
                    <span className="field-hint">{t(`fam.${f.family}.goal` as TKey, { n: f.nextGoal })}</span>
                    <div
                      className="tier-bar"
                      role="progressbar"
                      aria-valuenow={f.value}
                      aria-valuemin={f.prevGoal}
                      aria-valuemax={f.nextGoal}
                    >
                      <div style={{ width: `${pct}%` }} />
                    </div>
                  </div>
                  <span className="label mono tier-count">
                    {t("st.progress", { value: f.value, next: f.nextGoal })}
                  </span>
                </li>
              );
            })}
          </ul>

          {stats.specials.length > 0 && (
            <>
              <div className="panel-head" style={{ marginTop: 22 }}>
                <h3 className="h-display" style={{ fontSize: 18 }}>{t("st.special")}</h3>
                <span className="label mono">{stats.specials.length}</span>
              </div>
              <ul className="achievements">
                {stats.specials.map((a) => (
                  <li key={a.id} className="unlocked">
                    <span className="ach-badge" aria-hidden>◆</span>
                    <div>
                      <strong>{t(`ach.${a.id}` as TKey)}</strong>
                      <span className="field-hint">{t(`ach.${a.id}.d` as TKey)}</span>
                    </div>
                  </li>
                ))}
              </ul>
            </>
          )}
        </section>
      </div>
    </div>
  );
}
