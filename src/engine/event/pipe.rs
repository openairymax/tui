// SPDX-FileCopyrightText: 2025-2026 SPHARX Ltd.
// SPDX-License-Identifier: AGPL-3.0-or-later OR Apache-2.0

// Copyright (c) 2026 SPHARX Ltd. All Rights Reserved.
//
// 事件源管道（0.1.21 X1-01a，§23.1.5 子进程模型，对接 D-15 事件流出口）。
//
// 数据面由 `airy_cli --events ndjson` 子进程独占：stdout 只写 NDJSON 帧，
// stdin 收输入帧（hello/prompt/paste/cancel/control_response），stderr 是
// 人类可读输出、不属于任何契约。本模块只做三件事——起进程、持有两端管道
// 句柄、按时限停止；不解析帧（X1-02）、不做重连（X1-19）、不定策略（策略
// 归 CLI）。子进程起不来即失败，绝不退回直连 gateway 的旧路径。
//
// 停止语义：先关 stdin，D-15 以 EOF 结束会话；宽限期内等其自行写完终态帧，
// 超时才强杀后回收。任何返回路径上子进程都已回收，不留僵尸。TUI 侧不设
// PDEATHSIG：渲染进程被强杀时，CLI 会话与 daemon 任务必须活得比它久。
//
// 断裂语义：子进程先退时，写端立刻收到 BrokenPipe（std 已按平台把 EPIPE 与
// ERROR_NO_DATA 归一），调用方据错误种类即可判定「链路已断」，无需解析文案；
// 已退出的子进程再 stop 仍返回真实终态，不误报为强杀失败。

#![cfg_attr(not(test), allow(dead_code))]

