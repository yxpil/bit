import { useEffect, useRef, useState } from "react";
import { api } from "../api.js";
import {
  IconSend,
  IconTrash,
  IconPlus,
  IconEdit,
  IconChat,
  IconImage,
  IconFile,
  IconLink,
  IconX,
} from "../components/Icons.jsx";
import ToolCallCard from "../components/ToolCallCard.jsx";
import Markdown from "../components/Markdown.jsx";

// AI 对话：多会话分组 + 工具调用可视化
export default function ChatPage({ onStats }) {
  const [sessions, setSessions] = useState([]);
  const [activeId, setActiveId] = useState("");
  const [messages, setMessages] = useState([]);
  const [input, setInput] = useState("");
  const [renaming, setRenaming] = useState(null); // {id, title}
  // 多会话并发：busyMap/liveMap 以会话 id 为键，A 会话流式时仍可切到 B 会话继续聊
  const [busyMap, setBusyMap] = useState({}); // { sessionId: true }
  const [liveMap, setLiveMap] = useState({}); // { sessionId: { text, cards } }
  const bottom = useRef(null);
  const activeRef = useRef(""); // 事件回调里判断用户当前正在看哪个会话

  // 附件：图片（多模态）/ 文档（Excel/Word/CSV 转文本）/ 网址
  const [images, setImages] = useState([]); // [{ name, dataUrl }]
  const [docs, setDocs] = useState([]); // [{ name, text }]
  const [attaching, setAttaching] = useState(false);
  const [attachErr, setAttachErr] = useState("");
  const [urlOpen, setUrlOpen] = useState(false);
  const [urlText, setUrlText] = useState("");
  const [urlParse, setUrlParse] = useState(true); // true=解析网址抓正文；false=仅作为文本插入
  const imgInput = useRef(null);
  const docInput = useRef(null);

  // 派生：当前会话状态与全局运行数
  const busy = !!busyMap[activeId]; // 当前会话是否执行中
  const live = liveMap[activeId] || null;
  const runningCount = Object.keys(busyMap).length;

  // 上下文用量估算（粗略：中英混合约 2 字符/token），仅用于预警，不阻断对话
  const CONTEXT_LIMIT = 128 * 1024; // 128K tokens
  const [compressing, setCompressing] = useState(false);
  const estimateTokens = (text) => Math.ceil((text || "").length / 2);
  const contextTokens = (() => {
    let n = 4; // system prompt 基数
    for (const m of messages) n += estimateTokens(m.content) + 4;
    if (live) n += estimateTokens(live.text);
    return n;
  })();
  const ctxPct = contextTokens / CONTEXT_LIMIT;
  const fmtK = (n) => (n >= 1024 ? `${(n / 1024).toFixed(1)}K` : String(n));

  useEffect(() => {
    activeRef.current = activeId;
  }, [activeId]);

  // 读文件为 dataURL
  const readAsDataURL = (file) =>
    new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => resolve(r.result);
      r.onerror = reject;
      r.readAsDataURL(file);
    });

  const onPickImages = async (e) => {
    const files = Array.from(e.target.files || []);
    e.target.value = ""; // 允许重复选同一文件
    setAttachErr("");
    for (const f of files) {
      try {
        const dataUrl = await readAsDataURL(f);
        setImages((arr) => [...arr, { name: f.name, dataUrl }]);
      } catch {
        setAttachErr(`图片读取失败：${f.name}`);
      }
    }
  };

  const onPickDocs = async (e) => {
    const files = Array.from(e.target.files || []);
    e.target.value = "";
    setAttachErr("");
    setAttaching(true);
    try {
      for (const f of files) {
        const dataUrl = await readAsDataURL(f);
        try {
          const r = await api.extractFile(f.name, dataUrl);
          if (r?.text) setDocs((arr) => [...arr, { name: f.name, text: r.text }]);
        } catch (err) {
          setAttachErr(`解析失败：${f.name} — ${err}`);
        }
      }
    } finally {
      setAttaching(false);
    }
  };

  const addUrl = async () => {
    const u = urlText.trim();
    if (!u) return;
    setAttachErr("");
    if (!urlParse) {
      // 仅作为文本插入
      setDocs((arr) => [...arr, { name: u, text: u }]);
      setUrlText("");
      setUrlOpen(false);
      return;
    }
    setAttaching(true);
    try {
      const r = await api.fetchWebpage(u);
      const title = r?.title ? `# ${r.title}\n\n` : "";
      setDocs((arr) => [...arr, { name: r?.title || u, text: `来源：${u}\n\n${title}${r?.text || ""}` }]);
      setUrlText("");
      setUrlOpen(false);
    } catch (err) {
      setAttachErr(`网页抓取失败：${err}`);
    } finally {
      setAttaching(false);
    }
  };

  const removeImage = (i) => setImages((arr) => arr.filter((_, j) => j !== i));
  const removeDoc = (i) => setDocs((arr) => arr.filter((_, j) => j !== i));
  const clearAttachments = () => {
    setImages([]);
    setDocs([]);
    setAttachErr("");
  };
  const hasAttachments = images.length > 0 || docs.length > 0;

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
    // 多会话并发：切换不受执行状态限制
    if (id === activeId) return;
    setActiveId(id);
    await api.setActiveSession(id).catch(() => {});
  };

  const newSession = async () => {
    const r = await api.createSession("").catch(() => null);
    if (r?.id) {
      await loadSessions();
      setActiveId(r.id);
      setMessages([]);
    }
  };

  const deleteSession = async (id, e) => {
    e?.stopPropagation();
    if (busyMap[id]) return; // 执行中的会话不能删除，避免流式结果写入已删会话
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
    // 允许只带附件（图片/文档）而无文字时也可发送
    if ((!text && !hasAttachments) || busy || !activeId) return;
    const sid = activeId; // 锁定目标会话：之后用户切换页面不影响本次执行

    // 文档正文（Excel/Word/CSV/网页）拼接到消息前作为上下文
    const docBlocks = docs
      .map((d) => `【附件：${d.name}】\n${d.text}`)
      .join("\n\n---\n\n");
    const composed = docBlocks
      ? `${docBlocks}${text ? `\n\n---\n\n${text}` : ""}`
      : text || "（见附件图片）";

    // 图片以 dataURL base64 传给多模态模型
    const imgData = images.map((im) => im.dataUrl);

    // 气泡里展示用户实际输入（不含附件正文），并标注附件数量
    const bubble =
      (text || (hasAttachments ? "" : "")) +
      (hasAttachments
        ? `${text ? "\n\n" : ""}📎 ${images.length ? `图片 ×${images.length}` : ""}${
            images.length && docs.length ? "，" : ""
          }${docs.length ? `文档/链接 ×${docs.length}` : ""}`
        : "");

    setInput("");
    setBusyMap((m) => ({ ...m, [sid]: true }));
    setLiveMap((m) => ({ ...m, [sid]: { text: "", cards: [] } }));
    if (activeRef.current === sid) {
      setMessages((msgs) => [...msgs, { role: "user", content: bubble || text }]);
    }
    // 发送后清空附件
    setImages([]);
    setDocs([]);
    setAttachErr("");
    // 结束时移除该会话的运行/流式状态
    const endLive = () =>
      setLiveMap((m) => {
        const n = { ...m };
        delete n[sid];
        return n;
      });
    try {
      const res = await api.chatStream(sid, composed, null, (ev) => {
        if (!ev || typeof ev !== "object") return;
        switch (ev.type) {
          case "round_start":
            // 新一轮开始：清空本轮实时文本，保留此前已完成轮次的工具卡片
            setLiveMap((m) => ({ ...m, [sid]: { text: "", cards: m[sid]?.cards || [] } }));
            break;
          case "delta":
            setLiveMap((m) => ({
              ...m,
              [sid]: { text: (m[sid]?.text || "") + (ev.text || ""), cards: m[sid]?.cards || [] },
            }));
            break;
          case "tools":
            // 本轮是工具调用：丢弃流式文本，沉淀为卡片（含可选的说明文字）
            setLiveMap((m) => ({
              ...m,
              [sid]: {
                text: "",
                cards: [...(m[sid]?.cards || []), { visible: ev.visible || "", calls: ev.calls || [] }],
              },
            }));
            break;
          case "final":
            // 仅当用户还停留在这个会话时刷新消息列表；后台会话结果已落库
            if (activeRef.current === sid && ev.messages) setMessages(ev.messages);
            endLive();
            break;
          case "error":
            if (activeRef.current === sid) {
              setMessages((msgs) => [...msgs, { role: "assistant", content: `调用失败：${ev.error}` }]);
            }
            endLive();
            break;
          default:
            break;
        }
      }, imgData);
      if (activeRef.current === sid && res?.messages) setMessages(res.messages);
      endLive();
      loadSessions();
      onStats?.();
    } catch (e) {
      if (activeRef.current === sid) {
        setMessages((msgs) => [...msgs, { role: "assistant", content: `调用失败：${e}` }]);
      }
      endLive();
    } finally {
      setBusyMap((m) => {
        const n = { ...m };
        delete n[sid];
        return n;
      });
    }
  };

  const clearCurrent = async () => {
    if (!activeId || busyMap[activeId]) return;
    await api.clearSession(activeId).catch(() => {});
    setMessages([]);
    loadSessions();
  };

  // 手动压缩：AI 把全部历史总结为摘要，释放上下文空间（不阻断会话本身）
  const compress = async () => {
    if (!activeId || compressing || busyMap[activeId]) return;
    setCompressing(true);
    try {
      const r = await api.compressSession(activeId);
      if (activeRef.current === activeId && r?.messages) setMessages(r.messages);
      loadSessions();
      onStats?.();
    } catch (e) {
      setAttachErr(`压缩失败：${e}`);
    } finally {
      setCompressing(false);
    }
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
                <IconChat size={14} className={`shrink-0 opacity-70 ${busyMap[s.id] ? "hidden" : ""}`} />
                {busyMap[s.id] && (
                  <span
                    title="执行中"
                    className="h-2.5 w-2.5 shrink-0 animate-pulse rounded-full bg-emerald-500 ring-2 ring-emerald-500/30"
                  />
                )}
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
          <div className="flex min-w-0 items-center gap-2">
            <h2 className="truncate text-lg font-semibold">
              {sessions.find((s) => s.id === activeId)?.title || "AI 对话"}
            </h2>
            {runningCount > 0 && (
              <span
                title="可切到其他会话继续对话，任务在后台不中断"
                className="flex shrink-0 items-center gap-1.5 rounded-full bg-emerald-500/10 px-2.5 py-1 text-[11px] font-medium text-emerald-600 dark:text-emerald-400"
              >
                <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-emerald-500" />
                {runningCount} 个对话执行中
              </span>
            )}
          </div>
          <button onClick={clearCurrent} disabled={busy} className="pill pill-outline pill-hover disabled:opacity-40">
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

        <div className="flex flex-col gap-2">
          {/* 上下文用量条：常显灰色 → 70% 黄色预警 → 128K 红色并出现压缩按钮（不阻断对话） */}
          {visibleMessages.length > 0 && (
            <div
              className={`flex items-center gap-2 self-start rounded-full px-3 py-1 text-[11px] ${
                ctxPct >= 1
                  ? "bg-red-500/10 text-red-600 dark:text-red-400"
                  : ctxPct >= 0.7
                    ? "bg-amber-500/10 text-amber-600 dark:text-amber-400"
                    : "text-neutral-400"
              }`}
            >
              <span>
                {ctxPct >= 1
                  ? "上下文较长，可能影响回复质量"
                  : ctxPct >= 0.7
                    ? "上下文占用较高"
                    : "上下文"}{" "}
                约 {fmtK(contextTokens)} / 128K tokens
              </span>
              {(ctxPct >= 1 || compressing) && (
                <button
                  onClick={compress}
                  disabled={compressing || busy}
                  title="AI 总结全部历史为摘要，释放上下文空间"
                  className="rounded-full bg-red-500/15 px-2.5 py-0.5 font-medium text-red-600 transition-colors hover:bg-red-500/25 disabled:opacity-40 dark:text-red-400"
                >
                  {compressing ? "压缩中…" : "压缩对话"}
                </button>
              )}
            </div>
          )}

          {/* 隐藏文件输入 */}
          <input
            ref={imgInput}
            type="file"
            accept="image/*"
            multiple
            className="hidden"
            onChange={onPickImages}
          />
          <input
            ref={docInput}
            type="file"
            accept=".xlsx,.xls,.csv,.docx,.txt,.md"
            multiple
            className="hidden"
            onChange={onPickDocs}
          />

          {/* 附件预览区 */}
          {(hasAttachments || attaching || attachErr) && (
            <div className="card flex flex-wrap items-center gap-2 p-2.5">
              {images.map((img, i) => (
                <div key={`img-${i}`} className="group relative">
                  <img
                    src={img.dataUrl}
                    alt={img.name}
                    title={img.name}
                    className="h-14 w-14 rounded-lg object-cover ring-1 ring-neutral-200 dark:ring-neutral-700"
                  />
                  <button
                    onClick={() => removeImage(i)}
                    title="移除"
                    className="absolute -right-1.5 -top-1.5 rounded-full bg-neutral-900 p-0.5 text-white opacity-0 shadow transition-opacity group-hover:opacity-100 dark:bg-white dark:text-black"
                  >
                    <IconX size={11} />
                  </button>
                </div>
              ))}
              {docs.map((doc, i) => (
                <div
                  key={`doc-${i}`}
                  className="flex items-center gap-1.5 rounded-full bg-neutral-900/5 px-3 py-1.5 text-xs dark:bg-white/10"
                >
                  {doc.name.startsWith("http") ? <IconLink size={13} /> : <IconFile size={13} />}
                  <span className="max-w-[160px] truncate" title={doc.name}>
                    {doc.name}
                  </span>
                  <button onClick={() => removeDoc(i)} title="移除" className="opacity-60 hover:opacity-100">
                    <IconX size={12} />
                  </button>
                </div>
              ))}
              {attaching && (
                <span className="text-xs text-neutral-400">解析中…</span>
              )}
              {attachErr && (
                <span className="text-xs text-red-500" title={attachErr}>
                  {attachErr}
                </span>
              )}
              {hasAttachments && (
                <button
                  onClick={clearAttachments}
                  className="ml-auto text-xs text-neutral-400 hover:text-red-500"
                >
                  清除全部
                </button>
              )}
            </div>
          )}

          {/* URL 输入面板 */}
          {urlOpen && (
            <div className="card flex flex-wrap items-center gap-2 p-2.5">
              <input
                autoFocus
                type="text"
                value={urlText}
                onChange={(e) => setUrlText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") addUrl();
                  if (e.key === "Escape") setUrlOpen(false);
                }}
                placeholder="粘贴网址，如 https://…"
                className="field min-w-0 flex-1"
              />
              <label className="flex shrink-0 items-center gap-1.5 text-xs text-neutral-600 dark:text-neutral-300">
                <input
                  type="checkbox"
                  checked={urlParse}
                  onChange={(e) => setUrlParse(e.target.checked)}
                />
                解析正文
              </label>
              <button
                onClick={addUrl}
                disabled={!urlText.trim() || attaching}
                className="pill pill-hover shrink-0"
              >
                添加
              </button>
              <button
                onClick={() => setUrlOpen(false)}
                title="关闭"
                className="shrink-0 rounded-full p-1.5 hover:bg-neutral-900/5 dark:hover:bg-white/10"
              >
                <IconX size={14} />
              </button>
            </div>
          )}

          <div className="flex items-center gap-2">
            {/* 附件工具栏 */}
            <button
              onClick={() => imgInput.current?.click()}
              disabled={busy || !activeId}
              title="上传图片（多模态）"
              className="shrink-0 rounded-full p-2 text-neutral-500 transition-colors hover:bg-neutral-900/5 hover:text-neutral-900 disabled:opacity-40 dark:hover:bg-white/10 dark:hover:text-white"
            >
              <IconImage size={18} />
            </button>
            <button
              onClick={() => docInput.current?.click()}
              disabled={busy || !activeId}
              title="上传文档（Excel/Word/CSV → 文本）"
              className="shrink-0 rounded-full p-2 text-neutral-500 transition-colors hover:bg-neutral-900/5 hover:text-neutral-900 disabled:opacity-40 dark:hover:bg-white/10 dark:hover:text-white"
            >
              <IconFile size={18} />
            </button>
            <button
              onClick={() => setUrlOpen((v) => !v)}
              disabled={busy || !activeId}
              title="添加网址（可解析正文）"
              className={`shrink-0 rounded-full p-2 transition-colors disabled:opacity-40 ${
                urlOpen
                  ? "bg-neutral-900 text-white dark:bg-white dark:text-black"
                  : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:hover:bg-white/10 dark:hover:text-white"
              }`}
            >
              <IconLink size={18} />
            </button>

            <input
              className="field flex-1"
              value={input}
              placeholder={
                !activeId
                  ? "请先新建或选择一个对话"
                  : busy
                    ? "本对话执行中，可切换其他对话继续…"
                    : "输入消息，Enter 发送"
              }
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && send()}
              disabled={busy || !activeId}
            />
            <button
              onClick={send}
              disabled={busy || (!input.trim() && !hasAttachments) || !activeId}
              className="pill pill-hover shrink-0"
            >
              <IconSend size={15} />
              发送
            </button>
          </div>
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
