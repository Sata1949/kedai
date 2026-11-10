#![cfg(test)]

use crate::models::ownership::WriteOwnership;
use crate::models::types::ExecScope;
use crate::tools::agent_tools_fs::*;
use crate::utils::test_support::TempDataDir;

fn grep_fixture(tag: &str) -> Fixture {
    let mut f = fixture(tag);
    f.ws = f.dir.join("grep-only");
    std::fs::create_dir(&f.ws).unwrap();
    f.scope = Arc::new(ExecScope::new(f.ws.canonicalize().unwrap(), true));
    f
}

#[test]
fn grep_stops_traversal_after_extra_hit_in_nested_directory() {
    let f = grep_fixture("grep-stop");
    std::fs::create_dir_all(f.ws.join("a")).unwrap();
    std::fs::write(f.ws.join("a/00-first.txt"), "needle\nneedle\n").unwrap();
    std::fs::write(f.ws.join("a/zz-tail.txt"), "needle\n").unwrap();
    std::fs::create_dir_all(f.ws.join("z-tail/deep")).unwrap();
    for i in 0..20 {
        std::fs::write(f.ws.join(format!("z-tail/deep/{i}.txt")), "needle\n").unwrap();
    }
    let (output, scan) =
        run_grep_scanned(&f.deps, &f.ctx(), &json!({"pattern":"needle","max":1})).unwrap();
    assert_eq!(
        output,
        format_matches(vec!["a/00-first.txt:1:needle".into()], 1, None, "命中")
    );
    eprintln!("提前命中时访问条目：{}", scan.entries);
    assert_eq!(
        scan.entries, 2,
        "只应进入 a 目录和首个文件，后续目录连 metadata 都不应读取"
    );
    assert!(scan.stop.is_some(), "停止必须传播到根层");
}

#[test]
fn grep_max_boundary_preserves_counts_order_and_skip_summary() {
    let f = grep_fixture("grep-boundary");
    std::fs::write(f.ws.join("a.bin"), b"needle\0").unwrap();
    std::fs::File::create(f.ws.join("b.txt"))
        .unwrap()
        .set_len(MAX_GREP_FILE_BYTES + 1)
        .unwrap();
    std::fs::write(f.ws.join("c.txt"), "其他\nneedle\nneedle\n").unwrap();
    std::fs::write(f.ws.join("z.txt"), "没有关键词\n").unwrap();
    let skip = format!("(已跳过 1 个二进制文件、1 个超过 {MAX_GREP_FILE_BYTES} 字节的文件)\n");
    for (max, known, visited) in [(1, None, 3), (2, Some(2), 4)] {
        let (output, scan) =
            run_grep_scanned(&f.deps, &f.ctx(), &json!({"pattern":"needle","max":max})).unwrap();
        let hits = vec!["c.txt:2:needle".into(), "c.txt:3:needle".into()];
        assert_eq!(output, format_matches(hits, max, known, "命中") + &skip);
        assert_eq!(scan.entries, visited);
        assert_eq!(scan.stop.is_some(), max == 1);
    }
    let (none, scan) =
        run_grep_scanned(&f.deps, &f.ctx(), &json!({"pattern":"不存在的匹配"})).unwrap();
    assert_eq!(none, "(无命中)\n".to_string() + &skip);
    assert_eq!(scan.entries, 4);
    assert!(scan.stop.is_none());
}

#[cfg(unix)]
#[test]
fn grep_skips_symlinks_and_unreadable_directories() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = grep_fixture("grep-links");
    let outside = f.dir.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "needle 外部合成夹具\n").unwrap();
    symlink(outside.join("secret.txt"), f.ws.join("linked.txt")).unwrap();
    symlink(&outside, f.ws.join("linked-dir")).unwrap();
    std::fs::write(f.ws.join("local.txt"), "needle 本地\n").unwrap();
    let hidden = f.ws.join("unreadable");
    std::fs::create_dir(&hidden).unwrap();
    std::fs::write(hidden.join("hidden.txt"), "needle 隐藏\n").unwrap();
    std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o0)).unwrap();
    let permission_denied = std::fs::read_dir(&hidden).is_err();
    let result = run_grep(&f.deps, &f.ctx(), &json!({"pattern":"needle"}));
    std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = result.unwrap();
    assert!(!output.contains("linked"));
    if permission_denied {
        assert_eq!(output, "local.txt:1:needle 本地\n(命中 1 条)\n");
    }
}

