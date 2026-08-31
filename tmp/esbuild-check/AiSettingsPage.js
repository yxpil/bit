import { jsx, jsxs } from "react/jsx-runtime";
import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n";
import PillSwitch from "../components/PillSwitch.jsx";
import { IconPlus, IconTrash, IconSettings, IconCheck, IconX } from "../components/Icons.jsx";
const PROTOCOLS = [
  { id: "openai", label: "OpenAI", base: "https://api.openai.com/v1", model: "gpt-4o-mini" },
  { id: "gemini", label: "Gemini", base: "https://generativelanguage.googleapis.com", model: "gemini-1.5-flash" },
  { id: "claude", label: "Claude", base: "https://api.anthropic.com", model: "claude-3-5-sonnet-latest" }
];
const protoLabel = (id) => PROTOCOLS.find((p) => p.id === id)?.label || id;
const EMPTY = { id: null, name: "", protocol: "openai", base_url: "", api_key: "", model: "" };
export default function AiSettingsPage({ onStats, stats }) {
  const { t } = useLang();
  const [providers, setProviders] = useState([]);
  const [form, setForm] = useState(EMPTY);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const load = () => api.listProviders().then((r) => setProviders(r.providers || [])).catch(() => {
  });
  useEffect(() => {
    load();
  }, []);
  const editing = form.id !== null;
  const pickProtocol = (protocol) => {
    const proto = PROTOCOLS.find((p) => p.id === protocol);
    setForm((f) => ({
      ...f,
      protocol,
      base_url: f.base_url && f.id ? f.base_url : proto?.base || "",
      model: f.model && f.id ? f.model : proto?.model || ""
    }));
  };
  const submit = async (e) => {
    e.preventDefault();
    setError("");
    try {
      if (editing) {
        await api.updateProvider(form.id, form.name, form.protocol, form.base_url, form.api_key, form.model);
      } else {
        await api.addProvider(form.name, form.protocol, form.base_url, form.api_key, form.model);
      }
      setForm(EMPTY);
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
      await load();
      onStats?.();
    } catch (err) {
      setError(String(err));
    }
  };
  const startEdit = (p) => {
    setForm({
      id: p.id,
      name: p.name,
      protocol: p.protocol,
      base_url: p.base_url,
      api_key: p.api_key,
      model: p.model
    });
    setError("");
  };
  const remove = async (id) => {
    await api.removeProvider(id);
    if (form.id === id) setForm(EMPTY);
    await load();
    onStats?.();
  };
  const toggleActive = async (p, next) => {
    await api.setProviderActive(p.id, next);
    await load();
    onStats?.();
  };
  return /* @__PURE__ */ jsxs("div", { className: "flex h-full flex-col gap-4 overflow-y-auto", children: [
    /* @__PURE__ */ jsxs("div", { children: [
      /* @__PURE__ */ jsx("h2", { className: "text-lg font-semibold", children: t("ai.title") }),
      /* @__PURE__ */ jsx("p", { className: "text-xs text-neutral-500", children: t("ai.subtitle") })
    ] }),
    stats && /* @__PURE__ */ jsxs("div", { className: "card", children: [
      /* @__PURE__ */ jsx("p", { className: "mb-3 text-xs font-medium text-neutral-900 dark:text-neutral-100", children: t("ai.statsTitle") }),
      /* @__PURE__ */ jsx("div", { className: "grid grid-cols-3 gap-3 sm:grid-cols-6", children: [
        [t("ai.statTools"), stats.tool_count],
        [t("ai.statMemory"), stats.memory_count],
        [t("ai.statSkills"), stats.skill_count],
        [t("ai.statGoals"), stats.goal_count],
        [t("ai.statTodos"), stats.todo_count],
        [t("ai.statAudit"), stats.audit_count]
      ].map(([label, n]) => /* @__PURE__ */ jsxs("div", { className: "text-center", children: [
        /* @__PURE__ */ jsx("div", { className: "text-xl font-bold tabular-nums", children: n ?? 0 }),
        /* @__PURE__ */ jsx("div", { className: "text-[11px] text-neutral-500", children: label })
      ] }, label)) }),
      /* @__PURE__ */ jsxs("div", { className: "mt-3 flex flex-wrap items-center gap-2 border-t border-neutral-200/70 pt-3 dark:border-neutral-800/70", children: [
        /* @__PURE__ */ jsx("span", { className: "chip", children: stats.ai_configured ? t("ai.aiActive") : t("ai.aiInactive") }),
        /* @__PURE__ */ jsx("span", { className: `chip ${stats.remote?.enabled ? "border-emerald-500/50 text-emerald-600 dark:text-emerald-400" : ""}`, children: stats.remote?.enabled ? `${t("ai.remoteOn")}${stats.remote.addr}` : t("ai.remoteOff") })
      ] })
    ] }),
    /* @__PURE__ */ jsxs("div", { className: "flex flex-col gap-2", children: [
      providers.length === 0 && /* @__PURE__ */ jsxs("div", { className: "card flex items-center justify-center gap-2 py-8 text-sm text-neutral-400", children: [
        /* @__PURE__ */ jsx(IconSettings, { size: 16 }),
        t("ai.emptyProviders")
      ] }),
      providers.map((p) => /* @__PURE__ */ jsxs(
        "div",
        {
          className: `card flex items-center gap-3 ${p.active ? "border-neutral-900/40 dark:border-white/40" : ""}`,
          children: [
            /* @__PURE__ */ jsx(
              PillSwitch,
              {
                checked: p.active,
                onChange: (next) => toggleActive(p, next),
                title: p.active ? t("ai.pauseTip") : t("ai.playTip")
              }
            ),
            /* @__PURE__ */ jsxs("div", { className: "min-w-0 flex-1", children: [
              /* @__PURE__ */ jsxs("div", { className: "flex items-center gap-2", children: [
                /* @__PURE__ */ jsx("span", { className: "truncate text-sm font-semibold", children: p.name }),
                /* @__PURE__ */ jsx("span", { className: "chip shrink-0", children: protoLabel(p.protocol) }),
                p.active && /* @__PURE__ */ jsx("span", { className: "chip shrink-0 border-emerald-500/50 text-emerald-600 dark:text-emerald-400", children: t("ai.inUse") }),
                !p.api_key && /* @__PURE__ */ jsx("span", { className: "chip shrink-0 border-amber-500/50 text-amber-600 dark:text-amber-400", children: t("ai.missingKey") })
              ] }),
              /* @__PURE__ */ jsxs("p", { className: "mt-0.5 truncate font-mono text-[11px] text-neutral-500", children: [
                p.model,
                " \xB7 ",
                p.base_url
              ] })
            ] }),
            /* @__PURE__ */ jsx(
              "button",
              {
                onClick: () => startEdit(p),
                title: t("common.edit"),
                className: "rounded-full p-2 text-neutral-500 transition-colors hover:bg-neutral-100 dark:hover:bg-neutral-800",
                children: /* @__PURE__ */ jsx(IconSettings, { size: 15 })
              }
            ),
            /* @__PURE__ */ jsx(
              "button",
              {
                onClick: () => remove(p.id),
                title: t("common.delete"),
                className: "rounded-full p-2 text-neutral-400 transition-colors hover:bg-red-50 hover:text-red-500 dark:hover:bg-red-950/40",
                children: /* @__PURE__ */ jsx(IconTrash, { size: 15 })
              }
            )
          ]
        },
        p.id
      ))
    ] }),
    /* @__PURE__ */ jsxs("form", { onSubmit: submit, className: "card flex flex-col gap-4", children: [
      /* @__PURE__ */ jsxs("div", { className: "flex items-center justify-between", children: [
        /* @__PURE__ */ jsx("p", { className: "text-sm font-medium", children: editing ? t("ai.editProvider") : t("ai.addProvider") }),
        editing && /* @__PURE__ */ jsxs(
          "button",
          {
            type: "button",
            onClick: () => setForm(EMPTY),
            className: "flex items-center gap-1 text-xs text-neutral-500 hover:text-neutral-900 dark:hover:text-white",
            children: [
              /* @__PURE__ */ jsx(IconX, { size: 13 }),
              t("ai.cancelEdit")
            ]
          }
        )
      ] }),
      /* @__PURE__ */ jsxs("div", { children: [
        /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: t("ai.protocol") }),
        /* @__PURE__ */ jsx("div", { className: "flex gap-2", children: PROTOCOLS.map((proto) => /* @__PURE__ */ jsx(
          "button",
          {
            type: "button",
            onClick: () => pickProtocol(proto.id),
            className: `flex-1 rounded-full px-3 py-2 text-xs font-medium transition-all ${form.protocol === proto.id ? "bg-neutral-900 text-white dark:bg-white dark:text-black" : "bg-neutral-100 text-neutral-500 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700"}`,
            children: proto.label
          },
          proto.id
        )) })
      ] }),
      /* @__PURE__ */ jsxs("div", { children: [
        /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: t("ai.nameLabel") }),
        /* @__PURE__ */ jsx(
          "input",
          {
            className: "field",
            value: form.name,
            onChange: (e) => setForm({ ...form, name: e.target.value }),
            placeholder: `${t("ai.nameExample")}${protoLabel(form.protocol)}`
          }
        )
      ] }),
      /* @__PURE__ */ jsxs("div", { children: [
        /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: "Base URL" }),
        /* @__PURE__ */ jsx(
          "input",
          {
            className: "field font-mono",
            value: form.base_url,
            onChange: (e) => setForm({ ...form, base_url: e.target.value }),
            placeholder: PROTOCOLS.find((p) => p.id === form.protocol)?.base
          }
        )
      ] }),
      /* @__PURE__ */ jsxs("div", { children: [
        /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: "API Key" }),
        /* @__PURE__ */ jsx(
          "input",
          {
            className: "field font-mono",
            type: "password",
            value: form.api_key,
            onChange: (e) => setForm({ ...form, api_key: e.target.value }),
            placeholder: form.protocol === "openai" ? "sk-\u2026" : t("ai.keyPlaceholder")
          }
        )
      ] }),
      /* @__PURE__ */ jsxs("div", { children: [
        /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: t("ai.model") }),
        /* @__PURE__ */ jsx(
          "input",
          {
            className: "field font-mono",
            value: form.model,
            onChange: (e) => setForm({ ...form, model: e.target.value }),
            placeholder: PROTOCOLS.find((p) => p.id === form.protocol)?.model
          }
        )
      ] }),
      error && /* @__PURE__ */ jsx("p", { className: "px-2 text-xs text-red-600", children: error }),
      /* @__PURE__ */ jsxs("div", { className: "flex items-center justify-end gap-3", children: [
        saved && /* @__PURE__ */ jsxs("span", { className: "flex items-center gap-1 text-xs text-neutral-500", children: [
          /* @__PURE__ */ jsx(IconCheck, { size: 14 }),
          t("common.saved")
        ] }),
        /* @__PURE__ */ jsxs("button", { type: "submit", className: "pill pill-hover", children: [
          editing ? /* @__PURE__ */ jsx(IconCheck, { size: 14 }) : /* @__PURE__ */ jsx(IconPlus, { size: 14 }),
          editing ? t("ai.saveChanges") : t("common.add")
        ] })
      ] })
    ] }),
    /* @__PURE__ */ jsxs("div", { className: "card text-xs leading-relaxed text-neutral-500", children: [
      /* @__PURE__ */ jsx("p", { className: "mb-2 font-medium text-neutral-900 dark:text-neutral-100", children: t("ai.autonomyTitle") }),
      /* @__PURE__ */ jsxs("ul", { className: "list-inside list-disc space-y-1", children: [
        /* @__PURE__ */ jsx("li", { children: t("ai.capTools") }),
        /* @__PURE__ */ jsx("li", { children: t("ai.capPlan") }),
        /* @__PURE__ */ jsx("li", { children: t("ai.capScript") }),
        /* @__PURE__ */ jsx("li", { children: t("ai.capSkill") }),
        /* @__PURE__ */ jsx("li", { children: t("ai.capInvoke") })
      ] }),
      /* @__PURE__ */ jsxs("p", { className: "mt-3 border-t border-neutral-200/70 pt-2 text-neutral-500 dark:border-neutral-800/70", children: [
        t("ai.memoryNote1"),
        /* @__PURE__ */ jsx("b", { className: "text-neutral-700 dark:text-neutral-300", children: t("ai.memoryNote2") }),
        t("ai.memoryNote3")
      ] })
    ] })
  ] });
}
