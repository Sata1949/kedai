#![cfg(test)]
use super::*;
use crate::api::app_state::AppState;
use crate::models::tool_policy::TaskAuthorizationMode;
use crate::models::types::{TaskRunMode, TaskToolAuthorization};
use crate::tools::permissions::PendingAuthorizationDecision;
use crate::utils::test_support::TempDataDir;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::watch;

struct Fixture {
    executor: TeamExecutor,
    app: Arc<AppState>,
    ctx: TaskRunContext,
    cancel: watch::Sender<bool>,
    workspace: PathBuf,
    guard: TempDataDir,
}

impl Fixture {
    fn new(mode: Option<TaskAuthorizationMode>) -> Self {
        let guard = TempDataDir::new("pre-seed-security");
        let workspace = guard.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("part.js"), "window.part = 1;").unwrap();
        let mut config = crate::config::test_config();
        config.data_dir = guard.join("data");
        config.images_dir = guard.join("images");
        config.imports_dir = guard.join("imports");
        config.task_scratch_dir = guard.join("scratch");
        config.log_dir = guard.join("logs");
        let app = AppState::new(config).unwrap();
        let task = app
            .tasks
            .create(
                "请交付单文件 HTML 页面",
                None,
                None,
                TaskRunMode::Team,
                None,
                None,
                None,
                Some(&workspace.to_string_lossy()),
                mode,
            )
            .unwrap();
        let scope = crate::tools::workspace_guard::scope_for_task_with_authorization(
            Some(&workspace.to_string_lossy()),
            mode,
        )
        .unwrap()
        .unwrap();
        let (cancel, receiver) = watch::channel(false);
        let ctx = TaskRunContext {
            task_id: task.id,
            token: 1,
            goal: "请交付单文件 HTML 页面".into(),
            settings: app.settings_snapshot(),
            executor_id: None,
            character_id: None,
            cancel: receiver,
            connection_id: None,
            scope: Some(scope),
            tool_authorization: mode,
            deadline: None,
        };
        let executor = TeamExecutor::new(app.tasks.clone(), app.engine.clone());
        Self {
            executor,
            app,
            ctx,
            cancel,
            workspace,
            guard,
        }
    }

    async fn run_files(&self, files: &[String], upstream: &[String]) -> Option<String> {
        self.executor
            .pre_seed_single_file(
                &self.ctx,
                &format!("task:{}:main:1", self.ctx.task_id),
                self.ctx.scope.as_ref(),
                files,
                upstream,
            )
            .await
    }

    async fn run(&self) -> Option<String> {
        self.run_files(&["index.html".into()], &["part.js".into()])
            .await
    }

    async fn pending(&self) -> TaskToolAuthorization {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(pending) = self
                    .app
                    .tool_registry
                    .permissions()
                    .pending_for_task(&self.ctx.task_id)
                    .into_iter()
                    .next()
                {
                    return pending;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("预置写入应进入逐步授权等待")
    }

    fn answer(&self, pending: &TaskToolAuthorization, answer: PendingAuthorizationDecision) {
        self.app
            .tool_registry
            .permissions()
            .resolve_wait(
                &pending.run_id,
                &pending.call_id,
                &pending.session_id,
                &pending.name,
                answer,
            )
            .unwrap();
    }
}

#[tokio::test]
async fn pre_seed_security_cancelled_task_never_writes() {
    let f = Fixture::new(None);
    f.cancel.send(true).unwrap();
    let note = f.run().await.unwrap();
    assert!(
        !f.workspace.join("index.html").exists(),
        "已取消任务不得由预置路径继续写盘"
    );
    assert!(note.contains("停止"), "{note}");
}

#[tokio::test]
async fn pre_seed_security_readonly_allowlist_never_writes() {
    let mut f = Fixture::new(None);
    f.ctx.settings.task_tool_policy = "allowlist".into();
    f.ctx.settings.task_tool_allowlist = vec!["fs_read".into()];
    let note = f.run().await.unwrap();
    assert!(
        !f.workspace.join("index.html").exists(),
        "只读工具策略不得被预置路径绕过"
    );
    assert!(note.contains("允许的工具清单"), "{note}");
}

#[tokio::test]
async fn pre_seed_security_step_cannot_authorize_outside_allowlist() {
    let mut f = Fixture::new(Some(TaskAuthorizationMode::Step));
    f.ctx.settings.task_tool_policy = "allowlist".into();
    f.ctx.settings.task_tool_allowlist = vec!["fs_read".into()];
    let note = tokio::time::timeout(Duration::from_secs(2), f.run())
        .await
        .expect("策略硬拒绝不能变成等待确认")
        .unwrap();
    assert!(note.contains("允许的工具清单"), "{note}");
    assert!(f
        .app
        .tool_registry
        .permissions()
        .pending_for_task(&f.ctx.task_id)
        .is_empty());
    assert!(!f.workspace.join("index.html").exists());
}

