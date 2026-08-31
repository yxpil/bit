import { useEffect, useState } from "react";
import { api } from "../api.js";
import { IconPlus, IconSkill } from "../components/Icons.jsx";

// 技能（SKILL）：Autopilot 从总结记忆中自动提炼，也可手动沉淀
export default function SkillsPage({ onStats }) {
  const [skills, setSkills] = useState([]);
  const [name, setName] = useState("");
  const [summary, setSummary] = useState("");

  const reload = () => api.listSkills().then((r) => setSkills(r.skills || []));
  useEffect(() => {
    reload();
  }, []);

  const add = async (e) => {
    e.preventDefault();
    try {
      await api.addSkill(name, summary);
      setName("");
      setSummary("");
      await reload();
      onStats?.();
    } catch {
      // 表单已做必填校验
    }
  };

  return (
    <div className="flex h-full flex-col gap-4 overflow-y-auto">
      <div>
        <h2 className="text-lg font-semibold">技能 SKILL</h2>
        <p className="text-xs text-neutral-500">
          AI 在对话中自己调用 skill（save/search）沉淀与检索 · 也可在此手动添加
        </p>
      </div>

      <form onSubmit={add} className="card flex flex-col gap-3">
        <input className="field" placeholder="技能名称" value={name}
          onChange={(e) => setName(e.target.value)} required />
        <input className="field" placeholder="一句话说明" value={summary}
          onChange={(e) => setSummary(e.target.value)} required />
        <div className="flex justify-end">
          <button className="pill pill-hover" disabled={!name.trim() || !summary.trim()}>
            <IconPlus size={15} />
            添加
          </button>
        </div>
      </form>

      <div className="flex flex-col gap-2.5">
        {skills.map((s) => (
          <div key={s.id} className="card flex items-start gap-3 py-4">
            <IconSkill size={18} className="mt-0.5 shrink-0" />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="font-semibold">{s.name}</span>
                <span className="chip">{s.source}</span>
              </div>
              <p className="mt-1 text-sm text-neutral-600">{s.summary}</p>
              <p className="mt-1 text-xs text-neutral-400">{s.ts}</p>
            </div>
          </div>
        ))}
        {skills.length === 0 && (
          <div className="card flex items-center justify-center gap-2 py-10 text-sm text-neutral-400">
            <IconSkill size={16} />
            暂无技能，AI 会在对话中自己沉淀
          </div>
        )}
      </div>
    </div>
  );
}