use std::io::{self, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

/// 停止宽限期：CLI 收到 EOF 后要补齐终态帧，给足写预算再强杀。
pub(crate) const STOP_GRACE: Duration = Duration::from_millis(500);

/// 宽限期内的回收轮询间隔。
const POLL: Duration = Duration::from_millis(10);

/// 子进程规格：程序、参数与环境。与 `Spec::cli` 解耦，用例可注入任意进程。
pub(crate) struct Spec {
    pub(crate) prog: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
}

impl Spec {
    /// 任意程序（用例与集成侧注入点）。
    pub(crate) fn new(prog: &str, args: &[&str]) -> Self {
        Self {
            prog: prog.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: Vec::new(),
        }
    }

    /// 追加一条子进程环境变量（只影响该子进程，不改本进程环境）。
    pub(crate) fn env(mut self, key: &str, val: &str) -> Self {
        self.env.push((key.to_string(), val.to_string()));
        self
    }

    /// D-15 出口：`airy_cli --events ndjson`，不带 -p，首轮 prompt 由 stdin
    /// 输入帧下发（管道多轮，见 cli_ev.h）。
    pub(crate) fn cli() -> Self {
        Self::new(&cli_bin(), &["--events", "ndjson"])
    }
}

/// `airy_cli` 的解析规则（F8 切换与事件源共用一处，避免两条路径各判一次）。
///
/// AIRY_HOME/HOME 均在时才落 `$AIRY_HOME/bin/airy_cli`；都缺失时按 PATH 查
/// 找——不能落到 `paths::airy_home()` 的相对回退（会去 ./.airymaxrt/bin）。
pub(crate) fn cli_bin() -> String {
    let env_present = std::env::var_os("AIRY_HOME").is_some() || std::env::var_os("HOME").is_some();
    if env_present {
        crate::paths::airy_home()
            .join("bin")
            .join("airy_cli")
            .to_string_lossy()
            .into_owned()
    } else {
        "airy_cli".to_string()
    }
}

/// 事件源子进程：stdout/stdin 两端管道由 `Feed` 持有，按需移交读取器与
/// 写入器；`Feed` 自身只保留进程句柄，负责存活判定与回收。
pub(crate) struct Feed {
    child: Child,
}

impl Feed {
    /// 起子进程并接上三根管道。stderr 丢弃：人类可读输出属于 CLI 直出的
    /// 终端画面，混进 alt screen 只会污染渲染。
    pub(crate) fn spawn(spec: &Spec) -> io::Result<Self> {
        let mut cmd = Command::new(&spec.prog);
        cmd.args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, val) in &spec.env {
            cmd.env(key, val);
        }
        Ok(Self {
            child: cmd.spawn()?,
        })
    }

    /// 取走 stdout 读端：帧读取器是唯一持有者，重复调用返回 `None`。
    pub(crate) fn take_out(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// 写一行输入帧（自动补换行并刷出）。子进程先退时返回 BrokenPipe；
    /// `stop` 之后写端已关，返回 NotConnected——两者都不重试、不降级。
    pub(crate) fn send(&mut self, text: &str) -> io::Result<()> {
        let Some(sink) = self.child.stdin.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "输入管道已关闭",
            ));
        };
        sink.write_all(text.as_bytes())?;
        sink.write_all(b"\n")?;
        sink.flush()
    }

    /// 子进程标识（日志与诊断用）。
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 非阻塞存活判定：返回 `Some(终态)` 表示已退出并已回收。
    pub(crate) fn alive(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    /// 停止并回收：关 stdin → 宽限等待 → 强杀 → 回收，返回终态。
    pub(crate) fn stop(&mut self, grace: Duration) -> io::Result<std::process::ExitStatus> {
        // 丢弃写端即关闭管道，CLI 侧读到 EOF 后走正常收尾。
        let _ = self.child.stdin.take();
        let started = Instant::now();
        while started.elapsed() < grace {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(POLL);
        }
        // 强杀与自行退出存在竞态：kill 失败时以 wait 的实际结果为准。
        if let Err(e) = self.child.kill() {
            return self.child.wait().map_err(|_| e);
        }
        self.child.wait()
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        if let Err(e) = self.stop(STOP_GRACE) {
            log::warn!("事件源子进程回收失败：{e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cli_bin, Feed, Spec};
    use crate::test_env::Home;
    #[cfg(unix)]
    use std::io::Read;
    use std::io::Write;
    #[cfg(unix)]
    use std::process::Command;
    use std::time::{Duration, Instant};

    /// 子进程自证标记：只有带此变量的子测试进程才执行管道逻辑。
    const CHILD_ENV: &str = "AIRY_EV_CHILD";

    /// 一个必然活过用例时长的子进程（不依赖 shell，三端同名语义）。
    #[cfg(unix)]
    fn idle() -> Spec {
        Spec::new("sleep", &["30"])
    }

    #[cfg(windows)]
    fn idle() -> Spec {
        Spec::new("ping", &["-n", "30", "127.0.0.1"])
    }

    /// 立刻以退出码 3 结束的子进程：用于「先退」路径。
    #[cfg(unix)]
    fn brief() -> Spec {
        Spec::new("sh", &["-c", "exit 3"])
    }

    #[cfg(windows)]
    fn brief() -> Spec {
        Spec::new("cmd", &["/C", "exit 3"])
    }

    /// D-15 出口固定为 `airy_cli --events ndjson`，路径按 AIRY_HOME 解析。
    #[test]
    fn cli_spec_matches_d15_outlet() {
        let home = Home::new("evspec");
        let spec = Spec::cli();
        assert_eq!(
            std::path::Path::new(&spec.prog),
            home.path().join("bin").join("airy_cli")
        );
        assert_eq!(spec.args, vec!["--events", "ndjson"]);
    }

    /// 环境全缺失时按 PATH 查找，不落相对目录。
    #[test]
    fn cli_bin_falls_back_to_path() {
        let _lock = crate::test_env::lock_env();
        let saved_home = std::env::var_os("AIRY_HOME");
        let saved_user = std::env::var_os("HOME");
        std::env::remove_var("AIRY_HOME");
        std::env::remove_var("HOME");
        assert_eq!(cli_bin(), "airy_cli");
        if let Some(v) = saved_home {
            std::env::set_var("AIRY_HOME", v);
        }
        if let Some(v) = saved_user {
            std::env::set_var("HOME", v);
        }
    }

    /// spawn 可起：子进程存活，stdout 读端只有一个持有者。
    #[cfg(any(unix, windows))]
    #[test]
    fn spawn_holds_both_pipes() {
        let mut feed = Feed::spawn(&idle()).expect("空闲子进程可起");
        assert!(feed.pid() > 0);
        assert!(
            feed.alive().expect("存活可判").is_none(),
            "刚起的子进程不应已退出"
        );
        assert!(feed.take_out().is_some(), "stdout 读端可移交");
        assert!(feed.take_out().is_none(), "stdout 读端只移交一次");
        feed.send("hello").expect("空闲子进程的 stdin 仍可写");
    }

    /// spawn 可停：宽限期超时后强杀并回收，返回时已无存活子进程。
    #[cfg(any(unix, windows))]
    #[test]
    fn stop_reaps_within_grace() {
        let mut feed = Feed::spawn(&idle()).expect("空闲子进程可起");
        let started = Instant::now();
        let status = feed.stop(Duration::from_millis(50)).expect("停止可完成");
        assert!(!status.success(), "被强杀的进程不应报成功");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "宽限后须即刻回收"
        );
        assert!(
            feed.alive().expect("终态可复取").is_some(),
            "回收后终态必须可取"
        );
    }

    /// 子进程不存在时 spawn 直接失败，不静默降级。
    #[test]
    fn spawn_missing_program_fails() {
        let spec = Spec::new("airy-cli-does-not-exist-0.1.21", &[]);
        assert!(Feed::spawn(&spec).is_err(), "缺失程序须报错");
    }

    /// Drop 即回收：不等待子进程自然退出，宽限到期即强杀。
    #[cfg(any(unix, windows))]
    #[test]
    fn drop_stops_child_promptly() {
        let mut feed = Feed::spawn(&idle()).expect("空闲子进程可起");
        assert!(feed.alive().expect("存活可判").is_none());
        let started = Instant::now();
        drop(feed);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "Drop 须在宽限内完成回收，而非等子进程自行结束"
        );
    }

    /// 子进程侧：读一行 stdin，回一行 JSON，然后正常退出。
    /// 父用例以 `--exact` 只跑本函数；未带标记时直接返回。
    #[test]
    fn child_frames() {
        if std::env::var(CHILD_ENV).as_deref() != Ok("frame") {
            return;
        }
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .expect("子进程读 stdin");
        println!("{{\"seq\":1,\"echo\":{}}}", line.trim().len());
        std::io::stdout().flush().expect("子进程刷 stdout");
    }

    /// 端到端：起真实子进程 → 经 stdin 下发 → 从 stdout 收回帧 → 关 stdin
    /// 让其自然退出并拿到成功终态。
    #[cfg(any(unix, windows))]
    #[test]
    fn send_reaches_child_end_to_end() {
        let exe = std::env::current_exe().expect("测试二进制路径");
        let name = "engine::event::pipe::tests::child_frames";
        let spec = Spec::new(
            &exe.to_string_lossy(),
            &["--exact", name, "--nocapture", "--test-threads=1"],
        )
        .env(CHILD_ENV, "frame");
        let mut feed = Feed::spawn(&spec).expect("子测试进程可起");
        let out = feed.take_out().expect("stdout 读端");
        feed.send("hello").expect("hello 帧可写入");

        let mut reader = std::io::BufReader::new(out);
        let frame = read_frame(&mut reader).expect("子进程须回一帧");
        assert_eq!(frame, "{\"seq\":1,\"echo\":5}", "回帧须与输入对应");

        let status = feed.stop(Duration::from_secs(5)).expect("停止可完成");
        assert!(status.success(), "读完 stdin 的子进程应正常退出");
    }

    /// 写端无读者：BrokenPipe 是可分辨的断链信号。
    ///
    /// 判定与进程回收时序无关，但仍须独占进程：同进程并发 fork 时，尚未
    /// exec 的子进程会短暂继承别的管道的读端（独立探针实测 400 次里 30
    /// 次），此时写入会成功。故把场景放进只跑一个用例的子进程（宿主用
    /// `--exact --test-threads=1`），而不是在并发套件里等运气。
    #[cfg(unix)]
    #[test]
    fn write_without_reader_reports_broken_pipe() {
        let name = "engine::event::pipe::tests::child_broken_pipe";
        let out = run_self(name, "epipe");
        assert!(
            out.status.success(),
            "子进程用例失败：{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// 子进程侧：关掉自身 stdin 的 shell 证明写端已无读者，随后继续存活。
    #[cfg(unix)]
    #[test]
    fn child_broken_pipe() {
        if std::env::var(CHILD_ENV).as_deref() != Ok("epipe") {
            return;
        }
        let mut feed = Feed::spawn(&Spec::new("sh", &["-c", "exec 0<&-; echo x; sleep 30"]))
            .expect("关 stdin 的子进程可起");
        let mut out = feed.take_out().expect("stdout 读端");
        let mut ready = [0u8; 1];
        out.read_exact(&mut ready).expect("子进程须先报到");

        let err = feed.send("hello").expect_err("读端已关，写入须失败");
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe, "EPIPE 须可分辨");
        feed.stop(Duration::from_secs(1)).expect("停止可完成");
    }

    /// 以指定用例名与标记启动本测试二进制（只跑一个用例、单线程）。
    #[cfg(unix)]
    fn run_self(name: &str, mode: &str) -> std::process::Output {
        let exe = std::env::current_exe().expect("测试二进制路径");
        Command::new(exe)
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, mode)
            .output()
            .expect("子测试进程可起")
    }

    /// 子进程先退：stop 返回它自己的终态（不是强杀失败）；此后写端已关，
    /// 写入报 NotConnected 且不再触 syscall。
    #[cfg(any(unix, windows))]
    #[test]
    fn stop_on_exited_child_returns_status() {
        let mut feed = Feed::spawn(&brief()).expect("短命子进程可起");
        let mut waited = 0;
        while feed.alive().expect("存活可判").is_none() {
            assert!(waited < 500, "短命子进程须在 5s 内退出");
            std::thread::sleep(Duration::from_millis(10));
            waited += 1;
        }
        let status = feed
            .stop(Duration::from_secs(1))
            .expect("已退出的子进程可回收");
        assert_eq!(status.code(), Some(3), "须返回子进程真实终态");
        let err = feed.send("hello").expect_err("stop 之后写端须已关");
        assert_eq!(err.kind(), std::io::ErrorKind::NotConnected);
    }

    /// 从子进程 stdout 里取出第一帧：libtest 把测试名与用例输出打在同一行，
    /// 故按帧头定位后取行尾。
    fn read_frame(reader: &mut impl std::io::BufRead) -> Option<String> {
        for _ in 0..64 {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                return None;
            }
            if let Some(at) = line.find("{\"seq\"") {
                return Some(line[at..].trim_end().to_string());
            }
        }
        None
    }
}
