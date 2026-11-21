// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 事件客户端（0.1.21 X1-25a，§23.1.5）：TUI 侧唯一的事件/投影数据入口。
//
// 本模块不向 daemon 发任何 RPC：它起一个 `airy_cli --events ndjson` 子进程
// （D-15 出口），把 stdout 的 NDJSON 帧按既有链路折进投影，渲染侧只读快照。
//
//   管道 → Frames（语法过滤，X1-04）
//        → Envelope（契约校验，X1-05）
//        → Tracker（序号断言，X1-08）
//        → map::events（语义归域，X1-06/07）
//        → Projection（唯一折叠点，X1-10）
//
// 读线程是 std 管道的唯一消费者：stdout 交出后只有它推进投影，渲染侧只能
// 克隆快照，无法回写。写方向只有五类输入帧（prompt/paste/cancel/confirm/
// query），与 CLI 的 cli_ev_in.c 一一对应；帧格式由本模块集中构造，调用方
// 不拼 JSON。观察面读统一走 query：CLI 取值后回 query_result，按 id 暂存，
// 面板以 take_query 摘取后自行判读 ok/error，不进入投影事件语义。
//
// 停止语义与 `pipe::Feed` 一致：关 stdin → 宽限 → 强杀 → 回收，随后等读线程
// 看到 EOF。`Clone` 只复制句柄（Arc），最后一个句柄丢弃才停子进程；后台任务
// 可以安全持有一份而不延长会话寿命。
//
// 运行轮与审批已全部收敛到本模块（`app::lane` 唯一入口）。观察面读也已
// 全部迁到 query 帧——tool.list、agent.turns、telemetry、llm.get_stats、
// mem.*、hall.*、运维命令、健康探测与槽位快照均经事件管道取值，网关读侧
// RPC 清零，网关直连客户端已删除（X1-29a）。
//
// 门禁口径按客户端入口计（当前共 0 处），基线见
// scripts/gate/rpc-sites.baseline，上限为 0（X1-29a）。

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::process::{ChildStdout, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

use crate::engine::event::envelope::Envelope;
use crate::engine::event::frame::Frames;
use crate::engine::event::map;
use crate::engine::event::model::{Body, Control, Event};
use crate::engine::event::pipe::{Feed, Spec, STOP_GRACE};
use crate::engine::event::seq::Watch;
use crate::engine::project::Projection;

/// 折叠游标与丢弃计数：读线程写，渲染侧读。
///
/// `watermark` 取自序号断言器而非投影：一帧可能不产出事件（未识别类型），
/// 但它仍然占号；续收握手必须用「已消费到第几帧」而不是「折了几条事件」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stats {
    pub(crate) applied: u64,
    pub(crate) watermark: u64,
    pub(crate) bad: u64,
    pub(crate) rejects: u64,
    pub(crate) gaps: u64,
    pub(crate) repeats: u64,
    pub(crate) unknown: u64,
    pub(crate) ended: bool,
}

/// 事件客户端：起事件源、写输入帧、把事件流折成投影快照。
#[derive(Clone)]
pub(crate) struct EventClient {
    inner: Arc<Inner>,
}

struct Inner {
    feed: Mutex<Feed>,
    state: Arc<Mutex<State>>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

/// 折叠状态：投影、序号断言器、查询应答与丢弃计数。
struct State {
    proj: Projection,
    watch: Watch,
    queries: HashMap<String, Value>,
    last_seq: u64,
    last_ts: u64,
    bad: u64,
    rejects: u64,
    unknown: u64,
    ended: bool,
}

impl EventClient {
    /// 从流首起（D-15 的 seq 自 1 开始）。
    pub(crate) fn start(spec: &Spec) -> io::Result<Self> {
        Self::open(spec, 0)
    }

    /// 断线续收：hello 带上已消费的水位，序号断言从下一帧续接。
    pub(crate) fn resume(spec: &Spec, last_seq: u64) -> io::Result<Self> {
        Self::open(spec, last_seq)
    }

    /// 起子进程 → 发 hello → 交出 stdout → 起读线程。
    ///
    /// hello 先于读线程发出：CLI 可能立刻回帧，先交出 stdout 再握手会让首帧
    /// 堆积在管道里（无害但延迟），而先握手后起线程不会丢帧——管道有缓冲。
    fn open(spec: &Spec, last_seq: u64) -> io::Result<Self> {
        let mut feed = Feed::spawn(spec)?;
        let hello = json!({"type": "hello", "last_seq": last_seq});
        feed.send(&hello.to_string())?;
        let out = feed
            .take_out()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "事件源 stdout 读端缺失"))?;

