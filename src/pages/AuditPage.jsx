import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n.js";
import { IconAudit, IconRefresh, IconTrash } from "../components/Icons.jsx";

// 审计日志：所有工具调用 / 注册 / HTTP 访问 / Autopilot 动作；点行弹层查看完整详情
export default function AuditPage() {
  const { t } = useLang();
  const [entries, setEntries] = useState([]);
  const [filter, setFilter] = useState("");
  const [detail, setDetail] = useState(null);

  const reload = () => api.listAudit().then((r) => setEntries(r.entries || []));
  useEffect(() => {
    reload();
    const timer = setInterval(reload, 5000);
    return () => clearInterval(timer);
  }, []);

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
