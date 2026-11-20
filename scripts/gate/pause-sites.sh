#!/bin/sh
# SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
# SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

# 本地暂停态门禁（0.1.21 X1-27a）。
#
# 暂停是运行结果的事实态，唯一来源是 task_state 事件（C-15 / D-13 /
# IS-12）：网关回报 paused 后由 App::observe_task_state 推进，TUI 不再
# 用按键自持一个「看起来暂停、实际没暂停」的本地状态。本门禁统计源码内
# 本地暂停态的残留——TaskControl::Paused 分支、pause_task / resume_task
# 方法——要求恒为 0。
#
# 口径边界：TaskState::Paused（事件驱动的暂停事实）不在本门禁内；
# 中止（TaskControl::Aborted）是用户按键产生的本地事实，同样不在内。
#
# 用法：
#   sh scripts/gate/pause-sites.sh          校验
#   sh scripts/gate/pause-sites.sh --list   打印当前计数

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
BASE="$ROOT/scripts/gate/pause-sites.baseline"
SEARCH="$ROOT/src"

# 本地暂停态模式：枚举分支与方法名。
PATTERNS='TaskControl::Paused|pause_task|resume_task'

count() {
    grep -rn --include='*.rs' -E "$PATTERNS" $SEARCH 2>/dev/null |
        grep -v -E ':[0-9]+:[[:space:]]*//' |
        wc -l | tr -d ' \t'
}

base_of() {
    awk -v key="$1" '$1 == key { print $2; hit = 1 } END { if (!hit) print 0 }' "$BASE"
}

if [ ! -d "$ROOT/src" ]; then
    echo "门禁: 找不到源码目录 $ROOT/src" >&2
    exit 2
fi

if [ "${1:-}" = "--list" ]; then
    printf '%-24s %s\n' local-pause "$(count)"
    exit 0
fi

if [ ! -f "$BASE" ]; then
    echo "门禁: 缺少基线文件 $BASE（缺失即失败）" >&2
    exit 2
fi

now=$(count)
was=$(base_of local-pause)

if [ "$now" -gt "$was" ]; then
    echo "门禁: 本地暂停态增长 $was -> $now" >&2
    grep -rn --include='*.rs' -E "$PATTERNS" $SEARCH 2>/dev/null |
        grep -v -E ':[0-9]+:[[:space:]]*//' >&2
    echo "门禁失败：暂停请走 task_state 事件（observe_task_state），不得本地自持。" >&2
    exit 1
fi

if [ "$now" -lt "$was" ]; then
    echo "门禁通过：本地暂停态已减少，请同步下调 scripts/gate/pause-sites.baseline。"
else
    echo "门禁通过：本地暂停态为 $now（基线 $was）。"
fi
