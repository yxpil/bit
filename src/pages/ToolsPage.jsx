import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api.js";
import PillSwitch from "../components/PillSwitch.jsx";
import { useLang } from "../i18n.js";
import {
  IconPlus,
  IconTrash,
  IconPlay,
  IconTool,
  IconServer,
  IconTerminal,
  IconCode,
  IconRefresh,
  IconGlobe,
} from "../components/Icons.jsx";

// 语言默认脚手架：读 stdin 的 JSON，把结果写 stdout
const SCAFFOLD = {
  js: `// 从 stdin 读取参数 JSON，把结果打印到 stdout
const p = JSON.parse(require('fs').readFileSync(0, 'utf8') || '{}');
console.log(JSON.stringify({ echo: p, ts: Date.now() }));`,
  py: `# 从 stdin 读取参数 JSON，把结果打印到 stdout
import sys, json
p = json.loads(sys.stdin.read() or '{}')
print(json.dumps({ 'echo': p }))`,
  ts: `const raw = await new Response(Deno.stdin.readable).text();
const p = JSON.parse(raw || '{}');
console.log(JSON.stringify({ echo: p }));`,
  // Java：JDK 11+ 单文件源码直跑，从 stdin 读、stdout 写
  java: `import java.util.Scanner;
public class Main {
  public static void main(String[] args) {
    Scanner sc = new Scanner(System.in);
    StringBuilder in = new StringBuilder();
    while (sc.hasNextLine()) in.append(sc.nextLine());
    // 简单回显；如需解析 JSON 可自行引入库或手写
    System.out.println("{\\"echo\\": " + (in.length() == 0 ? "{}" : in) + "}");
  }
}`,
  // Rust：rustc 临时编译后运行，从 stdin 读、stdout 写
  rs: `use std::io::Read;
fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).ok();
    let body = if s.trim().is_empty() { "{}".to_string() } else { s.trim().to_string() };
    println!("{{\\"echo\\": {}}}", body);
}`,
  // Go：go run 单文件，从 stdin 读、stdout 写
  go: `package main
import ("bufio"; "fmt"; "os"; "strings")
func main() {
    r := bufio.NewReader(os.Stdin)
    var b strings.Builder
    buf := make([]byte, 4096)
    for { n, _ := r.Read(buf); if n == 0 { break }; b.Write(buf[:n]) }
    body := strings.TrimSpace(b.String()); if body == "" { body = "{}" }
    fmt.Printf("{\\"echo\\": %s}", body)
}`,
  // C：gcc 编译后运行，从 stdin 读、stdout 写
  c: `#include <stdio.h>
int main(){ char buf[65536]; size_t n=fread(buf,1,sizeof(buf)-1,stdin); buf[n]=0;
  printf("{\\"echo\\": %s}", n? buf : "{}"); return 0; }`,
  // Lua：从 stdin 读、stdout 写
  lua: `local s = io.read("*a") or ""
if s == "" then s = "{}" end
io.write('{"echo": ' .. s .. '}')`,
  exe: "",
};

