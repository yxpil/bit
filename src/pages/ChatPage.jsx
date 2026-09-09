import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api.js";
import { useLang, t } from "../i18n.js";
import {
  IconSend,
  IconTrash,
  IconPlus,
  IconEdit,
  IconChat,
  IconFile,
  IconLink,
  IconX,
  IconStop,
  IconEye,
  IconShield,
  IconQueue,
  IconCopy,
  IconCheck,
  IconTarget,
  IconStar,
  IconStarSolid,
  IconTerminal,
} from "../components/Icons.jsx";
import PillSwitch from "../components/PillSwitch.jsx";
import ToolCallCard from "../components/ToolCallCard.jsx";
import PendingToolCard from "../components/PendingToolCard.jsx";
import FileCard from "../components/FileCard.jsx";
import Markdown from "../components/Markdown.jsx";
import { useShellJobs } from "../hooks/useShellJobs.js";

// 对话彩色标签调色板（十六进制色值）
const TAG_COLORS = [
  "#ef4444", "#f97316", "#f59e0b", "#eab308", "#84cc16", "#22c55e",
  "#10b981", "#14b8a6", "#06b6d4", "#0ea5e9", "#3b82f6", "#6366f1",
  "#8b5cf6", "#d946ef", "#ec4899", "#78716c",
];