/// 注册工具族所需的依赖(测试用空依赖)与工作区
struct Fixture {
    deps: Arc<ToolDeps>,
    ws: PathBuf,
    /// 每任务**一份**作用域:生产同款(TaskRunContext 构造一次,随 ToolContext 克隆共享)。
    /// 若每次调用现建一份,读记录就存不住,「先读后写」永远不成立——那会掩盖被测语义。
    scope: Arc<ExecScope>,
    dir: TempDataDir,
    _dir: TempDataDir,
}

fn fixture(tag: &str) -> Fixture {
    let (_guard, deps) = ToolDeps::dummy_for_test();
    let dir = TempDataDir::new(&format!("fstools-{tag}"));
    let ws = dir.join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("src/main.rs"),
        "fn main() {\n    println!(\"hi\");\n}\n",
    )
    .unwrap();
    std::fs::write(ws.join("README.md"), "hello\nworld\n").unwrap();
    std::fs::create_dir_all(ws.join("node_modules/pkg")).unwrap();
    std::fs::write(ws.join("node_modules/pkg/x.js"), "nope\n").unwrap();
    std::fs::create_dir_all(ws.join(".git")).unwrap();
    std::fs::write(ws.join(".git/config"), "nope\n").unwrap();
    std::fs::create_dir_all(ws.join("target")).unwrap();
    std::fs::write(ws.join("target/out.bin"), "nope\n").unwrap();
    let scope = {
        // 先取 owned 字符串:`scope_for_task` 收 `Option<&str>`,直接借用临时的 Cow
        // 会因临时值在语句末被丢弃而报借用错误
        let ws_str = ws.to_string_lossy().into_owned();
        crate::tools::workspace_guard::scope_for_task(Some(&ws_str))
            .unwrap()
            .unwrap()
    };
    Fixture {
        _dir: _guard,
        deps: Arc::new(deps),
        ws,
        dir,
        scope,
    }
}

impl Fixture {
    fn scope(&self) -> Arc<ExecScope> {
        self.scope.clone()
    }

    fn ctx(&self) -> ToolContext {
        ToolContext {
            session_id: "task:t1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: Some(self.scope()),
            budget: None,
        }
    }

    /// TMR-4:指定派生 scope 的 ctx(所有权测试用)
    fn ctx_with_scope(&self, scope: Arc<ExecScope>) -> ToolContext {
        ToolContext {
            session_id: "task:t1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: Some(scope),
            budget: None,
        }
    }

    fn bindless_ctx() -> ToolContext {
        ToolContext {
            session_id: "s1".into(),
            character_id: String::new(),
            agent_depth: 0,
            scope: None,
            budget: None,
        }
    }

    fn registry(&self) -> ToolRegistry {
        let reg = ToolRegistry::new();
        register_fs_tools(&reg, self.deps.clone());
        reg
    }

    /// 已裁决放行:fs_write / fs_edit 是危险级工具,任务模式下由 ToolGate 名单放行
    /// (见 task_engine/tool_policy.rs 的工具名例外);测试走同一「已获授权」路径,
    /// 否则会被 permissions 的通用授权弹窗分支拦下。未授权路径本身由 permissions
    /// 的既有测试覆盖,此处不重复。
    fn allow() -> crate::tools::permissions::PermissionDecision {
        crate::tools::permissions::PermissionDecision {
            allowed: true,
            risk: crate::tools::permissions::ToolRisk::Dangerous,
            reason: "测试放行".into(),
            code: None,
        }
    }

    async fn call(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
        self.registry()
            .execute_with_decision(name, &args.to_string(), self.ctx(), &Self::allow())
            .await
    }

    async fn call_with_scope(
        &self,
        name: &str,
        args: serde_json::Value,
        scope: Arc<ExecScope>,
    ) -> Result<String, String> {
        self.registry()
            .execute_with_decision(
                name,
                &args.to_string(),
                self.ctx_with_scope(scope),
                &Self::allow(),
            )
            .await
    }

