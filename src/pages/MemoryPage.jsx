import { useEffect, useState } from "react";
import { api } from "../api.js";
import { IconPlus, IconMemory } from "../components/Icons.jsx";
import { useLang } from "../i18n.js";

// 记忆：原始记忆 + Autopilot 自动生成的总结
export default function MemoryPage({ onStats }) {
  const { t } = useLang();
  const [memories, setMemories] = useState([]);
  const [content, setContent] = useState("");
  const [error, setError] = useState("");

  const reload = () => api.listMemories().then((r) => setMemories(r.memories || []));
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

  return (
    <div className="flex h-full flex-col gap-4 overflow-y-auto">
      <div>
        <h2 className="text-lg font-semibold">{t("memory.title")}</h2>
        <p className="text-xs text-neutral-500">
          {t("memory.desc")}
        </p>
      </div>

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

      <div className="flex flex-col gap-2.5">
        {memories.map((m) => (
          <div key={m.id} className="card py-4">
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