        let state = Arc::new(Mutex::new(State {
            proj: Projection::new(),
            watch: Watch::resume(last_seq),
            queries: HashMap::new(),
            last_seq,
            last_ts: 0,
            bad: 0,
            rejects: 0,
            unknown: 0,
            ended: false,
        }));
        let reader = std::thread::Builder::new()
            .name("airy-events".into())
            .spawn({
                let state = Arc::clone(&state);
                move || pump(out, state)
            })?;

        Ok(Self {
            inner: Arc::new(Inner {
                feed: Mutex::new(feed),
                state,
                reader: Mutex::new(Some(reader)),
            }),
        })
    }

    /// 下发一轮输入。CLI 侧以 `kind` 分派，帧格式只在本模块构造。
    ///
    /// `opts` 是该轮运行侧选项（history / model / gccp_answers / resume_token
    /// 等），CLI 将其并入 task.submit_stream 的 params；goal/mode/client 仍由
    /// CLI 权威赋值。None 表示纯文本轮。
    pub(crate) fn prompt(&self, text: &str, opts: Option<&Value>) -> io::Result<()> {
        let mut frame = json!({"type": "input", "kind": "prompt", "text": text});
        if let Some(o) = opts {
            frame["opts"] = o.clone();
        }
        self.write(&frame)
    }

    /// 粘贴输入：与 prompt 同语义，只为保留终端侧的粘贴来源。
    pub(crate) fn paste(&self, text: &str) -> io::Result<()> {
        self.write(&json!({"type": "input", "kind": "paste", "text": text}))
    }

    /// 中止当前运行；无运行中任务时由 CLI 裁决，客户端不做本地判断。
    pub(crate) fn cancel(&self) -> io::Result<()> {
        self.write(&json!({"type": "input", "kind": "cancel"}))
    }

    /// 回应审批请求：request_id 必须来自 `control_request` 帧。
    pub(crate) fn confirm(&self, request_id: &str, allow: bool) -> io::Result<()> {
        let behavior = if allow { "allow" } else { "deny" };
        self.write(&json!({
            "type": "input",
            "kind": "confirm",
            "request_id": request_id,
            "behavior": behavior,
        }))
    }

    /// 观察面读：只读面板数据，不走运行事件语义。
    ///
    /// `id` 由调用方生成并在应答里回显，用于把异步返回配回发起查询的面板；
    /// 结果由 `take_query` 摘取，未摘取的应答留在表里（按 id 覆盖）。
    pub(crate) fn query(&self, id: &str, method: &str, params: &Value) -> io::Result<()> {
        self.write(&json!({
            "type": "input",
            "kind": "query",
            "id": id,
            "method": method,
            "params": params,
        }))
    }

    /// 摘取一条查询应答（`{"id","ok","result"|"error"}`），取走即删。
    pub(crate) fn take_query(&self, id: &str) -> Option<Value> {
        lock(&self.inner.state).queries.remove(id)
    }

    /// 回应审批并把本地裁决折进投影：同一请求只可能有一个结论。
    ///
    /// 帧直达 CLI 后，投影表不会自己变——线上没有「裁决已收到」的回帧，
    /// 行会悬读到超时。这里按事件流最后时刻补一条本地 `Control::Answer`，
    /// 让界面与用户动作同拍收口；不推进 Watch，续传水位仍只由线上帧推进。
    pub(crate) fn answer(&self, request_id: &str, allow: bool) -> io::Result<()> {
        self.confirm(request_id, allow)?;
        let mut s = lock(&self.inner.state);
        let ev = Event {
            seq: s.last_seq,
            ts: s.last_ts,
            body: Body::Control(Control::Answer {
                id: request_id.to_owned(),
                ok: allow,
            }),
        };
        s.proj.apply(&ev);
        Ok(())
    }

    fn write(&self, frame: &Value) -> io::Result<()> {
        let mut feed = lock(&self.inner.feed);
        feed.send(&frame.to_string())
    }

    /// 当前投影快照：克隆后与后续帧无关，渲染侧可放心跨帧持有。
    pub(crate) fn snapshot(&self) -> Projection {
        lock(&self.inner.state).proj.clone()
    }

    /// 折叠游标与丢弃计数（状态栏与诊断直接读）。
    pub(crate) fn stats(&self) -> Stats {
        let s = lock(&self.inner.state);
        Stats {
            applied: s.proj.applied(),
            watermark: s.watch.next().saturating_sub(1),
            bad: s.bad,
            rejects: s.rejects,
            gaps: s.watch.gaps(),
            repeats: s.watch.repeats(),
            unknown: s.unknown,
            ended: s.ended,
        }
    }

    /// 子进程是否仍存活：不等待、不回收，只做非阻塞判定。
    pub(crate) fn alive(&self) -> bool {
        let mut feed = lock(&self.inner.feed);
        matches!(feed.alive(), Ok(None))
    }

    /// 子进程标识（日志与诊断用）。
    pub(crate) fn pid(&self) -> u32 {
        lock(&self.inner.feed).pid()
    }

    /// 停止并回收：子进程先退，读线程随后看到 EOF。
    ///
    /// 先停子进程再 join：反过来会在子进程仍写 stdout 时等一个不会到来的
    /// EOF。宽限期到点即强杀，join 因此有上界。
    pub(crate) fn stop(&self, grace: Duration) -> io::Result<ExitStatus> {
        let status = {
            let mut feed = lock(&self.inner.feed);
            feed.stop(grace)
        };
        let handle = lock(&self.inner.reader).take();
        if let Some(h) = handle {
            let _ = h.join();
        }
        status
    }
}

