#!/bin/sh
# SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
# SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

# 会话呈现调用点门禁（0.1.21 X1-26a）。
#
# session 的呈现查询已收拢为 engine::session 的只读快照：渲染侧先取
# App::session_view()，再读快照字段。本门禁统计 src/ 内对 App 会话状态的
# 直呼——`self.`/`app.` 限定的 tab_count/tab_title/replay_left/list_tabs
# 访问器调用，以及 `.session_tabs` 字段直读——要求恒为 0。
#
# 快照读取（`view.replay_left()` 这类经 session_view 的调用）不计：那正是
# 本卡迁移的目标，不是债。
#
# 豁免（会话状态的归属层，非呈现读取）：
#   src/app/session.rs        快照组装与 tab 槽位写路径
#   src/app/session_store.rs  布局落盘/恢复（写路径，属 D-14 范围）
#   src/app/tests/            用例夹具断言
#
# 用法：
#   sh scripts/gate/session-sites.sh          校验
#   sh scripts/gate/session-sites.sh --list   打印当前计数

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
SRC="$ROOT/src"
BASE="$ROOT/scripts/gate/session-sites.baseline"

# 直呼模式：字段直读 + App 侧访问器调用（限定 self./app. 接收者）。
PATTERNS='\.session_tabs|(self|app)\.tab_count\(|(self|app)\.tab_title\(|(self|app)\.replay_left\(|(self|app)\.list_tabs\('

count() {
    grep -rn --include='*.rs' -E "$PATTERNS" "$SRC" 2>/dev/null |
        grep -v -F '/src/app/session.rs:' |
        grep -v -F '/src/app/session_store.rs:' |
        grep -v -F '/src/app/tests/' |
        grep -v -E ':[0-9]+:[[:space:]]*//' |
        wc -l | tr -d ' \t'
}

# 基线里的计数；未登记视为 0。
base_of() {
    awk -v key="$1" '$1 == key { print $2; hit = 1 } END { if (!hit) print 0 }' "$BASE"
}

if [ ! -d "$SRC" ]; then
    echo "门禁: 找不到源码目录 $SRC" >&2
    exit 2
fi

if [ "${1:-}" = "--list" ]; then
    printf '%-24s %s\n' direct "$(count)"
    exit 0
fi

if [ ! -f "$BASE" ]; then
    echo "门禁: 缺少基线文件 $BASE（缺失即失败）" >&2
    exit 2
fi

now=$(count)
was=$(base_of direct)
total_base=$(base_of total)

if [ "$now" -gt "$was" ]; then
    echo "门禁: 会话呈现直呼增长 $was -> $now" >&2
    grep -rn --include='*.rs' -E "$PATTERNS" "$SRC" 2>/dev/null |
        grep -v -F '/src/app/session.rs:' |
        grep -v -F '/src/app/session_store.rs:' |
        grep -v -F '/src/app/tests/' |
        grep -v -E ':[0-9]+:[[:space:]]*//' >&2
    echo "门禁失败：渲染侧请改用 App::session_view() 的只读快照。" >&2
    exit 1
fi

if [ "$now" -ne "$total_base" ]; then
    printf '门禁: 合计 %s（基线 %s）\n' "$now" "$total_base"
fi

if [ "$now" -lt "$was" ]; then
    echo "门禁通过：直呼已减少，请同步下调 scripts/gate/session-sites.baseline。"
else
    echo "门禁通过：会话呈现直呼为 $now（基线 $was）。"
fi
