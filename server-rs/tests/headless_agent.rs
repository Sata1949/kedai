// kedai-agent headless 单次入口的端到端测试(HARNESS3-5,2026-10-01)。
//
// 用 `CARGO_BIN_EXE` 子进程**真跑入口本体**——本批交付的形态就是「一题一进程、不起服务」,
// 进程内复用的 AppState/任务引擎只有以子进程方式才被完整检验(而不是直接调函数)。
// 环境隔离:临时 DATA_DIR / KEDAI_TASK_SCRATCH_DIR / 工作区 + `CONNECTOR=mock`
// (与 `build_test_app` 同一套 mock 钩子;`[[tool:...]]` 钩子驱动**真实工具调用**,
// 于是 files_changed 与工作区落盘是端到端断言,而非形状空转)。
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use kedai_server::utils::test_support::TempDataDir;

struct RunOut {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 启动 bin 并等待退出(上限 120s;超时 kill 后 panic——防挂死拖垮测试进程)。
fn run_headless(args: &[String], data: &Path, scratch: &Path) -> RunOut {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kedai-agent"))
        .args(args)
        .env("CONNECTOR", "mock")
        .env("DATA_DIR", data)
        .env("KEDAI_TASK_SCRATCH_DIR", scratch)
        .env("LOG_LEVEL", "error")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 kedai-agent 失败");
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let out = child.wait_with_output().expect("读取子进程输出失败");
                return RunOut {
                    code: out.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!("kedai-agent 超时未退出");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("等待子进程失败:{e}"),
        }
    }
}

fn base_args(prompt_file: &Path, workspace: &Path, out: &Path) -> Vec<String> {
    vec![
        "--prompt-file".into(),
        prompt_file.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace.to_string_lossy().into_owned(),
        "--out".into(),
        out.to_string_lossy().into_owned(),
    ]
}

/// 正路径:mock 驱动一次真实 `fs_write` 工具调用 → solo 跑到 done → 结果 JSON 落盘。
/// 断言四件事:退出码 0;status=done;files_changed 含 hello.txt/create(tool 记账);
/// 工作区内文件真实落盘(工作区闸门与工具链全程有效)。
#[test]
fn headless_solo_runs_and_writes_result_json() {
    let data = TempDataDir::new("ha5-data");
    let scratch = TempDataDir::new("ha5-scratch");
    let ws = TempDataDir::new("ha5-ws");
    let out_dir = TempDataDir::new("ha5-out");

    let prompt = r#"[[tool:fs_write {"path":"hello.txt","content":"你好,headless"}]] 请在工作区创建 hello.txt"#;
    let prompt_file = out_dir.path().join("prompt.txt");
    std::fs::write(&prompt_file, prompt).expect("写题面失败");
    let out = out_dir.path().join("result.json");

    let r = run_headless(
        &base_args(&prompt_file, ws.path(), &out),
        data.path(),
        scratch.path(),
    );

    assert_eq!(
        r.code, 0,
        "应退出码 0;stdout={};stderr={}",
        r.stdout, r.stderr
    );
    assert!(r.stdout.contains("[OK]"), "stdout 应含 [OK]:{}", r.stdout);
    let raw = std::fs::read_to_string(&out).expect("结果 JSON 应存在");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("结果 JSON 应可解析");
    assert_eq!(v["status"], "done", "终态应为 done:{v}");
    assert!(
        !v["summary"].as_str().unwrap_or("").is_empty(),
        "summary 应为最终回复:{v}"
    );
    let files = v["files_changed"]
        .as_array()
        .expect("files_changed 应为数组");
    assert!(
        files
            .iter()
            .any(|f| f["path"] == "hello.txt" && f["op"] == "create" && f["source"] == "tool"),
        "files_changed 应含 hello.txt/create/tool:{files:?}"
    );
    assert_eq!(
        std::fs::read_to_string(ws.path().join("hello.txt")).expect("工作区内应真实落盘"),
        "你好,headless"
    );
    assert!(v["usage"]["prompt_tokens"].is_i64());
    assert!(!v["task_id"].as_str().unwrap_or("").is_empty());
}

/// 负路径:工作区不存在 → 退出码 2,且文案与创建任务同一把尺(不是第三条实现)。
/// 该路径在校验处即退出,不会建库/起任务。
#[test]
fn headless_rejects_missing_workspace() {
    let data = TempDataDir::new("ha5-data2");
    let scratch = TempDataDir::new("ha5-scratch2");
    let out_dir = TempDataDir::new("ha5-out2");

    let prompt_file = out_dir.path().join("prompt.txt");
    std::fs::write(&prompt_file, "随便写点什么").expect("写题面失败");
    let missing = data.path().join("并不存在的目录");
    let out = out_dir.path().join("result.json");

    let r = run_headless(
        &base_args(&prompt_file, &missing, &out),
        data.path(),
        scratch.path(),
    );

    assert_eq!(r.code, 2, "非法工作区应退出码 2;stderr={}", r.stderr);
    assert!(
        r.stderr.contains("工作区不存在或不是目录"),
        "应复用创建期同一文案:{stderr}",
        stderr = r.stderr
    );
    assert!(!out.exists(), "校验失败不应产出结果文件");
}

/// 负路径:缺 --out → 退出码 2(参数错误不建库)。
#[test]
fn headless_rejects_missing_out_flag() {
    let data = TempDataDir::new("ha5-data3");
    let scratch = TempDataDir::new("ha5-scratch3");

    let args = vec![
        "--prompt-file".to_string(),
        "p.txt".to_string(),
        "--workspace".to_string(),
        "ws".to_string(),
    ];
    let r = run_headless(&args, data.path(), scratch.path());
    assert_eq!(r.code, 2, "缺 --out 应退出码 2;stderr={}", r.stderr);
    assert!(
        r.stderr.contains("--out"),
        "错误应点名缺项:{stderr}",
        stderr = r.stderr
    );
}

/// 负路径:首版仅支持 solo;其他模式显式拒绝(退出码 2)。
#[test]
fn headless_rejects_non_solo_mode() {
    let data = TempDataDir::new("ha5-data4");
    let scratch = TempDataDir::new("ha5-scratch4");
    let out_dir = TempDataDir::new("ha5-out4");

    let prompt_file = out_dir.path().join("prompt.txt");
    std::fs::write(&prompt_file, "随便写点什么").expect("写题面失败");
    let out = out_dir.path().join("result.json");

    let mut args = base_args(&prompt_file, out_dir.path(), &out);
    args.push("--mode".into());
    args.push("multi".into());

    let r = run_headless(&args, data.path(), scratch.path());
    assert_eq!(r.code, 2, "非 solo 应退出码 2;stderr={}", r.stderr);
    assert!(
        r.stderr.contains("仅支持") && r.stderr.contains("multi"),
        "应点名不支持的模式:{stderr}",
        stderr = r.stderr
    );
}
