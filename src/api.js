import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const api = {
  overview: () => invoke("get_overview"),
  listTools: () => invoke("list_tools"),
  registerTool: (name, description, url) => invoke("register_tool", { name, description, url }),
  registerScriptTool: (name, description, runtime, code) =>
    invoke("register_script_tool", { name, description, runtime, code }),
  removeTool: (id) => invoke("remove_tool", { id }),
  setToolEnabled: (id, enabled) => invoke("set_tool_enabled", { id, enabled }),
  invokeTool: (id, params) => invoke("invoke_tool", { id, params }),
  listRuntimes: () => invoke("list_runtimes"),
  refreshRuntimes: () => invoke("refresh_runtimes"),
  addRuntime: (id, name, path, lang) => invoke("add_runtime", { id, name, path, lang }),
  removeRuntime: (id) => invoke("remove_runtime", { id }),
  setRuntimeEnabled: (id, enabled) => invoke("set_runtime_enabled", { id, enabled }),
  runScript: (runtime, code, params) => invoke("run_script", { runtime, code, params }),
  listAudit: () => invoke("list_audit"),
  getRemoteConfig: () => invoke("get_remote_config"),
  saveRemoteConfig: (remote_enabled, host, port) =>
    invoke("save_remote_config", { remote_enabled, host, port }),
  regenerateClientKey: () => invoke("regenerate_client_key"),
  saveAccessPassword: (password, password_enabled) =>
    invoke("save_access_password", { password, password_enabled }),
  regenerateAccessPassword: () => invoke("regenerate_access_password"),
  testConnectivity: () => invoke("test_connectivity"),
  listProviders: () => invoke("list_providers"),
  addProvider: (name, protocol, base_url, api_key, model) =>
    invoke("add_provider", { name, protocol, baseUrl: base_url, apiKey: api_key, model }),
  updateProvider: (id, name, protocol, base_url, api_key, model) =>
    invoke("update_provider", { id, name, protocol, baseUrl: base_url, apiKey: api_key, model }),
  removeProvider: (id) => invoke("remove_provider", { id }),
  setProviderActive: (id, active) => invoke("set_provider_active", { id, active }),
  // 模型采样参数：temperature null=默认（0-2）；reasoningEffort ""=默认 / low / medium / high
  getAiParams: () => invoke("get_ai_params"),
  setAiParams: (temperature, reasoning_effort) =>
    invoke("set_ai_params", { temperature, reasoningEffort: reasoning_effort }),
  // 从提供方 API 拉取可用模型列表（OpenAI /models、Gemini /v1beta/models、Claude /v1/models）
  listProviderModels: (protocol, base_url, api_key) =>
    invoke("list_provider_models", { protocol, baseUrl: base_url, apiKey: api_key }),
  chat: (session_id, message, images) =>
    invoke("chat", { sessionId: session_id, message, images: images || null }),
  // 流式对话：过程通过 Tauri 事件 event_name 推送增量，onEvent 收到 {type, ...} payload。
  // images 为可选的图片 base64 data URL 数组，仅随当前用户轮发给多模态模型。
  // 返回一个 Promise，resolve 为最终完整消息列表；调用方应先订阅事件再 await。
  chatStream: async (session_id, message, event_name, onEvent, images) => {
    const ev = event_name || `chat-stream-${Date.now()}`;
    const unlisten = await listen(ev, (e) => onEvent?.(e.payload));
    try {
      return await invoke("chat_stream", {
        sessionId: session_id,
        message,
        eventName: ev,
        images: images || null,
      });
    } finally {
      unlisten();
    }
  },
  // 解析上传文件：Excel→Markdown 表格 / Word(.docx)→纯文本 / CSV→原文。data 为 base64（可含 data:URL 前缀）
  extractFile: (filename, data) => invoke("extract_file", { filename, data }),
  // 用系统默认程序打开文件；reveal=true 时打开所在文件夹并定位
  openPath: (path, reveal) => invoke("open_path", { path, reveal: !!reveal }),
  // 抓取网页正文，返回 { title, text }
  fetchWebpage: (url) => invoke("fetch_webpage", { url }),
  // 端口冲突检测：返回 { available, addr, reason? }
  checkPort: (host, port) => invoke("check_port", { host, port }),
  // 手动压缩会话：AI 总结全部历史为一条摘要，返回新消息列表
  compressSession: (session_id) => invoke("compress_session", { sessionId: session_id || "" }),

  // ── MCP（Model Context Protocol）接入 ──
  mcpDiscover: (host, start, end) => invoke("mcp_discover", { host, start, end }),
  mcpConnect: (url) => invoke("mcp_connect", { url }),
  mcpList: () => invoke("mcp_list"),
  mcpToggle: (id, enabled) => invoke("mcp_toggle", { id, enabled }),
  mcpRemove: (id) => invoke("mcp_remove", { id }),
  mcpImport: (id) => invoke("mcp_import", { id }),

  // ── 中断 / 工具审批 / 上下文预览 ──
  chatInterrupt: (session_id) => invoke("chat_interrupt", { sessionId: session_id || "" }),
  toolApprove: (id, allow) => invoke("tool_approve", { id, allow }),
  setToolApproval: (mode) => invoke("set_tool_approval", { mode }),
  getToolApproval: () => invoke("get_tool_approval"),
  contextPreview: (session_id) => invoke("context_preview", { sessionId: session_id || "" }),
  contextMetrics: (session_id) => invoke("context_metrics", { sessionId: session_id || "" }),
  listSessions: () => invoke("list_sessions"),
  getSession: (session_id) => invoke("get_session", { sessionId: session_id || "" }),
  createSession: (title) => invoke("create_session", { title: title || "" }),
  setActiveSession: (session_id) => invoke("set_active_session", { sessionId: session_id }),
  renameSession: (session_id, title) => invoke("rename_session", { sessionId: session_id, title }),
  deleteSession: (session_id) => invoke("delete_session", { sessionId: session_id }),
  clearSession: (session_id) => invoke("clear_session", { sessionId: session_id || "" }),
  listMemories: () => invoke("list_memories"),
  addMemory: (content) => invoke("add_memory", { content }),
  deleteMemories: (ids) => invoke("delete_memories", { ids }),
  listSkills: () => invoke("list_skills"),
  addSkill: (name, summary) => invoke("add_skill", { name, summary }),
  deleteSkills: (ids) => invoke("delete_skills", { ids }),
  listGoals: () => invoke("list_goals"),
  createGoal: (title, detail) => invoke("create_goal", { title, detail }),
  updateGoalStatus: (id, status) => invoke("update_goal_status", { id, status }),
  removeGoal: (id) => invoke("remove_goal", { id }),
  listTodos: () => invoke("list_todos"),
  addTodo: (content, goal_id) => invoke("add_todo", { content, goal_id }),
  updateTodoStatus: (id, status) => invoke("update_todo_status", { id, status }),
  removeTodo: (id) => invoke("remove_todo", { id }),
  quitApp: () => invoke("quit_app"),
};