#[tokio::test]
async fn pre_seed_security_step_waits_then_records_approved_write() {
    let f = Fixture::new(Some(TaskAuthorizationMode::Step));
    let run = f.run();
    tokio::pin!(run);
    let pending = tokio::select! {
        result = &mut run => panic!("未确认就结束预置:{result:?}"),
        pending = f.pending() => pending,
    };
    assert_eq!(pending.name, INLINE_TOOL);
    assert!(
        !f.workspace.join("index.html").exists(),
        "用户确认前不得读写预置产物"
    );
    f.answer(&pending, PendingAuthorizationDecision::AllowOnce);
    let note = run.await.unwrap();
    assert!(f.workspace.join("index.html").is_file(), "{note}");
    let changes = crate::services::task_change_service::list(&f.app.db, &f.ctx.task_id);
    assert_eq!(changes.len(), 1, "预置必须进入任务文件变更台账");
    assert_eq!(changes[0].path, "index.html");
    assert!(changes[0].before_hash.is_none(), "新文件的基线应为空");
}

#[tokio::test]
async fn pre_seed_security_step_denial_never_writes() {
    let f = Fixture::new(Some(TaskAuthorizationMode::Step));
    let run = f.run();
    tokio::pin!(run);
    let pending = tokio::select! {
        result = &mut run => panic!("未确认就结束预置:{result:?}"),
        pending = f.pending() => pending,
    };
    f.answer(&pending, PendingAuthorizationDecision::Deny);
    assert!(run.await.unwrap().contains("拒绝"));
    assert!(!f.workspace.join("index.html").exists());
    assert!(crate::services::task_change_service::list(&f.app.db, &f.ctx.task_id).is_empty());
}

#[tokio::test]
async fn pre_seed_security_cancellation_clears_pending_authorization() {
    let f = Fixture::new(Some(TaskAuthorizationMode::Step));
    let run = f.run();
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => panic!("未确认就结束预置:{result:?}"),
        _ = f.pending() => (),
    }
    f.cancel.send(true).unwrap();
    let _ = run.await;
    assert!(!f.workspace.join("index.html").exists());
    assert!(f
        .app
        .tool_registry
        .permissions()
        .pending_for_task(&f.ctx.task_id)
        .is_empty());
}

#[tokio::test]
async fn pre_seed_security_existing_entry_is_preserved() {
    let f = Fixture::new(None);
    std::fs::write(f.workspace.join("index.html"), "用户已有入口").unwrap();
    let note = f.run().await.unwrap();
    assert!(note.contains("未覆盖"), "{note}");
    assert_eq!(
        std::fs::read_to_string(f.workspace.join("index.html")).unwrap(),
        "用户已有入口"
    );
    assert!(crate::services::task_change_service::list(&f.app.db, &f.ctx.task_id).is_empty());
}

#[tokio::test]
async fn pre_seed_security_rejects_output_traversal() {
    let f = Fixture::new(None);
    let note = f
        .run_files(&["../escaped.html".into()], &["part.js".into()])
        .await
        .unwrap();
    assert!(!f.guard.join("escaped.html").exists(), "预置不能写穿工作区");
    assert!(note.contains("未完成"), "{note}");
}

#[tokio::test]
async fn pre_seed_security_rejects_other_owners_file() {
    let mut f = Fixture::new(None);
    let ownership = Arc::new(crate::models::ownership::WriteOwnership::new([(
        "主 Agent-2".to_string(),
        "index.html".to_string(),
    )]));
    f.ctx.scope = f.ctx.scope.as_ref().map(|scope| {
        scope.with_write_ownership(ownership, "主 Agent-1".into(), vec!["index.html".into()])
    });
    let note = f.run().await.unwrap();
    assert!(!f.workspace.join("index.html").exists());
    assert!(note.contains("主 Agent-2"), "{note}");
}

fn directory_link(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .creation_flags(crate::utils::win::CREATE_NO_WINDOW)
            .status()
            .expect("创建测试 junction 失败");
        assert!(status.success(), "测试环境应允许创建目录 junction");
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[tokio::test]
async fn pre_seed_security_rejects_linked_output_directory() {
    let f = Fixture::new(None);
    let outside = f.guard.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    directory_link(&f.workspace.join("linked"), &outside);
    let note = f
        .run_files(&["linked/index.html".into()], &["part.js".into()])
        .await
        .unwrap();
    assert!(
        !outside.join("index.html").exists(),
        "预置不得通过链接写到工作区外"
    );
    assert!(note.contains("未完成"), "{note}");
}

#[tokio::test]
async fn pre_seed_security_rejects_linked_source_directory() {
    let f = Fixture::new(None);
    let outside = f.guard.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("part.js"), "window.privateOutside = 1;").unwrap();
    directory_link(&f.workspace.join("linked"), &outside);
    let note = f
        .run_files(&["index.html".into()], &["linked/part.js".into()])
        .await
        .unwrap();
    assert!(
        !f.workspace.join("index.html").exists(),
        "工作区外分片不得被预置读入产物"
    );
    assert!(note.contains("未完成"), "{note}");
}

#[tokio::test]
async fn pre_seed_security_expired_budget_never_writes() {
    let mut f = Fixture::new(None);
    f.ctx.deadline = Some(std::time::Instant::now());
    assert!(f.run().await.unwrap().contains("预算"));
    assert!(!f.workspace.join("index.html").exists());
}
