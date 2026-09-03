import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n.js";
import { IconGlobe, IconCheck, IconRefresh } from "../components/Icons.jsx";

// 远程访问：端口/Client Key/访问密码管理，测试通过才可保存
export default function RemotePage({ onStats }) {
  const { t } = useLang();
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
  const [cli, setCli] = useState(null); // {path, hint} | {error}
  const [cliBusy, setCliBusy] = useState(false);

  const installCli = async () => {
    setCliBusy(true);
    setCli(null);
    try {
      setCli(await api.installCli());
    } catch (e) {
      setCli({ error: String(e) });
    } finally {
      setCliBusy(false);
    }
  };

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
    // 端口冲突预检测：仅当端口发生变化时检测（服务自身监听中的端口不算冲突）
    if (changed && Number(port) !== cfg.port) {
      try {
        const p = await api.checkPort(host.trim() || "127.0.0.1", Number(port));
        if (!p.available) {
          setTestState({
            error: `${t("remote.portConflict")}${p.addr} ${p.reason || t("remote.portInUse")}${t("remote.portConflictFix")}`,
          });
          return null;
        }
      } catch (e) {
        setTestState({ error: String(e) });
        return null;
      }
    }
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
      setError(t("remote.saveRequiresTest"));
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
        <h2 className="text-lg font-semibold">{t("remote.title")}</h2>
        <p className="text-xs text-neutral-500">
          {t("remote.subtitle")}{cfg.revision}{t("remote.subtitleSuffix")}
        </p>
      </div>

      <div className="card flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <div>
            <p className="font-medium">{t("remote.enableTitle")}</p>
            <p className="text-xs text-neutral-500">{t("remote.enableDesc")}</p>
          </div>
          {/* 小圆片开关 */}
          <button
            onClick={() => {
              setEnabled((v) => !v);
              setTestState(null);
            }}
            className={`relative h-7 w-14 rounded-full border transition-colors ${
              enabled
                ? "accent-solid"
                : "border-neutral-300 bg-neutral-200 dark:border-neutral-700 dark:bg-neutral-800"
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
            <label className="mb-1 block px-2 text-xs text-neutral-500">{t("remote.listenHost")}</label>
            <input className="field" value={host} onChange={(e) => { setHost(e.target.value); setTestState(null); }}
              placeholder={t("remote.hostPlaceholder")} />
          </div>
          <div>
            <label className="mb-1 block px-2 text-xs text-neutral-500">{t("remote.port")}</label>
            <input className="field" value={port} onChange={(e) => { setPort(e.target.value); setTestState(null); }}
              placeholder="8600" />
          </div>
        </div>

        <div>
          <label className="mb-1 block px-2 text-xs text-neutral-500">{t("remote.clientKeyLabel")}</label>
          <div className="flex gap-2">
            <input className="field flex-1 font-mono" value={cfg.client_key} readOnly />
            <button onClick={() => copy(cfg.client_key, "key")} className="pill pill-outline pill-hover shrink-0">
              {copied === "key" ? <IconCheck size={14} /> : <IconGlobe size={14} />}
              {copied === "key" ? t("common.copied") : t("common.copy")}
            </button>
            <button onClick={rotateKey} className="pill pill-outline pill-hover shrink-0">
              <IconRefresh size={14} />
              {t("remote.rotate")}
            </button>
          </div>
        </div>

        {/* 访问密码（第二重认证） */}
        <div className="rounded-2xl border border-neutral-200 bg-neutral-50 p-4 dark:border-neutral-800 dark:bg-neutral-900">
          <div className="mb-3 flex items-center justify-between">
            <div>
              <p className="font-medium">{t("remote.pwdTitle")}</p>
              <p className="text-xs text-neutral-500">
                {t("remote.pwdDesc")}
              </p>
            </div>
            {/* 密码启用小圆片开关 */}
            <button
              onClick={() => setPwdEnabled((v) => !v)}
              className={`relative h-7 w-14 shrink-0 rounded-full border transition-colors ${
                pwdEnabled
                  ? "accent-solid"
                  : "border-neutral-300 bg-white dark:border-neutral-700 dark:bg-neutral-800"
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
              {showPwd ? t("common.hide") : t("common.show")}
            </button>
            <button onClick={() => copy(pwd, "pwd")} className="pill pill-outline pill-hover shrink-0">
              {copied === "pwd" ? <IconCheck size={14} /> : <IconGlobe size={14} />}
              {copied === "pwd" ? t("common.copied") : t("common.copy")}
            </button>
            <button onClick={rotatePwd} className="pill pill-outline pill-hover shrink-0">
              <IconRefresh size={14} />
              {t("remote.rotate")}
            </button>
          </div>

          <div className="mt-3 flex gap-2">
            <input
              className="field flex-1"
              type={showPwd ? "text" : "password"}
              placeholder={t("remote.pwdPlaceholder")}
              value={pwdInput}
              onChange={(e) => setPwdInput(e.target.value)}
            />
            <button onClick={savePwd} className="pill pill-hover shrink-0">
              {t("remote.savePwd")}
            </button>
          </div>
          {pwdMsg && <p className="mt-2 px-2 text-xs text-red-600">{pwdMsg}</p>}
        </div>

        {testState === "pass" && (
          <p className="flex items-center gap-2 rounded-full bg-neutral-100 px-4 py-2 text-xs">
            <IconCheck size={14} />
            {t("remote.testPass")}http://{cfg.host}:{cfg.port}
          </p>
        )}
        {testState?.error && (
          <p className="rounded-full bg-red-50 px-4 py-2 text-xs text-red-600">{testState.error}</p>
        )}
        {error && <p className="px-2 text-xs text-red-600">{error}</p>}

        <div className="flex justify-end gap-2">
          <button onClick={test} className="pill pill-outline pill-hover">
            <IconGlobe size={14} />
            {t("remote.test")}
          </button>
          <button onClick={save} disabled={!changed || (testState !== "pass" && changed)} className="pill pill-hover">
            {t("common.save")}
          </button>
        </div>
      </div>

      <div className="card">
        <p className="mb-3 font-medium">{t("remote.openaiTitle")}</p>
        <p className="mb-3 text-xs text-neutral-500">
          {t("remote.openaiDesc")}
        </p>
        <p className="mb-3 rounded-xl bg-neutral-100 px-3 py-2 text-xs text-neutral-500 dark:bg-neutral-900">
          {t("remote.mcpNote")}
        </p>
        <div className="mb-2 flex gap-2">
          <input className="field flex-1 font-mono" readOnly value={`http://${cfg.host}:${cfg.port}/v1`} />
          <button onClick={() => copy(`http://${cfg.host}:${cfg.port}/v1`, "baseurl")} className="pill pill-outline pill-hover shrink-0">
            {copied === "baseurl" ? <IconCheck size={14} /> : <IconGlobe size={14} />}
            {copied === "baseurl" ? t("common.copied") : t("common.copy")}
          </button>
        </div>
        <pre className="overflow-x-auto rounded-2xl bg-neutral-900 p-4 font-mono text-xs leading-relaxed text-neutral-100">{`# ${t("remote.curlListModels")}
curl http://${cfg.host}:${cfg.port}/v1/models \\
  -H "Authorization: Bearer ${cfg.client_key}"

# ${t("remote.curlChat")}
curl -X POST http://${cfg.host}:${cfg.port}/v1/chat/completions \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"bit","messages":[{"role":"user","content":"${t("remote.curlChatContent")}"}]}'`}</pre>
      </div>

      <div className="card">
        <p className="mb-3 font-medium">{t("remote.agentTitle")}</p>
        <pre className="overflow-x-auto rounded-2xl bg-neutral-900 p-4 font-mono text-xs leading-relaxed text-neutral-100">{`# ${t("remote.curlHealth")}
curl http://${cfg.host}:${cfg.port}/api/health

# ${t("remote.curlRegister")}
curl -X POST http://${cfg.host}:${cfg.port}/api/tools \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -H "Content-Type: application/json" \\
  -d '{"name":"my_tool","description":"...","url":"http://agent:9000/callback"}'

# ${t("remote.curlInvoke")}
curl -X POST http://${cfg.host}:${cfg.port}/api/tools/<id>/invoke \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -d '{"params":{}}'

# ${t("remote.curlAIChat")}
curl -X POST http://${cfg.host}:${cfg.port}/api/chat \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -d '{"message":"${t("remote.curlAIChatContent")}"}'`}</pre>
      </div>

      {/* 终端命令：安装 bit 命令到 PATH，bit tui 进入简约终端模式 */}
      <div className="card">
        <p className="mb-3 font-medium">{t("remote.cliTitle")}</p>
        <p className="mb-3 text-xs text-neutral-500">{t("remote.cliDesc")}</p>
        <div className="flex items-center gap-3">
          <button onClick={installCli} disabled={cliBusy} className="pill pill-hover shrink-0">
            {cliBusy ? "..." : t("remote.cliInstall")}
          </button>
          {cli && !cli.error && (
            <code className="truncate font-mono text-xs text-neutral-500">{cli.path}</code>
          )}
          {cli?.error && <span className="text-xs text-red-500">{cli.error}</span>}
        </div>
        {cli && !cli.error && cli.hint && (
          <p className="mt-2 text-xs text-neutral-500">{cli.hint}</p>
        )}
      </div>
    </div>
  );
}
