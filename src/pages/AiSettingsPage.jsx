import { useEffect, useRef, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n.js";
import PillSwitch from "../components/PillSwitch.jsx";
import { IconPlus, IconTrash, IconSettings, IconCheck, IconX } from "../components/Icons.jsx";

// 支持的协议：原生对接各家 API
const PROTOCOLS = [
  { id: "openai", label: "OpenAI", base: "https://api.openai.com/v1", model: "gpt-4o-mini" },
  { id: "gemini", label: "Gemini", base: "https://generativelanguage.googleapis.com", model: "gemini-1.5-flash" },
  { id: "claude", label: "Claude", base: "https://api.anthropic.com", model: "claude-3-5-sonnet-latest" },
];
const protoLabel = (id) => PROTOCOLS.find((p) => p.id === id)?.label || id;

// 主流模型预设：点选自动填充协议 / Base URL / 模型名（OpenAI 兼容的国内外主流端全覆盖）
const PRESETS = [
  { name: "OpenAI GPT-4o", protocol: "openai", base: "https://api.openai.com/v1", model: "gpt-4o" },
  { name: "DeepSeek", protocol: "openai", base: "https://api.deepseek.com/v1", model: "deepseek-chat" },
  { name: "Kimi 月之暗面", protocol: "openai", base: "https://api.moonshot.cn/v1", model: "kimi-k2-0905-preview" },
  { name: "通义千问 Qwen", protocol: "openai", base: "https://dashscope.aliyuncs.com/compatible-mode/v1", model: "qwen-max" },
  { name: "智谱 GLM", protocol: "openai", base: "https://open.bigmodel.cn/api/paas/v4", model: "glm-4.6" },
  { name: "xAI Grok", protocol: "openai", base: "https://api.x.ai/v1", model: "grok-4" },
  { name: "OpenRouter", protocol: "openai", base: "https://openrouter.ai/api/v1", model: "openrouter/auto" },
  { name: "硅基流动", protocol: "openai", base: "https://api.siliconflow.cn/v1", model: "deepseek-ai/DeepSeek-V3" },
  { name: "Ollama（本地）", protocol: "openai", base: "http://127.0.0.1:11434/v1", model: "qwen2.5" },
  { name: "Google Gemini", protocol: "gemini", base: "https://generativelanguage.googleapis.com", model: "gemini-2.5-flash" },
  { name: "Anthropic Claude", protocol: "claude", base: "https://api.anthropic.com", model: "claude-sonnet-4-5" },
];

const EMPTY = { id: null, name: "", protocol: "openai", base_url: "", api_key: "", model: "" };

// AI 设置：可增删的多家提供方，用播放/暂停切换，每次仅一个激活
export default function AiSettingsPage({ onStats, stats }) {
  const { t } = useLang();
  const [providers, setProviders] = useState([]);
  const [form, setForm] = useState(EMPTY); // 新增 / 编辑用同一张表单
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  // 模型采样参数：temperature null=默认；reasoning_effort ""=默认 / low / medium / high
  const [params, setParams] = useState({ temperature: null, reasoning_effort: "" });
  const [paramsSaved, setParamsSaved] = useState(false);

  const load = () =>
    api.listProviders().then((r) => setProviders(r.providers || [])).catch(() => {});
  useEffect(() => {
    load();
    api.getAiParams().then((r) => setParams({ temperature: r?.temperature ?? null, reasoning_effort: r?.reasoning_effort || "" })).catch(() => {});
  }, []);

  // 变更即保存（温度滑块拖动用 300ms 防抖，避免频繁落盘）
  const debounceRef = useRef(null);
  const saveParams = (next, debounce = false) => {
    setParams(next);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    const doSave = () => {
      api.setAiParams(next.temperature, next.reasoning_effort).then(() => {
        setParamsSaved(true);
        setTimeout(() => setParamsSaved(false), 1500);
      }).catch(() => {});
    };
    if (debounce) debounceRef.current = setTimeout(doSave, 300);
    else doSave();
  };

  const editing = form.id !== null;

  // 选协议时自动带出该协议的默认 base_url / model（仅当用户未填时）
  const pickProtocol = (protocol) => {
    const proto = PROTOCOLS.find((p) => p.id === protocol);
    setForm((f) => ({
      ...f,
      protocol,
      base_url: f.base_url && f.id ? f.base_url : proto?.base || "",
      model: f.model && f.id ? f.model : proto?.model || "",
    }));
  };

  // 选预设：一键填充协议 / Base URL / 模型 / 名称（Key 需用户自填）
  const pickPreset = (preset) => {
    setForm((f) => ({
      ...f,
      name: preset.name,
      protocol: preset.protocol,
      base_url: preset.base,
      model: preset.model,
    }));
    setError("");
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
      model: p.model,
    });
    setError("");
  };

  const remove = async (id) => {
    await api.removeProvider(id);
    if (form.id === id) setForm(EMPTY);
    await load();
    onStats?.();
  };

  // 播放/暂停：激活某项（互斥）或暂停当前项
  const toggleActive = async (p, next) => {
    await api.setProviderActive(p.id, next);
    await load();
    onStats?.();
  };

  return (
    <div className="flex h-full flex-col gap-4 overflow-y-auto">
      <div>
        <h2 className="text-lg font-semibold">{t("ai.title")}</h2>
        <p className="text-xs text-neutral-500">
          {t("ai.subtitle")}
        </p>
      </div>

      {/* 运行总览 */}
      {stats && (
        <div className="card">
          <p className="mb-3 text-xs font-medium text-neutral-900 dark:text-neutral-100">{t("ai.statsTitle")}</p>
          <div className="grid grid-cols-3 gap-3 sm:grid-cols-6">
            {[
              [t("ai.statTools"), stats.tool_count],
              [t("ai.statMemory"), stats.memory_count],
              [t("ai.statSkills"), stats.skill_count],
              [t("ai.statGoals"), stats.goal_count],
              [t("ai.statTodos"), stats.todo_count],
              [t("ai.statAudit"), stats.audit_count],
            ].map(([label, n]) => (
              <div key={label} className="text-center">
                <div className="text-xl font-bold tabular-nums">{n ?? 0}</div>
                <div className="text-[11px] text-neutral-500">{label}</div>
              </div>
            ))}
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-2 border-t border-neutral-200/70 pt-3 dark:border-neutral-800/70">
            <span className="chip">{stats.ai_configured ? t("ai.aiActive") : t("ai.aiInactive")}</span>
            <span className={`chip ${stats.remote?.enabled ? "border-emerald-500/50 text-emerald-600 dark:text-emerald-400" : ""}`}>
              {stats.remote?.enabled ? `${t("ai.remoteOn")}${stats.remote.addr}` : t("ai.remoteOff")}
            </span>
          </div>
        </div>
      )}

      {/* 提供方列表 */}
      <div className="flex flex-col gap-2">
        {providers.length === 0 && (
          <div className="card flex items-center justify-center gap-2 py-8 text-sm text-neutral-400">
            <IconSettings size={16} />
            {t("ai.emptyProviders")}
          </div>
        )}
        {providers.map((p) => (
          <div
            key={p.id}
            className={`card flex items-center gap-3 ${
              p.active ? "border-neutral-900/40 dark:border-white/40" : ""
            }`}
          >
            <PillSwitch
              checked={p.active}
              onChange={(next) => toggleActive(p, next)}
              title={p.active ? t("ai.pauseTip") : t("ai.playTip")}
            />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate text-sm font-semibold">{p.name}</span>
                <span className="chip shrink-0">{protoLabel(p.protocol)}</span>
                {p.active && (
                  <span className="chip shrink-0 border-emerald-500/50 text-emerald-600 dark:text-emerald-400">
                    {t("ai.inUse")}
                  </span>
                )}
                {!p.api_key && (
                  <span className="chip shrink-0 border-amber-500/50 text-amber-600 dark:text-amber-400">
                    {t("ai.missingKey")}
                  </span>
                )}
              </div>
              <p className="mt-0.5 truncate font-mono text-[11px] text-neutral-500">
                {p.model} · {p.base_url}
              </p>
            </div>
            <button
              onClick={() => startEdit(p)}
              title={t("common.edit")}
              className="rounded-full p-2 text-neutral-500 transition-colors hover:bg-neutral-100 dark:hover:bg-neutral-800"
            >
              <IconSettings size={15} />
            </button>
            <button
              onClick={() => remove(p.id)}
              title={t("common.delete")}
              className="rounded-full p-2 text-neutral-400 transition-colors hover:bg-red-50 hover:text-red-500 dark:hover:bg-red-950/40"
            >
              <IconTrash size={15} />
            </button>
          </div>
        ))}
      </div>

      {/* 模型参数：思考强度 + 温度 */}
      <div className="card flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <p className="text-sm font-medium">{t("ai.paramsTitle")}</p>
          {paramsSaved && (
            <span className="flex items-center gap-1 text-xs text-neutral-500">
              <IconCheck size={14} />
              {t("common.saved")}
            </span>
          )}
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">{t("ai.reasoningLabel")}</label>
          <div className="flex gap-2">
            {[
              ["", t("ai.thinkDefault")],
              ["low", t("ai.thinkLow")],
              ["medium", t("ai.thinkMedium")],
              ["high", t("ai.thinkHigh")],
            ].map(([v, label]) => (
              <button
                key={v || "default"}
                type="button"
                onClick={() => saveParams({ ...params, reasoning_effort: v })}
                className={`flex-1 rounded-full px-3 py-2 text-xs font-medium transition-all ${
                  params.reasoning_effort === v
                    ? "bg-neutral-900 text-white dark:bg-white dark:text-black"
                    : "bg-neutral-100 text-neutral-500 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
          <p className="mt-1 px-2 text-[11px] text-neutral-400">{t("ai.reasoningHint")}</p>
        </div>

        <div>
          <div className="mb-1 flex items-center justify-between px-2">
            <label className="text-xs text-neutral-500">{t("ai.temperatureLabel")}</label>
            <div className="flex gap-1">
              <button
                type="button"
                onClick={() => params.temperature !== null && saveParams({ ...params, temperature: null })}
                className={`rounded-full px-2.5 py-1 text-[11px] font-medium transition-colors ${
                  params.temperature === null
                    ? "bg-neutral-900 text-white dark:bg-white dark:text-black"
                    : "bg-neutral-100 text-neutral-500 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700"
                }`}
              >
                {t("ai.thinkDefault")}
              </button>
              <button
                type="button"
                onClick={() => (params.temperature === null ? saveParams({ ...params, temperature: 0.7 }) : saveParams({ ...params, temperature: Math.min(2, Math.max(0, params.temperature)) }))}
                className={`rounded-full px-2.5 py-1 text-[11px] font-medium transition-colors ${
                  params.temperature !== null
                    ? "bg-neutral-900 text-white dark:bg-white dark:text-black"
                    : "bg-neutral-100 text-neutral-500 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700"
                }`}
              >
                {t("ai.tempCustom")}
              </button>
            </div>
          </div>
          {params.temperature !== null ? (
            <div className="flex items-center gap-3 px-2">
              <input
                type="range"
                min="0"
                max="2"
                step="0.1"
                value={params.temperature}
                onChange={(e) => saveParams({ ...params, temperature: parseFloat(e.target.value) }, true)}
                className="flex-1"
              />
              <span className="w-10 text-right text-xs tabular-nums">{params.temperature.toFixed(1)}</span>
            </div>
          ) : (
            <p className="px-2 text-[11px] text-neutral-400">{t("ai.temperatureDefaultHint")}</p>
          )}
        </div>
      </div>

      {/* 新增 / 编辑表单 */}
      <form onSubmit={submit} className="card flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <p className="text-sm font-medium">{editing ? t("ai.editProvider") : t("ai.addProvider")}</p>
          {editing && (
            <button
              type="button"
              onClick={() => setForm(EMPTY)}
              className="flex items-center gap-1 text-xs text-neutral-500 hover:text-neutral-900 dark:hover:text-white"
            >
              <IconX size={13} />
              {t("ai.cancelEdit")}
            </button>
          )}
        </div>

        {/* 主流模型预设：点选一键填充 */}
        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">{t("ai.preset")}</label>
          <div className="flex flex-wrap gap-1.5 px-2">
            {PRESETS.map((preset) => (
              <button
                key={preset.name}
                type="button"
                onClick={() => pickPreset(preset)}
                className={`rounded-full border px-2.5 py-1 text-[11px] font-medium transition-colors ${
                  form.model === preset.model && form.base_url === preset.base
                    ? "border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white dark:text-black"
                    : "border-neutral-200 text-neutral-500 hover:border-neutral-400 hover:text-neutral-900 dark:border-neutral-700 dark:hover:border-neutral-500 dark:hover:text-white"
                }`}
              >
                {preset.name}
              </button>
            ))}
          </div>
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">{t("ai.protocol")}</label>
          <div className="flex gap-2">
            {PROTOCOLS.map((proto) => (
              <button
                key={proto.id}
                type="button"
                onClick={() => pickProtocol(proto.id)}
                className={`flex-1 rounded-full px-3 py-2 text-xs font-medium transition-all ${
                  form.protocol === proto.id
                    ? "bg-neutral-900 text-white dark:bg-white dark:text-black"
                    : "bg-neutral-100 text-neutral-500 hover:bg-neutral-200 dark:bg-neutral-800 dark:hover:bg-neutral-700"
                }`}
              >
                {proto.label}
              </button>
            ))}
          </div>
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">{t("ai.nameLabel")}</label>
          <input
            className="field"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder={`${t("ai.nameExample")}${protoLabel(form.protocol)}`}
          />
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">Base URL</label>
          <input
            className="field font-mono"
            value={form.base_url}
            onChange={(e) => setForm({ ...form, base_url: e.target.value })}
            placeholder={PROTOCOLS.find((p) => p.id === form.protocol)?.base}
          />
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">API Key</label>
          <input
            className="field font-mono"
            type="password"
            value={form.api_key}
            onChange={(e) => setForm({ ...form, api_key: e.target.value })}
            placeholder={form.protocol === "openai" ? "sk-…" : t("ai.keyPlaceholder")}
          />
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">{t("ai.model")}</label>
          <input
            className="field font-mono"
            value={form.model}
            onChange={(e) => setForm({ ...form, model: e.target.value })}
            placeholder={PROTOCOLS.find((p) => p.id === form.protocol)?.model}
          />
        </div>

        {error && <p className="px-2 text-xs text-red-600">{error}</p>}
        <div className="flex items-center justify-end gap-3">
          {saved && (
            <span className="flex items-center gap-1 text-xs text-neutral-500">
              <IconCheck size={14} />
              {t("common.saved")}
            </span>
          )}
          <button type="submit" className="pill pill-hover">
            {editing ? <IconCheck size={14} /> : <IconPlus size={14} />}
            {editing ? t("ai.saveChanges") : t("common.add")}
          </button>
        </div>
      </form>

      {/* AI 自主能力说明 */}
      <div className="card text-xs leading-relaxed text-neutral-500">
        <p className="mb-2 font-medium text-neutral-900 dark:text-neutral-100">{t("ai.autonomyTitle")}</p>
        <ul className="list-inside list-disc space-y-1">
          <li>{t("ai.capTools")}</li>
          <li>{t("ai.capPlan")}</li>
          <li>{t("ai.capScript")}</li>
          <li>{t("ai.capSkill")}</li>
          <li>{t("ai.capInvoke")}</li>
        </ul>
        <p className="mt-3 border-t border-neutral-200/70 pt-2 text-neutral-500 dark:border-neutral-800/70">
          {t("ai.memoryNote1")}
          <b className="text-neutral-700 dark:text-neutral-300">{t("ai.memoryNote2")}</b>
          {t("ai.memoryNote3")}
        </p>
      </div>
    </div>
  );
}
