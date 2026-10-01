// kedai-agent:headless 单次任务入口(HARNESS3-5;定位 = 开发/评测工具,不进产品面叙事)。
//
// 一题一进程:读题面文件 → 建一个绑定 `--workspace` 沙箱的 solo 任务 → 进程内直跑到底 →
// 写出结构化 JSON(status / summary / files_changed / usage)+ 退出码(0/1/2)。
// **不起 HTTP 服务、不依赖 web/dist、不装 MCP 服务器**——「先起服务 + 手工开 exec_enabled」
// 这一评测评(eval/bench)痛点由此取消。
//
// 工作区走**同一套闸门**:创建前校验复用 `api::workspace::validate_workspace`(与界面创建
// 任务、画像探测同一出处);运行期路径 jail(`workspace_guard`)与命令级硬门照常生效。
//
// 命令执行:`exec_enabled` 在本进程**内存态**默认为开(不写用户设置文件)——本入口的用途
// 就是跑编码任务,工作区即沙箱;bash 的破坏性/提权命令硬门不受影响(任务模式本就无 UI
// 可确认,直接拒绝)。
//
// 设置(连接/模型/编码包开关等)读取与 Kedai 服务相同的 DATA_DIR(settings.json);
// 空闲看守按 `run_server` 同款挂载(60s tick,阈值读设置)。
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kedai_server::api::app_state::AppState;
use kedai_server::api::workspace::validate_workspace;
use kedai_server::config;
use kedai_server::models::types::{TaskRecord, TaskRunMode, TaskStatus};
use serde_json::json;

/// 等待上限缺省值(秒):0 = 不限。取 1h——比单命令上限(1800s)与单次模型调用上限留足叠加余量。
const DEFAULT_TIMEOUT_SECS: u64 = 3600;
/// 到点收尾(发 stop)后给任务落终态的最长等待。
const STOP_SETTLE_SECS: u64 = 10;
/// 轮询间隔。
const POLL_INTERVAL_MS: u64 = 250;

const USAGE: &str = "\
kedai-agent:headless 单次任务入口(开发/评测用)

用法:
  kedai-agent --prompt-file <题面> --workspace <沙箱目录> --out <结果 JSON>
              [--mode solo] [--timeout-secs N]

选项:
  --prompt-file <路径>   题面文件(UTF-8);内容作为任务目标
  --workspace <目录>     工作区(沙箱)目录,须已存在;校验与界面创建任务同一把尺
  --out <路径>           结果 JSON 输出路径(父目录须已存在)
  --mode <模式>          执行模式;首版仅支持 solo(缺省 solo)
  --timeout-secs <N>     等待上限秒数,0 = 不限;缺省 3600。超时对任务发 stop 后按 timeout 收尾
  -h, --help             显示本帮助

退出码:0 = 任务 done;1 = partial / error / ended / timeout;2 = 参数或环境错误

说明:
  - 进程内直跑(复用 AppState/TaskService),不起 HTTP 服务、不依赖 web/dist、不装 MCP 服务器;
  - 命令执行在本进程内存态默认开启(不写用户设置文件);工作区闸门与命令级硬门照常生效;
  - 连接/模型/开关等读取与 Kedai 服务相同的 DATA_DIR(settings.json);
  - 结果 JSON:{task_id, status, summary, files_changed:[{path,op,source}],
    usage:{prompt_tokens,completion_tokens,reasoning_tokens}}
";

#[derive(Debug)]
struct CliArgs {
    prompt_file: PathBuf,
    workspace: PathBuf,
    out: PathBuf,
    mode: String,
    timeout_secs: u64,
    help: bool,
}

fn parse_args<I: Iterator<Item = String>>(mut iter: I) -> Result<CliArgs, String> {
    fn next_value<I: Iterator<Item = String>>(iter: &mut I, flag: &str) -> Result<String, String> {
        iter.next().ok_or_else(|| format!("{flag} 缺少取值"))
    }

    let mut prompt_file: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut mode = "solo".to_string();
    let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
    let mut help = false;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                help = true;
                break;
            }
            "--prompt-file" => {
                prompt_file = Some(PathBuf::from(next_value(&mut iter, "--prompt-file")?));
            }
            "--workspace" => {
                workspace = Some(PathBuf::from(next_value(&mut iter, "--workspace")?));
            }
            "--out" => {
                out = Some(PathBuf::from(next_value(&mut iter, "--out")?));
            }
            "--mode" => mode = next_value(&mut iter, "--mode")?,
            "--timeout-secs" => {
                let raw = next_value(&mut iter, "--timeout-secs")?;
                timeout_secs = raw
                    .parse::<u64>()
                    .map_err(|_| format!("--timeout-secs 需为非负整数(收到:{raw})"))?;
            }
            other => return Err(format!("未知参数:{other}")),
        }
    }

    if help {
        // 帮助优先:其余必填项不参与判定
        return Ok(CliArgs {
            prompt_file: PathBuf::new(),
            workspace: PathBuf::new(),
            out: PathBuf::new(),
            mode,
            timeout_secs,
            help: true,
        });
    }
    Ok(CliArgs {
        prompt_file: prompt_file.ok_or("缺少 --prompt-file")?,
        workspace: workspace.ok_or("缺少 --workspace")?,
        out: out.ok_or("缺少 --out")?,
        mode,
        timeout_secs,
        help,
    })
}

