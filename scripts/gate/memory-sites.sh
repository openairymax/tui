#!/bin/sh
# SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
# SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

# 记忆呈现读取门禁（0.1.21 X1-26b）。
#
# 记忆库的呈现读取已收拢为 engine::memory 的只读快照：渲染侧先取
# App::memory_snapshot()/memory_records()，再读快照字段。本门禁统计呈现层
# （src/panels、src/present）对 App 记忆后端的直呼——`self.`/`app.` 限定的
# `.memory` 字段读取——要求恒为 0。
#
# 口径边界：app 层的召回（recall）、写入（push）与再水合（refresh）是编排/
# 写路径，不是呈现读取，不在本门禁内；`memory_view`（面板缓存）亦非后端
# 直呼，模式尾部的词边界已将其排除。
#
# 用法：
#   sh scripts/gate/memory-sites.sh          校验
#   sh scripts/gate/memory-sites.sh --list   打印当前计数

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
BASE="$ROOT/scripts/gate/memory-sites.baseline"
SEARCH="$ROOT/src/panels $ROOT/src/present"

# 直呼模式：字段直读（词边界排除 memory_view 等派生名）。
PATTERNS='(self|app)\.memory\b'

count() {
    grep -rn --include='*.rs' -E "$PATTERNS" $SEARCH 2>/dev/null |
        grep -v -E ':[0-9]+:[[:space:]]*//' |
        wc -l | tr -d ' \t'
}

# 基线里的计数；未登记视为 0。
base_of() {
    awk -v key="$1" '$1 == key { print $2; hit = 1 } END { if (!hit) print 0 }' "$BASE"
}

if [ ! -d "$ROOT/src/panels" ]; then
    echo "门禁: 找不到呈现层目录 $ROOT/src/panels" >&2
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

if [ "$now" -gt "$was" ]; then
    echo "门禁: 记忆呈现直呼增长 $was -> $now" >&2
    grep -rn --include='*.rs' -E "$PATTERNS" $SEARCH 2>/dev/null |
        grep -v -E ':[0-9]+:[[:space:]]*//' >&2
    echo "门禁失败：渲染侧请改用 App::memory_snapshot()/memory_records()。" >&2
    exit 1
fi

if [ "$now" -lt "$was" ]; then
    echo "门禁通过：直呼已减少，请同步下调 scripts/gate/memory-sites.baseline。"
else
    echo "门禁通过：记忆呈现直呼为 $now（基线 $was）。"
fi