/// 读线程：逐帧折叠，EOF 或读错误即收尾。
///
/// 单帧的坏不影响整条链路：语法层已滤掉非对象行，契约层拒收版本不匹配的
/// 帧，语义层对未识别类型只计数不产出。读取错误（管道断裂）无法恢复，
/// 记一次告警后退出——重连归 X1-19，不在本模块偷偷重试。
fn pump(out: ChildStdout, state: Arc<Mutex<State>>) {
    let mut frames = Frames::new(BufReader::new(out));
    loop {
        let frame = match frames.next() {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => {
                log::warn!("事件流读取失败：{e}");
                break;
            }
        };
        let env = match Envelope::read(frame) {
            Ok(env) => env,
            Err(_) => {
                let mut s = lock(&state);
                s.bad = frames.bad();
                s.rejects = s.rejects.saturating_add(1);
                continue;
            }
        };
        let mut s = lock(&state);
        s.bad = frames.bad();
        let _ = s.watch.feed(env.seq);
        s.last_seq = s.last_seq.max(env.seq);
        s.last_ts = s.last_ts.max(env.ts);
        if env.kind == "query_result" {
            if let Some(id) = env.payload["id"].as_str() {
                s.queries.insert(id.to_owned(), env.payload);
            }
            continue;
        }
        let events = map::events(env);
        if events.is_empty() {
            s.unknown = s.unknown.saturating_add(1);
            continue;
        }
        for ev in &events {
            s.proj.apply(ev);
        }
    }
    let mut s = lock(&state);
    s.bad = frames.bad();
    s.ended = true;
}