    async fn call_unbound(&self, name: &str, args: serde_json::Value) -> Result<String, String> {
        self.registry()
            .execute_with_decision(
                name,
                &args.to_string(),
                Self::bindless_ctx(),
                &Self::allow(),
            )
            .await
    }
}

/// 未绑定工作区:5 个工具一律拒绝,且文案点明「未绑定工作区」
#[tokio::test]
async fn unbound_scope_rejects_all_five_tools() {
    let f = fixture("unbound");
    let cases: Vec<(&str, serde_json::Value)> = vec![
        (READ_TOOL, json!({ "path": "README.md" })),
        (WRITE_TOOL, json!({ "path": "a.txt", "content": "x" })),
        (
            EDIT_TOOL,
            json!({ "path": "README.md", "old": "hello", "new": "hi" }),
        ),
        (GLOB_TOOL, json!({ "pattern": "*.md" })),
        (GREP_TOOL, json!({ "pattern": "hello" })),
    ];
    for (name, args) in cases {
        let err = f.call_unbound(name, args).await.unwrap_err();
        assert!(
            err.contains("未绑定工作区"),
            "{name} 在未绑定时应报未绑定工作区: {err}"
        );
    }
}

/// TMR-4:文件所有权在工具层生效——声明归属优先、未声明首写者认领、
/// 同 owner 重复写放行、fs_edit 同样先过所有权再谈先读后写。
#[tokio::test]
async fn write_ownership_blocks_cross_owner_and_first_writer_claims() {
    let f = fixture("ownership");
    let ownership = Arc::new(WriteOwnership::new(vec![
        ("主 Agent-1".to_string(), "parts/a.js".to_string()),
        ("主 Agent-2".to_string(), "parts/b.js".to_string()),
    ]));
    let s1 = f.scope.with_write_ownership(
        ownership.clone(),
        "主 Agent-1".into(),
        vec!["parts/a.js".into()],
    );
    let s2 = f.scope.with_write_ownership(
        ownership.clone(),
        "主 Agent-2".into(),
        vec!["parts/b.js".into()],
    );

    // 声明归属者可写(新建文件无需先读),重复写也放行
    f.call_with_scope(
        WRITE_TOOL,
        json!({ "path": "parts/a.js", "content": "one\n" }),
        s1.clone(),
    )
    .await
    .unwrap();
    f.call_with_scope(
        WRITE_TOOL,
        json!({ "path": "parts/a.js", "content": "one again\n" }),
        s1.clone(),
    )
    .await
    .unwrap();

    // 另一个主写别人声明的文件 → 拒绝且文件保持原样
    let err = f
        .call_with_scope(
            WRITE_TOOL,
            json!({ "path": "parts/a.js", "content": "two\n" }),
            s2.clone(),
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("文件所有权冲突") && err.contains("主 Agent-1"),
        "越权写应点明归属: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(f.ws.join("parts/a.js")).unwrap(),
        "one again\n",
        "被拒绝的写入不得落盘"
    );

    // 未声明文件:先写者认领,后写者被拒
    f.call_with_scope(
        WRITE_TOOL,
        json!({ "path": "shared.txt", "content": "from-2" }),
        s2.clone(),
    )
    .await
    .unwrap();
    let err = f
        .call_with_scope(
            WRITE_TOOL,
            json!({ "path": "shared.txt", "content": "from-1" }),
            s1,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("主 Agent-2"),
        "首写者认领后,另一主应看到认领者: {err}"
    );

    // fs_edit:所有权检查先于先读后写(即使没读过也报所有权,而不是误导为「先读」)
    let err = f
        .call_with_scope(
            EDIT_TOOL,
            json!({ "path": "parts/a.js", "old": "one", "new": "two" }),
            s2,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("文件所有权冲突"),
        "fs_edit 也必须受所有权约束: {err}"
    );
}

