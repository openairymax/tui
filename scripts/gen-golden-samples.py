#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
# SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0
#
# 生成 golden 事件样本（0.1.21 X1-31b）。
#
# 每份样本是一段完整的 run，等宽 NDJSON：一行一帧，帧头固定为
#   {"schema":1,"seq":N,"ts":<epoch-ms>,"type":"<type>","payload":{...}}
# 与 airy_cli --events ndjson 的产物同构，因此样本既可由本脚本生成，
# 也可用 AIRY_TUI_EVENT_RECORD=1 现场录制后同样入库。
#
# 场景表在 golden_scenarios.py，本脚本只做编码：服务端透传帧的 payload 保持
# run-stream 事件信封原样（v/type/id/run_id/session_id/ts/epoch/data），本地帧
# 的 payload 与 cli_ev 装配一致。编码不裁剪语义，同一份场景表重复执行产出
# 逐字节相同的样本。
#
# 用法：scripts/gen-golden-samples.py [--out DIR]
# 退出码：0 成功；2 场景表或输出目录异常。

import argparse
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import golden_scenarios as sc  # noqa: E402

COUNT = 21


def render(index, frames):
    """把一份场景编码为 NDJSON 文本。"""
    run, sess = f"run-{index:02d}", f"sess-{index:02d}"
    lines = []
    for seq, (kind, kind_type, data) in enumerate(frames, start=1):
        ts = sc.BASE_TS + index * 1000 + seq * sc.STEP_MS
        payload = data
        if kind == "evt":
            payload = {
                "v": 1,
                "type": kind_type,
                "id": f"{run}-e{seq}",
                "run_id": run,
                "session_id": sess,
                "ts": ts,
                "epoch": 1,
                "data": data,
            }
        frame = {"schema": 1, "seq": seq, "ts": ts, "type": kind_type, "payload": payload}
        lines.append(json.dumps(frame, ensure_ascii=False, separators=(",", ":")))
    return "\n".join(lines) + "\n"


def main():
    root = pathlib.Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description="生成 golden 事件样本（X1-31b）")
    ap.add_argument("--out", type=pathlib.Path, default=root / "golden" / "samples")
    args = ap.parse_args()

    if len(sc.SCENARIOS) != COUNT:
        print(f"场景数须为 {COUNT}，实际 {len(sc.SCENARIOS)}", file=sys.stderr)
        return 2

    try:
        args.out.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        print(f"无法创建输出目录 {args.out}: {e}", file=sys.stderr)
        return 2

    for stale in args.out.glob("sample-*.jsonl"):
        stale.unlink()
    for index, (name, frames) in enumerate(sc.SCENARIOS, start=1):
        path = args.out / f"sample-{index:02d}-{name}.jsonl"
        with path.open("w", encoding="utf-8", newline="\n") as out:
            out.write(render(index, frames))
    print(f"wrote {len(sc.SCENARIOS)} samples to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
