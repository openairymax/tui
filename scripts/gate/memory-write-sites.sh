#!/bin/sh
# SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
# SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

# 记忆写入口门禁（0.1.21 X1-28a）。
#
# 记忆的写入归运行侧机制（CLI / daemon），TUI 侧只读：本门禁统计源码内
# 记忆写路径的残留——memory.rs 的 push 接口、轮次标注写入口、会话绑定
# 写方法——要求恒为 0（IS-12：本地写入与服务端状态分叉）。
#
# 口径边界：读取侧（recall / recent / refresh / is_ready）不在本门禁内；
# 单测夹具 seed 与 tests/golden 目录属测试面，不参与生产写入口计数；
# 技能写路径（skills.rs 的 save 与 mem.write）由 X1-28b 门禁负责。
#
# 用法：
#   sh scripts/gate/memory-write-sites.sh          校验
#   sh scripts/gate/memory-write-sites.sh --list   打印当前计数

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
BASE="$ROOT/scripts/gate/memory-write-sites.baseline"
SEARCH="$ROOT/src"
MEMFILE="$ROOT/src/memory.rs"

# 记忆写入口模式：写实现、轮次标注写入口、会话绑定写方法。
PATTERNS='push_impl|mem_push_tagged|set_conversation'

# 测试面：单测文件、app/tests 与 golden（仅测试构建编入）。
SKIP='/(memory/tests\.rs|app/tests/|golden/)'

list_hits() {
    grep -rn --include='*.rs' -E "$PATTERNS" $SEARCH 2>/dev/null |
        grep -v -E "$SKIP" |
        grep -v -E ':[0-9]+:[[:space:]]*//'
    if [ -f "$MEMFILE" ]; then
        grep -n -E 'fn push\(' "$MEMFILE" |
            grep -v -E '^[0-9]+:[[:space:]]*//'
    fi
}

count() {
    list_hits | wc -l | tr -d ' \t'
}

base_of() {
    awk -v key="$1" '$1 == key { print $2; hit = 1 } END { if (!hit) print 0 }' "$BASE"
}

if [ ! -d "$ROOT/src" ]; then
    echo "门禁: 找不到源码目录 $ROOT/src" >&2
    exit 2
fi

if [ "${1:-}" = "--list" ]; then
    printf '%-24s %s\n' memory-write "$(count)"
    exit 0
fi

if [ ! -f "$BASE" ]; then
    echo "门禁: 缺少基线文件 $BASE（缺失即失败）" >&2
    exit 2
fi

now=$(count)
was=$(base_of memory-write)

if [ "$now" -gt "$was" ]; then
    echo "门禁: 记忆写入口增长 $was -> $now" >&2
    list_hits >&2
    echo "门禁失败：记忆写入归运行侧机制，TUI 只读。" >&2
    exit 1
fi

if [ "$now" -lt "$was" ]; then
    echo "门禁通过：记忆写入口已减少，请同步下调 scripts/gate/memory-write-sites.baseline。"
else
    echo "门禁通过：记忆写入口为 $now（基线 $was）。"
fi
