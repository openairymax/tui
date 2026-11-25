#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
# SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0
#
# golden 事件样本的场景表（0.1.21 X1-31b）。
#
# 这里只描述「一段 run 长什么样」：每项是一份样本的文件名后缀与帧序列，
# 帧分两类——服务端透传帧（ev，payload 为 run-stream 事件信封）与本地帧
# （loc，payload 由 cli_ev 自行装配）。编码、时间轴与落盘由
# gen-golden-samples.py 负责，本表不含任何 I/O。
#
# 表长固定为 21 项，且必须覆盖 run-stream 的全部对外类型；新增类型时先补
# 场景，再由 gen-golden-samples.py 重新生成，样本集才不会悄悄失去覆盖。
#
# 时间基准：2026-01-01T00:00:00Z，帧间隔 40ms；同一场景重复生成逐字节一致。

BASE_TS = 1767225600000
STEP_MS = 40

START = "run_start"
STATE = "task_state"
TOKEN = "token_delta"
MSG = "message"
END = "run_end"
ERR = "error"
TOOL_S = "tool_start"
TOOL_D = "tool_delta"
TOOL_E = "tool_end"
SUB_S = "subagent_start"
SUB_E = "subagent_end"


def ev(kind, data):
    """服务端透传帧。"""
    return ("evt", kind, data)


def loc(kind, payload):
    """本地帧（cli_ev 自行装配的 payload）。"""
    return ("local", kind, payload)


def start(prompt, mode="direct", confirmed=False):
    return ev(
        START,
        {
            "prompt": prompt,
            "model": "airy-default",
            "task_id": "task-0001",
            "mode": mode,
            "mode_reason": "planner" if mode == "plan" else "single-shot",
            "gccp_confirmed": confirmed,
        },
    )


def state(name, reason=None):
    data = {"task_id": "task-0001", "state": name}
    if reason:
        data["reason"] = reason
    return ev(STATE, data)


def message(text):
    return ev(MSG, {"role": "assistant", "content": text})


def finish(status="completed", **extra):
    data = {
        "status": status,
        "duration_ms": 1840,
        "think_ms": 120,
        "llm_ms": 1420,
        "tool_ms": 300,
        "use_ticks": 268,
        "rounds_used": 2,
        "rounds_limit": 8,
        "usage": {"prompt_tokens": 64, "completion_tokens": 204, "total_tokens": 268},
    }
    data.update(extra)
    return ev(END, data)


def failure(code, stage, text, suggest, retryable=False):
    return {
        "code": code,
        "stage": stage,
        "message": text,
        "suggest_action": suggest,
        "retryable": retryable,
    }


def tool_start(name, args, tid="call-1"):
    return ev(TOOL_S, {"tool": name, "tool_id": tid, "args": args})


def tool_end(tid, status, digest):
    return ev(TOOL_E, {"tool_id": tid, "status": status, "result_hash": digest})


def sub_start(sid, goal, depth=1, budget=4096):
    """子任务委派开始：目标与预算随帧下发（N-8）。"""
    return ev(
        SUB_S,
        {"sub_id": sid, "goal": goal, "depth": depth, "budget": budget},
    )


def sub_end(sid, status="done", tokens=512):
    """子任务收尾：status 用 run-stream 的 stop 词表，tokens 是子任务实耗。"""
    return ev(SUB_E, {"sub_id": sid, "status": status, "tokens": tokens})


