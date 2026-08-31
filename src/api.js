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
  chat: (session_id, message) => invoke("chat", { sessionId: session_id, message }),
  // 流式对话：过程通过 Tauri 事件 event_name 推送增量，onEvent 收到 {type, ...} payload。
  // 返回一个 Promise，resolve 为最终完整消息列表；调用方应先订阅事件再 await。
  chatStream: async (session_id, message, event_name, onEvent) => {
    const ev = event_name || `chat-stream-${Date.now()}`;
    const unlisten = await listen(ev, (e) => onEvent?.(e.payload));
    try {
      return await invoke("chat_stream", { sessionId: session_id, message, eventName: ev });
    } finally {
      unlisten();
    }
  },
  listSessions: () => invoke("list_sessions"),
  getSession: (session_id) => invoke("get_session", { sessionId: session_id || "" }),
  createSession: (title) => invoke("create_session", { title: title || "" }),
  setActiveSession: (session_id) => invoke("set_active_session", { sessionId: session_id }),
  renameSession: (session_id, title) => invoke("rename_session", { sessionId: session_id, title }),
  deleteSession: (session_id) => invoke("delete_session", { sessionId: session_id }),
  clearSession: (session_id) => invoke("clear_session", { sessionId: session_id || "" }),
  listMemories: () => invoke("list_memories"),
  addMemory: (content) => invoke("add_memory", { content }),
  listSkills: () => invoke("list_skills"),
  addSkill: (name, summary) => invoke("add_skill", { name, summary }),
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