// 工具中心：解释器注册 + 让 AI/用户写一段脚本变成工具 + 工具列表
export default function ToolsPage({ onStats }) {
  const { t } = useLang();
  const [tools, setTools] = useState([]);
  const [runtimes, setRuntimes] = useState([]);
  const [refreshing, setRefreshing] = useState(false);

  // 脚本工具编辑器
  const [runtime, setRuntime] = useState("");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [code, setCode] = useState(SCAFFOLD.js);
  const [testParams, setTestParams] = useState('{ "a": 1, "b": 2 }');
  const [error, setError] = useState("");
  const [result, setResult] = useState(null);

  // 手动添加解释器
  const [showAddRt, setShowAddRt] = useState(false);
  const [rtId, setRtId] = useState("");
  const [rtName, setRtName] = useState("");
  const [rtPath, setRtPath] = useState("");
  const [rtLang, setRtLang] = useState("js");

  // MCP 服务器：自动发现 + 接入 + 暂停/继续
  const [mcpServers, setMcpServers] = useState([]);
  const [mcpFound, setMcpFound] = useState(null);
  const [mcpHost, setMcpHost] = useState("127.0.0.1");
  const [mcpStart, setMcpStart] = useState("8000");
  const [mcpEnd, setMcpEnd] = useState("8100");
  const [mcpUrl, setMcpUrl] = useState("");
  const [mcpBusy, setMcpBusy] = useState(false);
  const [mcpMsg, setMcpMsg] = useState("");

  const reload = async () => {
    const [t, r, m] = await Promise.all([api.listTools(), api.listRuntimes(), api.mcpList()]);
    setTools(t.tools || []);
    setRuntimes(r.runtimes || []);
    setMcpServers(m.servers || []);
    if (!runtime && r.runtimes?.length) setRuntime(r.runtimes[0].id);
  };
  useEffect(() => {
    reload();
    // 启动时解释器在后台探测，完成后刷新列表
    const un = listen("runtimes-updated", () => reload());
    // 热加载：AI 注册/更新/删除工具或 MCP 接入后即时刷新清单（免重启）
    const un2 = listen("tools-updated", () => reload());
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  const refreshRt = async () => {
    setRefreshing(true);
    try {
      const r = await api.refreshRuntimes();
      setRuntimes(r.runtimes || []);
      if (r.runtimes?.length && !r.runtimes.some((x) => x.id === runtime)) {
        setRuntime(r.runtimes[0].id);
      }
    } finally {
      setRefreshing(false);
    }
  };

  const langOf = (id) => runtimes.find((r) => r.id === id)?.lang || "js";

  const onPickRuntime = (id) => {
    setRuntime(id);
    // 若代码还是空/默认脚手架，换成对应语言脚手架
    const lang = langOf(id);
    setCode(SCAFFOLD[lang] || SCAFFOLD.js);
  };

  const parseParams = () => {
    try {
      return JSON.parse(testParams || "{}");
    } catch {
      throw new Error(t("tools.badTestParams"));
    }
  };

  // 先测试再保存：验证脚本可通讯
  const test = async () => {
    setError("");
    setResult({ loading: true });
    try {
      const params = parseParams();
      const r = await api.runScript(runtime, code, params);
      setResult({ ok: true, data: r });
    } catch (e) {
      setResult({ ok: false, data: String(e) });
    }
  };

  const save = async () => {
    setError("");
    if (!name.trim()) return setError(t("tools.errNameRequired"));
    if (!runtime) return setError(t("tools.errRuntimeRequired"));
    try {
      await api.registerScriptTool(name, description, runtime, code);
      setName("");
      setDescription("");
      setResult(null);
      await reload();
      onStats?.();
    } catch (e) {
      setError(String(e));
    }
  };

  const remove = async (id) => {
    await api.removeTool(id);
    await reload();
    onStats?.();
  };

  // 暂停 / 启用工具：暂停后 AI 与远程都不能调用，但保留定义
  const toggle = async (t) => {
    await api.setToolEnabled(t.id, !(t.enabled ?? true));
    await reload();
    onStats?.();
  };

  const addRuntime = async (e) => {
    e.preventDefault();
    setError("");
    try {
      await api.addRuntime(rtId, rtName, rtPath, rtLang);
      setRtId("");
      setRtName("");
      setRtPath("");
      setShowAddRt(false);
      await reload();
    } catch (err) {
      setError(String(err));
    }
  };

  const removeRuntime = async (id) => {
    await api.removeRuntime(id);
    await reload();
  };

  // 暂停 / 启用解释器：暂停后 AI 不能用它执行代码或注册工具
  const toggleRuntime = async (r) => {
    await api.setRuntimeEnabled(r.id, !(r.enabled ?? true));
    await reload();
  };

  // ── MCP：扫描 / 接入 / 开关 / 导入 / 移除 ──
  const scanMcp = async () => {
    setMcpBusy(true);
    setMcpMsg("");
    setMcpFound(null);
    try {
      const r = await api.mcpDiscover(mcpHost.trim() || "127.0.0.1", parseInt(mcpStart) || 8000, parseInt(mcpEnd) || 8100);
      setMcpFound(r.servers || []);
    } catch (e) {
      setMcpMsg(String(e));
    } finally {
      setMcpBusy(false);
    }
  };

  // 接入 = MCP 握手保存 + 立即导入工具清单到注册中心
  const connectMcp = async (url) => {
    setMcpBusy(true);
    setMcpMsg("");
    try {
      const r = await api.mcpConnect(url);
      const imp = await api.mcpImport(r.id);
      setMcpMsg(`${r.server.name}: ${t("tools.mcpImported")} ${imp.imported} · ${t("tools.mcpSkipped")} ${imp.skipped}`);
      setMcpUrl("");
      setMcpFound(null);
      await reload();
      onStats?.();
    } catch (e) {
      setMcpMsg(String(e));
    } finally {
      setMcpBusy(false);
    }
  };

  const toggleMcp = async (s) => {
    await api.mcpToggle(s.id, !(s.enabled ?? true));
    await reload();
  };

  const removeMcp = async (id) => {
    await api.mcpRemove(id);
    await reload();
    onStats?.();
  };

  const reimportMcp = async (s) => {
    setMcpBusy(true);
    setMcpMsg("");
    try {
      const imp = await api.mcpImport(s.id);
      setMcpMsg(`${s.name}: ${t("tools.mcpImported")} ${imp.imported} · ${t("tools.mcpSkipped")} ${imp.skipped}`);
      await reload();
      onStats?.();
    } catch (e) {
      setMcpMsg(String(e));
    } finally {
      setMcpBusy(false);
    }
  };

  const kindTag = (kind) => {
    switch (kind?.kind) {
      case "builtin":
        return { text: t("tools.kindBuiltin"), cls: "border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white dark:text-black" };
      case "remote":
        return { text: t("tools.kindRemote"), cls: "" };
      case "script":
        return { text: t("tools.kindScript"), cls: "" };
      case "interpreter":
        return { text: kind.runtime, cls: "border-neutral-900 dark:border-white" };
      case "mcp":
        return { text: "MCP", cls: "" };
      default:
        return { text: t("tools.kindTool"), cls: "" };
    }
  };

  return (
    <div className="flex h-full flex-col gap-5 overflow-y-auto">
      {/* ── 解释器注册 ── */}
      <section className="card">
        <div className="mb-3 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <IconServer size={18} />
            <div>
              <h2 className="text-sm font-semibold">{t("tools.runtimesTitle")}</h2>
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                {t("tools.runtimesDesc")}
              </p>
            </div>
          </div>
          <div className="flex gap-2">
            <button onClick={refreshRt} className="pill pill-outline pill-hover" disabled={refreshing}>
              <IconRefresh size={14} />
              {refreshing ? t("tools.probing") : t("tools.refreshDetect")}
            </button>
            <button onClick={() => setShowAddRt((v) => !v)} className="pill pill-outline pill-hover">
              <IconPlus size={14} />
              {t("tools.addManual")}
            </button>
          </div>
        </div>

        {showAddRt && (
          <form onSubmit={addRuntime} className="mb-3 grid grid-cols-4 gap-2">
            <input className="field" placeholder={t("tools.rtIdPlaceholder")} value={rtId} onChange={(e) => setRtId(e.target.value)} required />
            <input className="field" placeholder={t("tools.rtPathPlaceholder")} value={rtPath} onChange={(e) => setRtPath(e.target.value)} required />
            <input className="field" placeholder={t("tools.rtNamePlaceholder")} value={rtName} onChange={(e) => setRtName(e.target.value)} />
            <select className="field" value={rtLang} onChange={(e) => setRtLang(e.target.value)}>
              <option value="js">js</option>
              <option value="py">py</option>
              <option value="ts">ts</option>
              <option value="php">php</option>
              <option value="rb">rb</option>
              <option value="pl">pl{t("tools.suffixPerl")}</option>
              <option value="lua">lua</option>
              <option value="r">r</option>
              <option value="jl">jl{t("tools.suffixJulia")}</option>
              <option value="ps1">ps1</option>
              <option value="java">java{t("tools.suffixCompile")}</option>
              <option value="rs">rs{t("tools.suffixCompile")}</option>
              <option value="go">go{t("tools.suffixCompile")}</option>
              <option value="c">c{t("tools.suffixCompile")}</option>
              <option value="cpp">cpp{t("tools.suffixCompile")}</option>
              <option value="cs">cs{t("tools.suffixCompile")}</option>
              <option value="kt">kt{t("tools.suffixCompile")}</option>
              <option value="exe">exe{t("tools.suffixExecutable")}</option>
            </select>
            <div className="col-span-4 flex justify-end">
              <button type="submit" className="pill pill-hover">{t("common.add")}</button>
            </div>
          </form>
        )}

        <div className="flex flex-col gap-2">
          {runtimes.map((r) => {
            const on = r.enabled ?? true;
            return (
              <div key={r.id} className="flex items-center gap-3 rounded-2xl border border-neutral-200/70 px-4 py-2.5 dark:border-neutral-800/70">
                <div className={`flex min-w-0 flex-1 items-center gap-3 transition-opacity ${on ? "" : "opacity-45"}`}>
                  <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-neutral-100 dark:bg-neutral-900">
                    <IconTerminal size={16} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-sm font-semibold">{r.id}</span>
                      <span className="chip">{r.lang}</span>
                      {r.mode === "compile" && <span className="chip">{t("tools.chipCompile")}</span>}
                      {r.mode === "exec" && <span className="chip">{t("tools.chipExec")}</span>}
                      {r.manual && <span className="chip">{t("tools.chipManual")}</span>}
                    </div>
                    <p className="truncate text-xs text-neutral-500 dark:text-neutral-400">
                      {r.version} · {r.path}
                    </p>
                  </div>
                </div>
                <PillSwitch
                  checked={on}
                  onChange={() => toggleRuntime(r)}
                  title={on ? t("tools.rtEnabledTitle") : t("tools.pausedTitle")}
                />
                <button onClick={() => removeRuntime(r.id)} className="icon-btn shrink-0" title={t("common.remove")}>
                  <IconTrash size={13} />
                </button>
              </div>
            );
          })}
          {runtimes.length === 0 && (
            <p className="py-4 text-center text-sm text-neutral-400">
              {t("tools.noRuntimes")}
            </p>
          )}
        </div>
      </section>

      {/* ── MCP 服务器：自动发现 + 接入 ── */}
      <section className="card">
        <div className="mb-3 flex items-center gap-2">
          <IconGlobe size={18} />
          <div>
            <h2 className="text-sm font-semibold">{t("tools.mcpTitle")}</h2>
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              {t("tools.mcpDesc")}
            </p>
          </div>
        </div>

        {/* 扫描行：host + 端口范围 + 手动 URL */}
        <div className="mb-3 grid grid-cols-6 gap-2">
          <input className="field font-mono" title="Host" value={mcpHost} onChange={(e) => setMcpHost(e.target.value)} />
          <input className="field font-mono" placeholder={t("tools.mcpFrom")} value={mcpStart} onChange={(e) => setMcpStart(e.target.value)} inputMode="numeric" />
          <input className="field font-mono" placeholder={t("tools.mcpTo")} value={mcpEnd} onChange={(e) => setMcpEnd(e.target.value)} inputMode="numeric" />
          <button onClick={scanMcp} className="pill pill-outline pill-hover" disabled={mcpBusy}>
            <IconRefresh size={14} />
            {mcpBusy ? t("tools.mcpScanning") : t("tools.mcpScan")}
          </button>
          <input
            className="field col-span-2 font-mono"
            placeholder={t("tools.mcpManualUrl")}
            value={mcpUrl}
            onChange={(e) => setMcpUrl(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && mcpUrl.trim() && connectMcp(mcpUrl.trim())}
          />
        </div>

        {mcpMsg && <p className="mb-2 px-2 text-xs text-neutral-500 dark:text-neutral-400">{mcpMsg}</p>}

        {/* 扫描结果 */}
        {mcpFound && mcpFound.length > 0 && (
          <div className="mb-3 flex flex-col gap-2">
            {mcpFound.map((d) => (
              <div key={d.url} className="flex items-center gap-3 rounded-2xl border border-dashed border-neutral-300 px-4 py-2.5 dark:border-neutral-700">
                <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-neutral-100 dark:bg-neutral-900">
                  <IconGlobe size={16} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-semibold">{d.name}</span>
                    <span className="chip">{d.version || "MCP"}</span>
                  </div>
                  <p className="truncate font-mono text-xs text-neutral-500 dark:text-neutral-400">{d.url}</p>
                </div>
                <button onClick={() => connectMcp(d.url)} className="pill pill-hover" disabled={mcpBusy}>
                  <IconPlus size={14} />
                  {t("tools.mcpConnect")}
                </button>
              </div>
            ))}
          </div>
        )}
        {mcpFound && mcpFound.length === 0 && (
          <p className="mb-3 px-2 text-xs text-neutral-400">{t("tools.mcpNone")}</p>
        )}

        {/* 已接入列表：圆片开关（暂停/继续）+ 重新导入 + 移除 */}
        <div className="flex flex-col gap-2">
          {mcpServers.map((s) => {
            const on = s.enabled ?? true;
            const toolCount = tools.filter(
              (tl) => tl.kind?.kind === "mcp" && tl.kind?.server_id === s.id
            ).length;
            return (
              <div key={s.id} className="flex items-center gap-3 rounded-2xl border border-neutral-200/70 px-4 py-2.5 dark:border-neutral-800/70">
                <div className={`flex min-w-0 flex-1 items-center gap-3 transition-opacity ${on ? "" : "opacity-45"}`}>
                  <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-neutral-100 dark:bg-neutral-900">
                    <IconGlobe size={16} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-semibold">{s.name}</span>
                      <span className="chip">MCP</span>
                      <span className="chip">{toolCount} {t("tools.mcpTools")}</span>
                    </div>
                    <p className="truncate font-mono text-xs text-neutral-500 dark:text-neutral-400">{s.url}</p>
                  </div>
                </div>
                <button onClick={() => reimportMcp(s)} className="icon-btn shrink-0" title={t("tools.mcpImport")} disabled={mcpBusy}>
                  <IconRefresh size={13} />
                </button>
                <PillSwitch
                  checked={on}
                  onChange={() => toggleMcp(s)}
                  title={on ? t("tools.mcpEnabledTitle") : t("tools.pausedTitle")}
                />
                <button onClick={() => removeMcp(s.id)} className="icon-btn shrink-0" title={t("common.remove")}>
                  <IconTrash size={13} />
                </button>
              </div>
            );
          })}
          {mcpServers.length === 0 && (
            <p className="py-4 text-center text-sm text-neutral-400">{t("tools.mcpEmpty")}</p>
          )}
        </div>
      </section>

      {/* ── 写脚本变工具 ── */}
      <section className="card">
        <div className="mb-3 flex items-center gap-2">
          <IconCode size={18} />
          <div>
            <h2 className="text-sm font-semibold">{t("tools.scriptTitle")}</h2>
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              {t("tools.scriptDesc")}
            </p>
          </div>
        </div>

        <div className="mb-2 grid grid-cols-3 gap-2">
          <select className="field" value={runtime} onChange={(e) => onPickRuntime(e.target.value)}>
            {runtimes.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name}{t("tools.parenOpen")}{r.lang}{t("tools.parenClose")}
              </option>
            ))}
          </select>
          <input className="field" placeholder={t("tools.namePlaceholder")} value={name} onChange={(e) => setName(e.target.value)} />
          <input className="field" placeholder={t("tools.descPlaceholder")} value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>

        <textarea
          className="h-44 w-full rounded-2xl border border-neutral-200 bg-neutral-50 p-3 font-mono text-xs outline-none focus:border-neutral-900 dark:border-neutral-800 dark:bg-neutral-900 dark:focus:border-neutral-200"
          value={code}
          onChange={(e) => setCode(e.target.value)}
          spellCheck={false}
        />

        <div className="mt-2 grid grid-cols-3 gap-2">
          <input
            className="field col-span-3 font-mono"
            placeholder={t("tools.testParamsPlaceholder")}
            value={testParams}
            onChange={(e) => setTestParams(e.target.value)}
          />
        </div>

        {error && <p className="mt-2 px-2 text-xs text-red-600">{error}</p>}

        <div className="mt-3 flex justify-end gap-2">
          <button onClick={test} className="pill pill-outline pill-hover" disabled={!runtime}>
            <IconPlay size={14} />
            {t("tools.testRun")}
          </button>
          <button onClick={save} className="pill pill-hover" disabled={!runtime}>
            <IconTool size={14} />
            {t("tools.saveAsTool")}
          </button>
        </div>

        {result && (
          <div className="mt-3">
            <div className="mb-1.5 flex items-center gap-2">
              <span className="text-xs font-semibold">{t("tools.runResult")}</span>
              <span className={`chip ${result.ok ? "border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white dark:text-black" : "text-red-600"}`}>
                {result.loading ? t("common.running") : result.ok ? t("common.success") : t("common.failed")}
              </span>
            </div>
            <textarea
              className="h-28 w-full rounded-2xl border border-neutral-200 bg-neutral-50 p-3 font-mono text-xs outline-none dark:border-neutral-800 dark:bg-neutral-900"
              readOnly
              value={result.loading ? "…" : JSON.stringify(result.data, null, 2)}
            />
          </div>
        )}
      </section>

      {/* ── 已注册工具 ── */}
      <section className="card">
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <IconTool size={18} />
          {t("tools.registeredTitle")}
        </h2>

        {tools.length > 0 && (
          <div className="flex items-center gap-3 border-b border-neutral-200/70 px-4 pb-2 text-[11px] font-medium text-neutral-400 dark:border-neutral-800/70">
            <span className="flex-1">{t("tools.colName")}</span>
            <span className="w-16 text-center">{t("tools.colEnabled")}</span>
            <span className="w-8 text-center">{t("tools.colActions")}</span>
          </div>
        )}

        <div className="flex flex-col">
          {tools.map((tool) => {
            const k = kindTag(tool.kind);
            const on = tool.enabled ?? true;
            return (
              <div
                key={tool.id}
                className="flex items-center gap-3 border-b border-neutral-200/50 px-4 py-3 last:border-0 dark:border-neutral-800/50"
              >
                <div className={`min-w-0 flex-1 transition-opacity ${on ? "" : "opacity-45"}`}>
                  <div className="flex items-center gap-2">
                    <span className="font-semibold">{tool.name}</span>
                    <span className={`chip ${k.cls}`}>{k.text}</span>
                    <span className="chip">{tool.created_by}</span>
                  </div>
                  <p className="mt-0.5 truncate text-xs text-neutral-500 dark:text-neutral-400">
                    {tool.description || t("tools.noDesc")}
                  </p>
                </div>
                <div className="flex w-16 justify-center">
                  <PillSwitch
                    checked={on}
                    onChange={() => toggle(tool)}
                    title={on ? t("tools.enabledTitle") : t("tools.pausedTitle")}
                  />
                </div>
                <div className="flex w-8 justify-center">
                  <button onClick={() => remove(tool.id)} className="icon-btn shrink-0" title={t("common.delete")}>
                    <IconTrash size={13} />
                  </button>
                </div>
              </div>
            );
          })}
          {tools.length === 0 && (
            <p className="py-6 text-center text-sm text-neutral-400">{t("tools.empty")}</p>
          )}
        </div>
      </section>
    </div>
  );
}
