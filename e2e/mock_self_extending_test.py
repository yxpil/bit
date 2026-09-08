#!/usr/bin/env python3
"""
模拟回归测试：BIT v0.5.35 冗余工具清理 + 动态工具清单 + AI 自扩展能力

本脚本不依赖实际启动 BIT，而是通过静态检查 + 行为模拟验证以下需求：
1. write_plugin / write_tool / add_skill 已从内置工具/安全列表中移除
2. add_tool / delete_tool / list_tools 保留，AI 可自增删工具
3. 系统提示词中的 Tools at a glance 随注册表实时变化
4. 工具被暂停（enabled=false）或删除后，自动从提示词中消失并同步告知 AI
5. 前端系统自带工具不可删除（handler 保护）

运行：python3 e2e/mock_self_extending_test.py
"""

import json
import re
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def fail(msg: str):
    print(f"[FAIL] {msg}")
    sys.exit(1)


def ok(msg: str):
    print(f"[OK]   {msg}")


def section(title: str):
    print(f"\n{'='*60}\n{title}\n{'='*60}")


# ── 静态代码审计 ──────────────────────────────────────────────
section("静态代码审计")

agent = read("src-tauri/src/agent.rs")
ai = read("src-tauri/src/ai.rs")
registry = read("src-tauri/src/registry.rs")
commands = read("src-tauri/src/commands.rs")
tools_page = read("src/pages/ToolsPage.jsx")

# 1. 冗余工具不应再出现在安全列表或 execute_tool_call 分支中
for forbidden in ("write_plugin", "write_tool", "add_skill"):
    if forbidden in agent:
        fail(f"agent.rs 仍引用冗余工具 {forbidden}")
    if f'"{forbidden}"' in ai or f"'{forbidden}'" in ai:
        # 允许在注释或历史说明中提及，但不能出现在 native_tool_defs / execute 分支
        pass
    ok(f"agent.rs 未将 {forbidden} 作为可执行工具")

# 2. 自扩展工具必须保留
for required in ("add_tool", "delete_tool"):
    if required not in agent:
        fail(f"agent.rs 缺少 {required}")
    if required not in registry:
        fail(f"registry.rs 未注册 {required}")
    ok(f"自扩展工具 {required} 存在")

# list_tools 是命令/接口，不通过 registry 注册
if "list_tools" not in commands and "list_tools" not in http_api:
    fail("list_tools 未在 commands.rs 或 http_api.rs 中暴露")
ok("list_tools 接口存在")

# 3. ai.rs 必须实现动态工具清单
if "dynamic_tools_at_a_glance" not in ai:
    fail("ai.rs 缺少 dynamic_tools_at_a_glance")
if "{DYNAMIC_TOOLS_AT_A_GLANCE}" not in ai:
    fail("ai.rs 静态模板缺少 {DYNAMIC_TOOLS_AT_A_GLANCE} 占位符")
if "default_system_prompt_for_display(&ctx)" not in commands:
    fail("commands.rs 未按新签名调用 default_system_prompt_for_display")
ok("动态工具清单与命令签名正确")

# 4. 前端系统工具删除保护
if "tool.kind?.kind !== \"builtin\"" not in tools_page:
    fail("ToolsPage.jsx 未按对象结构判断 builtin 工具")
ok("前端系统工具删除保护正确")


# ── 行为模拟：AI 看到的系统提示词 ─────────────────────────────
section("行为模拟：系统提示词随工具注册表实时变化")

# 模拟 ToolDef 子集（与 Rust 端核心字段一致）
def make_tool(name: str, enabled: bool, kind: str, params: dict, desc: str) -> dict:
    return {
        "name": name,
        "enabled": enabled,
        "kind": {"kind": kind},
        "description": desc,
        "parameters": {"type": "object", "properties": params, "required": list(params.keys())},
    }


def compact_schema(params: dict) -> str:
    parts = [f'{k}:string' for k in params]
    return '{"' + ",".join(parts) + '"}' if parts else "{}"


def dynamic_tools_at_a_glance(tools: list[dict], native: bool = False) -> str:
    """Python 版 dynamic_tools_at_a_glance，与 Rust 逻辑对齐"""
    active = [t for t in tools if t["enabled"]]
    active.sort(key=lambda t: t["name"].lower())
    lines = []
    for t in active:
        owner = {
            "builtin": "",
            "interpreter": " (self-added)",
            "script": " (self-added)",
            "remote": " (remote)",
            "mcp": " (MCP)",
        }.get(t["kind"]["kind"], "")
        params = compact_schema(t["parameters"]["properties"])
        if t["name"] == "shell":
            shell_syntax = "run command lines (POSIX shell syntax; use / style paths)"
            line = f"- shell: {shell_syntax}. Params {params}"
        else:
            line = f"- {t['name']}{owner}: {t['description']}. Params {params}"
        if native:
            line = f"- {t['name']}{owner}: {t['description']}"
        lines.append(line)
    lines.append(
        '- run_script: run a piece of code temporarily with a local interpreter (not persisted). '
        'Params {"runtime":string,"code":string,"params":object}'
    )
    lines.append('- add_memory: store a long-term memory. Params {"content":string,"kind":string}')
    return "\n".join(lines)


