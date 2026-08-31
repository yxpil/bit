import { useEffect, useRef, useState } from "react";
import { api } from "../api.js";
import {
  IconSend,
  IconTrash,
  IconPlus,
  IconEdit,
  IconChat,
} from "../components/Icons.jsx";
import ToolCallCard from "../components/ToolCallCard.jsx";
import Markdown from "../components/Markdown.jsx";

// AI 对话：多会话分组 + 工具调用可视化
export default function ChatPage({ onStats }) {
  const [sessions, setSessions] = useState([]);
  const [activeId, setActiveId] = useState("");
  const [messages, setMessages] = useState([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState(null); // {id, title}
  // 流式过程中：live.text 为本轮实时增量文本；live.cards 为已完成轮次的工具卡片
  const [live, setLive] = useState(null); // null | { text, cards }
  const bottom = useRef(null);

  const loadSessions = async () => {
    const r = await api.listSessions().catch(() => null);
    if (!r) return;
    setSessions(r.sessions || []);
    setActiveId((cur) => cur || r.active || "");
    return r;
  };

  const loadMessages = async (id) => {
    const r = await api.getSession(id).catch(() => null);
    setMessages(r?.messages || []);
  };

  useEffect(() => {
    loadSessions();
  }, []);

  useEffect(() => {
    if (activeId) loadMessages(activeId);
  }, [activeId]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, busy, live]);

  const selectSession = async (id) => {
    if (id === activeId || busy) return;
    setActiveId(id);
    await api.setActiveSession(id).catch(() => {});
  };

  const newSession = async () => {
    if (busy) return;
    const r = await api.createSession("").catch(() => null);
    if (r?.id) {
      await loadSessions();
      setActiveId(r.id);
      setMessages([]);
    }
  };

  const deleteSession = async (id, e) => {
    e?.stopPropagation();
    if (busy) return;
    const r = await api.deleteSession(id).catch(() => null);
    if (r) {
      await loadSessions();
      setActiveId(r.active || "");
    }
  };

  const submitRename = async () => {
    if (!renaming) return;
    await api.renameSession(renaming.id, renaming.title).catch(() => {});
    setRenaming(null);
    loadSessions();
  };

  const send = async () => {
    const text = input.trim();
    if (!text || busy || !activeId) return;
    setInput("");
    setBusy(true);
    setMessages((m) => [...m, { role: "user", content: text }]);
    setLive({ text: "", cards: [] });
    try {
      const res = await api.chatStream(activeId, text, null, (ev) => {
        if (!ev || typeof ev !== "object") return;
        switch (ev.type) {
          case "round_start":
            // 新一轮开始：清空本轮实时文本，保留此前已完成轮次的工具卡片
            setLive((l) => ({ text: "", cards: l?.cards || [] }));
            break;
          case "delta":
            setLive((l) => ({ text: (l?.text || "") + (ev.text || ""), cards: l?.cards || [] }));
            break;
          case "tools":
            // 本轮是工具调用：丢弃流式文本，沉淀为卡片（含可选的说明文字）
            setLive((l) => ({
              text: "",
              cards: [...(l?.cards || []), { visible: ev.visible || "", calls: ev.calls || [] }],
            }));
            break;
          case "final":
            if (ev.messages) setMessages(ev.messages);
            setLive(null);
            break;
          case "error":
            setMessages((m) => [...m, { role: "assistant", content: `调用失败：${ev.error}` }]);
            setLive(null);
            break;
          default:
            break;
        }
      });
      if (res?.messages) setMessages(res.messages);
      setLive(null);
      loadSessions();
      onStats?.();
    } catch (e) {
      setMessages((m) => [...m, { role: "assistant", content: `调用失败：${e}` }]);
      setLive(null);
    } finally {
      setBusy(false);
    }
  };

  const clearCurrent = async () => {
    if (!activeId) return;
    await api.clearSession(activeId).catch(() => {});
    setMessages([]);
    loadSessions();
  };

  const visibleMessages = messages.filter((m) => m.role !== "system");

  return (
    <div className="flex h-full gap-3">
      {/* 会话侧栏 */}
      <div className="flex w-52 shrink-0 flex-col gap-2">
        <button onClick={newSession} className="pill pill-hover w-full justify-center">
          <IconPlus size={15} />
          新对话
        </button>
        <div className="card flex-1 space-y-1 overflow-y-auto p-2">
          {sessions.length === 0 && (
            <div className="px-2 py-4 text-center text-xs text-neutral-400">暂无对话</div>
          )}
          {sessions.map((s) => {
            const active = s.id === activeId;
            return (
              <div
                key={s.id}
                onClick={() => selectSession(s.id)}
                className={`group flex cursor-pointer items-center gap-2 rounded-xl px-2.5 py-2 transition-colors ${
                  active
                    ? "bg-neutral-900 text-white dark:bg-white dark:text-black"
                    : "text-neutral-600 hover:bg-neutral-900/5 dark:text-neutral-300 dark:hover:bg-white/5"
                }`}
              >
                <IconChat size={14} className="shrink-0 opacity-70" />
                {renaming?.id === s.id ? (
                  <input
                    autoFocus
                    value={renaming.title}
                    onClick={(e) => e.stopPropagation()}
                    onChange={(e) => setRenaming({ id: s.id, title: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") submitRename();
                      if (e.key === "Escape") setRenaming(null);
                    }}
                    onBlur={submitRename}
                    className="min-w-0 flex-1 rounded bg-white/20 px-1 text-[13px] outline-none dark:bg-black/20"
                  />
                ) : (
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-[13px] font-medium">{s.title || "新对话"}</div>
                    {s.preview && (
                      <div
                        className={`truncate text-[10px] ${
                          active ? "text-white/60 dark:text-black/50" : "text-neutral-400"
                        }`}
                      >
                        {s.preview}
                      </div>
                    )}
                  </div>
                )}
                {renaming?.id !== s.id && (
                  <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                    <button
                      title="重命名"
                      onClick={(e) => {
                        e.stopPropagation();
                        setRenaming({ id: s.id, title: s.title || "" });
                      }}
                      className={`rounded p-1 ${active ? "hover:bg-white/20 dark:hover:bg-black/20" : "hover:bg-neutral-900/10 dark:hover:bg-white/10"}`}
                    >
                      <IconEdit size={12} />
                    </button>
                    <button
                      title="删除"
                      onClick={(e) => deleteSession(s.id, e)}
                      className={`rounded p-1 ${active ? "hover:bg-white/20 dark:hover:bg-black/20" : "hover:bg-neutral-900/10 dark:hover:bg-white/10"}`}
                    >
                      <IconTrash size={12} />
                    </button>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      {/* 对话主区 */}
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex items-center justify-between">
          <h2 className="truncate text-lg font-semibold">
            {sessions.find((s) => s.id === activeId)?.title || "AI 对话"}
          </h2>
          <button onClick={clearCurrent} className="pill pill-outline pill-hover">
            <IconTrash size={14} />
            清空
          </button>
        </div>

        <div className="card flex-1 overflow-y-auto">
          {visibleMessages.length === 0 && !busy && (
            <div className="flex h-full items-center justify-center px-6 text-center text-sm text-neutral-400">
              发送消息开始对话。AI 会自动记忆重要内容，并可通过工具调用完成任务，调用过程会在下方以卡片展示。
            </div>
          )}
          <div className="flex flex-col gap-3">
            {visibleMessages.map((m, i) => (
              <MessageBubble key={i} message={m} />
            ))}
            {/* 流式实时区：已完成轮次的工具卡片 + 本轮增量文本 */}
            {live && (
              <div className="mr-auto flex max-w-[85%] flex-col gap-2">
                {live.cards.map((c, i) => (
                  <div key={i} className="flex flex-col gap-2">
                    {c.calls.length > 0 && (
                      <div className="flex flex-col gap-1.5">
                        {c.calls.map((call, j) => (
                          <ToolCallCard key={j} call={call} />
                        ))}
                      </div>
                    )}
                    {c.visible && c.visible.trim() && (
                      <div className="rounded-3xl rounded-bl-lg border border-neutral-200 bg-white px-4 py-2.5 dark:border-neutral-800 dark:bg-neutral-900">
                        <Markdown>{c.visible}</Markdown>
                      </div>
                    )}
                  </div>
                ))}
                {live.text ? (
                  <div className="rounded-3xl rounded-bl-lg border border-neutral-200 bg-white px-4 py-2.5 dark:border-neutral-800 dark:bg-neutral-900">
                    <Markdown>{live.text}</Markdown>
                    <span className="ml-0.5 inline-block h-3.5 w-[2px] animate-pulse bg-neutral-900 align-middle dark:bg-white" />
                  </div>
                ) : (
                  <div className="mr-auto flex items-center gap-2 rounded-3xl rounded-bl-lg border border-neutral-200 bg-white px-4 py-3 dark:border-neutral-800 dark:bg-neutral-900">
                    <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-neutral-900 [animation-delay:0ms] dark:bg-white" />
                    <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-neutral-900 [animation-delay:150ms] dark:bg-white" />
                    <span className="h-1.5 w-1.5 animate-bounce rounded-full bg-neutral-900 [animation-delay:300ms] dark:bg-white" />
                  </div>
                )}
              </div>
            )}
            <div ref={bottom} />
          </div>
        </div>

        <div className="flex gap-2">
          <input
            className="field flex-1"
            value={input}
            placeholder={activeId ? "输入消息，Enter 发送" : "请先新建或选择一个对话"}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && send()}
            disabled={busy || !activeId}
          />
          <button onClick={send} disabled={busy || !input.trim() || !activeId} className="pill pill-hover">
            <IconSend size={15} />
            发送
          </button>
        </div>
      </div>
    </div>
  );
}

// 单条消息气泡：user / assistant，assistant 可携带工具调用卡片
function MessageBubble({ message }) {
  const isUser = message.role === "user";
  const calls = message.tool_calls || [];
  const hasText = message.content && message.content.trim().length > 0;

  if (isUser) {
    return (
      <div className="ml-auto max-w-[80%] whitespace-pre-wrap rounded-3xl rounded-br-lg bg-neutral-900 px-4 py-2.5 text-sm leading-relaxed text-white dark:bg-white dark:text-black">
        {message.content}
      </div>
    );
  }

  return (
    <div className="mr-auto flex max-w-[85%] flex-col gap-2">
      {calls.length > 0 && (
        <div className="flex flex-col gap-1.5">
          {calls.map((c, i) => (
            <ToolCallCard key={i} call={c} />
          ))}
        </div>
      )}
      {hasText && (
        <div className="rounded-3xl rounded-bl-lg border border-neutral-200 bg-white px-4 py-2.5 dark:border-neutral-800 dark:bg-neutral-900">
          <Markdown>{message.content}</Markdown>
        </div>
      )}
    </div>
  );
}
