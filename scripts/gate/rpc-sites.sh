#!/bin/sh
# SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
# SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

# 网关 RPC 调用点棘轮门禁（0.1.21 X1-25b）。
#
# 迁移期的硬约束不是「立刻归零」，而是「只减不增」：事件/投影 API 尚未覆盖
# 的读路径仍走网关，一旦放任新增调用点，迁移目标会在实现过程中被稀释。本
# 门禁按方法统计 src/ 内的生产调用点（测试文件不计），与基线逐项比对——
# 任一项高于基线即失败，低于基线则提示下调，保证账本与代码同步收敛。
#
# 基线由任务卡推进时下调，终局为 0（X1-29a 退役 GatewayClient）：
#   sh scripts/gate/rpc-sites.sh          校验
#   sh scripts/gate/rpc-sites.sh --list   打印当前计数
#
# 只数生产代码：测试文件（tests.rs）里的调用属于用例夹具，不构成迁移债。

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
SRC="$ROOT/src"
BASE="$ROOT/scripts/gate/rpc-sites.baseline"

# 受控方法：网关客户端向外发 RPC 的入口，按迁移批次登记。
METHODS="health_check send_message cancel_session resolve_approval list_pending_approvals rpc_call"

count() {
    grep -rn --include='*.rs' --exclude='tests.rs' -E "\.$1\(" "$SRC" 2>/dev/null |
        wc -l | tr -d ' \t'
}

# 基线里某方法的计数；未登记视为 0。
base_of() {
    awk -v key="$1" '$1 == key { print $2; hit = 1 } END { if (!hit) print 0 }' "$BASE"
}

if [ ! -d "$SRC" ]; then
    echo "门禁: 找不到源码目录 $SRC" >&2
    exit 2
fi

if [ "${1:-}" = "--list" ]; then
    for m in $METHODS; do
        printf '%-24s %s\n' "$m" "$(count "$m")"
    done
    exit 0
fi

if [ ! -f "$BASE" ]; then
    echo "门禁: 缺少基线文件 $BASE（迁移期必须显式登记，缺失即失败）" >&2
    exit 2
fi

total_now=0
total_base=0
grew=0
shrunk=""

for m in $METHODS; do
    now=$(count "$m")
    was=$(base_of "$m")
    total_now=$((total_now + now))
    total_base=$((total_base + was))
    if [ "$now" -gt "$was" ]; then
        printf '门禁: %s 调用点增长 %s -> %s\n' "$m" "$was" "$now" >&2
        grew=1
    elif [ "$now" -lt "$was" ]; then
        printf '门禁: %s 调用点下降 %s -> %s，请下调基线\n' "$m" "$was" "$now"
        shrunk="$shrunk $m"
    fi
done

if [ "$total_now" -ne "$total_base" ]; then
    printf '门禁: 合计 %s（基线 %s）\n' "$total_now" "$total_base"
fi

if [ "$grew" -ne 0 ]; then
    echo "门禁失败：迁移期禁止新增网关 RPC 调用点，请改用事件/投影 API。" >&2
    exit 1
fi

if [ -n "$shrunk" ]; then
    echo "门禁通过：调用点已减少，请同步下调 scripts/gate/rpc-sites.baseline。"
else
    echo "门禁通过：RPC 调用点与基线一致（合计 $total_now）。"
fi
