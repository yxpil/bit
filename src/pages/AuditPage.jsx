import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n.js";
import { IconAudit, IconRefresh } from "../components/Icons.jsx";

// 审计日志：所有工具调用 / 注册 / HTTP 访问 / Autopilot 动作
export default function AuditPage() {
  const { t } = useLang();
  const [entries, setEntries] = useState([]);
  const [filter, setFilter] = useState("");

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
        <button onClick={reload} className="pill pill-outline pill-hover">
          <IconRefresh size={14} />
          {t("common.refresh")}
        </button>
      </div>

      <input
        className="field"
        placeholder={t("audit.filterPlaceholder")}
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />

      <div className="card flex-1 overflow-y-auto p-0">
        <table className="w-full text-left text-xs">
          <thead className="sticky top-0 bg-white border-b border-neutral-200">
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
              <tr key={e.id} className="border-b border-neutral-100 last:border-0 hover:bg-neutral-50">
                <td className="whitespace-nowrap px-4 py-2.5 text-neutral-400">{e.ts}</td>
                <td className="whitespace-nowrap px-4 py-2.5">{e.actor}</td>
                <td className="whitespace-nowrap px-4 py-2.5 font-medium">{e.action}</td>
                <td className="max-w-40 truncate px-4 py-2.5">{e.target}</td>
                <td className="max-w-64 truncate px-4 py-2.5 font-mono text-neutral-500">
                  {JSON.stringify(e.detail)}
                </td>
                <td className="px-4 py-2.5">
                  <span className={`inline-block h-2 w-2 rounded-full ${e.ok ? "bg-neutral-900" : "bg-red-500"}`} />
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
    </div>
  );
}
