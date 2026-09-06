import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n.js";
import { IconAudit, IconRefresh, IconTrash } from "../components/Icons.jsx";

// 审计日志：所有工具调用 / 注册 / HTTP 访问 / Autopilot 动作；点行弹层查看完整详情。
// 顶部内置诊断报告（折叠）：版本 / 守护 / 数据文件 / 最近崩溃 / 低成功率工具
export default function AuditPage() {
  const { t } = useLang();
  const [entries, setEntries] = useState([]);
  const [filter, setFilter] = useState("");
  const [detail, setDetail] = useState(null);
  const [diag, setDiag] = useState(null);
  const [diagOpen, setDiagOpen] = useState(false);

  const reload = () => api.listAudit().then((r) => setEntries(r.entries || []));
  const loadDiag = () => api.getDiagnostics().then(setDiag).catch(() => {});
  useEffect(() => {
    reload();
    loadDiag();
    const timer = setInterval(reload, 5000);
    return () => clearInterval(timer);
  }, []);

  // 诊断在展开时每 30 秒刷新一次（折叠时不打扰）
  useEffect(() => {
    if (!diagOpen) return;
    loadDiag();
    const timer = setInterval(loadDiag, 30000);
    return () => clearInterval(timer);
  }, [diagOpen]);

  const fmtUptime = (s) => {
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    return d > 0 ? `${d}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m ${s % 60}s`;
  };
  const fmtBytes = (b) =>
    b >= 1048576 ? `${(b / 1048576).toFixed(1)} MB` : b >= 1024 ? `${(b / 1024).toFixed(1)} KB` : `${b} B`;

  const shown = entries.filter(
    (e) =>
      !filter ||
      `${e.actor} ${e.action} ${e.target}`.toLowerCase().includes(filter.toLowerCase())
  );

  return (
    <div className="flex h-full flex-col gap-4">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-lg font-semibold">{t("audit.title")}</h2>
          <p className="text-xs text-neutral-500">
            {t("audit.subtitle")}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => api.clearAudit().then(reload)}
            className="pill pill-outline pill-hover"
          >
            <IconTrash size={14} />
            {t("audit.clear")}
          </button>
          <button onClick={reload} className="pill pill-outline pill-hover">
            <IconRefresh size={14} />
            {t("common.refresh")}
          </button>
        </div>
      </div>

      <input
        className="field"
        placeholder={t("audit.filterPlaceholder")}
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />

      {/* 诊断报告：默认折叠，展开查看版本/守护/数据文件/崩溃记录/低成功率工具 */}
      <div className="card shrink-0 p-0">
        <button
          onClick={() => setDiagOpen((v) => !v)}
          className="flex w-full items-center justify-between px-4 py-3"
        >
          <span className="flex items-center gap-2 text-sm font-semibold">
            <span
              className={`inline-block text-[10px] transition-transform ${diagOpen ? "rotate-90" : ""}`}
            >
              ▶
            </span>
            {t("diag.title")}
          </span>
          {diag && (
            <span className="text-xs text-neutral-400">
              {diag.version} · {diag.platform}
              {diag.crashes?.length > 0 && (
                <span className="ml-2 text-red-500 dark:text-red-400">
                  {t("diag.crashBadge", { n: diag.crashes.length })}
                </span>
              )}
            </span>
          )}
        </button>
        {diagOpen && (
          <div className="border-t border-neutral-200/70 p-4 text-xs dark:border-neutral-800/70">
            {!diag ? (
              <p className="py-4 text-center text-neutral-400">{t("diag.loading")}</p>
            ) : (
              <div className="flex flex-col gap-3">
                <div className="grid grid-cols-2 gap-x-6 gap-y-1.5 md:grid-cols-3">
                  {[
                    [t("diag.version"), diag.version],
                    [t("diag.platform"), diag.platform],
                    [t("diag.elevated"), diag.elevated ? t("diag.yes") : t("diag.no")],
                    [t("diag.uptime"), fmtUptime(diag.uptime_secs)],
                    [
                      t("diag.remote"),
                      diag.remote?.enabled
                        ? `${t("diag.on")} :${diag.remote.port}`
                        : t("diag.off"),
                    ],
                    [
                      t("diag.sessions"),
                      `${diag.sessions?.count ?? 0} (${t("diag.messages")}: ${diag.sessions?.messages ?? 0})`,
                    ],
                    [
                      t("diag.tools"),
                      `${diag.tools?.enabled ?? 0} / ${diag.tools?.total ?? 0}`,
                    ],
                    [
                      t("diag.guardian"),
                      diag.guardian?.armed ? t("diag.on") : t("diag.off"),
                    ],
                  ].map(([k, v]) => (
                    <div key={k} className="flex min-w-0 items-baseline gap-2">
                      <span className="shrink-0 text-neutral-500">{k}</span>
                      <span className="truncate font-medium">{v}</span>
                    </div>
                  ))}
                </div>

                <div>
                  <p className="mb-1 font-medium text-neutral-500">{t("diag.dataDir")}</p>
                  <p className="truncate font-mono text-neutral-400">{diag.data_dir}</p>
                </div>

                <div>
                  <p className="mb-1 font-medium text-neutral-500">{t("diag.files")}</p>
                  <div className="grid grid-cols-2 gap-x-4 gap-y-1 sm:grid-cols-3">
                    {(diag.files || []).map((f) => (
                      <div key={f.name} className="flex items-baseline justify-between gap-2">
                        <span className="truncate font-mono">{f.name}</span>
                        <span
                          className={
                            f.exists
                              ? "shrink-0 text-neutral-400"
                              : "shrink-0 text-amber-600 dark:text-amber-400"
                          }
                        >
                          {f.exists ? fmtBytes(f.bytes) : t("diag.fileMissing")}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>

                {(diag.worst_tools || []).length > 0 && (
                  <div>
                    <p className="mb-1 font-medium text-neutral-500">{t("diag.worstTools")}</p>
                    <div className="flex flex-col gap-1">
                      {diag.worst_tools.map((w) => (
                        <div
                          key={w.id}
                          className="flex items-baseline justify-between gap-2"
                          title={w.last_err || ""}
                        >
                          <span className="truncate font-medium">{w.name}</span>
                          <span
                            className={
                              (w.recent_rate ?? 1) * 100 >= 50
                                ? "shrink-0 text-neutral-400"
                                : "shrink-0 text-amber-600 dark:text-amber-400"
                            }
                          >
                            {t("tools.successRate")}{" "}
                            {Math.round((w.recent_rate ?? 0) * 100)}% ·{" "}
                            {w.fail}/{w.total} {t("diag.failOf")}
                          </span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                <div>
                  <p className="mb-1 font-medium text-neutral-500">{t("diag.crashes")}</p>
                  {(diag.crashes || []).length === 0 ? (
                    <p className="text-neutral-400">{t("diag.noCrashes")}</p>
                  ) : (
                    <pre className="max-h-48 overflow-y-auto whitespace-pre-wrap rounded-xl bg-neutral-100 p-3 font-mono leading-relaxed dark:bg-neutral-800/60">
                      {diag.crashes
                        .map(
                          (c) =>
                            `${c.time} [${c.thread}] ${c.msg}\n  ${c.loc || "?"}`
                        )
                        .join("\n")}
                    </pre>
                  )}
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      <div className="card flex-1 overflow-y-auto p-0">
        <table className="w-full text-left text-xs">
          <thead className="sticky top-0 border-b border-neutral-200 bg-white dark:border-neutral-800 dark:bg-neutral-900">
            <tr className="text-neutral-500">
              <th className="px-4 py-3 font-medium">{t("audit.time")}</th>
              <th className="px-4 py-3 font-medium">{t("audit.actor")}</th>
              <th className="px-4 py-3 font-medium">{t("audit.action")}</th>
              <th className="px-4 py-3 font-medium">{t("audit.target")}</th>
              <th className="px-4 py-3 font-medium">{t("audit.detail")}</th>
              <th className="px-4 py-3 font-medium">{t("audit.result")}</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((e) => (
              <tr
                key={e.id}
                onClick={() => setDetail(e)}
                className="cursor-pointer border-b border-neutral-100 last:border-0 hover:bg-neutral-50 dark:border-neutral-800/60 dark:hover:bg-neutral-800/60"
              >
                <td className="whitespace-nowrap px-4 py-2.5 text-neutral-400">{e.ts}</td>
                <td className="whitespace-nowrap px-4 py-2.5">{e.actor}</td>
                <td className="whitespace-nowrap px-4 py-2.5 font-medium">{e.action}</td>
                <td className="max-w-40 truncate px-4 py-2.5">{e.target}</td>
                <td className="max-w-64 truncate px-4 py-2.5 font-mono text-neutral-500">
                  {JSON.stringify(e.detail)}
                </td>
                <td className="px-4 py-2.5">
                  <div className="flex items-center justify-end gap-2 pr-1">
                    <span className={`inline-block h-2 w-2 rounded-full ${e.ok ? "bg-neutral-900" : "bg-red-500"}`} />
                    <button
                      onClick={(ev) => {
                        ev.stopPropagation();
                        api.deleteAuditEntry(e.id).then(reload);
                      }}
                      title={t("audit.delete")}
                      className="text-neutral-300 transition-colors hover:text-red-500"
                    >
                      <IconTrash size={13} />
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {shown.length === 0 && (
          <div className="flex items-center justify-center gap-2 py-10 text-sm text-neutral-400">
            <IconAudit size={16} />
            {t("audit.empty")}
          </div>
        )}
      </div>

      {/* 详情弹层：完整字段 + 格式化 JSON，点遮罩关闭 */}
      {detail && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
          onClick={() => setDetail(null)}
        >
          <div
            onClick={(ev) => ev.stopPropagation()}
            className="card max-h-[80vh] w-full max-w-2xl overflow-y-auto rounded-2xl p-5"
          >
            <div className="mb-3 flex items-center justify-between">
              <h3 className="text-sm font-semibold">
                {detail.action} · {detail.target}
              </h3>
              <button
                onClick={() => setDetail(null)}
                className="pill pill-outline pill-hover px-3 py-1 text-xs"
              >
                {t("common.close")}
              </button>
            </div>
            <div className="mb-3 grid grid-cols-2 gap-x-4 gap-y-1.5 text-xs">
              <div className="text-neutral-500">{t("audit.time")}</div>
              <div className="font-mono">{detail.ts}</div>
              <div className="text-neutral-500">{t("audit.actor")}</div>
              <div className="font-mono">{detail.actor}</div>
              <div className="text-neutral-500">{t("audit.result")}</div>
              <div>
                <span
                  className={`inline-block h-2 w-2 rounded-full ${detail.ok ? "bg-neutral-900 dark:bg-white" : "bg-red-500"}`}
                />
                <span className="ml-1.5">{detail.ok ? t("toolcard.success") : t("toolcard.fail")}</span>
              </div>
            </div>
            <pre className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-xl bg-neutral-100 p-3 font-mono text-[11px] leading-relaxed dark:bg-neutral-800">
              {JSON.stringify(detail.detail, null, 2)}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