/// fs_read:带行号分页 + 读记录写入(供 fs_write 的先读后写)
#[tokio::test]
async fn read_numbers_lines_and_notes_read() {
    let f = fixture("read");
    let out = f
        .call(READ_TOOL, json!({ "path": "src/main.rs" }))
        .await
        .unwrap();
    assert!(
        out.starts_with("src/main.rs (共 3 行)"),
        "首行应含路径与总行数: {out}"
    );
    assert!(out.contains("     1\tfn main() {"), "应带行号正文: {out}");
    // 分页:只取第 2 行(输出 = 路径行 + 1 行正文 + 截断提示)
    let page = f
        .call(
            READ_TOOL,
            json!({ "path": "src/main.rs", "offset": 2, "limit": 1 }),
        )
        .await
        .unwrap();
    assert!(page.contains("     2\t"), "应只给第 2 行: {page}");
    assert!(!page.contains("fn main"), "第 1 行不应出现: {page}");
    assert!(page.contains("已截断"), "应提示后续 offset: {page}");
    // 越界路径被闸门拒
    assert!(f
        .call(READ_TOOL, json!({ "path": "../README.md" }))
        .await
        .is_err());
}

/// fs_write:先读后写(未读拒、读过通过),新建文件不受先读限制
#[tokio::test]
async fn write_requires_prior_read_for_existing_file() {
    let f = fixture("write");
    let err = f
        .call(WRITE_TOOL, json!({ "path": "README.md", "content": "new" }))
        .await
        .unwrap_err();
    assert!(err.contains("尚未读取"), "未读应先被拦: {err}");
    // 新建文件:没有可盲写的旧内容 → 放行
    f.call(
        WRITE_TOOL,
        json!({ "path": "src/new.rs", "content": "// new\n" }),
    )
    .await
    .unwrap();
    assert!(f.ws.join("src/new.rs").exists(), "新文件应落盘");
    // 读过之后可以覆盖,且连续两次不被自己拦下(写后刷新读记录)
    f.call(READ_TOOL, json!({ "path": "README.md" }))
        .await
        .unwrap();
    f.call(
        WRITE_TOOL,
        json!({ "path": "README.md", "content": "one\n" }),
    )
    .await
    .unwrap();
    f.call(
        WRITE_TOOL,
        json!({ "path": "README.md", "content": "two\n" }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.ws.join("README.md")).unwrap(),
        "two\n"
    );
}

/// fs_write/fs_edit 的越界与符号链接逃逸一律被闸门拦下(执行侧兜底)
#[tokio::test]
async fn write_and_edit_are_gated_by_guard() {
    let f = fixture("gated");
    let outside = f.dir.join("outside.txt");
    std::fs::write(&outside, "x").unwrap();
    let abs = outside.to_string_lossy().into_owned();
    for (name, args) in [
        (WRITE_TOOL, json!({ "path": abs, "content": "y" })),
        (
            EDIT_TOOL,
            json!({ "path": "../outside.txt", "old": "x", "new": "y" }),
        ),
    ] {
        assert!(f.call(name, args).await.is_err(), "{name} 越界应被拒");
    }
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "x",
        "外部文件不得被改"
    );
}

/// fs_edit 的三种分支:0 处 / 2 处(未给 replace_all)/ replace_all=true
#[tokio::test]
async fn edit_uniqueness_branches() {
    let f = fixture("edit");
    std::fs::write(f.ws.join("dup.txt"), "aaa\nbbb\naaa\n").unwrap();
    f.call(READ_TOOL, json!({ "path": "dup.txt" }))
        .await
        .unwrap();
    let miss = f
        .call(
            EDIT_TOOL,
            json!({ "path": "dup.txt", "old": "zzz", "new": "y" }),
        )
        .await
        .unwrap_err();
    assert!(miss.contains("未找到"), "0 处文案: {miss}");
    let many = f
        .call(
            EDIT_TOOL,
            json!({ "path": "dup.txt", "old": "aaa", "new": "y" }),
        )
        .await
        .unwrap_err();
    assert!(many.contains("匹配到 2 处"), "2 处应回报实际次数: {many}");
    // 加上下文后唯一,可替换
    f.call(
        EDIT_TOOL,
        json!({ "path": "dup.txt", "old": "bbb\naaa", "new": "bbb\nzzz" }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.ws.join("dup.txt")).unwrap(),
        "aaa\nbbb\nzzz\n"
    );
    // replace_all 放行全部出现处(先重读:文件已被自己改过)
    f.call(READ_TOOL, json!({ "path": "dup.txt" }))
        .await
        .unwrap();
    let all = f
        .call(
            EDIT_TOOL,
            json!({ "path": "dup.txt", "old": "zzz", "new": "w", "replace_all": true }),
        )
        .await
        .unwrap();
    assert!(all.contains("替换 1 处"), "应回报替换处数: {all}");
    // 空 old 直接拒
    assert!(f
        .call(
            EDIT_TOOL,
            json!({ "path": "dup.txt", "old": "", "new": "x" })
        )
        .await
        .is_err());
}

