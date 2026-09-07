import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n.js";
import { IconGlobe, IconCheck, IconRefresh } from "../components/Icons.jsx";

// STUN SocketAddr 形态："[v6]:p" / "v4:p"；只取 IP 部分（实际端口由 BIT 监听口决定）
const ipOnly = (s) => {
  const x = String(s || "");
  if (x.startsWith("[")) return x.substring(1, x.indexOf("]"));
  return /:\d+$/.test(x) ? x.substring(0, x.lastIndexOf(":")) : x;
};

// 远程访问：端口/Client Key/访问密码管理，测试通过才可保存
export default function RemotePage({ onStats }) {
  const { t } = useLang();
  const natLabel = {
    none: t("remote.qrNatNone"),
    cone: t("remote.qrNatCone"),
    symmetric: t("remote.qrNatSymmetric"),
  };
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
  // 扫码连接二维码：payload 含全部连接信息（地址候选/端口/密钥/密码/会话/云中继）
  const [qr, setQr] = useState(null);
  const [qrBusy, setQrBusy] = useState(false);
  const [qrErr, setQrErr] = useState("");
  const [cloudInput, setCloudInput] = useState("https://osbt.space");
  const [cloudSaved, setCloudSaved] = useState(false);
  const [stunInput, setStunInput] = useState("");
  const [stunSaved, setStunSaved] = useState(false);

  const loadQr = async () => {
    setQrBusy(true);
    setQrErr("");
    try {
      setQr(await api.getRemoteQr());
    } catch (e) {
      setQr(null);
      setQrErr(String(e));
    } finally {
      setQrBusy(false);
    }
  };

  const saveCloud = async () => {
    setQrErr("");
    try {
      await api.saveCloudRelay(cloudInput);
      setCloudSaved(true);
      setTimeout(() => setCloudSaved(false), 1500);
      await loadQr(); // 云中继进入二维码 payload
    } catch (e) {
      setQrErr(String(e));
    }
  };

  // 自定 STUN 列表：逗号分隔，留空恢复内置免费列表；保存后重探 NAT 进二维码
  const saveStun = async () => {
    setQrErr("");
    try {
      const list = stunInput.split(/[,;，；]/).map((s) => s.trim()).filter(Boolean);
      await api.saveStunServers(list);
      setStunSaved(true);
      setTimeout(() => setStunSaved(false), 1500);
      await loadQr();
    } catch (e) {
      setQrErr(String(e));
    }
  };

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
      setCloudInput(c.cloud_relay_url || "");
      setStunInput((c.stun_servers || []).join(", "));
    });
    loadQr();
  }, []);

  const changed =
    cfg && (host.trim() !== cfg.host || Number(port) !== cfg.port || enabled !== cfg.remote_enabled);

  // IPv6 地址的 URL 形态：加方括号（http://[::1]:8600），IPv4/域名原样
  const base = (() => {
    const h = String(cfg?.host || "");
    const hp = h.includes(":") ? `[${h.replace(/^\[|\]$/g, "")}]:${cfg?.port}` : `${h}:${cfg?.port}`;
    return `http://${hp}`;
  })();

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
      // 输入框重新同步：后端可能因端口被占用已自动切换端口
      setHost(c.host);
      setPort(String(c.port));
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
          <p className="flex items-center gap-2 rounded-full bg-neutral-100 px-4 py-2 text-xs text-neutral-800 dark:bg-neutral-800 dark:text-neutral-200">
            <IconCheck size={14} />
            {t("remote.testPass")}{base}
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

      {/* 扫码连接：二维码固定黑码白底（扫码可靠性优先于主题），深浅主题下均放在白色圆角面板内 */}
      <div className="card flex flex-col gap-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="font-medium">{t("remote.qrTitle")}</p>
            <p className="mt-0.5 text-xs text-neutral-500">{t("remote.qrDesc")}</p>
          </div>
          <button onClick={loadQr} disabled={qrBusy} className="pill pill-outline pill-hover shrink-0">
            <IconRefresh size={14} />
            {qrBusy ? "…" : t("remote.qrRefresh")}
          </button>
        </div>

        <div className="flex flex-col gap-4 sm:flex-row">
          <div className="mx-auto shrink-0 rounded-2xl border border-neutral-200 bg-white p-3 dark:border-neutral-700 sm:mx-0">
            {qr?.svg ? (
              <div className="h-44 w-44 aspect-square [&>svg]:block [&>svg]:h-full [&>svg]:w-full" dangerouslySetInnerHTML={{ __html: qr.svg }} />
            ) : (
              <div className="flex h-44 w-44 items-center justify-center px-3 text-center text-xs text-neutral-400">
                {qrErr ? t("remote.qrProbeFail") : t("remote.qrGenerate")}
              </div>
            )}
          </div>

          <div className="min-w-0 flex-1 space-y-2.5 text-xs">
            {qr?.payload && (
              <>
                <p className="flex flex-wrap items-center gap-2">
                  <span
                    className={`chip ${
                      ["cone", "none"].includes(qr.payload.nat)
                        ? "border-emerald-500/50 text-emerald-600 dark:text-emerald-400"
                        : ""
                    }`}
                  >
                    {natLabel[qr.payload.nat] || t("remote.qrNatUnknown")}
                  </span>
                  {qr.payload.rid && (
                    <span className="chip font-mono" title={t("remote.qrRidTip")}>
                      {t("remote.qrRid")}: {String(qr.payload.rid).slice(0, 8)}…
                    </span>
                  )}
                </p>
                <div className="min-w-0 space-y-2">
                  {/* 方式一：局域网直连 */}
                  <div className="min-w-0">
                    <p className="mb-1 font-medium text-neutral-600 dark:text-neutral-300">
                      {t("remote.qrMethodLan")}
                    </p>
                    <div className="space-y-0.5 font-mono text-[11px] text-neutral-500">
                      {(() => {
                        const urls = qr.payload.methods?.lan?.length
                          ? qr.payload.methods.lan
                          : (qr.payload.addrs?.lan || []).map((a) => `http://${a}:${qr.payload.port}`);
                        return urls.length ? urls.map((u) => <p key={`lan-${u}`} className="truncate">{u}</p>)
                          : <p className="text-neutral-400">{t("remote.qrNone")}</p>;
                      })()}
                    </div>
                  </div>
                  {/* 方式二：IPv6 直连（NAT1 / 锥形 NAT 且有全球 v6） */}
                  <div className="min-w-0">
                    <p className="mb-1 font-medium text-neutral-600 dark:text-neutral-300">
                      {t("remote.qrMethodDirect6")}
                    </p>
                    <div className="space-y-0.5 font-mono text-[11px] text-neutral-500">
                      {(() => {
                        const urls = qr.payload.methods?.direct6 || [];
                        return urls.length ? urls.map((u) => <p key={`d6-${u}`} className="truncate">{u}</p>)
                          : <p className="text-neutral-400">{t("remote.qrNone")}</p>;
                      })()}
                    </div>
                  </div>
                  {/* 方式三：云中继（对称 NAT / 直连失败兜底；只转发不留存） */}
                  <div className="min-w-0">
                    <p className="mb-1 font-medium text-neutral-600 dark:text-neutral-300">
                      {t("remote.qrMethodRelay")}
                    </p>
                    <div className="space-y-0.5 font-mono text-[11px] text-neutral-500">
                      {qr.payload.methods?.relay && <p className="truncate">{qr.payload.methods.relay}</p>}
                      {qr.payload.cloud && qr.payload.cloud !== qr.payload.methods?.relay && (
                        <p className="truncate text-neutral-400">{qr.payload.cloud}</p>
                      )}
                    </div>
                  </div>
                </div>
              </>
            )}

            <div>
              <div className="flex gap-2">
                <input
                  className="field flex-1 font-mono"
                  placeholder={t("remote.qrCloudPlaceholder")}
                  value={cloudInput}
                  onChange={(e) => setCloudInput(e.target.value)}
                />
                <button onClick={saveCloud} className="pill pill-hover shrink-0">
                  {cloudSaved ? <IconCheck size={14} /> : t("common.save")}
                </button>
              </div>
              <p className="mt-1 px-2 text-[11px] text-neutral-400">{t("remote.qrCloudLabel")}</p>
            </div>

            <div>
              <div className="flex gap-2">
                <input
                  className="field flex-1 font-mono"
                  placeholder={t("remote.stunPlaceholder")}
                  value={stunInput}
                  onChange={(e) => setStunInput(e.target.value)}
                />
                <button onClick={saveStun} className="pill pill-hover shrink-0">
                  {stunSaved ? <IconCheck size={14} /> : t("common.save")}
                </button>
              </div>
              <p className="mt-1 px-2 text-[11px] text-neutral-400">{t("remote.stunLabel")}</p>
            </div>

            {qrErr && <p className="px-2 text-[11px] text-red-600">{qrErr}</p>}

            <p className="rounded-xl bg-neutral-100 px-3 py-2 text-[11px] leading-relaxed text-neutral-500 dark:bg-neutral-800/60 dark:text-neutral-400">
              {t("remote.qrPrivacy")}
            </p>
          </div>
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
          <input className="field flex-1 font-mono" readOnly value={`${base}/v1`} />
          <button onClick={() => copy(`${base}/v1`, "baseurl")} className="pill pill-outline pill-hover shrink-0">
            {copied === "baseurl" ? <IconCheck size={14} /> : <IconGlobe size={14} />}
            {copied === "baseurl" ? t("common.copied") : t("common.copy")}
          </button>
        </div>
        <pre className="overflow-x-auto rounded-2xl bg-neutral-900 p-4 font-mono text-xs leading-relaxed text-neutral-100">{`# ${t("remote.curlListModels")}
curl ${base}/v1/models \\
  -H "Authorization: Bearer ${cfg.client_key}"

# ${t("remote.curlChat")}
curl -X POST ${base}/v1/chat/completions \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"bit","messages":[{"role":"user","content":"${t("remote.curlChatContent")}"}]}'`}</pre>
      </div>

      <div className="card">
        <p className="mb-3 font-medium">{t("remote.agentTitle")}</p>
        <pre className="overflow-x-auto rounded-2xl bg-neutral-900 p-4 font-mono text-xs leading-relaxed text-neutral-100">{`# ${t("remote.curlHealth")}
curl ${base}/api/health

# ${t("remote.curlRegister")}
curl -X POST ${base}/api/tools \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -H "Content-Type: application/json" \\
  -d '{"name":"my_tool","description":"...","url":"http://agent:9000/callback"}'

# ${t("remote.curlInvoke")}
curl -X POST ${base}/api/tools/<id>/invoke \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "X-Access-Password: ${pwd}" \\
  -d '{"params":{}}'

# ${t("remote.curlAIChat")}
curl -X POST ${base}/api/chat \\
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