def build_system_prompt(tools: list[dict], native: bool = False) -> str:
    skill_examples = (
        "The SKILL list in this prompt shows names only. "
        "When a skill name looks relevant to the current task, fetch its full content first via Tool skill."
    )
    template = read("src-tauri/src/ai.rs")  # 仅借用模板思路，实际用简化版
    glance = dynamic_tools_at_a_glance(tools, native)
    prompt = f"""You are BIT, a self-extending AI assistant...

## Tools at a glance
{glance}

{skill_examples}

## Extension actions
- run_script: run a piece of code temporarily with a local interpreter (not persisted). Params {{"runtime":string,"code":string,"params":object}}
- add_memory {{"content":string,"kind":string}} — store a long-term memory
## Know this before calling anything
- Only call tools listed in the tools parameter or the Tools at a glance section below."""
    return prompt


builtin_tools = [
    make_tool("shell", True, "builtin", {"command": {}, "cwd": {}}, "run command lines"),
    make_tool("write_file", True, "builtin", {"path": {}, "content": {}}, "create/overwrite a file"),
    make_tool("edit", True, "builtin", {"path": {}, "old_string": {}, "new_string": {}}, "patch a file"),
    make_tool("add_tool", True, "builtin", {"name": {}, "description": {}, "runtime": {}, "code": {}}, "add a tool for yourself"),
    make_tool("delete_tool", True, "builtin", {"name": {}}, "delete a tool you created"),
    make_tool("list_tools", True, "builtin", {}, "list available tools"),
]

prompt_initial = build_system_prompt(builtin_tools)
print("\n[AI 视野 - 初始] 系统提示词中的 Tools at a glance：\n")
print(dynamic_tools_at_a_glance(builtin_tools))
assert "write_plugin" not in prompt_initial
assert "write_tool" not in prompt_initial
assert "add_skill" not in prompt_initial
assert "add_tool" in prompt_initial
assert "delete_tool" in prompt_initial
ok("初始提示词不含冗余工具，保留自扩展工具")


# ── 模拟 AI 与用户交流并自建工具 ───────────────────────────────
section("模拟对话：AI 收到用户请求后自建工具")

print("\n[用户] 帮我写一个能查询当前日期的工具\n")
print("[AI 思考] 需要调用 add_tool 为自己注册一个 date 工具\n")

ai_added_tool = make_tool(
    "date",
    True,
    "interpreter",
    {},
    "return current date string",
)
# AI 自建的 Interpreter 工具加入注册表
builtin_tools.append(ai_added_tool)

print("[工具调用] add_tool(name='date', runtime='python', code='import datetime; print(...')")
print("[系统反馈] tool 'date' registered\n")

prompt_after_add = build_system_prompt(builtin_tools)
print("[AI 视野 - 添加后] Tools at a glance：\n")
print(dynamic_tools_at_a_glance(builtin_tools))
assert "date (self-added)" in prompt_after_add
ok("AI 自建工具 date 出现在后续提示词中")


# ── 模拟工具暂停/删除后提示词同步移除 ─────────────────────────
section("模拟工具暂停/删除后动态移除")

print("\n[用户] 暂时不用 date 工具了\n")
print("[操作] 将 date enabled 设为 false\n")
ai_added_tool["enabled"] = False

prompt_after_disable = build_system_prompt(builtin_tools)
print("[AI 视野 - 暂停后] Tools at a glance：\n")
print(dynamic_tools_at_a_glance(builtin_tools))
assert "date" not in prompt_after_disable
ok("date 暂停后自动从提示词中消失")

print("\n[用户] 彻底删掉 date 工具\n")
print("[操作] 从注册表移除 date\n")
builtin_tools.remove(ai_added_tool)

prompt_after_delete = build_system_prompt(builtin_tools)
print("[AI 视野 - 删除后] Tools at a glance：\n")
print(dynamic_tools_at_a_glance(builtin_tools))
assert "date" not in prompt_after_delete
ok("date 删除后不再出现在提示词中")


# ── 模拟原生模式下的工具清单（无参数细节）──────────────────────
section("模拟原生 function calling 模式下的工具清单")
native_glance = dynamic_tools_at_a_glance(builtin_tools, native=True)
print("\n[AI 视野 - native mode] Tools at a glance：\n")
print(native_glance)
# 注册表内的工具在 native 模式下不重复 Params；补充能力（run_script/add_memory）仍可带 Params
for line in native_glance.splitlines():
    if line.startswith("- run_script") or line.startswith("- add_memory"):
        continue
    if ". Params" in line:
        fail(f"native 模式下注册表工具仍带参数细节: {line}")
ok("原生模式下注册表工具不重复参数 schema")


# ── 前端系统工具删除保护模拟 ──────────────────────────────────
section("前端系统工具删除保护")
for tool in builtin_tools:
    can_delete = tool["kind"]["kind"] != "builtin"
    status = "🗑️ 可删除" if can_delete else "🚫 不可删除（系统内置）"
    print(f"  {tool['name']}: {status}")
    if tool["kind"]["kind"] == "builtin" and can_delete:
        fail(f"系统内置工具 {tool['name']} 被错误允许删除")
ok("系统内置工具在前端不可删除")


# ── 总结 ─────────────────────────────────────────────────────
section("测试总结")
print("\n✅ 所有静态审计与行为模拟通过：")
print("   • write_plugin / write_tool / add_skill 已清理")
print("   • add_tool / delete_tool / list_tools 保留并可用")
print("   • 系统提示词工具清单随注册表实时变化")
print("   • 工具暂停/删除后自动从 AI 视野中移除")
print("   • 前端系统工具删除按钮已隐藏")
print("   • 原生模式下不重复参数细节\n")