/// fs_glob:字典序相对路径 + 排除表生效 + 截断如实说明
#[tokio::test]
async fn glob_sorts_and_excludes() {
    let f = fixture("glob");
    let out = f
        .call(GLOB_TOOL, json!({ "pattern": "**/*.rs" }))
        .await
        .unwrap();
    assert!(out.contains("src/main.rs"), "应命中工作区源码: {out}");
    assert!(
        !out.contains("node_modules"),
        "node_modules 应被排除: {out}"
    );
    let all = f.call(GLOB_TOOL, json!({ "pattern": "**" })).await.unwrap();
    for excluded in [".git", "target", "node_modules"] {
        assert!(!all.contains(excluded), "{excluded} 应被排除: {all}");
    }
    let none = f
        .call(GLOB_TOOL, json!({ "pattern": "*.ts" }))
        .await
        .unwrap();
    assert!(none.contains("无匹配"), "空集应如实说明: {none}");
    let capped = f
        .call(GLOB_TOOL, json!({ "pattern": "**", "max": 1 }))
        .await
        .unwrap();
    assert!(capped.contains("还有更多"), "截断应如实说明: {capped}");
}

#[tokio::test]
async fn globstar_matches_zero_or_more_directories() {
    let f = grep_fixture("globstar-root");
    std::fs::create_dir_all(f.ws.join("src/nested")).unwrap();
    for path in [
        "index.html",
        ".kedai-verify.json",
        "src/lib.rs",
        "src/nested/mod.rs",
    ] {
        std::fs::write(f.ws.join(path), "marker\n").unwrap();
    }
    let inventory = f.call(GLOB_TOOL, json!({"pattern":"**/*"})).await.unwrap();
    assert!(inventory.contains("index.html\n"), "{inventory}");
    assert!(inventory.contains(".kedai-verify.json\n"), "{inventory}");
    let rs = f
        .call(GLOB_TOOL, json!({"pattern":r".\src\**\*.rs"}))
        .await
        .unwrap();
    assert!(rs.starts_with("src/lib.rs\nsrc/nested/mod.rs\n"), "{rs}");
    let root_match = f
        .call(GREP_TOOL, json!({"pattern":"marker","glob":"**/*.html"}))
        .await
        .unwrap();
    assert!(root_match.contains("index.html:1:marker"), "{root_match}");
    let flat = f.call(GLOB_TOOL, json!({"pattern":"*"})).await.unwrap();
    assert!(!flat.contains("src/lib.rs"), "{flat}");
}

/// fs_grep:命中格式 path:line:text、glob 过滤、非法正则回原错
#[tokio::test]
async fn grep_reports_path_line_and_filters() {
    let f = fixture("grep");
    let out = f
        .call(GREP_TOOL, json!({ "pattern": "fn main" }))
        .await
        .unwrap();
    assert!(out.contains("src/main.rs:1:fn main() {"), "命中格式: {out}");
    let filtered = f
        .call(GREP_TOOL, json!({ "pattern": "hello", "glob": "**/*.rs" }))
        .await
        .unwrap();
    assert!(
        filtered.contains("无命中"),
        "glob 过滤后应无命中: {filtered}"
    );
    let bad = f
        .call(GREP_TOOL, json!({ "pattern": "([", }))
        .await
        .unwrap_err();
    assert!(bad.contains("正则表达式非法"), "应回 regex 原错: {bad}");
    let skipped = f
        .call(GREP_TOOL, json!({ "pattern": "nope" }))
        .await
        .unwrap();
    assert!(
        skipped.contains("无命中"),
        "被排除目录不应被检索: {skipped}"
    );
}