// AI 对话：多会话分组 + 工具调用可视化
export default function ChatPage({ onStats, visible }) {
  const { t } = useLang();
  const [sessions, setSessions] = useState([]);
  const [activeId, setActiveId] = useState("");
  const [messages, setMessages] = useState([]);
  const [input, setInput] = useState("");
  const [renaming, setRenaming] = useState(null); // {id, title}
  // 工具栏：后台任务 popover
  const [bgJobsOpen, setBgJobsOpen] = useState(false);
  // 后台长命令状态（集中 hook：拉取 + 事件订阅 + 停止）
  const shellJobs = useShellJobs();
  // 会话收藏 / 多选批量删除 / 彩色标签（右滑调出调色板）
  const [multiMode, setMultiMode] = useState(false); // 是否处于多选删除模式
  const [selSet, setSelSet] = useState(() => new Set()); // 多选模式下勾选的会话 id
  const [colorFor, setColorFor] = useState(null); // 正在打彩色标签的会话 id（null=无）
  const [batchHint, setBatchHint] = useState(""); // 多选/删除的临时反馈文字
  const dragRef = useRef(null); // {x, y, id, armed} 右滑手势起点
  const swipedRef = useRef(null); // 刚完成右滑的会话 id（抑制随后的 click）
  const hintTimer = useRef(null);
  // 多会话并发：busyMap/liveMap 以会话 id 为键，A 会话流式时仍可切到 B 会话继续聊
  const [busyMap, setBusyMap] = useState({}); // { sessionId: true }
  const [liveMap, setLiveMap] = useState({}); // { sessionId: { text, cards } }
  // 缓存命中率统计（会话累计，由后端 usage/chat-usage 事件推送）
  const [usageMap, setUsageMap] = useState({}); // { sessionId: { requests, prompt_tokens, cache_read_tokens, completion_tokens, hit_rate } }
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
  const inputRef = useRef(null); // 输入框自动增高用
  const [attachMenu, setAttachMenu] = useState(false); // 附件 + 菜单（图片/文档/网址）

  // 输入框自动增高：随内容增长，超过 160px 后内部滚动
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = Math.min(el.scrollHeight, 160) + "px";
  }, [input]);

  // 等待发送队列：会话执行中提交的新消息先排队，任务结束后自动按顺序发送
  const [queues, setQueues] = useState({}); // {sid: [{bubble, composed, imgData}]}
  const [dragIdx, setDragIdx] = useState(null); // 正在拖拽的队列项索引
  const [queuePaused, setQueuePaused] = useState({}); // 中断后暂停自动续发
  const pausedRef = useRef({});
  const queuesRef = useRef({}); // 队列同步 ref：pumpQueue 读取用，避免在 setState updater 里产生副作用
  const runningRef = useRef(new Set()); // runTask 重入保护
  // 工具审批：ask = 每次询问 / auto = 自动审批 / allow_all = 完全放行
  const [approvals, setApprovals] = useState([]); // 待审批 [{id, tool, params}]
  const [approvalMode, setApprovalMode] = useState("allow_all");
  const [approvalMenu, setApprovalMenu] = useState(false);
  const [planMenu, setPlanMenu] = useState(false); // 计划/待办 popover（无进行中内容时不显示按钮）
  // AI 接收内容预览（system / 消息 / 工具清单）
  const [preview, setPreview] = useState(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [contextMeta, setContextMeta] = useState({ est_tokens: 0 });

  // 子代理（宿主调度）：模型侧没有这个工具，只有宿主/用户能派生；这里做生命周期可视化
  const [subs, setSubs] = useState({}); // { session_id: { title, phase, ms, error, at, parent } }
  const [delegateOpen, setDelegateOpen] = useState(false);
  // 设置里「子代理自动委派」开关：true 时 popover 切观察态，不暴露手动派出入口
  const [autoDelegate, setAutoDelegate] = useState(false);
  // 观察态用：活跃目标与未开始待办（仅在 popover 打开 + 自动模式时拉取并定期刷新）
  const [delegateGoals, setDelegateGoals] = useState([]);
  const [delegateTodos, setDelegateTodos] = useState([]);
  const subList = Object.entries(subs).map(([id, v]) => ({ session_id: id, ...v }));

  // 停止单个在跑的子代理（委派面板「停止」按钮；只停该子会话，保留其进度）
  const stopSubagent = async (sid) => {
    try {
      await api.stopSubagent(sid);
    } catch (e) {
      console.warn("stop subagent failed:", e);
    }
  };

  // 审批模式初始化 + 全局审批请求监听 + 非流式对话的用量统计监听
  useEffect(() => {
    api.getToolApproval().then((r) => r?.mode && setApprovalMode(r.mode)).catch(() => {});
    const un = listen("tool-approval", (e) => {
      setApprovals((arr) => [...arr, e.payload]);
    });
    // 非流式对话路径通过全局 chat-usage 事件上报缓存命中率
    const unUsage = listen("chat-usage", (e) => {
      const p = e.payload || {};
      if (p.session && p.usage) setUsageMap((m) => ({ ...m, [p.session]: { ...p.usage, type: "usage" } }));
    });
    return () => {
      un.then((f) => f());
      unUsage.then((f) => f());
    };
  }, []);

  // 子代理生命周期：spawn → start → done|error（宿主管控，AI 不能自行派生）
  useEffect(() => {
    const un = listen("subagent-lifecycle", (e) => {
      const p = e.payload || {};
      if (!p.session_id) return;
      setSubs((m) => ({ ...m, [p.session_id]: { ...(m[p.session_id] || {}), ...p } }));
      // 终态短暂停留后移除：让用户看到「完成 / 失败」结果
      if (p.phase === "done" || p.phase === "error") {
        setTimeout(() => {
          setSubs((m) => {
            const next = { ...m };
            delete next[p.session_id];
            return next;
          });
        }, 8000);
      }
      loadSessions();
    });
    return () => un.then((f) => f());
  }, []);

  // 取「子代理自动委派」开关：决定 popover 走手动态（手动输入任务+派出）还是观察态（只读）。
  // 页面常驻挂载不销毁，设置页里改掉后这里必须实时重读：每次切回本页 / 打开弹层都刷新，
  // 否则会一直停留应用启动时的旧值（表现为已关闭仍显示「已开启」）
  useEffect(() => {
    let stop = false;
    api
      .getBehaviorSettings()
      .then((r) => {
        if (!stop && r) setAutoDelegate(!!r.auto_delegate);
      })
      .catch(() => {});
    return () => {
      stop = true;
    };
  }, [delegateOpen, visible]);

  // 对话页直接开始/暂停「子代理自动委派」，与设置页 auto_delegate 双向联动
  const toggleAutoDelegate = async () => {
    try {
      const cur = await api.getBehaviorSettings();
      const next = {
        auto_drive: cur?.auto_drive ?? true,
        tool_approval: cur?.tool_approval || "ask",
        moderation_enabled: cur?.moderation_enabled ?? true,
        auto_delegate: !autoDelegate,
        compat_mode: cur?.compat_mode ?? false,
        subagent_max: cur?.subagent_max ?? 3,
      };
      await api.setBehaviorSettings(
        next.auto_drive,
        next.tool_approval,
        next.moderation_enabled,
        next.auto_delegate,
        next.compat_mode,
        next.subagent_max,
      );
      setAutoDelegate(next.auto_delegate);
    } catch (e) {
      console.warn('[delegate] 保存失败', e);
    }
  };

  // 观察态：popover 打开时拉活跃目标 + 未开始待办；自动模式下每 5s 刷新一次
  useEffect(() => {
    if (!delegateOpen) return;
    let stop = false;
    const fetchAll = () => {
      api
        .listGoals()
        .then((r) => {
          if (!stop) setDelegateGoals((r?.goals || []).filter((g) => g.status === "active"));
        })
        .catch(() => {});
      api
        .listTodos()
        .then((r) => {
          if (!stop) setDelegateTodos((r?.todos || []).filter((t) => t.status === "pending"));
        })
        .catch(() => {});
    };
    fetchAll();
    const timer = autoDelegate ? setInterval(fetchAll, 5000) : null;
    return () => {
      stop = true;
      if (timer) clearInterval(timer);
    };
  }, [delegateOpen, autoDelegate]);

  // 拖拽文件 / 文件夹到窗口：插入链接到输入框
  // 注意：拖拽事件由 Tauri 发在 Webview 目标上，用全局 listen（Any 目标）确保能收到，
  // Window.onDragDropEvent 的目标过滤会导致收不到事件
  const [dragOver, setDragOver] = useState(false);
  useEffect(() => {
    const onDrop = (e) => {
      setDragOver(false);
      const p = e.payload || {};
      const paths = p.paths || [];
      const links = paths.map((path) => {
        const clean = path.replace(/[\\/]+$/, "");
        const name = clean.split(/[\\/]/).pop() || path;
        // 尖括号包裹：路径含空格也不会破坏 markdown 链接
        return `[${name}](<file:///${clean.replace(/\\/g, "/")}>)`;
      });
      if (links.length) setInput((v) => (v ? `${v}\n` : "") + links.join("\n"));
    };
    const uns = [
      listen("tauri://drag-enter", () => setDragOver(true)),
      listen("tauri://drag-over", () => setDragOver(true)),
      listen("tauri://drag-leave", () => setDragOver(false)),
      listen("tauri://drag-drop", onDrop),
    ];
    return () => uns.forEach((u) => u.then((f) => f()));
  }, []);

  // ── 计划栏：输入框上方展示目标与待办 ──
  // 完成项绿色圆点+绿字，未完成白色圆点；小箭头折叠/展开（localStorage 记忆展开状态）
  const [planGoals, setPlanGoals] = useState([]);
  const [planTodos, setPlanTodos] = useState([]);
  const loadPlans = () => {
    api.listGoals().then((r) => setPlanGoals(r.goals || [])).catch(() => {});
    api.listTodos().then((r) => setPlanTodos(r.todos || [])).catch(() => {});
  };
  useEffect(() => {
    loadPlans();
    const timer = setInterval(loadPlans, 3000); // AI 执行 plan 工具后自动刷新
    const unFocus = listen("tauri://focus", loadPlans);
    return () => {
      clearInterval(timer);
      unFocus.then((f) => f());
    };
  }, []);

  // 后台会话变动（如子智能体新建会话 / 完成任务）：自动刷新侧栏与当前会话内容
  useEffect(() => {
    const un = listen("sessions-updated", (e) => {
      loadSessions();
      if (typeof e.payload === "string" && e.payload && e.payload === activeId) {
        loadMessages(activeId);
      }
    });
    return () => un.then((f) => f());
    // 仅随 activeId 重挂监听；loadSessions/loadMessages 闭包引用刻意冻结
  }, [activeId]);

  // 派生：当前会话状态与全局运行数
  const busy = !!busyMap[activeId]; // 当前会话是否执行中
  const live = liveMap[activeId] || null;
  const runningCount = Object.keys(busyMap).length;

  // 计划栏派生：非放弃目标 + 归属待办；无目标待办的独立待办单独成组
  const planVisibleGoals = planGoals.filter((g) => g.status !== "abandoned");
  const planGoalIds = new Set(planVisibleGoals.map((g) => g.id));
  const planVisibleTodos = planTodos.filter((td) => !td.goal_id || planGoalIds.has(td.goal_id));
  const planLooseTodos = planVisibleTodos.filter((td) => !td.goal_id);
  const planGroups = [
    ...planVisibleGoals.map((g) => ({ goal: g, todos: planVisibleTodos.filter((td) => td.goal_id === g.id) })),
    ...(planLooseTodos.length ? [{ goal: null, todos: planLooseTodos }] : []),
  ];
  const planTodoDone = planVisibleTodos.filter((td) => td.status === "completed").length;
  const planAllDone =
    planVisibleTodos.length > 0 &&
    planTodoDone === planVisibleTodos.length &&
    planVisibleGoals.every((g) => g.status === "achieved");

  // 上下文用量估算：优先使用后端按真实上下文构造得到的统一口径，
  // 前端仅在结果返回前用消息长度做兜底估算。
  // 阈值优先取后端从模型 API 拉到的最大上下文（model_context 缓存），
  // 拿不到时退回手动设置（默认 128K，localStorage 持久化）
  const [ctxLimitK, setCtxLimitK] = useState(() => {
    const v = parseInt(localStorage.getItem("bit.ctxLimitK"));
    return Number.isFinite(v) && v >= 4 && v <= 2000 ? v : 128;
  });
  const [limitEdit, setLimitEdit] = useState(false);
  const [limitInput, setLimitInput] = useState("");
  const ctxLimitKnown = Number.isFinite(contextMeta?.max_context) && contextMeta.max_context > 0;
  const CONTEXT_LIMIT = ctxLimitKnown ? contextMeta.max_context : ctxLimitK * 1024;
  const [compressing, setCompressing] = useState(false);
  const estimateTokens = (text) => Math.ceil((text || "").length / 2);
  const localContextTokens = (() => {
    let n = 4; // system prompt 基数
    for (const m of messages) n += estimateTokens(m.content) + 4;
    return n;
  })();
  const contextTokens = Math.max(contextMeta.est_tokens || 0, localContextTokens) + (live ? estimateTokens(live.text) : 0);
  const ctxPct = contextTokens / CONTEXT_LIMIT;
  const fmtK = (n) => (n >= 1024 ? `${(n / 1024).toFixed(1)}K` : String(n));
  const saveLimit = () => {
    setLimitEdit(false);
    const v = parseInt(limitInput);
    if (Number.isFinite(v) && v >= 4 && v <= 2000 && v !== ctxLimitK) {
      setCtxLimitK(v);
      localStorage.setItem("bit.ctxLimitK", String(v));
    }
  };
  const usage = usageMap[activeId] || null;
  const usageKnown = !!usage?.prompt_tokens;

  useEffect(() => {
    let cancelled = false;
    if (!activeId) {
      setContextMeta({ est_tokens: 0 });
      return;
    }
    setContextMeta((prev) => (prev.est_tokens ? { est_tokens: 0 } : prev));
    const timer = setTimeout(async () => {
      try {
        const r = await api.contextMetrics(activeId);
        if (!cancelled) setContextMeta(r || { est_tokens: 0 });
      } catch {
        if (!cancelled) setContextMeta({ est_tokens: 0 });
      }
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [activeId, messages]);

  // 页眉仪表盘数据广播（版本/内存由页眉自取）
  useEffect(() => {
    window.dispatchEvent(
      new CustomEvent("bit-dash", {
        detail: {
          sessions: sessions.length,
          tokens: contextTokens,
          limitK: ctxLimitK,
          running: runningCount > 0,
          cacheHitRate: usage?.cache_known ? usage?.hit_rate || 0 : null,
          cacheKnown: !!usage?.cache_known,
          showCache: usageKnown,
        },
      }),
    );
  }, [sessions.length, contextTokens, ctxLimitK, ctxLimitKnown, runningCount, usage?.hit_rate, usageKnown, usage?.cache_known]);

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
        setAttachErr(t("chat.imageReadFailed") + f.name);
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
          setAttachErr(t("chat.parseFailed") + `${f.name} — ${err}`);
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
      setAttachErr(t("chat.webFetchFailed") + err);
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
    // bit 命令行等外部进程创建的会话也要出现：定期重拉列表（后端按 mtime 合并磁盘变更，代价极低）
    const timer = setInterval(loadSessions, 5000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    if (activeId) loadMessages(activeId);
  }, [activeId]);

  // 滚动跟随：页面隐藏时 display:none，scrollIntoView 静默失败，故隐藏期间用瞬时模式；
  // 切回对话页时立即校正一次滚动位置，恢复流式动画区的可见性
  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: visible ? "smooth" : "auto" });
  }, [messages, busy, live]);
  useEffect(() => {
    if (visible) requestAnimationFrame(() => bottom.current?.scrollIntoView());
  }, [visible]);

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

  // 图标栏「+」小加号 → 新建会话（跨组件事件）
  useEffect(() => {
    const h = () => newSession();
    window.addEventListener("bit-new-session", h);
    return () => window.removeEventListener("bit-new-session", h);
  }, []);

  // 后台 shell / 子代理等面板请求：打开指定会话
  useEffect(() => {
    const h = (e) => {
      const id = e.detail?.id;
      if (!id) return;
      selectSession(id);
    };
    window.addEventListener("bit-open-session", h);
    return () => window.removeEventListener("bit-open-session", h);
    // selectSession 只依赖稳定 api 与 setState，空依赖即可
  }, []);

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

  // 临时提示（自动消失）
  const flashHint = (msg) => {
    setBatchHint(msg);
    clearTimeout(hintTimer.current);
    hintTimer.current = setTimeout(() => setBatchHint(""), 3000);
  };

  const toggleFavorite = async (id) => {
    const s = sessions.find((x) => x.id === id);
    if (!s) return;
    await api.setSessionFavorite(id, !s.favorite).catch(() => {});
    loadSessions();
  };

  const setTagColor = async (id, color) => {
    setColorFor(null);
    await api.setSessionColor(id, color || "").catch(() => {});
    loadSessions();
  };

  // 向右拖动（按住右移超过阈值）→ 召唤该会话的彩色调色板
  useEffect(() => {
    const move = (e) => {
      const d = dragRef.current;
      if (!d) return;
      const dx = e.clientX - d.x;
      const dy = e.clientY - d.y;
      if (!d.armed && dx > 26 && Math.abs(dx) > Math.abs(dy) * 1.3) d.armed = true;
      if (d.armed) e.preventDefault();
    };
    const up = () => {
      const d = dragRef.current;
      dragRef.current = null;
      if (d?.armed) {
        swipedRef.current = d.id;
        setColorFor(d.id);
        setTimeout(() => (swipedRef.current = null), 0);
      }
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
    return () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
  }, []);

  const startRowDrag = (e, id) => {
    if (e.button !== 0 || multiMode) return; // 多选模式下不做右滑手势
    if (busyMap[id]) return;
    dragRef.current = { x: e.clientX, y: e.clientY, id, armed: false };
  };

  // 会话行点击：多选=勾选；普通=切换；刚右滑完的这一次点击被抑制
  const onRowClick = (id) => {
    if (swipedRef.current === id) {
      swipedRef.current = null;
      return;
    }
    if (multiMode) {
      if (busyMap[id]) return;
      setSelSet((prev) => {
        const n = new Set(prev);
        if (n.has(id)) n.delete(id);
        else n.add(id);
        return n;
      });
      return;
    }
    if (colorFor) setColorFor(null);
    selectSession(id);
  };

  const enterMulti = () => {
    setMultiMode(true);
    setSelSet(new Set());
    setColorFor(null);
  };
  const exitMulti = () => {
    setMultiMode(false);
    setSelSet(new Set());
    setColorFor(null);
    setBatchHint("");
  };

  const allSelectableIds = sessions.filter((s) => !busyMap[s.id]).map((s) => s.id);
  const allSelected = allSelectableIds.length > 0 && allSelectableIds.every((id) => selSet.has(id));
  const toggleSelectAll = () => {
    setSelSet(allSelected ? new Set() : new Set(allSelectableIds));
  };

  // 批量删除：收藏与执行中的对话自动跳过；有跳过时留在多选态并给出反馈
  const batchDelete = async () => {
    if (selSet.size === 0) return;
    const picked = sessions.filter((s) => selSet.has(s.id));
    const favIds = picked.filter((s) => s.favorite).map((s) => s.id);
    const busyPicked = picked.filter((s) => busyMap[s.id]);
    const ids = picked.filter((s) => !s.favorite && !busyMap[s.id]).map((s) => s.id);
    if (ids.length === 0) {
      flashHint(favIds.length ? t("chat.favOnly") : t("chat.skipBusy"));
      return;
    }
    const r = await api.deleteSessions(ids).catch(() => null);
    if (!r) return;
    await loadSessions();
    if (ids.includes(activeId)) setActiveId(r.active || "");
    // 保留未被删除的勾选项（收藏 / 执行中），供用户继续处理
    setSelSet(new Set(picked.filter((s) => !ids.includes(s.id)).map((s) => s.id)));
    const notes = [];
    if (favIds.length) notes.push(t("chat.favProtected"));
    if (busyPicked.length) notes.push(t("chat.skipBusy"));
    if (notes.length) flashHint(notes.join(" · "));
    else exitMulti();
  };

  const send = async () => {
    const text = input.trim();
    // 「停止 <job_id>」：直接取消对应后台长命令，不再发给 AI（AI 提示用户时可这么用）
    const stopM = text.match(/^(?:停止|stop)\s+(sh\d+)\s*$/i);
    if (stopM) {
      const jid = stopM[1];
      setInput("");
      try {
        await api.cancelShell(jid);
        if (activeRef.current)
          setMessages((msgs) => [...msgs, { role: "assistant", content: `⏹ 已请求停止后台命令 ${jid}（进程即将终止）。` }]);
      } catch (e) {
        const emsg = typeof e === "string" ? e : e?.message || String(e);
        if (activeRef.current)
          setMessages((msgs) => [...msgs, { role: "assistant", content: `⚠️ ${emsg}` }]);
      }
      return;
    }
    // 允许只带附件（图片/文档）而无文字时也可发送
    if ((!text && !hasAttachments) || !activeId) return;
    const sid = activeId; // 锁定目标会话：之后用户切换页面不影响本次执行

    // 文档正文（Excel/Word/CSV/网页）拼接到消息前作为上下文
    const docBlocks = docs
      .map((d) => `【附件：${d.name}】\n${d.text}`)
      .join("\n\n---\n\n");
    const composed = docBlocks
      ? `${docBlocks}${text ? `\n\n---\n\n${text}` : ""}`
      : text || t("chat.seeAttachedImages");

    // 图片以 dataURL base64 传给多模态模型
    const imgData = images.map((im) => im.dataUrl);

    // 气泡里展示用户实际输入（不含附件正文），并标注附件数量
    const bubble =
      (text || "") +
      (hasAttachments
        ? `${text ? "\n\n" : ""}[${images.length ? t("chat.attachImages") + ` ×${images.length}` : ""}${
            images.length && docs.length ? t("chat.attachSep") : ""
          }${docs.length ? t("chat.attachDocs") + ` ×${docs.length}` : ""}]`
        : "");
    const item = { composed, bubble: bubble || text, imgData };

    setInput("");
    setImages([]);
    setDocs([]);
    setAttachErr("");

    // 会话执行中：加入等待队列，当前任务结束后自动按顺序发送
    if (busyMap[sid] || runningRef.current.has(sid)) {
      const next = { ...queuesRef.current, [sid]: [...(queuesRef.current[sid] || []), item] };
      queuesRef.current = next;
      setQueues(next);
      if (activeRef.current === sid) {
        setMessages((msgs) => [...msgs, { role: "user", content: item.bubble }]);
      }
      return;
    }

    if (activeRef.current === sid) {
      setMessages((msgs) => [...msgs, { role: "user", content: item.bubble }]);
    }
    // 用户主动发消息 = 想继续对话：解除此前中断造成的队列暂停
    setPausedFor(sid, false);
    runTask(sid, item);
  };

  // 实际执行一次对话任务（流式），结束时自动续发该会话的等待队列。
  // busy 标记统一在此设置：无论是直接发送还是队列续发，执行期间新消息都会正确排队
  const runTask = async (sid, item) => {
    if (runningRef.current.has(sid)) return; // 重入保护：同一会话绝不并发两个任务
    runningRef.current.add(sid);
    setBusyMap((m) => ({ ...m, [sid]: true }));
    setLiveMap((m) => ({ ...m, [sid]: m[sid] || { text: "", think: "", cards: [] } }));
    // 结束时移除该会话的运行/流式状态
    const endLive = () =>
      setLiveMap((m) => {
        const n = { ...m };
        delete n[sid];
        return n;
      });
    try {
      const res = await api.chatStream(sid, item.composed, null, (ev) => {
        if (!ev || typeof ev !== "object") return;
        switch (ev.type) {
          case "round_start":
            // 新一轮开始：清空本轮实时文本，保留此前已完成轮次的工具卡片；
            // 思考过程跨轮累积（落库的是整个回合的思考，实时面板保持一致）
            setLiveMap((m) => ({ ...m, [sid]: { text: "", think: m[sid]?.think || "", cards: m[sid]?.cards || [] } }));
            break;
          case "think":
            // 思考过程增量（reasoning/thinking）：实时展示
            setLiveMap((m) => ({
              ...m,
              [sid]: { text: m[sid]?.text || "", think: (m[sid]?.think || "") + (ev.text || ""), cards: m[sid]?.cards || [] },
            }));
            break;
          case "delta":
            setLiveMap((m) => ({
              ...m,
              [sid]: { text: (m[sid]?.text || "") + (ev.text || ""), think: m[sid]?.think || "", cards: m[sid]?.cards || [] },
            }));
            break;
          case "round_tools_starting":
            // 工具轮开始：先渲染灰色 spinner 占位（tools 事件到达后会覆盖同一张卡）
            setLiveMap((m) => ({
              ...m,
              [sid]: {
                text: "",
                think: m[sid]?.think || "",
                cards: [...(m[sid]?.cards || []), { pending: ev.pending || [], visible: "", calls: [] }],
              },
            }));
            break;
          case "tools":
            // 本轮是工具调用：丢弃流式文本，沉淀为卡片（含可选的说明文字）；
            // 思考跨轮保留（落库的是整个回合的思考，实时面板保持一致，调工具不清空）
            // 同时把前面的 pending spinner 卡替换成完整结果卡
            setLiveMap((m) => {
              const oldCards = m[sid]?.cards || [];
              // 找到最后一张 pending 卡（没有 calls 只有 pending），替换成结果卡
              const newCards = oldCards.slice();
              const pendingIdx = newCards.findLastIndex((c) => c.pending && (!c.calls || c.calls.length === 0));
              const resultCard = { visible: ev.visible || "", calls: ev.calls || [] };
              if (pendingIdx >= 0) {
                newCards[pendingIdx] = resultCard;
              } else {
                newCards.push(resultCard);
              }
              return {
                ...m,
                [sid]: { text: "", think: m[sid]?.think || "", cards: newCards },
              };
            });
            break;
          case "continue":
            // 后端检测到回复被截断，自动续发「继续」：用清洗后的片段替换原始流式文本
            setLiveMap((m) => ({ ...m, [sid]: { text: ev.visible || "", think: m[sid]?.think || "", cards: m[sid]?.cards || [] } }));
            break;
          case "net_retry":
            // 网络瞬断自动重试：本轮半截内容重新生成（历史未落库），清空实时文本与思考避免重复；
            // 已完成轮次的工具卡片保留
            setLiveMap((m) => ({ ...m, [sid]: { text: "", think: "", cards: m[sid]?.cards || [] } }));
            break;
          case "usage":
            // 本轮 token 用量与缓存命中率（会话累计）
            setUsageMap((m) => ({ ...m, [sid]: ev }));
            break;
          case "final":
            // 仅当用户还停留在这个会话时刷新消息列表；后台会话结果已落库
            if (activeRef.current === sid && ev.messages) setMessages(ev.messages);
            endLive();
            break;
          case "error":
            if (ev.interrupted) {
              // 中断：中间过程（工具调用等）已逐轮落库，重新加载完整历史而非清空现场
              setPausedFor(sid, true); // 队列暂停，不自动续发
              if (activeRef.current === sid) loadMessages(sid);
              endLive();
            } else {
              if (activeRef.current === sid) {
                setMessages(() => [
                  // 上游断流前已流出的部分回复先入列（后端已落库），避免已显示的内容消失
                  ...(ev.partial ? [{ role: "assistant", content: ev.partial }] : []),
                  { role: "assistant", content: t("chat.callFailed") + ev.error },
                ]);
              }
              endLive();
            }
            break;
          default:
            break;
        }
      }, item.imgData);
      if (activeRef.current === sid && res?.messages) setMessages(res.messages);
      endLive();
      loadSessions();
      onStats?.();
    } catch (e) {
      // 错误已通过 error 事件推送给用户，这里只兜底清理（避免双重错误消息）
      console.error("chat task failed:", e);
      endLive();
    } finally {
      runningRef.current.delete(sid);
      setBusyMap((m) => {
        const n = { ...m };
        delete n[sid];
        return n;
      });
      pumpQueue(sid);
    }
  };

  // ── 等待发送队列 ──
  const setPausedFor = (sid, v) => {
    pausedRef.current = { ...pausedRef.current, [sid]: v };
    setQueuePaused(pausedRef.current);
  };

  const clearQueue = (sid) => {
    const next = { ...queuesRef.current };
    delete next[sid];
    queuesRef.current = next;
    setQueues(next);
  };

  const removeQueueItem = (sid, i) => {
    const next = { ...queuesRef.current, [sid]: (queuesRef.current[sid] || []).filter((_, j) => j !== i) };
    queuesRef.current = next;
    setQueues(next);
  };

  // ── 队列拖拽排序（自实现 mouse 拖拽，兼容 WKWebView）──
  const queueBoxRef = useRef(null);
  useEffect(() => {
    if (dragIdx == null) return;
    const move = (e) => {
      if (!queueBoxRef.current) return;
      const arr = queuesRef.current[activeRef.current] || [];
      // 找到第一个「中点在鼠标右侧」的剩余 pill → 插到它前面；都没有则插到末尾
      let k = arr.length - 1;
      for (const p of queueBoxRef.current.querySelectorAll("[data-qidx]")) {
        const i = Number(p.dataset.qidx);
        if (i === dragIdx) continue;
        const r = p.getBoundingClientRect();
        if (e.clientX < r.left + r.width / 2) {
          k = i > dragIdx ? i - 1 : i;
          break;
        }
      }
      // 以移除后的数组为基准：from 移除后插入到 k
      if (k !== dragIdx) {
        const item = arr[dragIdx];
        if (!item) return;
        const next = [...arr];
        next.splice(dragIdx, 1);
        next.splice(k, 0, item);
        const qnext = { ...queuesRef.current, [activeRef.current]: next };
        queuesRef.current = qnext;
        setQueues(qnext);
        setDragIdx(k);
      }
    };
    const up = () => setDragIdx(null);
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
    return () => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
    };
  }, [dragIdx]);

  // 任务结束后按顺序续发；被中断的会话暂停续发，需手动「继续」。
  // 副作用（setTimeout 启动 runTask）在 updater 外执行，避免 StrictMode 下 updater 双调用导致重复发送
  const pumpQueue = (sid) => {
    if (pausedRef.current[sid]) return;
    const arr = queuesRef.current[sid] || [];
    if (arr.length === 0) return;
    const [next, ...rest] = arr;
    const updated = { ...queuesRef.current, [sid]: rest };
    queuesRef.current = updated;
    setQueues(updated);
    setTimeout(() => runTask(sid, next), 50);
  };

  const resumeQueue = (sid) => {
    setPausedFor(sid, false);
    pumpQueue(sid);
  };

  // 立即中断当前会话任务；队列保留并暂停
  const interrupt = async () => {
    if (!activeId) return;
    setPausedFor(activeId, true);
    await api.chatInterrupt(activeId).catch(() => {});
  };

  // ── 工具审批 ──
  const answerApproval = async (id, allow) => {
    setApprovals((arr) => arr.filter((a) => a.id !== id));
    await api.toolApprove(id, allow).catch(() => {});
  };

  const changeApprovalMode = (mode) => {
    setApprovalMode(mode);
    setApprovalMenu(false);
    api.setToolApproval(mode).catch(() => {});
  };

  // ── AI 接收内容预览 ──
  const openPreview = async () => {
    const r = await api.contextPreview(activeId).catch(() => null);
    if (r) {
      setPreview(r);
      setPreviewOpen(true);
    }
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
      setAttachErr(t("chat.compressFailed") + e);
    } finally {
      setCompressing(false);
    }
  };

  const visibleMessages = messages.filter((m) => m.role !== "system");

  return (
    <div className="relative flex h-full gap-3">
      {/* 拖拽文件 / 文件夹提示遮罩 */}
      {dragOver && (
        <div className="pointer-events-none absolute inset-0 z-40 flex items-center justify-center rounded-xl border-2 border-dashed border-neutral-400 bg-neutral-500/10 dark:border-neutral-500">
          <div className="card px-6 py-4 text-sm font-medium">{t("chat.dropHint")}</div>
        </div>
      )}
      {/* 会话侧栏：纯文字列表（仪表盘已上移页眉） */}
      <div className="flex w-52 shrink-0 flex-col gap-1.5">
        {/* 会话操作条：默认提供「多选」；多选态下可 全选 / 批量删除 / 完成 */}
        <div className="flex shrink-0 flex-wrap items-center gap-1 px-1">
          {!multiMode ? (
            <button
              onClick={enterMulti}
              title={`${t("chat.multi")} · ${t("chat.colorSwipeHint")}`}
              className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] font-medium text-neutral-500 transition-colors hover:bg-neutral-900/5 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-white/10 dark:hover:text-white"
            >
              <IconCheck size={12} />
              {t("chat.multi")}
            </button>
          ) : (
            <>
              <button
                onClick={toggleSelectAll}
                className="rounded-full px-2 py-1 text-[11px] font-medium text-neutral-500 transition-colors hover:bg-neutral-900/5 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-white/10 dark:hover:text-white"
              >
                {allSelected ? t("chat.unselectAll") : t("common.selectAll")}
              </button>
              <button
                onClick={batchDelete}
                disabled={selSet.size === 0}
                title={t("common.deleteSelected")}
                className="flex items-center gap-1 rounded-full px-2 py-1 text-[11px] font-medium text-red-500 transition-colors hover:bg-red-500/10 disabled:cursor-not-allowed disabled:opacity-40"
              >
                <IconTrash size={11} />
                {t("common.deleteSelected")}
                {selSet.size > 0 && `(${selSet.size})`}
              </button>
              <button
                onClick={exitMulti}
                className="ml-auto rounded-full px-2 py-1 text-[11px] font-medium text-neutral-500 transition-colors hover:bg-neutral-900/5 hover:text-neutral-900 dark:text-neutral-400 dark:hover:bg-white/10 dark:hover:text-white"
              >
                {t("chat.multiDone")}
              </button>
            </>
          )}
        </div>
        {multiMode && (
          <p
            className={`px-1 text-[10px] leading-snug ${
              batchHint ? "text-amber-500" : "text-neutral-400 dark:text-neutral-500"
            }`}
          >
            {batchHint || t("chat.multiHint")}
          </p>
        )}

        <div className="flex-1 space-y-0.5 overflow-y-auto">
          {sessions.length === 0 && (
            <div className="px-2 py-4 text-center text-xs text-neutral-400">{t("chat.noSessions")}</div>
          )}
          {sessions.map((s) => {
            const active = s.id === activeId;
            const selected = selSet.has(s.id);
            const colored = !!s.color && !multiMode;
            return (
              <div key={s.id} className="relative">
                <div
                  onClick={() => onRowClick(s.id)}
                  onMouseDown={(e) => startRowDrag(e, s.id)}
                  style={colored ? { boxShadow: `inset 3px 0 0 ${s.color}` } : undefined}
                  className={`anim-rise group flex cursor-pointer items-center gap-2 rounded-xl px-2.5 py-2 transition-all duration-200 hover:translate-x-0.5 ${
                    multiMode
                      ? selected
                        ? "accent-solid"
                        : busyMap[s.id]
                          ? "cursor-default text-neutral-400 opacity-60"
                          : "text-neutral-600 hover:bg-neutral-900/5 dark:text-neutral-300 dark:hover:bg-white/5"
                      : active
                        ? "accent-solid"
                        : "text-neutral-600 hover:bg-neutral-900/5 dark:text-neutral-300 dark:hover:bg-white/5"
                  }`}
                >
                  {multiMode ? (
                    busyMap[s.id] ? (
                      <span
                        title={t("chat.skipBusy")}
                        className="h-2.5 w-2.5 shrink-0 animate-pulse rounded-full bg-emerald-500 ring-2 ring-emerald-500/30"
                      />
                    ) : (
                      <span className="flex shrink-0 items-center gap-1">
                        {s.favorite && <IconStarSolid size={10} className="text-amber-400" />}
                        <span
                          className={`flex h-4 w-4 items-center justify-center rounded-md border ${
                            selected
                              ? "border-transparent bg-amber-400 text-white"
                              : "border-neutral-300 dark:border-neutral-600"
                          }`}
                        >
                          {selected && <IconCheck size={10} strokeWidth={3} />}
                        </span>
                      </span>
                    )
                  ) : (
                    <>
                      <IconChat
                        size={14}
                        className={`shrink-0 ${busyMap[s.id] ? "hidden" : ""} ${
                          colored ? "opacity-100" : "opacity-70"
                        }`}
                        style={colored ? { color: s.color } : undefined}
                      />
                      {busyMap[s.id] && (
                        <span
                          title={t("chat.running")}
                          className="h-2.5 w-2.5 shrink-0 animate-pulse rounded-full bg-emerald-500 ring-2 ring-emerald-500/30"
                        />
                      )}
                      {s.favorite && (
                        <IconStarSolid
                          size={10}
                          className={`shrink-0 text-amber-400 ${active ? "dark:text-amber-300" : ""}`}
                        />
                      )}
                    </>
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
                    <div
                      className="min-w-0 flex-1 truncate text-[13px] font-medium"
                      title={s.preview || s.title}
                    >
                      {s.title || t("chat.newChat")}
                    </div>
                  )}
                  {!multiMode && renaming?.id !== s.id && (
                    <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                      <button
                        title={s.favorite ? t("chat.favRemove") : t("chat.favAdd")}
                        onClick={(e) => {
                          e.stopPropagation();
                          toggleFavorite(s.id);
                        }}
                        className={`rounded p-1 ${
                          s.favorite
                            ? "text-amber-400"
                            : active
                              ? "hover:bg-white/20 dark:hover:bg-black/20"
                              : "text-neutral-300 hover:text-amber-400 dark:text-neutral-600 dark:hover:text-amber-400"
                        }`}
                      >
                        {s.favorite ? <IconStarSolid size={12} /> : <IconStar size={12} />}
                      </button>
                      <button
                        title={t("chat.rename")}
                        onClick={(e) => {
                          e.stopPropagation();
                          setRenaming({ id: s.id, title: s.title || "" });
                        }}
                        className={`rounded p-1 ${active ? "hover:bg-white/20 dark:hover:bg-black/20" : "hover:bg-neutral-900/10 dark:hover:bg-white/10"}`}
                      >
                        <IconEdit size={12} />
                      </button>
                      <button
                        title={t("common.delete")}
                        onClick={(e) => deleteSession(s.id, e)}
                        className={`rounded p-1 ${active ? "hover:bg-white/20 dark:hover:bg-black/20" : "hover:bg-neutral-900/10 dark:hover:bg-white/10"}`}
                      >
                        <IconTrash size={12} />
                      </button>
                    </div>
                  )}
                </div>
                {/* 右滑对话召出的彩色标签调色板 */}
                {colorFor === s.id && (
                  <div
                    onMouseDown={(e) => e.stopPropagation()}
                    className="mt-0.5 rounded-xl border border-neutral-200 bg-white p-2 shadow-sm dark:border-neutral-700 dark:bg-neutral-900"
                  >
                    <div className="mb-1.5 flex items-center justify-between">
                      <span className="text-[10px] font-medium text-neutral-500 dark:text-neutral-300">
                        {t("chat.colorTitle")}
                      </span>
                      <div className="flex items-center gap-2">
                        {s.color && (
                          <button
                            onClick={() => setTagColor(s.id, "")}
                            className="text-[10px] text-neutral-400 hover:text-red-500"
                          >
                            {t("chat.colorClear")}
                          </button>
                        )}
                        <button
                          onClick={() => setColorFor(null)}
                          title={t("common.close")}
                          className="text-neutral-400 hover:text-neutral-900 dark:hover:text-white"
                        >
                          <IconX size={11} />
                        </button>
                      </div>
                    </div>
                    <div className="flex flex-wrap gap-1">
                      {TAG_COLORS.map((c) => (
                        <button
                          key={c}
                          onClick={() => setTagColor(s.id, c)}
                          title={c}
                          className={`h-[18px] w-[18px] rounded-full transition-transform hover:scale-110 ${
                            s.color === c ? "ring-2 ring-neutral-900/40 ring-offset-1 dark:ring-white/50" : ""
                          }`}
                          style={{ background: c }}
                        />
                      ))}
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      {/* 对话主区：无标题行，主体完全留给消息 */}
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="card flex-1 overflow-y-auto">
          {messages.length === 0 && !busy && (
            <div className="flex h-full items-center justify-center px-6 text-center text-sm text-neutral-400">
              {t("chat.emptyHint")}
            </div>
          )}
          <div className="flex flex-col gap-3">
            {messages.map((m, i) =>
              m.role === "system" ? (
                // 压缩分割线：会话中的 system 消息即压缩摘要（仅 compress_session 写入），
                // 悬停可查看完整摘要；上方为压缩前历史，下方为压缩后新对话
                <div
                  key={i}
                  title={m.content}
                  className="my-2 flex items-center gap-3"
                >
                  <div className="h-px flex-1 bg-neutral-200 dark:bg-neutral-700" />
                  <span className="shrink-0 text-[11px] text-neutral-400">
                    {t("chat.compressedDivider")}
                  </span>
                  <div className="h-px flex-1 bg-neutral-200 dark:bg-neutral-700" />
                </div>
              ) : (
                <MessageBubble key={i} message={m} />
              )
            )}
            {/* 流式实时区：已完成轮次的工具卡片 + 本轮增量文本 */}
            {live && (
              <div className="mr-auto flex max-w-[85%] flex-col gap-2">
                {live.cards.map((c, i) => {
                  const fileCalls = (c.calls || []).filter((x) => x.tool === "send_file" && x.ok);
                  const toolCalls = (c.calls || []).filter((x) => !(x.tool === "send_file" && x.ok));
                  return (
                    <div key={i} className="flex flex-col gap-2">
                      {/* 执行中占位卡（round_tools_starting 事件先渲染，tools 事件替换） */}
                      {c.pending && (!c.calls || c.calls.length === 0) && (
                        <div className="flex flex-col gap-1.5">
                          {c.pending.map((p, j) => (
                            <PendingToolCard key={j} tool={p.tool} summary={p.summary} />
                          ))}
                        </div>
                      )}
                      {toolCalls.length > 0 && (
                        <div className="flex flex-col gap-1.5">
                          {toolCalls.map((call, j) => (
                            <ToolCallCard key={j} call={call} />
                          ))}
                        </div>
                      )}
                      {c.visible && c.visible.trim() && (
                        <div className="rounded-3xl rounded-bl-lg border border-neutral-200 bg-white px-4 py-2.5 dark:border-neutral-800 dark:bg-neutral-900">
                          <Markdown>{c.visible}</Markdown>
                        </div>
                      )}
                      {/* 文件卡片放在说明文字之后：先说话后发文件 */}
                      {fileCalls.length > 0 && (
                        <div className="flex flex-col gap-1.5">
                          {fileCalls.map((call, j) => (
                            <FileCard
                              key={j}
                              path={call.params?.path}
                              bytes={call.result?.bytes}
                              note={call.params?.note}
                            />
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })}
                {/* 本轮思考过程实时展示（默认展开，流式中） */}
                {live.think && live.think.trim() && <ThinkPanel text={live.think} openDefault streaming />}
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
          {/* 合并状态条：统一展示上下文用量与缓存命中，避免两处口径漂移 */}
          {(visibleMessages.length > 0 || usageKnown) && (
            <div
              className={`flex items-center gap-2 self-start rounded-full px-3 py-1 text-[11px] ${
                ctxPct >= 1
                  ? "bg-red-500/10 text-red-600 dark:text-red-400"
                  : ctxPct >= 0.7 || (usage?.cache_known && (usage.hit_rate || 0) < 0.5)
                    ? "bg-amber-500/10 text-amber-600 dark:text-amber-400"
                    : usage?.cache_known && (usage.hit_rate || 0) >= 0.8
                      ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                    : "text-neutral-400"
              }`}
              title={usageKnown ? t("chat.cacheTip") : undefined}
            >
              <span>
                {ctxPct >= 1
                  ? t("chat.ctxTooLong")
                  : ctxPct >= 0.7
                    ? t("chat.ctxHigh")
                    : t("chat.context")}{" "}
                {t("chat.ctxApprox") + ` ${fmtK(contextTokens)} / `}
                {limitEdit ? (
                  <input
                    autoFocus
                    type="number"
                    min={4}
                    max={2000}
                    value={limitInput}
                    onChange={(e) => setLimitInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") saveLimit();
                      if (e.key === "Escape") setLimitEdit(false);
                    }}
                    onBlur={saveLimit}
                    className="w-14 rounded-md bg-transparent px-1 text-center outline-none ring-1 ring-current/40"
                  />
                ) : ctxLimitKnown ? (
                  <span title={t("chat.ctxAutoTitle")}>{Math.round(CONTEXT_LIMIT / 1024)}K</span>
                ) : (
                  <button
                    onClick={() => {
                      setLimitInput(String(ctxLimitK));
                      setLimitEdit(true);
                    }}
                    title={t("chat.ctxLimitTitle")}
                    className="underline decoration-dotted underline-offset-2 hover:opacity-70"
                  >
                    {ctxLimitK}K
                  </button>
                )}{" "}
                tokens
                {usageKnown &&
                  ` · ${
                    usage?.cache_known
                      ? `${t("chat.cacheHit")} ${Math.round((usage.hit_rate || 0) * 100)}%`
                      : t("chat.cacheUnknown")
                  }`}
              </span>
              {(ctxPct >= 1 || compressing) && (
                <button
                  onClick={compress}
                  disabled={compressing || busy}
                  title={t("chat.compressTip")}
                  className="rounded-full bg-red-500/15 px-2.5 py-0.5 font-medium text-red-600 transition-colors hover:bg-red-500/25 disabled:opacity-40 dark:text-red-400"
                >
                  {compressing ? t("chat.compressing") : t("chat.compress")}
                </button>
              )}
            </div>
          )}

          {/* 等待发送队列（拖拽 pill 调整发送顺序） */}
          {queues[activeId]?.length > 0 && (
            <div ref={queueBoxRef} className="flex flex-wrap items-center gap-2 px-1">
              <span className="flex items-center gap-1 text-[11px] text-neutral-400">
                <IconQueue size={13} />
                {queues[activeId].length} {t("chat.queued")}
              </span>
              {queues[activeId].map((q, i) => (
                <span
                  key={`${i}-${(q.bubble || "").slice(0, 8)}`}
                  data-qidx={i}
                  onMouseDown={() => setDragIdx(i)}
                  onDoubleClick={() => removeQueueItem(activeId, i)}
                  className={`flex max-w-[220px] cursor-grab select-none items-center gap-1 rounded-full px-2.5 py-1 text-[11px] transition-colors active:cursor-grabbing ${
                    dragIdx === i
                      ? "bg-neutral-900/15 ring-1 ring-neutral-900/30 dark:bg-white/20 dark:ring-white/40"
                      : "bg-neutral-900/5 hover:bg-neutral-900/10 dark:bg-white/10 dark:hover:bg-white/15"
                  }`}
                  title={t("chat.queueDragHint")}
                >
                  <span className="text-neutral-400">{i + 1}</span>
                  <span className="truncate">{(q.bubble || "").split("\n")[0]}</span>
                  <button
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={() => removeQueueItem(activeId, i)}
                    className="shrink-0 text-neutral-400 hover:text-red-500"
                    title={t("common.remove")}
                  >
                    <IconX size={10} />
                  </button>
                </span>
              ))}
              {queuePaused[activeId] && (
                <>
                  <span className="text-[11px] text-amber-500">{t("chat.queuePaused")}</span>
                  <button onClick={() => resumeQueue(activeId)} className="text-[11px] text-emerald-500 hover:underline">
                    {t("chat.resumeQueue")}
                  </button>
                </>
              )}
              <button onClick={() => clearQueue(activeId)} className="text-[11px] text-neutral-400 hover:text-red-500">
                {t("chat.clearAll")}
              </button>
            </div>
          )}

          {/* 工具审批卡片 */}
          {approvals.map((a) => (
            <div key={a.id} className="card border-amber-400/60 p-3 dark:border-amber-500/40">
              <div className="mb-2 flex items-center gap-2 text-sm font-medium">
                <IconShield size={15} className="text-amber-500" />
                {t("chat.approvalTitle")}
                <span className="font-mono">{a.tool}</span>
              </div>
              <pre className="mb-2 max-h-32 overflow-auto rounded-xl bg-neutral-100 p-2 text-[11px] dark:bg-neutral-900">
                {JSON.stringify(a.params, null, 2)}
              </pre>
              <div className="flex gap-2">
                <button onClick={() => answerApproval(a.id, true)} className="pill pill-hover">
                  {t("chat.approve")}
                </button>
                <button onClick={() => answerApproval(a.id, false)} className="pill pill-outline text-red-500">
                  {t("chat.reject")}
                </button>
              </div>
            </div>
          ))}

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
                    title={t("common.remove")}
                    className="accent-solid absolute -right-1.5 -top-1.5 rounded-full p-0.5 opacity-0 shadow transition-opacity group-hover:opacity-100"
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
                  <button onClick={() => removeDoc(i)} title={t("common.remove")} className="opacity-60 hover:opacity-100">
                    <IconX size={12} />
                  </button>
                </div>
              ))}
              {attaching && (
                <span className="text-xs text-neutral-400">{t("chat.parsing")}</span>
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
                  {t("chat.clearAll")}
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
                placeholder={t("chat.urlPlaceholder")}
                className="field min-w-0 flex-1"
              />
              <label className="flex shrink-0 items-center gap-1.5 text-xs text-neutral-600 dark:text-neutral-300">
                <input
                  type="checkbox"
                  checked={urlParse}
                  onChange={(e) => setUrlParse(e.target.checked)}
                />
                {t("chat.parseContent")}
              </label>
              <button
                onClick={addUrl}
                disabled={!urlText.trim() || attaching}
                className="pill pill-hover shrink-0"
              >
                {t("common.add")}
              </button>
              <button
                onClick={() => setUrlOpen(false)}
                title={t("common.close")}
                className="shrink-0 rounded-full p-1.5 hover:bg-neutral-900/5 dark:hover:bg-white/10"
              >
                <IconX size={14} />
              </button>
            </div>
          )}

          {/* 后台长命令状态：原固定条已迁移到工具栏「后台运行任务状态」按钮 popover */}

          {/* 子代理状态条：宿主派出的子代理实时进度（点击查看子会话全过程） */}
          {subList.length > 0 && (
            <div className="flex flex-wrap items-center gap-2 rounded-xl border border-neutral-200 bg-neutral-50 px-2.5 py-1.5 text-xs dark:border-neutral-800 dark:bg-neutral-900">
              <IconQueue size={14} className="shrink-0 text-neutral-500" />
              {subList.map((s) => {
                const done = s.phase === "done";
                const failed = s.phase === "error";
                const secs = s.ms ? Math.max(1, Math.round(s.ms / 1000)) : 0;
                return (
                  <button
                    key={s.session_id}
                    onClick={() => selectSession(s.session_id)}
                    title={t("chat.subagentJump")}
                    className={`min-w-0 max-w-full truncate rounded-lg px-1.5 py-0.5 text-left hover:bg-neutral-900/5 dark:hover:bg-white/10 ${
                      failed
                        ? "text-red-500"
                        : done
                        ? "text-emerald-600 dark:text-emerald-400"
                        : "text-neutral-600 dark:text-neutral-300"
                    }`}
                  >
                    <span className="mr-1 font-medium">{s.title || t("chat.subagent")}</span>
                    <span className="text-neutral-400">
                      {failed
                        ? `${t("chat.subagentFailed")}${s.error ? `：${s.error}` : ""}`
                        : done
                        ? `${t("chat.subagentDone")} · ${secs}s`
                        : t("chat.thinking")}
                    </span>
                  </button>
                );
              })}
            </div>
          )}

          {/* 底部 pb-12 为工具栏/发送按钮预留独立一行：textarea 只在自己的区域滚动，永不进入按钮行 */}
          <div className="relative rounded-2xl border border-neutral-300 bg-white pb-12 transition-colors focus-within:border-neutral-900 dark:border-neutral-700 dark:bg-neutral-950 dark:focus-within:border-neutral-200">
            {/* 工具栏：左下角 absolute 排成一行（外层按钮收进输入框内） */}
            <div className="pointer-events-auto absolute bottom-2 left-2 z-10 flex items-center gap-0.5">
            {/* 上下文预览 */}
            <button
              onClick={openPreview}
              disabled={!activeId}
              title={t("chat.preview")}
              className="shrink-0 rounded-full p-2 text-neutral-500 transition-colors hover:bg-neutral-900/5 hover:text-neutral-900 disabled:opacity-40 dark:hover:bg-white/10 dark:hover:text-white"
            >
              <IconEye size={18} />
            </button>
            {/* 工具审批模式 */}
            <div className="relative shrink-0">
              <button
                onClick={() => setApprovalMenu((v) => !v)}
                title={t("chat.approvalMode")}
                className={`shrink-0 rounded-full p-2 transition-colors ${
                  approvalMode !== "allow_all"
                    ? "bg-amber-500/15 text-amber-600 dark:text-amber-400"
                    : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:hover:bg-white/10 dark:hover:text-white"
                }`}
              >
                <IconShield size={18} />
              </button>
              {approvalMenu && (
                <div className="card absolute bottom-11 left-0 z-20 w-60 p-1">
                  {[
                    ["ask", "chat.approvalAsk"],
                    ["auto", "chat.approvalAuto"],
                    ["allow_all", "chat.approvalAllowAll"],
                  ].map(([m, k]) => (
                    <button
                      key={m}
                      onClick={() => changeApprovalMode(m)}
                      className={`w-full rounded-lg px-3 py-1.5 text-left text-xs hover:bg-neutral-900/5 dark:hover:bg-white/10 ${
                        approvalMode === m ? "font-semibold" : "text-neutral-500 dark:text-neutral-400"
                      }`}
                    >
                      {t(k)}
                    </button>
                  ))}
                </div>
              )}
            </div>
            {/* 附件：图片 / 文档 / 网址 收进一个 + 菜单，减少一排重复图标 */}
            <div className="relative shrink-0">
              <button
                onClick={() => setAttachMenu((v) => !v)}
                disabled={busy || !activeId}
                title={t("chat.attach")}
                className={`shrink-0 rounded-full p-2 transition-colors disabled:opacity-40 ${
                  attachMenu
                    ? "accent-solid"
                    : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:hover:bg-white/10 dark:hover:text-white"
                }`}
              >
                <IconPlus size={18} />
              </button>
              {attachMenu && (
                <div className="card absolute bottom-11 left-0 z-20 w-44 p-1">
                  <button
                    onClick={() => {
                      setAttachMenu(false);
                      imgInput.current?.click();
                    }}
                    className="w-full rounded-lg px-3 py-1.5 text-left text-xs hover:bg-neutral-900/5 dark:hover:bg-white/10"
                  >
                    {t("chat.uploadImages")}
                  </button>
                  <button
                    onClick={() => {
                      setAttachMenu(false);
                      docInput.current?.click();
                    }}
                    className="w-full rounded-lg px-3 py-1.5 text-left text-xs hover:bg-neutral-900/5 dark:hover:bg-white/10"
                  >
                    {t("chat.uploadDocs")}
                  </button>
                  <button
                    onClick={() => {
                      setAttachMenu(false);
                      setUrlOpen(true);
                    }}
                    className="w-full rounded-lg px-3 py-1.5 text-left text-xs hover:bg-neutral-900/5 dark:hover:bg-white/10"
                  >
                    {t("chat.addUrl")}
                  </button>
                </div>
              )}
            </div>

            {/* 计划/待办：始终显示靶标按钮，点开 popover 查看/管理 */}
            <div className="relative shrink-0">
              <button
                onClick={() => setPlanMenu((v) => !v)}
                title={planVisibleTodos.length > 0 ? `${t("chat.plan")} ${planTodoDone}/${planVisibleTodos.length}` : t("chat.plan")}
                className={`shrink-0 rounded-full p-2 transition-colors ${
                  planMenu
                    ? "accent-solid"
                    : planAllDone && planVisibleTodos.length > 0
                    ? "text-emerald-500 hover:bg-neutral-900/5 dark:hover:bg-white/10"
                    : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:hover:bg-white/10 dark:hover:text-white"
                }`}
              >
                <IconTarget size={18} />
              </button>
              {planMenu && (
                <div className="card absolute bottom-11 left-0 z-20 max-h-64 w-72 overflow-y-auto p-3">
                  <div className="mb-2 flex items-center justify-between">
                    <span className="text-xs font-medium">{t("chat.plan")}</span>
                    {planVisibleTodos.length > 0 && (
                      <span className={`font-mono text-xs ${planAllDone ? "text-emerald-500" : "text-neutral-400"}`}>
                        {planTodoDone}/{planVisibleTodos.length}
                      </span>
                    )}
                  </div>
                  {planVisibleTodos.length === 0 && planVisibleGoals.length === 0 ? (
                    <p className="py-4 text-center text-xs text-neutral-400">(暂无目标与待办)</p>
                  ) : (
                    planGroups.map((grp, gi) => {
                      const goalDone = grp.goal && grp.goal.status === "achieved";
                      return (
                        <div key={grp.goal ? grp.goal.id : `loose-${gi}`} className="mb-2 last:mb-0">
                          {grp.goal && (
                            <div
                              className={`flex items-center gap-2 text-[13px] ${
                                goalDone
                                  ? "text-emerald-600 dark:text-emerald-400"
                                  : "text-neutral-900 dark:text-neutral-100"
                              }`}
                            >
                              <span
                                className={`h-2 w-2 shrink-0 rounded-full ${
                                  goalDone
                                    ? "bg-emerald-500"
                                    : "border border-neutral-400 bg-white dark:border-neutral-400"
                                }`}
                              />
                              <span className="min-w-0 truncate font-medium" title={grp.goal.title}>
                                {grp.goal.title}
                              </span>
                            </div>
                          )}
                          <div className={`space-y-1 ${grp.goal ? "mt-1 pl-4" : ""}`}>
                            {grp.todos.map((td) => {
                              const done = td.status === "completed";
                              return (
                                <div
                                  key={td.id}
                                  className={`flex items-center gap-2 text-xs ${
                                    done
                                      ? "text-emerald-600/80 dark:text-emerald-400/80"
                                      : "text-neutral-600 dark:text-neutral-300"
                                  }`}
                                >
                                  <span
                                    className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                                      done
                                        ? "bg-emerald-500"
                                        : "border border-neutral-400 bg-white dark:border-neutral-400"
                                    }`}
                                  />
                                  <span className="min-w-0 truncate" title={td.content}>
                                    {td.content}
                                  </span>
                                </div>
                              );
                            })}
                          </div>
                        </div>
                      );
                    })
                  )}
                </div>
              )}
            </div>

            {/* 委派子代理：宿主调度入口（模型侧无该工具，只有宿主/用户能派活） */}
            <div className="relative shrink-0">
              <button
                onClick={() => setDelegateOpen((v) => !v)}
                disabled={!activeId}
                title={t("chat.delegate")}
                className={`shrink-0 rounded-full p-2 transition-colors disabled:opacity-40 ${
                  delegateOpen
                    ? "accent-solid"
                    : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:hover:bg-white/10 dark:hover:text-white"
                }`}
              >
                <IconQueue size={18} />
              </button>
              {delegateOpen && (
                <div className="card absolute bottom-11 left-0 z-20 w-80 p-2.5">
                  {/* 开始/暂停与设置页 auto_delegate 双向联动：开启=观察态（只读进度），关闭=未开启提示 */}
                  <div className="mb-1.5 flex items-center justify-between gap-2">
                    <p className="text-xs font-medium">{t("chat.delegateAutoTitle")}</p>
                    <PillSwitch
                      size="sm"
                      checked={autoDelegate}
                      onChange={toggleAutoDelegate}
                      title={autoDelegate ? t("chat.delegateAutoPause") : t("chat.delegateAutoStart")}
                    />
                  </div>
                  <p className="text-[11px] leading-relaxed text-neutral-500">
                    {autoDelegate ? t("chat.delegateAutoHint") : t("chat.delegateAutoOffHint")}
                  </p>
                  {autoDelegate ? (
                  <div className="mt-2 space-y-1.5 text-[11px]">
                    <div>
                      <p className="mb-0.5 text-neutral-500">{t("chat.delegateAutoGoals")}</p>
                      {delegateGoals.length === 0 ? (
                        <p className="text-neutral-400">{t("chat.delegateAutoEmpty")}</p>
                      ) : (
                        <ul className="space-y-0.5">
                          {delegateGoals.map((g) => {
                            const n = delegateTodos.filter((t) => t.goal_id === g.id).length;
                            return (
                              <li
                                key={g.id}
                                className="flex items-center justify-between gap-2 rounded-md px-1.5 py-0.5 hover:bg-neutral-900/5 dark:hover:bg-white/10"
                              >
                                <span className="truncate">{g.title}</span>
                                <span className="shrink-0 text-neutral-400">
                                  {n} {t("chat.delegateAutoTodos")}
                                </span>
                              </li>
                            );
                          })}
                        </ul>
                      )}
                    </div>
                    <div>
                      <p className="mb-0.5 text-neutral-500">{t("chat.delegateAutoRunning")}</p>
                      {subList.length === 0 ? (
                        <p className="text-neutral-400">{t("chat.delegateAutoEmpty")}</p>
                      ) : (
                        <ul className="space-y-0.5">
                          {subList.map((s) => {
                            const done = s.phase === "done";
                            const failed = s.phase === "error";
                            const running = !done && !failed;
                            return (
                              <li
                                key={s.session_id}
                                className="group rounded-md px-1.5 py-1 hover:bg-neutral-900/5 dark:hover:bg-white/10"
                              >
                                <div className="flex items-center justify-between gap-1.5">
                                  <span
                                    title={s.task || s.title || ""}
                                    className={`min-w-0 truncate font-medium ${
                                      failed
                                        ? "text-red-500"
                                        : done
                                        ? "text-emerald-600 dark:text-emerald-400"
                                        : "text-neutral-700 dark:text-neutral-200"
                                    }`}
                                  >
                                    {s.title || t("chat.subagent")}
                                  </span>
                                  <span className="shrink-0 text-[10px] text-neutral-400">
                                    {running ? (
                                      <button
                                        onClick={() => stopSubagent(s.session_id)}
                                        title={t("chat.stop")}
                                        className="inline-flex items-center gap-0.5 rounded px-1 py-0.5 text-neutral-400 hover:bg-red-500/10 hover:text-red-500"
                                      >
                                        <IconStop size={10} />
                                      </button>
                                    ) : failed ? (
                                      "✗"
                                    ) : (
                                      "✓"
                                    )}
                                  </span>
                                </div>
                                {s.task && (
                                  <p className="mt-0.5 truncate leading-snug text-neutral-400 dark:text-neutral-500" title={s.task}>
                                    {s.task}
                                  </p>
                                )}
                              </li>
                            );
                          })}
                        </ul>
                      )}
                    </div>
                  </div>
                ) : (
                  <p className="mt-2 text-[11px] leading-relaxed text-neutral-400">{t("chat.delegateAutoEmpty")}</p>
                )}
                </div>
              )}
            </div>
            {/* 后台运行任务状态：弹出 popover 查看长命令，可逐条停止 */}
            <div className="relative shrink-0">
              <button
                onClick={() => setBgJobsOpen((v) => !v)}
                title={t("chat.bgJobs")}
                className={`relative shrink-0 rounded-full p-2 transition-colors ${
                  bgJobsOpen
                    ? "accent-solid"
                    : shellJobs.jobs.length > 0
                      ? "bg-sky-500/15 text-sky-600 hover:bg-sky-500/25 dark:text-sky-400"
                      : "text-neutral-500 hover:bg-neutral-900/5 hover:text-neutral-900 dark:hover:bg-white/10 dark:hover:text-white"
                }`}
              >
                <IconTerminal size={18} />
                {shellJobs.jobs.length > 0 && (
                  <span className="absolute -right-0.5 -top-0.5 min-w-[16px] rounded-full bg-sky-500 px-1 text-[9px] font-bold leading-4 text-white">
                    {shellJobs.jobs.length}
                  </span>
                )}
              </button>
              {bgJobsOpen && (
                <div className="card absolute bottom-11 left-0 z-20 w-80 p-2.5">
                  <div className="mb-1.5 flex items-center justify-between">
                    <p className="flex items-center gap-1.5 text-xs font-medium">
                      <IconTerminal size={12} />
                      {t("chat.bgJobs")}
                    </p>
                    <span className="text-[10px] text-neutral-400">
                      {t("chat.bgJobsCount", { n: shellJobs.jobs.length })}
                    </span>
                  </div>
                  <p className="mb-1.5 text-[11px] leading-relaxed text-neutral-500">
                    {t("chat.bgJobsHint")}
                  </p>
                  {shellJobs.jobs.length === 0 ? (
                    <p className="py-4 text-center text-xs text-neutral-400">{t("chat.bgJobsEmpty")}</p>
                  ) : (
                    <ul className="space-y-1">
                      {shellJobs.jobs.map((j) => {
                        const s = Math.max(0, Math.floor((shellJobs.now - j.startedAt) / 1000));
                        const mm = Math.floor(s / 60);
                        const ss = s % 60;
                        const dur = mm > 0 ? `${mm}m${ss}s` : `${ss}s`;
                        return (
                          <li
                            key={j.job_id}
                            className="group flex items-center gap-1.5 rounded-md bg-neutral-100 px-2 py-1 text-[11px] dark:bg-neutral-800"
                          >
                            <IconTerminal size={11} className="shrink-0 animate-pulse text-sky-500" />
                            <code className="min-w-0 flex-1 truncate font-mono" title={j.command}>
                              {j.command}
                            </code>
                            <span className="shrink-0 tabular-nums text-neutral-400">{dur}</span>
                            <button
                              onClick={() => shellJobs.stopJob(j.job_id)}
                              disabled={shellJobs.stopping.includes(j.job_id)}
                              title={`${t("chat.stop")} ${j.job_id}`}
                              className="shrink-0 rounded p-0.5 text-neutral-400 hover:bg-red-500/10 hover:text-red-500 disabled:opacity-50"
                            >
                              {shellJobs.stopping.includes(j.job_id) ? "…" : <IconStop size={10} />}
                            </button>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </div>
              )}
            </div>
            </div>

            {/* 多行输入：随内容自动增高，超出后内部滚动；Enter 换行，Ctrl/Shift/Cmd+Enter 发送 */}
            <textarea
              ref={inputRef}
              rows={1}
              className="block w-full max-h-[160px] min-h-[80px] resize-none overflow-y-auto rounded-t-2xl bg-transparent px-4 pt-3 pb-2 text-sm text-neutral-900 outline-none transition-colors placeholder:text-neutral-400 dark:text-neutral-100 dark:placeholder:text-neutral-500"
              value={input}
              placeholder={
                !activeId
                  ? t("chat.selectFirst")
                  : busy
                    ? t("chat.busyPlaceholder")
                    : t("chat.inputPlaceholder")
              }
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                // Enter 直接换行便于拼接长消息；Ctrl+Enter / Shift+Enter 发送；中文输入法选词回车不发送
                if (
                  e.key === "Enter" &&
                  (e.ctrlKey || e.shiftKey || e.metaKey) &&
                  !e.nativeEvent.isComposing
                ) {
                  e.preventDefault();
                  send();
                }
              }}
              disabled={!activeId}
            />
            <div className="absolute bottom-2 right-2 z-10 flex items-center gap-1.5">
              {busy ? (
                <button
                  onClick={interrupt}
                  title={t("chat.stop")}
                  className="flex h-10 w-10 items-center justify-center rounded-full bg-red-500/10 text-red-600 transition-colors hover:bg-red-500/25 dark:text-red-400"
                >
                  <IconStop size={16} />
                </button>
              ) : (
                <button
                  onClick={send}
                  disabled={(!input.trim() && !hasAttachments) || !activeId}
                  title={t("common.send")}
                  className="accent-solid flex h-10 w-10 items-center justify-center rounded-full shadow-sm transition-all duration-200 hover:brightness-110 active:scale-95 disabled:opacity-40"
                >
                  <IconSend size={16} />
                </button>
              )}
            </div>
          </div>
        </div>

        {/* AI 接收内容预览 */}
        {previewOpen && preview && (
          <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
            onClick={() => setPreviewOpen(false)}
          >
            <div
              className="card flex max-h-[85vh] w-full max-w-2xl flex-col gap-3 overflow-hidden p-4"
              onClick={(e) => e.stopPropagation()}
            >
              <div className="flex items-center justify-between">
                <h3 className="font-semibold">{t("chat.preview")}</h3>
                <div className="flex items-center gap-3 text-xs text-neutral-400">
                  <span>
                    {preview.messages.length - 1} {t("chat.previewMsgsCount")}
                  </span>
                  <span>
                    {preview.tools.length} {t("chat.previewToolsShort")}
                  </span>
                  <span>
                    {t("chat.previewTokens")} ~{preview.est_tokens}
                  </span>
                  <button onClick={() => setPreviewOpen(false)} title={t("common.close")}>
                    <IconX size={14} />
                  </button>
                </div>
              </div>
              <div className="min-h-0 flex-1 space-y-3 overflow-y-auto">
                <div>
                  <p className="mb-1 text-xs font-semibold text-neutral-500">{t("chat.previewSystem")}</p>
                  <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded-xl bg-neutral-100 p-2.5 text-[11px] leading-relaxed dark:bg-neutral-900">
                    {preview.system}
                  </pre>
                </div>
                <div>
                  <p className="mb-1 text-xs font-semibold text-neutral-500">{t("chat.previewMsgs")}</p>
                  <div className="space-y-1">
                    {preview.messages.slice(1).map((m) => (
                      <div
                        key={m.index}
                        className="rounded-xl bg-neutral-100 px-2.5 py-1.5 text-[11px] dark:bg-neutral-900"
                      >
                        <span className="mr-2 font-mono font-semibold">{m.role}</span>
                        <span className="text-neutral-500">
                          {m.preview}
                          {m.content.length > m.preview.length ? "…" : ""}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
                <div>
                  <p className="mb-1 text-xs font-semibold text-neutral-500">{t("chat.previewTools")}</p>
                  <div className="flex flex-wrap gap-1">
                    {preview.tools.map((tl, i) => (
                      <span
                        key={i}
                        title={tl.description}
                        className="rounded-full border border-neutral-200 px-2 py-0.5 font-mono text-[10px] dark:border-neutral-700"
                      >
                        {tl.name}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

// 复制按钮：复制消息原文，成功短暂打勾（悬停气泡时浮现）
function CopyBtn({ text }) {
  const { t } = useLang();
  const [done, setDone] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setDone(true);
      setTimeout(() => setDone(false), 1200);
    } catch {
      // 剪贴板不可用（非安全上下文/无权限）时静默放弃
    }
  };
  return (
    <button
      onClick={copy}
      title={done ? t("common.copied") : t("common.copy")}
      className="icon-btn h-6 w-6 rounded-full opacity-0 transition-opacity group-hover:opacity-100"
    >
      {done ? <IconCheck size={12} /> : <IconCopy size={12} />}
    </button>
  );
}

// 思考过程折叠面板：流式期间默认展开（openDefault），历史消息默认折叠
function ThinkPanel({ text, openDefault = false, streaming = false }) {
  const [open, setOpen] = useState(openDefault);
  if (!text || !text.trim()) return null;
  return (
    <div className="w-full overflow-hidden rounded-2xl border border-neutral-200 bg-neutral-50 dark:border-neutral-800 dark:bg-neutral-900">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1 px-3 py-1.5 text-left text-xs text-neutral-500 dark:text-neutral-400"
      >
        <span className={`inline-block transition-transform ${open ? "rotate-90" : ""}`}>›</span>
        {streaming ? t("chat.thinking") : t("chat.thinkLabel")}
        {streaming && <span className="h-1 w-1 animate-pulse rounded-full bg-neutral-500" />}
      </button>
      {open && (
        <div className="max-h-56 overflow-y-auto whitespace-pre-wrap border-t border-neutral-200 px-3 py-2 text-xs leading-relaxed text-neutral-500 dark:border-neutral-800 dark:text-neutral-400">
          {text}
        </div>
      )}
    </div>
  );
}

// 单条消息气泡：user / assistant，assistant 可携带工具调用卡片
// send_file 成功的调用渲染为文件卡片（如同收文件），不出工具卡
function MessageBubble({ message }) {
  const isUser = message.role === "user";
  // 后台 shell 结束时由宿主注入的说明消息（用户没说话，别渲染成用户的气泡）。
  // 三种结束都带 [后台任务…] 前缀：完成 / 已手动停止 / 超时终止
  const isBgDone =
    isUser && typeof message.content === "string" && message.content.startsWith("[后台任务");
  const calls = message.tool_calls || [];
  const fileCalls = calls.filter((c) => c.tool === "send_file" && c.ok);
  const toolCalls = calls.filter((c) => !(c.tool === "send_file" && c.ok));
  const hasText = message.content && message.content.trim().length > 0;

  if (isBgDone) {
    return (
      <div className="group mr-auto flex max-w-[85%] flex-col gap-0.5">
        <div className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-2xl border border-dashed border-neutral-300 bg-neutral-50 px-3 py-2 font-mono text-[11px] leading-relaxed text-neutral-500 dark:border-neutral-700 dark:bg-neutral-800/60 dark:text-neutral-400">
          {message.content}
        </div>
        <CopyBtn text={message.content || ""} />
      </div>
    );
  }

  if (isUser) {
    return (
      <div className="group ml-auto flex max-w-[80%] flex-col items-end gap-0.5">
        <div className="accent-solid whitespace-pre-wrap rounded-3xl rounded-br-lg px-4 py-2.5 text-sm leading-relaxed">
          {message.content}
        </div>
        <CopyBtn text={message.content || ""} />
      </div>
    );
  }

  return (
    <div className="mr-auto flex max-w-[85%] flex-col gap-2">
      {toolCalls.length > 0 && (
        <div className="flex flex-col gap-1.5">
          {toolCalls.map((c, i) => (
            <ToolCallCard key={i} call={c} />
          ))}
        </div>
      )}
      {message.thinking && message.thinking.trim() && <ThinkPanel text={message.thinking} />}
      {hasText && (
        <div className="group flex flex-col gap-0.5">
          <div className="rounded-3xl rounded-bl-lg border border-neutral-200 bg-white px-4 py-2.5 dark:border-neutral-800 dark:bg-neutral-900">
            <Markdown>{message.content}</Markdown>
          </div>
          <CopyBtn text={message.content || ""} />
        </div>
      )}
      {/* 文件卡片放在文字之后：先说话后发文件，如同聊天里收到附件 */}
      {fileCalls.length > 0 && (
        <div className="flex flex-col gap-1.5">
          {fileCalls.map((c, i) => (
            <FileCard
              key={i}
              path={c.params?.path}
              bytes={c.result?.bytes}
              note={c.params?.note}
            />
          ))}
        </div>
      )}
    </div>
  );
}