/// 取锁并吞掉毒化：锁内只有计数器与纯数据，前一个 panic 不影响后续读取。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Err(e) = lock(&self.feed).stop(STOP_GRACE) {
            log::warn!("事件源子进程回收失败：{e}");
        }
        let handle = lock(&self.reader).take();
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EventClient;
    use crate::engine::event::pipe::Spec;
    use crate::engine::project::Phase;
    use std::time::{Duration, Instant};

    /// 子进程角色标记：只有带此变量的子测试进程才执行帧逻辑。
    const CHILD_ENV: &str = "AIRY_EV_CLIENT_CHILD";

    /// 子进程用例全名（宿主以 `--exact` 只跑它）。
    const CHILD: &str = "client::event::tests::child";

    /// 起一个本测试进程的副本，按角色驱动管道。
    fn spec(mode: &str) -> Spec {
        let exe = std::env::current_exe().expect("测试二进制路径");
        Spec::new(
            &exe.to_string_lossy(),
            &[
                "--exact",
                CHILD,
                "--nocapture",
                "--quiet",
                "--test-threads=1",
            ],
        )
        .env(CHILD_ENV, mode)
    }

    /// 轮询直到条件成立；超时即失败（用例的唯一时间上界）。
    fn until(what: &str, cond: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if cond() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("等待超时：{what}");
    }

    /// 事件流端到端：hello 被接收 → prompt 开跑 → run_end 收口 → 快照与
    /// 水位到位 → 停止后子进程正常退出。
    #[test]
    fn folds_frames_into_snapshot() {
        let c = EventClient::start(&spec("session")).expect("事件客户端可起");
        assert!(c.pid() > 0, "子进程标识可读");
        until("心跳入表", || c.stats().applied >= 1);
        c.prompt("你好", None).expect("prompt 帧可写");
        until("运行收口", || c.stats().ended);

        let st = c.stats();
        assert_eq!(st.applied, 3, "心跳 + 开跑 + 收口各折一条");
        assert_eq!(st.watermark, 3, "水位按帧号推进");
        // 子进程是本测试二进制的 libtest 宿主，用例开跑前的横幅行必然先于首帧
        // 抵达，按语法过滤掉它们正是 Frames 的约定；此处只断言语义层零丢弃。
        assert_eq!((st.rejects, st.gaps, st.unknown), (0, 0, 0));
        let snap = c.snapshot();
        assert_eq!(snap.phase(), Phase::Closed);
        assert_eq!(snap.tasks().len(), 1, "任务身份随 run_start 入表");

        let status = c.stop(Duration::from_secs(5)).expect("停止可完成");
        assert!(status.success(), "读到 EOF 的子进程应正常退出");
        assert!(!c.alive(), "停止后不再存活");
    }

    /// 三类丢弃各自计数：坏行、版本不符、未识别类型；序号跳号只报一次。
    #[test]
    fn counts_dropped_frames_by_cause() {
        let c = EventClient::start(&spec("messy")).expect("事件客户端可起");
        until("流结束", || c.stats().ended);

        let st = c.stats();
        assert_eq!(st.applied, 1, "只有心跳能折出事件");
        assert_eq!(st.rejects, 1, "schema 不匹配的帧被拒收");
        assert_eq!(st.unknown, 1, "未识别类型只计数不产出");
        assert_eq!(st.gaps, 1, "seq 1→3 只报一次断点");
        assert_eq!(st.repeats, 0);
        assert!(st.bad >= 1, "非 JSON 行计入坏帧");
        assert_eq!(st.watermark, 3, "被拒收的帧不推进水位");

        c.stop(Duration::from_secs(5)).expect("停止可完成");
    }

    /// 续收握手：hello 带上次水位，断言器从下一帧续接。
    #[test]
    fn resume_carries_last_consumed_seq() {
        let c = EventClient::resume(&spec("hello7"), 7).expect("事件客户端可起");
        assert_eq!(c.stats().watermark, 7, "水位继承自握手");
        let status = c.stop(Duration::from_secs(5)).expect("停止可完成");
        assert!(status.success(), "子进程须收到 last_seq=7 后正常退出");
    }

    /// 停止后五类输入帧一律失败：不重试、不降级，错误种类可判定。
    ///
    /// 五帧同测而不是只测 prompt：写路径共用 `write`，但帧构造函数各不相同，
    /// 任一构造错误都必须在同一处暴露，不能等接线时才在运行期发现。
    #[test]
    fn writes_fail_after_stop() {
        let c = EventClient::start(&spec("hang")).expect("事件客户端可起");
        c.stop(Duration::from_millis(50)).expect("停止可完成");
        let errs = [
            c.prompt("x", None).expect_err("prompt 写端已关"),
            c.paste("y").expect_err("paste 写端已关"),
            c.cancel().expect_err("cancel 写端已关"),
            c.confirm("c1", true).expect_err("confirm 写端已关"),
            c.query("q1", "hall.board", &serde_json::json!({}))
                .expect_err("query 写端已关"),
        ];
        for err in errs {
            assert_eq!(err.kind(), std::io::ErrorKind::NotConnected);
        }
    }

    /// 查询应答：按 id 暂存、摘取即删，且不进入投影事件流。
    #[test]
    fn answers_query_by_id() {
        let c = EventClient::start(&spec("query")).expect("事件客户端可起");
        until("心跳入表", || c.stats().applied >= 1);
        c.query("q7", "hall.board", &serde_json::json!({"limit": 2}))
            .expect("query 帧可写");
        until("应答入表", || c.stats().watermark >= 2);

        let v = c.take_query("q7").expect("应答按 id 可摘");
        assert_eq!(v["ok"], serde_json::json!(true));
        assert_eq!(v["result"]["agents"][0]["id"], serde_json::json!("a1"));
        assert!(c.take_query("q7").is_none(), "摘取即删");
        let st = c.stats();
        assert_eq!((st.applied, st.unknown), (1, 0), "应答不产事件、不计未知");

        let status = c.stop(Duration::from_secs(5)).expect("停止可完成");
        assert!(status.success(), "查询应答后子进程正常退出");
    }

    /// 克隆共享同一条流：投影与存活判定对两个句柄一致。
    #[test]
    fn clones_share_one_stream() {
        let c = EventClient::start(&spec("session")).expect("事件客户端可起");
        let c2 = c.clone();
        until("心跳入表", || c2.stats().applied >= 1);
        assert_eq!(c.snapshot(), c2.snapshot());
        c.stop(Duration::from_millis(50)).expect("停止可完成");
        assert!(!c2.alive(), "停进程对所有句柄生效");
    }

    /// 子进程侧：按角色读写管道，写完即退出（不经过 libtest 汇总）。
    /// 未带角色标记时直接返回——宿主套件里的同名用例是空操作。
    #[test]
    fn child() {
        match std::env::var(CHILD_ENV).as_deref() {
            Ok("session") => child_session(),
            Ok("messy") => child_messy(),
            Ok("hello7") => child_hello(7),
            Ok("query") => child_query(),
            Ok("hang") => std::thread::sleep(Duration::from_secs(30)),
            _ => {}
        }
    }

    /// 会话角色：读 hello → 回心跳；读 prompt → 回开跑与收口。
    fn child_session() {
        if !line().contains("\"hello\"") {
            std::process::exit(2);
        }
        emit(r#"{"schema":1,"seq":1,"ts":1,"type":"heartbeat","payload":{}}"#);
        if !line().contains("\"prompt\"") {
            std::process::exit(2);
        }
        emit(r#"{"schema":1,"seq":2,"ts":2,"type":"run_start","payload":{"task_id":"t1"}}"#);
        emit(r#"{"schema":1,"seq":3,"ts":3,"type":"run_end","payload":{"status":"completed"}}"#);
        std::process::exit(0);
    }

    /// 脏流角色：坏行、旧版本帧、跳号与未识别类型各一。
    fn child_messy() {
        emit("not a frame");
        emit(r#"{"schema":2,"seq":1,"ts":0,"type":"heartbeat","payload":{}}"#);
        emit(r#"{"schema":1,"seq":1,"ts":0,"type":"heartbeat","payload":{}}"#);
        emit(r#"{"schema":1,"seq":3,"ts":0,"type":"mystery","payload":{}}"#);
        std::process::exit(0);
    }

    /// 查询角色：读 hello → 回心跳；读 query 帧 → 按 id 回 query_result。
    fn child_query() {
        if !line().contains("\"hello\"") {
            std::process::exit(2);
        }
        emit(r#"{"schema":1,"seq":1,"ts":1,"type":"heartbeat","payload":{}}"#);
        let q = line();
        if !q.contains("\"query\"")
            || !q.contains("\"hall.board\"")
            || !q.contains("\"q7\"")
            || !q.contains("\"limit\"")
        {
            std::process::exit(2);
        }
        emit(
            r#"{"schema":1,"seq":2,"ts":2,"type":"query_result","payload":{"id":"q7","ok":true,"result":{"agents":[{"id":"a1"}]}}}"#,
        );
        std::process::exit(0);
    }

    /// 握手校验角色：last_seq 不对即以 2 退出。
    fn child_hello(want: u64) {
        let hello = line();
        let ok = hello.contains(&format!("\"last_seq\":{want}"));
        std::process::exit(if ok { 0 } else { 2 });
    }

    /// 读一行 stdin；EOF 返回空串，调用方据此判失败。
    fn line() -> String {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).expect("子进程读 stdin");
        s
    }

    /// 写一行帧并立即刷出：父进程按行消费，不依赖退出时的缓冲区。
    fn emit(text: &str) {
        use std::io::Write;
        println!("{text}");
        std::io::stdout().flush().expect("子进程刷 stdout");
    }
}
