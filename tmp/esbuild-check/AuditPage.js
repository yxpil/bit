import { jsx, jsxs } from "react/jsx-runtime";
import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n";
import { IconAudit, IconRefresh } from "../components/Icons.jsx";
export default function AuditPage() {
  const { t } = useLang();
  const [entries, setEntries] = useState([]);
  const [filter, setFilter] = useState("");
  const reload = () => api.listAudit().then((r) => setEntries(r.entries || []));
  useEffect(() => {
    reload();
    const timer = setInterval(reload, 5e3);
    return () => clearInterval(timer);
  }, []);
  const shown = entries.filter(
    (e) => !filter || `${e.actor} ${e.action} ${e.target}`.toLowerCase().includes(filter.toLowerCase())
  );
  return /* @__PURE__ */ jsxs("div", { className: "flex h-full flex-col gap-4", children: [
    /* @__PURE__ */ jsxs("div", { className: "flex items-center justify-between", children: [
      /* @__PURE__ */ jsxs("div", { children: [
        /* @__PURE__ */ jsx("h2", { className: "text-lg font-semibold", children: t("audit.title") }),
        /* @__PURE__ */ jsx("p", { className: "text-xs text-neutral-500", children: t("audit.subtitle") })
      ] }),
      /* @__PURE__ */ jsxs("button", { onClick: reload, className: "pill pill-outline pill-hover", children: [
        /* @__PURE__ */ jsx(IconRefresh, { size: 14 }),
        t("common.refresh")
      ] })
    ] }),
    /* @__PURE__ */ jsx(
      "input",
      {
        className: "field",
        placeholder: t("audit.filterPlaceholder"),
        value: filter,
        onChange: (e) => setFilter(e.target.value)
      }
    ),
    /* @__PURE__ */ jsxs("div", { className: "card flex-1 overflow-y-auto p-0", children: [
      /* @__PURE__ */ jsxs("table", { className: "w-full text-left text-xs", children: [
        /* @__PURE__ */ jsx("thead", { className: "sticky top-0 bg-white border-b border-neutral-200", children: /* @__PURE__ */ jsxs("tr", { className: "text-neutral-500", children: [
          /* @__PURE__ */ jsx("th", { className: "px-4 py-3 font-medium", children: t("audit.time") }),
          /* @__PURE__ */ jsx("th", { className: "px-4 py-3 font-medium", children: t("audit.actor") }),
          /* @__PURE__ */ jsx("th", { className: "px-4 py-3 font-medium", children: t("audit.action") }),
          /* @__PURE__ */ jsx("th", { className: "px-4 py-3 font-medium", children: t("audit.target") }),
          /* @__PURE__ */ jsx("th", { className: "px-4 py-3 font-medium", children: t("audit.detail") }),
          /* @__PURE__ */ jsx("th", { className: "px-4 py-3 font-medium", children: t("audit.result") })
        ] }) }),
        /* @__PURE__ */ jsx("tbody", { children: shown.map((e) => /* @__PURE__ */ jsxs("tr", { className: "border-b border-neutral-100 last:border-0 hover:bg-neutral-50", children: [
          /* @__PURE__ */ jsx("td", { className: "whitespace-nowrap px-4 py-2.5 text-neutral-400", children: e.ts }),
          /* @__PURE__ */ jsx("td", { className: "whitespace-nowrap px-4 py-2.5", children: e.actor }),
          /* @__PURE__ */ jsx("td", { className: "whitespace-nowrap px-4 py-2.5 font-medium", children: e.action }),
          /* @__PURE__ */ jsx("td", { className: "max-w-40 truncate px-4 py-2.5", children: e.target }),
          /* @__PURE__ */ jsx("td", { className: "max-w-64 truncate px-4 py-2.5 font-mono text-neutral-500", children: JSON.stringify(e.detail) }),
          /* @__PURE__ */ jsx("td", { className: "px-4 py-2.5", children: /* @__PURE__ */ jsx("span", { className: `inline-block h-2 w-2 rounded-full ${e.ok ? "bg-neutral-900" : "bg-red-500"}` }) })
        ] }, e.id)) })
      ] }),
      shown.length === 0 && /* @__PURE__ */ jsxs("div", { className: "flex items-center justify-center gap-2 py-10 text-sm text-neutral-400", children: [
        /* @__PURE__ */ jsx(IconAudit, { size: 16 }),
        t("audit.empty")
      ] })
    ] })
  ] });
}