SCENARIOS = [
    (
        "plain-reply",
        [
            start("帮我总结一下今天的待办"),
            state("submitted"),
            ev(TOKEN, {"delta": "今天共有"}),
            ev(TOKEN, {"delta": "三项待办："}),
            ev(TOKEN, {"delta": "评审、发布、复盘。"}),
            message("今天共有三项待办：评审、发布、复盘。"),
            finish(),
        ],
    ),
    (
        "tool-call-ok",
        [
            start("读一下 README 的第一段"),
            ev(TOKEN, {"delta": "我来读取文件。"}),
            tool_start("read_file", '{"path":"README.md"}'),
            ev(TOOL_D, {"tool_id": "call-1", "delta": "READ"}),
            tool_end("call-1", "ok", "# AirymaxRT\n\n标准底座。"),
            message("README 开头是项目名与一句话定位。"),
            finish(tool_trace=[{"tool": "read_file", "status": "ok"}]),
        ],
    ),
    (
        "tool-call-fail",
        [
            start("运行测试并修复失败"),
            state("running"),
            tool_start("run_shell", '{"cmd":"cargo test"}', "call-9"),
            tool_end("call-9", "error", "error: test failed, 2 passed; 1 failed"),
            message("测试失败，我先定位失败用例。"),
            finish("failed", failure=failure(512, "tool", "测试未通过", "检查失败用例的断言")),
            ev(
                ERR,
                {
                    "code": -32603,
                    "error_code": 512,
                    "message": "测试未通过：1 个用例失败",
                    "retryable": True,
                    "rounds_used": 2,
                    "rounds_limit": 8,
                    "failure": failure(512, "tool", "测试未通过", "检查失败用例的断言", True),
                },
            ),
        ],
    ),
    (
        "plan-steps",
        [
            start("拆解发布流程", mode="plan", confirmed=True),
            state("submitted", "等待确认"),
            ev(
                "plan",
                {
                    "plan": '["检查工作区","构建三端","发布版本"]',
                    "task_id": "task-0001",
                    "plan_id": "plan-0001",
                    "step_count": 3,
                },
            ),
            ev(
                "step",
                {
                    "task_id": "task-0001",
                    "plan_id": "plan-0001",
                    "step_index": 1,
                    "step_count": 3,
                    "step_status": "active",
                    "step": "检查工作区",
                },
            ),
            ev(
                "step",
                {
                    "task_id": "task-0001",
                    "plan_id": "plan-0001",
                    "step_index": 1,
                    "step_count": 3,
                    "step_status": "done",
                    "step": "检查工作区",
                },
            ),
            message("第一步已完成：工作区干净。"),
            finish(),
        ],
    ),
    (
        "catalog",
        [
            start("有哪些工具可用"),
            ev(
                "catalog",
                {
                    "tools": ["read_file", "write_file", "run_shell", "web_fetch"],
                    "available": 4,
                    "denied": 0,
                },
            ),
            message("当前有四个工具可用：读写文件、执行命令、抓取网页。"),
            finish(),
        ],
    ),
    (
        "cancelled",
        [
            start("遍历整个仓库并生成报告"),
            ev(TOKEN, {"delta": "开始遍历"}),
            message("已取消本次遍历。"),
            finish("failed"),
            ev(
                ERR,
                {
                    "code": -32800,
                    "error_code": 130,
                    "message": "run 已被用户取消",
                    "retryable": False,
                    "rounds_used": 1,
                    "rounds_limit": 8,
                    "failure": failure(130, "cancel", "run 已被用户取消", "确认后重新发起"),
                },
            ),
        ],
    ),
    (
        "empty-reply",
        [
            start("只回一个空串"),
            finish(
                use_ticks=0,
                rounds_used=1,
                rounds_limit=8,
                duration_ms=40,
                think_ms=0,
                llm_ms=0,
                tool_ms=0,
                usage={"prompt_tokens": 8, "completion_tokens": 0, "total_tokens": 8},
            ),
        ],
    ),
    (
        "cjk",
        [
            start("用中文解释一下幂等"),
            ev(TOKEN, {"delta": "幂等是指"}),
            ev(TOKEN, {"delta": "同一操作执行多次"}),
            ev(TOKEN, {"delta": "与执行一次的结果相同。"}),
            message("幂等是指同一操作执行多次与执行一次的结果相同。"),
            finish(),
        ],
    ),
    (
        "markdown-code",
        [
            start("给一个 Rust 的二分查找"),
            message("可以这样写：\n\n```rust\nfn bisect(xs: &[i32], key: i32) -> Option<usize> {\n    xs.binary_search(&key).ok()\n}\n```\n"),
            finish(),
        ],
    ),
    (
        "ansi-escape",
        [
            start("输出带颜色的错误"),
            message("\u001b[31m错误\u001b[0m: 工具执行失败，退出码 1"),
            finish("failed", failure=failure(1, "tool", "工具执行失败", "查看工具输出")),
            ev(
                ERR,
                {
                    "code": -32603,
                    "error_code": 1,
                    "message": "工具执行失败，退出码 1",
                    "retryable": True,
                    "rounds_used": 1,
                    "rounds_limit": 8,
                },
            ),
        ],
    ),
    (
        "long-stream",
        [
            start("分点说明稳定性的三层保障"),
            *[ev(TOKEN, {"delta": f"第{i}层保障。 "}) for i in range(1, 13)],
            message("稳定性分三层：进程、协议、存储。"),
            finish(),
        ],
    ),
    (
        "tool-delta",
        [
            start("统计源码行数"),
            tool_start("run_shell", '{"cmd":"wc -l src/*.rs"}', "call-2"),
            ev(TOOL_D, {"tool_id": "call-2", "delta": "   812 src/app.rs\n"}),
            ev(TOOL_D, {"tool_id": "call-2", "delta": "  1044 src/client.rs\n"}),
            tool_end("call-2", "ok", "1856 total"),
            message("源码合计 1856 行。"),
            finish(),
        ],
    ),
    (
        "plan-unavailable",
        [
            start("规划一次迁移", mode="plan"),
            ev("plan_unavailable", {"reason": "planner daemon 不可达", "task_id": "task-0001"}),
            message("规划服务暂不可用，我按单步模式继续。"),
            finish(),
        ],
    ),
    (
        "two-turns",
        [
            start("第一问：什么是背压"),
            ev(TOKEN, {"delta": "背压是上游限速。"}),
            message("背压是上游限速。"),
            finish(duration_ms=900, rounds_used=1),
            start("第二问：TUI 怎么实现背压"),
            ev(TOKEN, {"delta": "队列满时"}),
            ev(TOKEN, {"delta": "丢弃中间态。"}),
            message("队列满时丢弃中间态，保留首尾。"),
            finish(duration_ms=1100, rounds_used=1),
        ],
    ),
    (
        "stream-interrupted",
        [
            start("持续输出直到中断"),
            ev(TOKEN, {"delta": "已输出部分"}),
            finish("paused", failure=failure(504, "stream", "上游流中断", "重连后继续", True)),
            ev(
                ERR,
                {
                    "code": -32603,
                    "error_code": 504,
                    "message": "上游流中断",
                    "retryable": True,
                    "rounds_used": 1,
                    "rounds_limit": 8,
                    "resume_token": "resume-0001",
                },
            ),
        ],
    ),
    (
        "tool-denied",
        [
            start("删除临时目录", confirmed=True),
            loc(
                "control_request",
                {
                    "type": "control_request",
                    "request_id": "c1",
                    "subtype": "can_use_tool",
                    "tool": "run_shell",
                    "input": "rm -rf /tmp/airy-scratch",
                    "timeout_ms": 30000,
                },
            ),
            tool_end("call-3", "error", "denied by user"),
            message("已跳过删除操作。"),
            finish("failed", failure=failure(403, "control", "用户拒绝该工具调用", "确认后重试")),
            ev(
                ERR,
                {
                    "code": -32603,
                    "error_code": 403,
                    "message": "用户拒绝该工具调用",
                    "retryable": False,
                    "rounds_used": 1,
                    "rounds_limit": 8,
                },
            ),
        ],
    ),
    (
        "heartbeat-notice",
        [
            start("等待上游响应"),
            loc("heartbeat", {"state": "running"}),
            loc("notice", {"message": "上游响应较慢，正在重试"}),
            ev(TOKEN, {"delta": "已恢复。"}),
            loc("heartbeat", {"state": "running"}),
            message("已恢复。"),
            finish(),
        ],
    ),
    (
        "snapshot-resume",
        [
            loc(
                "snapshot",
                {
                    "state": "running",
                    "last_seq": 0,
                    "recent": [
                        {
                            "v": 1,
                            "type": TOKEN,
                            "ts": BASE_TS + 19000,
                            "data": {"delta": "续传前的内容"},
                        }
                    ],
                },
            ),
            start("继续上次的任务"),
            ev(TOKEN, {"delta": "续传后的内容。"}),
            message("已继续并完成。"),
            finish(),
        ],
    ),
    (
        "wide-content",
        [
            start("输出一整段不换行的文本"),
            message(
                "稳定性的前提是可观测性。" * 24
                + "这一行刻意不换行，用于验证窄终端下的折行、截断与滚动边界。"
            ),
            finish(),
        ],
    ),
    (
        "mixed-plan-tool",
        [
            start("检查并修复配置漂移", mode="plan", confirmed=True),
            ev(
                "plan",
                {
                    "plan": '["读取配置","比对基线","回写修正"]',
                    "task_id": "task-0001",
                    "plan_id": "plan-0002",
                    "step_count": 3,
                },
            ),
            tool_start("read_file", '{"path":"etc/agent.yaml"}', "call-4"),
            tool_end("call-4", "ok", "timeout_ms: 3000"),
            ev(TOOL_D, {"tool_id": "call-4", "delta": "diff: timeout_ms 2000 -> 3000"}),
            ev(
                "step",
                {
                    "task_id": "task-0001",
                    "plan_id": "plan-0002",
                    "step_index": 2,
                    "step_count": 3,
                    "step_status": "done",
                    "step": "比对基线",
                },
            ),
            message("配置已与基线对齐，回写一处超时值。"),
            finish(
                verification={
                    "ok": True,
                    "verified_by": "run_verifier",
                    "checks": [{"name": "config_matches_baseline", "ok": True}],
                },
                tool_trace=[{"tool": "read_file", "status": "ok"}],
            ),
        ],
    ),
    (
        "subagent-tree",
        [
            start("核对两处依赖的版本约束并给出结论"),
            ev(TOKEN, {"delta": "分派一个子任务核对依赖。"}),
            sub_start("sub-1", "核对 commons 与 atoms 的版本约束"),
            ev(
                TOOL_S,
                {
                    "tool": "read_file",
                    "tool_id": "call-9",
                    "args": '{"path":"CMakeLists.txt"}',
                    "sub_id": "sub-1",
                    "depth": 1,
                },
            ),
            ev(
                TOOL_E,
                {
                    "tool_id": "call-9",
                    "status": "ok",
                    "result_hash": "两处依赖均已钉定",
                    "sub_id": "sub-1",
                    "depth": 1,
                },
            ),
            sub_end("sub-1", "done", 512),
            message("两处依赖均已钉定，结论一致。"),
            finish(tool_trace=[{"tool": "subagent.spawn", "status": "ok"}]),
        ],
    ),
]
