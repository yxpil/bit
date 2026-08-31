import { useEffect, useState } from "react";
import { api } from "../api.js";
import PillSwitch from "../components/PillSwitch.jsx";
import { IconPlus, IconTrash, IconSettings, IconCheck, IconX } from "../components/Icons.jsx";

// 支持的协议：原生对接各家 API
const PROTOCOLS = [
  { id: "openai", label: "OpenAI", base: "https://api.openai.com/v1", model: "gpt-4o-mini" },
  { id: "gemini", label: "Gemini", base: "https://generativelanguage.googleapis.com", model: "gemini-1.5-flash" },
  { id: "claude", label: "Claude", base: "https://api.anthropic.com", model: "claude-3-5-sonnet-latest" },
];
const protoLabel = (id) => PROTOCOLS.find((p) => p.id === id)?.label || id;

const EMPTY = { id: null, name: "", protocol: "openai", base_url: "", api_key: "", model: "" };

// AI 设置：可增删的多家提供方，用播放/暂停切换，每次仅一个激活
export default function AiSettingsPage({ onStats, stats }) {
  const [providers, setProviders] = useState([]);
  const [form, setForm] = useState(EMPTY); // 新增 / 编辑用同一张表单
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);

  const load = () =>
    api.listProviders().then((r) => setProviders(r.providers || [])).catch(() => {});
  useEffect(() => {
    load();
  }, []);

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
        <h2 className="text-lg font-semibold">AI 设置</h2>
        <p className="text-xs text-neutral-500">
          可同时接入多家（OpenAI / Gemini / Claude），用播放按钮激活其一 · 每次仅一个生效
        </p>
      </div>

      {/* 运行总览 */}
      {stats && (
        <div className="card">
          <p className="mb-3 text-xs font-medium text-neutral-900 dark:text-neutral-100">运行总览</p>
          <div className="grid grid-cols-3 gap-3 sm:grid-cols-6">
            {[
              ["工具", stats.tool_count],
              ["记忆", stats.memory_count],
              ["技能", stats.skill_count],
              ["目标", stats.goal_count],
              ["待办", stats.todo_count],
              ["审计", stats.audit_count],
            ].map(([label, n]) => (
              <div key={label} className="text-center">
                <div className="text-xl font-bold tabular-nums">{n ?? 0}</div>
                <div className="text-[11px] text-neutral-500">{label}</div>
              </div>
            ))}
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-2 border-t border-neutral-200/70 pt-3 dark:border-neutral-800/70">
            <span className="chip">{stats.ai_configured ? "AI 已激活" : "无激活的 AI"}</span>
            <span className={`chip ${stats.remote?.enabled ? "border-emerald-500/50 text-emerald-600 dark:text-emerald-400" : ""}`}>
              {stats.remote?.enabled ? `远程 ${stats.remote.addr}` : "远程关闭"}
            </span>
          </div>
        </div>
      )}

      {/* 提供方列表 */}
      <div className="flex flex-col gap-2">
        {providers.length === 0 && (
          <div className="card flex items-center justify-center gap-2 py-8 text-sm text-neutral-400">
            <IconSettings size={16} />
            还没有 AI 提供方，在下方添加一个
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
              title={p.active ? "暂停（停用此提供方）" : "播放（激活此提供方，其余自动暂停）"}
            />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="truncate text-sm font-semibold">{p.name}</span>
                <span className="chip shrink-0">{protoLabel(p.protocol)}</span>
                {p.active && (
                  <span className="chip shrink-0 border-emerald-500/50 text-emerald-600 dark:text-emerald-400">
                    使用中
                  </span>
                )}
                {!p.api_key && (
                  <span className="chip shrink-0 border-amber-500/50 text-amber-600 dark:text-amber-400">
                    缺 Key
                  </span>
                )}
              </div>
              <p className="mt-0.5 truncate font-mono text-[11px] text-neutral-500">
                {p.model} · {p.base_url}
              </p>
            </div>
            <button
              onClick={() => startEdit(p)}
              title="编辑"
              className="rounded-full p-2 text-neutral-500 transition-colors hover:bg-neutral-100 dark:hover:bg-neutral-800"
            >
              <IconSettings size={15} />
            </button>
            <button
              onClick={() => remove(p.id)}
              title="删除"
              className="rounded-full p-2 text-neutral-400 transition-colors hover:bg-red-50 hover:text-red-500 dark:hover:bg-red-950/40"
            >
              <IconTrash size={15} />
            </button>
          </div>
        ))}
      </div>

      {/* 新增 / 编辑表单 */}
      <form onSubmit={submit} className="card flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <p className="text-sm font-medium">{editing ? "编辑提供方" : "添加提供方"}</p>
          {editing && (
            <button
              type="button"
              onClick={() => setForm(EMPTY)}
              className="flex items-center gap-1 text-xs text-neutral-500 hover:text-neutral-900 dark:hover:text-white"
            >
              <IconX size={13} />
              取消编辑
            </button>
          )}
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">协议</label>
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
          <label className="mb-1 block px-2 text-xs text-neutral-500">名称（可选）</label>
          <input
            className="field"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder={`如：我的 ${protoLabel(form.protocol)}`}
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
            placeholder={form.protocol === "openai" ? "sk-…" : "填写该家的密钥"}
          />
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">模型</label>
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
              已保存
            </span>
          )}
          <button type="submit" className="pill pill-hover">
            {editing ? <IconCheck size={14} /> : <IconPlus size={14} />}
            {editing ? "保存修改" : "添加"}
          </button>
        </div>
      </form>

      {/* AI 自主能力说明 */}
      <div className="card text-xs leading-relaxed text-neutral-500">
        <p className="mb-2 font-medium text-neutral-900 dark:text-neutral-100">AI 自主能力（对话中自动可用）</p>
        <ul className="list-inside list-disc space-y-1">
          <li>shell / write_file / edit — 执行命令、写文件、增量补丁改文件</li>
          <li>plan — 自己制定计划，登记目标与分步待办</li>
          <li>add_tool / run_script — 用本机解释器写一段代码，即刻执行或沉淀为常驻工具</li>
          <li>skill（save / search） — 自己写技能、搜索已有技能；add_memory 沉淀记忆</li>
          <li>调用任意已注册工具（内置 / 远程 / AI 自写脚本）</li>
        </ul>
        <p className="mt-3 border-t border-neutral-200/70 pt-2 text-neutral-500 dark:border-neutral-800/70">
          记忆与技能的<b className="text-neutral-700 dark:text-neutral-300">总结/沉淀由 AI 在对话中自己调用工具完成</b>（add_memory、skill），无需手动开关。
        </p>
      </div>
    </div>
  );
}
