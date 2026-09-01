import { useEffect, useState } from "react";
import { api } from "../api.js";
import { IconPlus, IconMemory, IconTrash } from "../components/Icons.jsx";
import { useLang } from "../i18n.js";

// 记忆页：计划目标 / 待办事项管理（可删除）+ 原始记忆（单条删除与多选批量删除）
export default function MemoryPage({ onStats }) {
  const { t } = useLang();
  const [memories, setMemories] = useState([]);
  const [goals, setGoals] = useState([]);
  const [todos, setTodos] = useState([]);
  const [sessionNames, setSessionNames] = useState({});
  const [content, setContent] = useState("");
  const [error, setError] = useState("");
  const [selected, setSelected] = useState(() => new Set());
  const [busy, setBusy] = useState(false);

  const reload = () => {
    api.listMemories().then((r) => {
      setMemories(r.memories || []);
      // 列表刷新后清掉已不存在的选中项
      setSelected((prev) => {
        const alive = new Set((r.memories || []).map((m) => m.id));
        const next = new Set([...prev].filter((id) => alive.has(id)));
        return next.size === prev.size ? prev : next;
      });
    });
    api.listGoals().then((r) => setGoals(r.goals || []));
    api.listTodos().then((r) => setTodos(r.todos || []));
    // 会话标题映射：用于标注目标/待办归属的会话
    api.listSessions().then((r) => {
      const map = {};
      for (const s of r.sessions || []) map[s.id] = s.title;
      setSessionNames(map);
    });
  };
  useEffect(() => {
    reload();
  }, []);

  const add = async (e) => {
    e.preventDefault();
    setError("");
    try {
      await api.addMemory(content);
      setContent("");
      await reload();
      onStats?.();
    } catch (err) {
      setError(String(err));
    }
  };

  const toggle = (id) =>
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  const allChecked = memories.length > 0 && selected.size === memories.length;
  const toggleAll = () =>
    setSelected(allChecked ? new Set() : new Set(memories.map((m) => m.id)));

  const removeOne = async (id) => {
    if (!window.confirm(t("common.confirmDelete"))) return;
    setBusy(true);
    setError("");
    try {
      await api.deleteMemories([id]);
      await reload();
      onStats?.();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const removeSelected = async () => {
    if (!selected.size || !window.confirm(t("common.confirmDelete"))) return;
    setBusy(true);
    setError("");
    try {
      await api.deleteMemories([...selected]);
      setSelected(new Set());
      await reload();
      onStats?.();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const removeGoal = async (id) => {
    if (!window.confirm(t("common.confirmDelete"))) return;
    setBusy(true);
    setError("");
    try {
      await api.removeGoal(id);
      await reload();
      onStats?.();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const removeTodo = async (id) => {
    if (!window.confirm(t("common.confirmDelete"))) return;
    setBusy(true);
    setError("");
    try {
      await api.removeTodo(id);
      await reload();
      onStats?.();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const goalStatusLabel = {
    active: t("memory.goalActive"),
    achieved: t("memory.goalAchieved"),
    abandoned: t("memory.goalAbandoned"),
  };
  const todoStatusLabel = {
    pending: t("memory.todoPending"),
    in_progress: t("memory.todoInProgress"),
    completed: t("memory.todoCompleted"),
  };

  return (
    <div className="flex h-full flex-col gap-4 overflow-y-auto">
      <div>
        <h2 className="text-lg font-semibold">{t("memory.title")}</h2>
        <p className="text-xs text-neutral-500">
          {t("memory.desc")}
        </p>
      </div>

      {/* 计划目标 */}
      <div>
        <h3 className="px-1 text-sm font-semibold">{t("memory.goalsTitle")}</h3>
        <p className="mb-2 px-1 text-xs text-neutral-500">{t("memory.goalsDesc")}</p>
        <div className="flex flex-col gap-2.5">
          {goals.map((g) => (
            <div key={g.id} className="card group flex gap-3 py-4">
              <div className="min-w-0 flex-1">
                <div className="mb-1.5 flex flex-wrap items-center gap-2">
                  <span
                    className={`chip ${
                      g.status === "achieved"
                        ? "border-neutral-900 bg-neutral-900 text-white"
                        : g.status === "abandoned"
                          ? "border-red-200 text-red-500"
                          : ""
                    }`}
                  >
                    {goalStatusLabel[g.status] || g.status}
                  </span>
                  {g.session_id && (
                    <span className="chip max-w-40 truncate" title={sessionNames[g.session_id] || g.session_id}>
                      {sessionNames[g.session_id] || g.session_id}
                    </span>
                  )}
                  <span className="text-xs text-neutral-400">{g.updated_ts || g.ts}</span>
                </div>
                <p className="text-sm font-medium">{g.title}</p>
                {g.detail && <p className="mt-1 text-xs leading-relaxed text-neutral-500">{g.detail}</p>}
              </div>
              <button
                title={t("common.delete")}
                className="mt-0.5 h-7 w-7 shrink-0 rounded-full text-neutral-300 transition-colors hover:bg-red-50 hover:text-red-600"
                disabled={busy}
                onClick={() => removeGoal(g.id)}
              >
                <IconTrash size={15} className="mx-auto" />
              </button>
            </div>
          ))}
          {goals.length === 0 && (
            <div className="card flex items-center justify-center py-6 text-sm text-neutral-400">
              {t("memory.goalEmpty")}
            </div>
          )}
        </div>
      </div>

      {/* 待办事项 */}
      {todos.length > 0 && (
        <div>
          <h3 className="mb-2 px-1 text-sm font-semibold">{t("memory.todosTitle")}</h3>
          <div className="flex flex-col gap-2.5">
            {todos.map((td) => (
              <div key={td.id} className="card group flex gap-3 py-3.5">
                <div className="min-w-0 flex-1">
                  <div className="mb-1 flex flex-wrap items-center gap-2">
                    <span
                      className={`chip ${
                        td.status === "completed"
                          ? "border-neutral-900 bg-neutral-900 text-white"
                          : td.status === "in_progress"
                            ? ""
                            : "border-neutral-200 text-neutral-500"
                      }`}
                    >
                      {todoStatusLabel[td.status] || td.status}
                    </span>
                    {td.session_id && (
                      <span className="chip max-w-40 truncate" title={sessionNames[td.session_id] || td.session_id}>
                        {sessionNames[td.session_id] || td.session_id}
                      </span>
                    )}
                    <span className="text-xs text-neutral-400">{td.ts}</span>
                  </div>
                  <p className="text-sm">{td.content}</p>
                </div>
                <button
                  title={t("common.delete")}
                  className="mt-0.5 h-7 w-7 shrink-0 rounded-full text-neutral-300 transition-colors hover:bg-red-50 hover:text-red-600"
                  disabled={busy}
                  onClick={() => removeTodo(td.id)}
                >
                  <IconTrash size={15} className="mx-auto" />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="my-1 border-t border-neutral-100" />

      <form onSubmit={add} className="card flex gap-2">
        <input
          className="field flex-1"
          placeholder={t("memory.placeholder")}
          value={content}
          onChange={(e) => setContent(e.target.value)}
        />
        <button className="pill pill-hover" disabled={!content.trim()}>
          <IconPlus size={15} />
          {t("common.add")}
        </button>
      </form>
      {error && <p className="px-2 text-xs text-red-600">{error}</p>}

      {memories.length > 0 && (
        <div className="flex items-center gap-3 px-1">
          <label className="flex cursor-pointer items-center gap-2 text-sm text-neutral-600 select-none">
            <input
              type="checkbox"
              className="size-4 accent-neutral-800"
              checked={allChecked}
              onChange={toggleAll}
            />
            {t("common.selectAll")}
          </label>
          {selected.size > 0 && (
            <button
              className="pill pill-hover ml-auto border-red-200 text-red-600 disabled:opacity-50"
              disabled={busy}
              onClick={removeSelected}
            >
              <IconTrash size={14} />
              {t("common.deleteSelected")} ({selected.size})
            </button>
          )}
        </div>
      )}

      <div className="flex flex-col gap-2.5">
        {memories.map((m) => (
          <div key={m.id} className="card group flex gap-3 py-4">
            <input
              type="checkbox"
              className="mt-1 size-4 shrink-0 accent-neutral-800"
              checked={selected.has(m.id)}
              onChange={() => toggle(m.id)}
            />
            <div className="min-w-0 flex-1">
              <div className="mb-1.5 flex items-center gap-2">
                <span className={`chip ${m.kind === "summary" ? "border-neutral-900 bg-neutral-900 text-white" : ""}`}>
                  {m.kind === "summary" ? t("memory.chipSummary") : t("memory.chipRaw")}
                </span>
                <span className="text-xs text-neutral-400">
                  {m.ts} · {m.source}
                </span>
              </div>
              <p className="text-sm leading-relaxed">{m.content}</p>
            </div>
            <button
              title={t("common.delete")}
              className="mt-0.5 h-7 w-7 shrink-0 rounded-full text-neutral-300 transition-colors hover:bg-red-50 hover:text-red-600"
              disabled={busy}
              onClick={() => removeOne(m.id)}
            >
              <IconTrash size={15} className="mx-auto" />
            </button>
          </div>
        ))}
        {memories.length === 0 && (
          <div className="card flex items-center justify-center gap-2 py-10 text-sm text-neutral-400">
            <IconMemory size={16} />
            {t("memory.empty")}
          </div>
        )}
      </div>
    </div>
  );
}
