import { useEffect, useState } from "react";
import { api } from "../api.js";
import PillSwitch from "../components/PillSwitch.jsx";
import {
  IconPlus,
  IconTrash,
  IconPlay,
  IconTool,
  IconServer,
  IconTerminal,
  IconCode,
  IconRefresh,
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

  const reload = async () => {
    const [t, r] = await Promise.all([api.listTools(), api.listRuntimes()]);
    setTools(t.tools || []);
    setRuntimes(r.runtimes || []);
    if (!runtime && r.runtimes?.length) setRuntime(r.runtimes[0].id);
  };
  useEffect(() => {
    reload();
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
      throw new Error("测试参数不是合法 JSON");
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
    if (!name.trim()) return setError("请填写工具名称");
    if (!runtime) return setError("请选择解释器");
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

  const kindTag = (kind) => {
    switch (kind?.kind) {
      case "builtin":
        return { text: "内置", cls: "border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white dark:text-black" };
      case "remote":
        return { text: "远程", cls: "" };
      case "script":
        return { text: "Rhai 插件", cls: "" };
      case "interpreter":
        return { text: kind.runtime, cls: "border-neutral-900 dark:border-white" };
      default:
        return { text: "工具", cls: "" };
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
              <h2 className="text-sm font-semibold">本机解释器</h2>
              <p className="text-xs text-neutral-500 dark:text-neutral-400">
                已注册的运行时，AI 只需写一段该语言代码即可成为工具
              </p>
            </div>
          </div>
          <div className="flex gap-2">
            <button onClick={refreshRt} className="pill pill-outline pill-hover" disabled={refreshing}>
              <IconRefresh size={14} />
              {refreshing ? "探测中…" : "刷新探测"}
            </button>
            <button onClick={() => setShowAddRt((v) => !v)} className="pill pill-outline pill-hover">
              <IconPlus size={14} />
              手动添加
            </button>
          </div>
        </div>

        {showAddRt && (
          <form onSubmit={addRuntime} className="mb-3 grid grid-cols-4 gap-2">
            <input className="field" placeholder="id（如 node）" value={rtId} onChange={(e) => setRtId(e.target.value)} required />
            <input className="field" placeholder="路径（可执行文件）" value={rtPath} onChange={(e) => setRtPath(e.target.value)} required />
            <input className="field" placeholder="显示名（可选）" value={rtName} onChange={(e) => setRtName(e.target.value)} />
            <select className="field" value={rtLang} onChange={(e) => setRtLang(e.target.value)}>
              <option value="js">js</option>
              <option value="py">py</option>
              <option value="ts">ts</option>
              <option value="php">php</option>
              <option value="rb">rb</option>
              <option value="pl">pl（Perl）</option>
              <option value="lua">lua</option>
              <option value="r">r</option>
              <option value="jl">jl（Julia）</option>
              <option value="ps1">ps1</option>
              <option value="java">java（编译）</option>
              <option value="rs">rs（编译）</option>
              <option value="go">go（编译）</option>
              <option value="c">c（编译）</option>
              <option value="cpp">cpp（编译）</option>
              <option value="cs">cs（编译）</option>
              <option value="kt">kt（编译）</option>
              <option value="exe">exe（可执行）</option>
            </select>
            <div className="col-span-4 flex justify-end">
              <button type="submit" className="pill pill-hover">添加</button>
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
                      {r.mode === "compile" && <span className="chip">编译型</span>}
                      {r.mode === "exec" && <span className="chip">可执行</span>}
                      {r.manual && <span className="chip">手动</span>}
                    </div>
                    <p className="truncate text-xs text-neutral-500 dark:text-neutral-400">
                      {r.version} · {r.path}
                    </p>
                  </div>
                </div>
                <PillSwitch
                  checked={on}
                  onChange={() => toggleRuntime(r)}
                  title={on ? "已启用（点击暂停：AI 将无法用它执行代码）" : "已暂停（点击启用）"}
                />
                <button onClick={() => removeRuntime(r.id)} className="icon-btn shrink-0" title="移除">
                  <IconTrash size={13} />
                </button>
              </div>
            );
          })}
          {runtimes.length === 0 && (
            <p className="py-4 text-center text-sm text-neutral-400">
              未探测到解释器，点击「刷新探测」或手动添加
            </p>
          )}
        </div>
      </section>

      {/* ── 写脚本变工具 ── */}
      <section className="card">
        <div className="mb-3 flex items-center gap-2">
          <IconCode size={18} />
          <div>
            <h2 className="text-sm font-semibold">写一段脚本，变成工具</h2>
            <p className="text-xs text-neutral-500 dark:text-neutral-400">
              约定：从 stdin 读参数 JSON，把结果打印到 stdout。先测试通过再保存
            </p>
          </div>
        </div>

        <div className="mb-2 grid grid-cols-3 gap-2">
          <select className="field" value={runtime} onChange={(e) => onPickRuntime(e.target.value)}>
            {runtimes.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name}（{r.lang}）
              </option>
            ))}
          </select>
          <input className="field" placeholder="工具名称（唯一）" value={name} onChange={(e) => setName(e.target.value)} />
          <input className="field" placeholder="工具描述" value={description} onChange={(e) => setDescription(e.target.value)} />
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
            placeholder="测试参数（JSON）"
            value={testParams}
            onChange={(e) => setTestParams(e.target.value)}
          />
        </div>

        {error && <p className="mt-2 px-2 text-xs text-red-600">{error}</p>}

        <div className="mt-3 flex justify-end gap-2">
          <button onClick={test} className="pill pill-outline pill-hover" disabled={!runtime}>
            <IconPlay size={14} />
            测试运行
          </button>
          <button onClick={save} className="pill pill-hover" disabled={!runtime}>
            <IconTool size={14} />
            保存为工具
          </button>
        </div>

        {result && (
          <div className="mt-3">
            <div className="mb-1.5 flex items-center gap-2">
              <span className="text-xs font-semibold">运行结果</span>
              <span className={`chip ${result.ok ? "border-neutral-900 bg-neutral-900 text-white dark:border-white dark:bg-white dark:text-black" : "text-red-600"}`}>
                {result.loading ? "执行中…" : result.ok ? "成功" : "失败"}
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
          已注册工具
        </h2>

        {tools.length > 0 && (
          <div className="flex items-center gap-3 border-b border-neutral-200/70 px-4 pb-2 text-[11px] font-medium text-neutral-400 dark:border-neutral-800/70">
            <span className="flex-1">名称</span>
            <span className="w-16 text-center">启用</span>
            <span className="w-8 text-center">操作</span>
          </div>
        )}

        <div className="flex flex-col">
          {tools.map((t) => {
            const k = kindTag(t.kind);
            const on = t.enabled ?? true;
            return (
              <div
                key={t.id}
                className="flex items-center gap-3 border-b border-neutral-200/50 px-4 py-3 last:border-0 dark:border-neutral-800/50"
              >
                <div className={`min-w-0 flex-1 transition-opacity ${on ? "" : "opacity-45"}`}>
                  <div className="flex items-center gap-2">
                    <span className="font-semibold">{t.name}</span>
                    <span className={`chip ${k.cls}`}>{k.text}</span>
                    <span className="chip">{t.created_by}</span>
                  </div>
                  <p className="mt-0.5 truncate text-xs text-neutral-500 dark:text-neutral-400">
                    {t.description || "（无描述）"}
                  </p>
                </div>
                <div className="flex w-16 justify-center">
                  <PillSwitch
                    checked={on}
                    onChange={() => toggle(t)}
                    title={on ? "已启用（点击暂停：AI 与远程将无法调用）" : "已暂停（点击启用）"}
                  />
                </div>
                <div className="flex w-8 justify-center">
                  <button onClick={() => remove(t.id)} className="icon-btn shrink-0" title="删除">
                    <IconTrash size={13} />
                  </button>
                </div>
              </div>
            );
          })}
          {tools.length === 0 && (
            <p className="py-6 text-center text-sm text-neutral-400">暂无工具</p>
          )}
        </div>
      </section>
    </div>
  );
}