fn is_terminal(status: &TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Done | TaskStatus::Partial | TaskStatus::Error | TaskStatus::Ended
    )
}

#[tokio::main]
async fn main() {
    std::process::exit(run().await);
}

async fn run() -> i32 {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[错误] {e}");
            eprintln!("用法见 kedai-agent --help");
            return 2;
        }
    };
    if args.help {
        println!("{USAGE}");
        return 0;
    }
    if args.mode != "solo" {
        eprintln!("[错误] 首版仅支持 --mode solo(收到:{})", args.mode);
        return 2;
    }
    let prompt = match std::fs::read_to_string(&args.prompt_file) {
        Ok(t) if !t.trim().is_empty() => t,
        Ok(_) => {
            eprintln!("[错误] 题面文件为空:{}", args.prompt_file.display());
            return 2;
        }
        Err(e) => {
            eprintln!(
                "[错误] 读取题面文件失败({}):{e}",
                args.prompt_file.display()
            );
            return 2;
        }
    };
    let ws_raw = match args.workspace.to_str() {
        Some(s) => s.to_string(),
        None => {
            eprintln!("[错误] --workspace 路径含非 UTF-8 字符,无法校验");
            return 2;
        }
    };

    let config = config::AppConfig::from_env();
    // 创建前校验与界面创建任务同一把尺(api::workspace::validate_workspace 单一出处)
    let workspace = match validate_workspace(Some(&ws_raw), &config.data_dir) {
        Ok(Some(p)) => p,
        Ok(None) => {
            eprintln!("[错误] --workspace 不能为空");
            return 2;
        }
        Err(e) => {
            eprintln!("[错误] {e}");
            return 2;
        }
    };

    let state = match AppState::new(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[错误] 初始化失败:{e}");
            return 2;
        }
    };
    // headless 语义:命令执行在本进程内存态默认开(不写用户设置文件)
    {
        state
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .exec_enabled = true;
    }
    // 与 run_server 同款空闲看守(阈值读设置;0 = 关)
    state.tasks.spawn_idle_watchdog(Duration::from_secs(60));

    let title = prompt.trim().to_string();
    let svc = state.tasks.clone();
    let ws_for_create = workspace.clone();
    let created = state
        .db_call(move || {
            svc.create(
                &title,
                None,
                None,
                TaskRunMode::Solo,
                None,
                None,
                None,
                Some(&ws_for_create),
            )
        })
        .await;
    let task = match created {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            eprintln!("[错误] 创建任务失败:{e}");
            return 2;
        }
        Err(e) => {
            eprintln!("[错误] {e}");
            return 2;
        }
    };
    let task_id = task.id.clone();

    let svc = state.tasks.clone();
    let tid = task_id.clone();
    match state.db_call(move || svc.run(&tid)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            eprintln!("[错误] 启动任务失败:{e}");
            return 2;
        }
        Err(e) => {
            eprintln!("[错误] {e}");
            return 2;
        }
    }

    let started = Instant::now();
    let deadline =
        (args.timeout_secs > 0).then(|| started + Duration::from_secs(args.timeout_secs));
    let mut timed_out = false;
    let last: TaskRecord = loop {
        let svc = state.tasks.clone();
        let tid = task_id.clone();
        let current = match state.db_call(move || svc.get(&tid)).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                eprintln!("[错误] 任务记录消失:{task_id}");
                return 2;
            }
            Err(e) => {
                eprintln!("[错误] {e}");
                return 2;
            }
        };
        if is_terminal(&current.status) {
            break current;
        }
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                timed_out = true;
                let svc = state.tasks.clone();
                let tid = task_id.clone();
                let _ = state.db_call(move || svc.stop(&tid)).await;
                // 给 stop 落终态的时间(其落库是异步收尾;拿不到终态也照常出结果)
                let settle = Instant::now() + Duration::from_secs(STOP_SETTLE_SECS);
                let mut settled: TaskRecord = current;
                while Instant::now() < settle {
                    let svc = state.tasks.clone();
                    let tid = task_id.clone();
                    if let Ok(Some(t)) = state.db_call(move || svc.get(&tid)).await {
                        let terminal = is_terminal(&t.status);
                        settled = t;
                        if terminal {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                }
                break settled;
            }
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    };

    let svc = state.tasks.clone();
    let tid = task_id.clone();
    let usage = state
        .db_call(move || svc.usage_total(&tid))
        .await
        .unwrap_or((0, 0, 0));
    let db = state.db.clone();
    let tid = task_id.clone();
    let changes = state
        .db_call(move || kedai_server::services::task_change_service::list(&db, &tid))
        .await
        .unwrap_or_default();
    let files: Vec<serde_json::Value> = changes
        .iter()
        .map(|c| json!({ "path": c.path, "op": c.op, "source": c.source }))
        .collect();
    let status_str = if timed_out {
        "timeout".to_string()
    } else {
        last.status.as_str().to_string()
    };
    let summary = if last.result.trim().is_empty() {
        last.error.clone()
    } else {
        last.result.clone()
    };
    let body = json!({
        "task_id": last.id,
        "status": status_str,
        "summary": summary,
        "files_changed": files,
        "usage": {
            "prompt_tokens": usage.0,
            "completion_tokens": usage.1,
            "reasoning_tokens": usage.2,
        },
    });
    let text = match serde_json::to_string_pretty(&body) {
        Ok(t) => format!("{t}\n"),
        Err(e) => {
            eprintln!("[错误] 结果序列化失败:{e}");
            return 2;
        }
    };
    if let Err(e) = std::fs::write(&args.out, text) {
        eprintln!("[错误] 写结果文件失败({}):{e}", args.out.display());
        return 2;
    }

    let elapsed = started.elapsed().as_secs_f64();
    if timed_out {
        eprintln!(
            "[超时] 任务未在 {}s 内到终态,已发 stop;结果(状态 timeout)写入 {}",
            args.timeout_secs,
            args.out.display()
        );
        return 1;
    }
    match last.status {
        TaskStatus::Done => {
            println!("[OK] 任务完成({elapsed:.1}s) → {}", args.out.display());
            0
        }
        other => {
            eprintln!(
                "[失败] 任务终态 {};详情见 {}",
                other.as_str(),
                args.out.display()
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(items: &[&str]) -> Result<CliArgs, String> {
        parse_args(items.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_required_flags_with_defaults() {
        let a = parse(&[
            "--prompt-file",
            "p.txt",
            "--workspace",
            "ws",
            "--out",
            "r.json",
        ])
        .expect("应解析成功");
        assert_eq!(a.prompt_file, PathBuf::from("p.txt"));
        assert_eq!(a.workspace, PathBuf::from("ws"));
        assert_eq!(a.out, PathBuf::from("r.json"));
        assert_eq!(a.mode, "solo");
        assert_eq!(a.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert!(!a.help);
    }

    #[test]
    fn parses_timeout_and_mode_override() {
        let a = parse(&[
            "--prompt-file",
            "p",
            "--workspace",
            "w",
            "--out",
            "r",
            "--mode",
            "solo",
            "--timeout-secs",
            "0",
        ])
        .expect("应解析成功");
        assert_eq!(a.timeout_secs, 0);
    }

    #[test]
    fn rejects_missing_out() {
        let e = parse(&["--prompt-file", "p", "--workspace", "w"]).unwrap_err();
        assert!(e.contains("--out"), "错误应点名缺项:{e}");
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse(&["--nope"]).unwrap_err();
        assert!(e.contains("未知参数"), "错误应为未知参数:{e}");
    }

    #[test]
    fn rejects_bad_timeout() {
        let e = parse(&["--timeout-secs", "abc"]).unwrap_err();
        assert!(e.contains("非负整数"), "错误应提示整数:{e}");
    }

    #[test]
    fn rejects_flag_without_value() {
        let e = parse(&["--prompt-file"]).unwrap_err();
        assert!(e.contains("缺少取值"), "错误应提示缺取值:{e}");
    }

    #[test]
    fn help_wins_without_required_flags() {
        let a = parse(&["--help"]).expect("--help 应成功");
        assert!(a.help);
        let a = parse(&["--prompt-file", "p", "-h"]).expect("-h 应成功");
        assert!(a.help);
    }
}
