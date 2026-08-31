import { useEffect, useState } from "react";
import { api } from "../api.js";
import { IconGlobe, IconCheck, IconRefresh } from "../components/Icons.jsx";

// 远程访问：端口/Client Key/访问密码管理，测试通过才可保存
export default function RemotePage({ onStats }) {
  const [cfg, setCfg] = useState(null);
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [enabled, setEnabled] = useState(false);
  const [pwd, setPwd] = useState("");
  const [pwdInput, setPwdInput] = useState("");
  const [pwdEnabled, setPwdEnabled] = useState(true);
  const [showPwd, setShowPwd] = useState(false);
  const [testState, setTestState] = useState(null); // null | 'pass' | {error}
  const [error, setError] = useState("");
  const [copied, setCopied] = useState("");
  const [pwdMsg, setPwdMsg] = useState("");

  useEffect(() => {
    api.getRemoteConfig().then((c) => {
      setCfg(c);
      setHost(c.host);
      setPort(String(c.port));
      setEnabled(c.remote_enabled);
      setPwd(c.access_password || "");
      setPwdEnabled(c.password_enabled !== false);
    });
  }, []);

  const changed =
    cfg && (host.trim() !== cfg.host || Number(port) !== cfg.port || enabled !== cfg.remote_enabled);

  const test = async () => {
    setTestState(null);
    setError("");
    try {
      const r = await api.saveRemoteConfig(enabled, host, Number(port));
      setTestState("pass");
      const c = await api.getRemoteConfig();
      setCfg(c);
      onStats?.();
      return r;
    } catch (e) {
      setTestState({ error: String(e) });
      return null;
    }
  };

  const save = async () => {
    if (testState !== "pass" && changed) {
      setError("请先通过连接测试再保存");
      return;
    }
    setError("");
    try {
      await api.saveRemoteConfig(enabled, host, Number(port));
      const c = await api.getRemoteConfig();
      setCfg(c);
      onStats?.();
    } catch (e) {
      setError(String(e));
    }
  };

  const rotateKey = async () => {
    await api.regenerateClientKey();
    const c = await api.getRemoteConfig();
    setCfg(c);
    onStats?.();
  };

  const rotatePwd = async () => {
    setPwdMsg("");
    try {
      const r = await api.regenerateAccessPassword();
      setPwd(r.access_password);
      setPwdInput("");
      const c = await api.getRemoteConfig();
      setCfg(c);
      onStats?.();
    } catch (e) {
      setPwdMsg(String(e));
    }
  };

  const savePwd = async () => {
    setPwdMsg("");
    try {
      await api.saveAccessPassword(pwdInput, pwdEnabled);
      const c = await api.getRemoteConfig();
      setPwd(c.access_password || "");
      setPwdInput("");
      setCfg(c);
      onStats?.();
    } catch (e) {
      setPwdMsg(String(e));
    }
  };

  const copy = async (text, tag) => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(tag);
      setTimeout(() => setCopied(""), 1500);
    } catch {
      // 剪贴板不可用时静默失败
    }
  };

  if (!cfg) return null;

  return (
    <div className="flex h-full flex-col gap-4 overflow-y-auto">
      <div>
        <h2 className="text-lg font-semibold">远程访问</h2>
        <p className="text-xs text-neutral-500">
          双重认证：Client Key + 访问密码 · 配置版本 v{cfg.revision}（保存自动递增）
        </p>
      </div>

      <div className="card flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <div>
            <p className="font-medium">启用远程访问</p>
            <p className="text-xs text-neutral-500">关闭后立即停止 HTTP 服务</p>
          </div>
          {/* 小圆片开关 */}
          <button
            onClick={() => {
              setEnabled((v) => !v);
              setTestState(null);
            }}
            className={`relative h-7 w-14 rounded-full border transition-colors ${
              enabled ? "border-neutral-900 bg-neutral-900" : "border-neutral-300 bg-neutral-200"
            }`}
          >
            <span
              className={`absolute top-1/2 h-5 w-5 -translate-y-1/2 rounded-full bg-white shadow transition-all ${
                enabled ? "left-8" : "left-1"
              }`}
            />
          </button>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="mb-1 block px-2 text-xs text-neutral-500">监听地址</label>
            <input className="field" value={host} onChange={(e) => { setHost(e.target.value); setTestState(null); }}
              placeholder="0.0.0.0 或 127.0.0.1" />
          </div>
          <div>
            <label className="mb-1 block px-2 text-xs text-neutral-500">端口</label>
            <input className="field" value={port} onChange={(e) => { setPort(e.target.value); setTestState(null); }}
              placeholder="8600" />
          </div>
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">Client Key（自动生成，无需手填）</label>
          <div className="flex gap-2">
            <input className="field flex-1 font-mono" value={cfg.client_key} readOnly />
            <button onClick={() => copy(cfg.client_key, "key")} className="pill pill-outline pill-hover shrink-0">
              {copied === "key" ? <IconCheck size={14} /> : <IconGlobe size={14} />}
              {copied === "key" ? "已复制" : "复制"}
            </button>
            <button onClick={rotateKey} className="pill pill-outline pill-hover shrink-0">
              <IconRefresh size={14} />
              轮换
            </button>
          </div>
        </div>

        {/* 访问密码（第二重认证） */}
        <div className="rounded-2xl border border-neutral-200 bg-neutral-50 p-4">
          <div className="mb-3 flex items-center justify-between">
            <div>
              <p className="font-medium">访问密码（第二重认证）</p>
              <p className="text-xs text-neutral-500">
                请求需额外携带 X-Access-Password 头，8 位数字默认自动生成
              </p>
            </div>
            {/* 密码启用小圆片开关 */}
            <button
              onClick={() => setPwdEnabled((v) => !v)}
              className={`relative h-7 w-14 shrink-0 rounded-full border transition-colors ${
                pwdEnabled ? "border-neutral-900 bg-neutral-900" : "border-neutral-300 bg-white"
              }`}
            >
              <span
                className={`absolute top-1/2 h-5 w-5 -translate-y-1/2 rounded-full bg-white shadow transition-all ${
                  pwdEnabled ? "left-8 border border-neutral-300" : "left-1 bg-neutral-400"
                }`}
              />
            </button>
          </div>

          <div className="flex gap-2">
            <input
              className="field flex-1 font-mono"
              type={showPwd ? "text" : "password"}
              value={pwd}
              readOnly
            />
            <button onClick={() => setShowPwd((v) => !v)} className="pill pill-outline pill-hover shrink-0">
              {showPwd ? "隐藏" : "显示"}
            </button>
            <button onClick={() => copy(pwd, "pwd")} className="pill pill-outline pill-hover shrink-0">
              {copied === "pwd" ? <IconCheck size={14} /> : <IconGlobe size={14} />}
              {copied === "pwd" ? "已复制" : "复制"}
            </button>
            <button onClick={rotatePwd} className="pill pill-outline pill-hover shrink-0">
              <IconRefresh size={14} />
              轮换
            </button>
          </div>

          <div className="mt-3 flex gap-2">
            <input
              className="field flex-1"
              type={showPwd ? "text" : "password"}
              placeholder="自定义新密码（4-64 位，留空则自动生成）"
              value={pwdInput}
              onChange={(e) => setPwdInput(e.target.value)}
            />
            <button onClick={savePwd} className="pill pill-hover shrink-0">
              保存密码
            </button>
          </div>
          {pwdMsg && <p className="mt-2 px-2 text-xs text-red-600">{pwdMsg}</p>}
        </div>

        {testState === "pass" && (
          <p className="flex items-center gap-2 rounded-full bg-neutral-100 px-4 py-2 text-xs">
            <IconCheck size={14} />
            连接测试通过（含双重认证），服务运行于 http://{cfg.host}:{cfg.port}
          </p>
        )}
        {testState?.error && (
          <p className="rounded-full bg-red-50 px-4 py-2 text-xs text-red-600">{testState.error}</p>
        )}
        {error && <p className="px-2 text-xs text-red-600">{error}</p>}

        <div className="flex justify-end gap-2">
          <button onClick={test} className="pill pill-outline pill-hover">
            <IconGlobe size={14} />
            测试连接
          </button>
          <button onClick={save} disabled={!changed || (testState !== "pass" && changed)} className="pill pill-hover">
            保存
          </button>
        </div>
      </div>

      <div className="card">
        <p className="mb-3 font-medium">Agent 接入示例（双重认证）</p>
        <pre className="overflow-x-auto rounded-2xl bg-neutral-900 p-4 font-mono text-xs leading-relaxed text-neutral-100">{`# 健康检查（无需认证）
curl http://${cfg.host}:${cfg.port}/api/health

# 注册工具（Agent 自助，需 Key + 密码）
curl -X POST http://${cfg.host}:${cfg.port}/api/tools \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -H "Content-Type: application/json" \\
  -d '{"name":"my_tool","description":"...","url":"http://agent:9000/callback"}'

# 调用工具
curl -X POST http://${cfg.host}:${cfg.port}/api/tools/<id>/invoke \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -d '{"params":{}}'

# AI 对话（含自写插件能力）
curl -X POST http://${cfg.host}:${cfg.port}/api/chat \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -d '{"message":"帮我写一个查天气的插件"}'`}</pre>
      </div>
    </div>
  );
}
