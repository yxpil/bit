import { jsx, jsxs } from "react/jsx-runtime";
import { useEffect, useState } from "react";
import { api } from "../api.js";
import { useLang } from "../i18n";
import { IconGlobe, IconCheck, IconRefresh } from "../components/Icons.jsx";
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
  const [testState, setTestState] = useState(null);
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
  const changed = cfg && (host.trim() !== cfg.host || Number(port) !== cfg.port || enabled !== cfg.remote_enabled);
  const test = async () => {
    setTestState(null);
    setError("");
    if (changed && Number(port) !== cfg.port) {
      try {
        const p = await api.checkPort(host.trim() || "127.0.0.1", Number(port));
        if (!p.available) {
          setTestState({
            error: `${t("remote.portConflict")}${p.addr} ${p.reason || t("remote.portInUse")}${t("remote.portConflictFix")}`
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
    }
  };
  if (!cfg) return null;
  return /* @__PURE__ */ jsxs("div", { className: "flex h-full flex-col gap-4 overflow-y-auto", children: [
    /* @__PURE__ */ jsxs("div", { children: [
      /* @__PURE__ */ jsx("h2", { className: "text-lg font-semibold", children: t("remote.title") }),
      /* @__PURE__ */ jsxs("p", { className: "text-xs text-neutral-500", children: [
        t("remote.subtitle"),
        cfg.revision,
        t("remote.subtitleSuffix")
      ] })
    ] }),
    /* @__PURE__ */ jsxs("div", { className: "card flex flex-col gap-4", children: [
      /* @__PURE__ */ jsxs("div", { className: "flex items-center justify-between", children: [
        /* @__PURE__ */ jsxs("div", { children: [
          /* @__PURE__ */ jsx("p", { className: "font-medium", children: t("remote.enableTitle") }),
          /* @__PURE__ */ jsx("p", { className: "text-xs text-neutral-500", children: t("remote.enableDesc") })
        ] }),
        /* @__PURE__ */ jsx(
          "button",
          {
            onClick: () => {
              setEnabled((v) => !v);
              setTestState(null);
            },
            className: `relative h-7 w-14 rounded-full border transition-colors ${enabled ? "border-neutral-900 bg-neutral-900" : "border-neutral-300 bg-neutral-200"}`,
            children: /* @__PURE__ */ jsx(
              "span",
              {
                className: `absolute top-1/2 h-5 w-5 -translate-y-1/2 rounded-full bg-white shadow transition-all ${enabled ? "left-8" : "left-1"}`
              }
            )
          }
        )
      ] }),
      /* @__PURE__ */ jsxs("div", { className: "grid grid-cols-2 gap-3", children: [
        /* @__PURE__ */ jsxs("div", { children: [
          /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: t("remote.listenHost") }),
          /* @__PURE__ */ jsx(
            "input",
            {
              className: "field",
              value: host,
              onChange: (e) => {
                setHost(e.target.value);
                setTestState(null);
              },
              placeholder: t("remote.hostPlaceholder")
            }
          )
        ] }),
        /* @__PURE__ */ jsxs("div", { children: [
          /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: t("remote.port") }),
          /* @__PURE__ */ jsx(
            "input",
            {
              className: "field",
              value: port,
              onChange: (e) => {
                setPort(e.target.value);
                setTestState(null);
              },
              placeholder: "8600"
            }
          )
        ] })
      ] }),
      /* @__PURE__ */ jsxs("div", { children: [
        /* @__PURE__ */ jsx("label", { className: "mb-1 block px-2 text-xs text-neutral-500", children: t("remote.clientKeyLabel") }),
        /* @__PURE__ */ jsxs("div", { className: "flex gap-2", children: [
          /* @__PURE__ */ jsx("input", { className: "field flex-1 font-mono", value: cfg.client_key, readOnly: true }),
          /* @__PURE__ */ jsxs("button", { onClick: () => copy(cfg.client_key, "key"), className: "pill pill-outline pill-hover shrink-0", children: [
            copied === "key" ? /* @__PURE__ */ jsx(IconCheck, { size: 14 }) : /* @__PURE__ */ jsx(IconGlobe, { size: 14 }),
            copied === "key" ? t("common.copied") : t("common.copy")
          ] }),
          /* @__PURE__ */ jsxs("button", { onClick: rotateKey, className: "pill pill-outline pill-hover shrink-0", children: [
            /* @__PURE__ */ jsx(IconRefresh, { size: 14 }),
            t("remote.rotate")
          ] })
        ] })
      ] }),
      /* @__PURE__ */ jsxs("div", { className: "rounded-2xl border border-neutral-200 bg-neutral-50 p-4", children: [
        /* @__PURE__ */ jsxs("div", { className: "mb-3 flex items-center justify-between", children: [
          /* @__PURE__ */ jsxs("div", { children: [
            /* @__PURE__ */ jsx("p", { className: "font-medium", children: t("remote.pwdTitle") }),
            /* @__PURE__ */ jsx("p", { className: "text-xs text-neutral-500", children: t("remote.pwdDesc") })
          ] }),
          /* @__PURE__ */ jsx(
            "button",
            {
              onClick: () => setPwdEnabled((v) => !v),
              className: `relative h-7 w-14 shrink-0 rounded-full border transition-colors ${pwdEnabled ? "border-neutral-900 bg-neutral-900" : "border-neutral-300 bg-white"}`,
              children: /* @__PURE__ */ jsx(
                "span",
                {
                  className: `absolute top-1/2 h-5 w-5 -translate-y-1/2 rounded-full bg-white shadow transition-all ${pwdEnabled ? "left-8 border border-neutral-300" : "left-1 bg-neutral-400"}`
                }
              )
            }
          )
        ] }),
        /* @__PURE__ */ jsxs("div", { className: "flex gap-2", children: [
          /* @__PURE__ */ jsx(
            "input",
            {
              className: "field flex-1 font-mono",
              type: showPwd ? "text" : "password",
              value: pwd,
              readOnly: true
            }
          ),
          /* @__PURE__ */ jsx("button", { onClick: () => setShowPwd((v) => !v), className: "pill pill-outline pill-hover shrink-0", children: showPwd ? t("common.hide") : t("common.show") }),
          /* @__PURE__ */ jsxs("button", { onClick: () => copy(pwd, "pwd"), className: "pill pill-outline pill-hover shrink-0", children: [
            copied === "pwd" ? /* @__PURE__ */ jsx(IconCheck, { size: 14 }) : /* @__PURE__ */ jsx(IconGlobe, { size: 14 }),
            copied === "pwd" ? t("common.copied") : t("common.copy")
          ] }),
          /* @__PURE__ */ jsxs("button", { onClick: rotatePwd, className: "pill pill-outline pill-hover shrink-0", children: [
            /* @__PURE__ */ jsx(IconRefresh, { size: 14 }),
            t("remote.rotate")
          ] })
        ] }),
        /* @__PURE__ */ jsxs("div", { className: "mt-3 flex gap-2", children: [
          /* @__PURE__ */ jsx(
            "input",
            {
              className: "field flex-1",
              type: showPwd ? "text" : "password",
              placeholder: t("remote.pwdPlaceholder"),
              value: pwdInput,
              onChange: (e) => setPwdInput(e.target.value)
            }
          ),
          /* @__PURE__ */ jsx("button", { onClick: savePwd, className: "pill pill-hover shrink-0", children: t("remote.savePwd") })
        ] }),
        pwdMsg && /* @__PURE__ */ jsx("p", { className: "mt-2 px-2 text-xs text-red-600", children: pwdMsg })
      ] }),
      testState === "pass" && /* @__PURE__ */ jsxs("p", { className: "flex items-center gap-2 rounded-full bg-neutral-100 px-4 py-2 text-xs", children: [
        /* @__PURE__ */ jsx(IconCheck, { size: 14 }),
        t("remote.testPass"),
        "http://",
        cfg.host,
        ":",
        cfg.port
      ] }),
      testState?.error && /* @__PURE__ */ jsx("p", { className: "rounded-full bg-red-50 px-4 py-2 text-xs text-red-600", children: testState.error }),
      error && /* @__PURE__ */ jsx("p", { className: "px-2 text-xs text-red-600", children: error }),
      /* @__PURE__ */ jsxs("div", { className: "flex justify-end gap-2", children: [
        /* @__PURE__ */ jsxs("button", { onClick: test, className: "pill pill-outline pill-hover", children: [
          /* @__PURE__ */ jsx(IconGlobe, { size: 14 }),
          t("remote.test")
        ] }),
        /* @__PURE__ */ jsx("button", { onClick: save, disabled: !changed || testState !== "pass" && changed, className: "pill pill-hover", children: t("common.save") })
      ] })
    ] }),
    /* @__PURE__ */ jsxs("div", { className: "card", children: [
      /* @__PURE__ */ jsx("p", { className: "mb-3 font-medium", children: t("remote.openaiTitle") }),
      /* @__PURE__ */ jsx("p", { className: "mb-3 text-xs text-neutral-500", children: t("remote.openaiDesc") }),
      /* @__PURE__ */ jsxs("div", { className: "mb-2 flex gap-2", children: [
        /* @__PURE__ */ jsx("input", { className: "field flex-1 font-mono", readOnly: true, value: `http://${cfg.host}:${cfg.port}/v1` }),
        /* @__PURE__ */ jsxs("button", { onClick: () => copy(`http://${cfg.host}:${cfg.port}/v1`, "baseurl"), className: "pill pill-outline pill-hover shrink-0", children: [
          copied === "baseurl" ? /* @__PURE__ */ jsx(IconCheck, { size: 14 }) : /* @__PURE__ */ jsx(IconGlobe, { size: 14 }),
          copied === "baseurl" ? t("common.copied") : t("common.copy")
        ] })
      ] }),
      /* @__PURE__ */ jsx("pre", { className: "overflow-x-auto rounded-2xl bg-neutral-900 p-4 font-mono text-xs leading-relaxed text-neutral-100", children: `# ${t("remote.curlListModels")}
curl http://${cfg.host}:${cfg.port}/v1/models \\
  -H "Authorization: Bearer ${cfg.client_key}"

# ${t("remote.curlChat")}
curl -X POST http://${cfg.host}:${cfg.port}/v1/chat/completions \\
  -H "Authorization: Bearer ${cfg.client_key}" \\
  -H "Content-Type: application/json" \\
  -d '{"model":"bit","messages":[{"role":"user","content":"${t("remote.curlChatContent")}"}]}'` })
    ] }),
    /* @__PURE__ */ jsxs("div", { className: "card", children: [
      /* @__PURE__ */ jsx("p", { className: "mb-3 font-medium", children: t("remote.agentTitle") }),
      /* @__PURE__ */ jsx("pre", { className: "overflow-x-auto rounded-2xl bg-neutral-900 p-4 font-mono text-xs leading-relaxed text-neutral-100", children: `# ${t("remote.curlHealth")}
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
  -d '{"message":"${t("remote.curlAIChatContent")}"}'` })
    ] })
  ] });
}
