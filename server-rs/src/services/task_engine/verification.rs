//! 版本绑定的独立验收证据与活上下文修复反馈（TQUAL-16/20）。
use crate::models::tool_policy::VerificationSpec;
use crate::models::types::{ImageRef, LlmMessage};
use crate::services::image_service::ImageService;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    NotRun,
    Unavailable,
}

impl VerificationStatus {
    pub(crate) fn event_title(self) -> &'static str {
        match self {
            Self::Passed => "独立验收通过",
            Self::Failed => "独立验收失败",
            Self::NotRun => "独立验收待执行",
            Self::Unavailable => "独立验收环境不可用",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationRecord {
    pub browser_required: bool,
    pub resources: BTreeMap<String, String>,
    pub status: VerificationStatus,
    pub artifact_version: Option<String>,
    pub checked_at: Option<String>,
    pub detail: String,
    pub original_goal: String,
    pub checks: Vec<String>,
    pub issues: Vec<String>,
    pub screenshots: Vec<ImageRef>,
    pub current: bool,
}

impl VerificationRecord {
    pub fn not_run(goal: &str) -> Self {
        Self {
            browser_required: false,
            resources: BTreeMap::new(),
            status: VerificationStatus::NotRun,
            artifact_version: None,
            checked_at: None,
            detail: "尚未执行独立验收".into(),
            original_goal: goal.into(),
            checks: vec![],
            issues: vec![],
            screenshots: vec![],
            current: false,
        }
    }
}

pub(crate) fn evidence_dir(scratch: &Path) -> PathBuf {
    scratch.with_extension("verification")
}

pub(crate) const BROWSER_PLAN_NOTE:&str = "\n[独立验收]\n交付网页时使用真实多文件入口；需构建的项目先执行实际构建，可用 web_root 指定 dist 等工作区内的静态服务根。可写 .kedai-verify.json 声明入口与关键交互，例如 {\"entry\":\"index.html\",\"actions\":[{\"action\":\"click\",\"selector\":\"#start\"},{\"action\":\"assert_text\",\"selector\":\"#status\",\"contains\":\"已开始\"}]}。支持 click、fill(selector/value)、assert_text(selector/contains)、assert_visible(selector)。断言必须对应原始需求；缺少交互断言只能验证加载，不能声称功能完成。宿主会把实际失败和截图反馈给当前上下文，请据此修复。\n";

pub(crate) fn make_spec(
    svc: &dyn crate::services::task_core::TaskBackend,
    task_id: &str,
    goal: &str,
    required_files: Vec<String>,
    screenshot: bool,
) -> Result<std::sync::Arc<VerificationSpec>, String> {
    let dir = evidence_dir(&svc.scratch_dir_for(task_id)?);
    Ok(std::sync::Arc::new(VerificationSpec {
        evidence_dir: dir,
        goal: goal.into(),
        required_files,
        screenshot,
    }))
}

pub(crate) fn save(dir: &Path, record: &VerificationRecord) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    crate::utils::fs_atomic::write_atomic(&dir.join("latest.json"), &bytes)
        .map_err(|e| format!("验收证据保存失败：{e}"))
}

/// 实际文件内容决定版本，不依赖写工具次数、mtime 或模型自述。
pub(crate) fn artifact_version(workspace: &Path) -> Result<String, String> {
    artifact_version_for(workspace, &[])
}

/// 流式计算内容指纹；metadata 后文件仍可能增长，不能以一次 stat 代替读取上限。
fn file_hash(path: &Path) -> Result<(String, u64), String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| format!("验收读取文件失败：{e}"))?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("验收读取文件失败：{e}"))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > 128 * 1024 * 1024 {
            return Err("验收读取期间文件超过 128MB".into());
        }
        digest.update(&buffer[..n]);
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

fn resources_match(workspace: &Path, resources: &BTreeMap<String, String>) -> Result<bool, String> {
    let root = std::fs::canonicalize(workspace).map_err(|e| e.to_string())?;
    for (path, expected) in resources {
        let file = std::fs::canonicalize(root.join(path))
            .map_err(|e| format!("受检资源不可用：{path}：{e}"))?;
        if !file.starts_with(&root) {
            return Err("受检资源越出工作区".into());
        }
        if file_hash(&file)?.0 != *expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn artifact_version_for(workspace: &Path, resources: &[String]) -> Result<String, String> {
    let canonical = std::fs::canonicalize(workspace).map_err(|e| e.to_string())?;
    let workspace = canonical.as_path();
    let mut files = Vec::new();
    let mut pending = vec![workspace.to_path_buf()];
    let mut total = 0u64;
    while let Some(dir) = pending.pop() {
        for item in std::fs::read_dir(&dir).map_err(|e| format!("验收读取目录失败：{e}"))?
        {
            let item = item.map_err(|e| e.to_string())?;
            let path = item.path();
            let kind = item.file_type().map_err(|e| e.to_string())?;
            if matches!(
                item.file_name().to_str(),
                Some(".git" | "node_modules" | "target" | ".cache")
            ) {
                continue;
            }
            if kind.is_symlink() {
                return Err(format!(
                    "验收版本暂不支持符号链接：{}",
                    item.file_name().to_string_lossy()
                ));
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                total += item.metadata().map_err(|e| e.to_string())?.len();
                files.push(path);
                if files.len() > 10000 || total > 128 * 1024 * 1024 {
                    return Err("验收版本范围超过 10000 文件或 128MB；请缩小工作区".into());
                }
            }
        }
    }
    // 常规源码遍历跳过依赖/缓存，但实际 HTTP 加载的文件必须补回，包括 target 内入口。
    for resource in resources {
        let path = std::fs::canonicalize(workspace.join(resource))
            .map_err(|e| format!("受检资源无法读取：{resource}：{e}"))?;
        if !path.starts_with(workspace) {
            return Err("受检资源已越出工作区".into());
        }
        files.push(path);
    }
    files.sort();
    files.dedup();
    let size = files
        .iter()
        .try_fold(0u64, |n, path| std::fs::metadata(path).map(|m| n + m.len()))
        .map_err(|e| e.to_string())?;
    if files.len() > 10000 || size > 128 * 1024 * 1024 {
        return Err("受检资源超过 10000 文件或 128MB；未进行部分哈希冒充完整版本".into());
    }
    let mut hash = Sha256::new();
    let mut actual_size = 0u64;
    for path in files {
        let name = path
            .strip_prefix(workspace)
            .map_err(|e| format!("受检文件不在工作区：{e}"))?
            .to_string_lossy()
            .replace('\\', "/");
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        let (digest, size) = file_hash(&path)?;
        actual_size += size;
        if actual_size > 128 * 1024 * 1024 {
            return Err("验收读取期间总文件量超过 128MB".into());
        }
        hash.update(size.to_le_bytes());
        hash.update(digest.as_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn load_current(dir: &Path, workspace: &Path, goal: &str) -> VerificationRecord {
    let read = std::fs::read(dir.join("latest.json"));
    let mut record = match read {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return VerificationRecord::not_run(goal)
        }
        result => match result.map_err(|e| e.to_string()).and_then(|bytes| {
            serde_json::from_slice::<VerificationRecord>(&bytes).map_err(|e| e.to_string())
        }) {
            Ok(record) => record,
            Err(e) => {
                let mut r = VerificationRecord::not_run(goal);
                r.status = VerificationStatus::Unavailable;
                r.detail = format!("无法读取验收证据：{e}");
                return r;
            }
        },
    };
    if record.artifact_version.is_none() {
        return record;
    }
    // 资源哈希来自真实 HTTP 响应；即使结束时版本扫描碰上并发写入，也不能认领未测的新字节。
    match resources_match(workspace, &record.resources) {
        Ok(true) => {}
        Ok(false) => {
            record.current = false;
            record.status = VerificationStatus::NotRun;
            record.detail = "受检资源已改变，旧截图/断言证据失效".into();
            return record;
        }
        Err(error) => {
            record.current = false;
            record.status = VerificationStatus::Unavailable;
            record.detail = error;
            return record;
        }
    }
    match artifact_version_for(
        workspace,
        &record.resources.keys().cloned().collect::<Vec<_>>(),
    ) {
        Ok(version) if record.artifact_version.as_ref() == Some(&version) => record.current = true,
        Ok(_) => {
            record.current = false;
            record.status = VerificationStatus::NotRun;
            record.detail = "产物已变化，先前验收证据失效，等待重新验证".into();
        }
        Err(e) => {
            record.current = false;
            record.status = VerificationStatus::Unavailable;
            record.detail = e;
        }
    }
    record
}

/// 验收属于交付证据，不属于“这次有没有写文件”门槛；续跑、自定义流程也不能略过已声明验收。
pub(crate) fn apply_terminal_gate(
    terminal: &mut crate::services::task_core::TaskTerminal,
    record: &VerificationRecord,
) {
    use crate::models::types::TaskStatus;
    use crate::services::task_core::TaskTerminal;
    let note = match record.status {
        VerificationStatus::Failed => &record.detail,
        VerificationStatus::Passed if record.current => return,
        _ if record.browser_required => &record.detail,
        _ => return,
    };
    let (status, error) = match terminal {
        TaskTerminal::Complete { status, error, .. }
        | TaskTerminal::Followup { status, error, .. } => (status, error),
        _ => return,
    };
    if matches!(status, TaskStatus::Done | TaskStatus::Partial) {
        *status = TaskStatus::Partial;
        let note = format!("独立验收未通过：{note}");
        *error = Some(super::delivery::merge_note(error.as_deref(), &note));
    }
}

/// 检查输入契约。可选 manifest 只描述操作与断言，不能替换冻结的用户目标。
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserPlan {
    pub entry: Option<String>,
    pub web_root: Option<String>,
    #[serde(default)]
    pub actions: Vec<BrowserAction>,
}

impl BrowserPlan {
    fn requires_browser(&self) -> bool {
        self.entry.is_some() || self.web_root.is_some() || !self.actions.is_empty()
    }
}
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum BrowserAction {
    Click { selector: String },
    Fill { selector: String, value: String },
    AssertText { selector: String, contains: String },
    AssertVisible { selector: String },
}

/// 执行前只冻结已存在的用户验收文件；不存在要显式记 null，不能把模型后来写的测试锁死。
pub(crate) fn prepare_run(dir: &Path, workspace: &Path, goal: &str) -> Result<(), String> {
    let path = dir.join("contract.json");
    let initial = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<Option<BrowserPlan>>(&bytes)
            .map_err(|e| format!("冻结验收契约损坏：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let initial = read_browser_plan(workspace)?;
            crate::utils::fs_atomic::write_atomic(
                &path,
                &serde_json::to_vec(&initial).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("保存执行前验收契约失败：{e}"))?;
            initial
        }
        Err(e) => return Err(format!("读取冻结验收契约失败：{e}")),
    };
    // 每次运行只初始化一次；团队分片/自定义节点不能把其他节点的验收证据清空。
    let mut record = VerificationRecord::not_run(goal);
    record.browser_required = initial.as_ref().is_some_and(BrowserPlan::requires_browser);
    save(dir, &record)
}

fn read_browser_plan(workspace: &Path) -> Result<Option<BrowserPlan>, String> {
    use std::io::Read;
    let file = match std::fs::File::open(workspace.join(".kedai-verify.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("无法读取验收步骤：{e}")),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读取验收步骤失败：{e}"))?;
    if bytes.len() > 65536 {
        return Err(".kedai-verify.json 超过 64KB".into());
    }
    let plan: BrowserPlan =
        serde_json::from_slice(&bytes).map_err(|e| format!("验收步骤格式错误：{e}"))?;
    if plan.actions.len() > 30 {
        return Err("浏览器验收最多 30 个交互/断言".into());
    }
    Ok(Some(plan))
}

pub(crate) fn browser_plan(workspace: &Path) -> Result<BrowserPlan, String> {
    Ok(read_browser_plan(workspace)?.unwrap_or_default())
}

pub(crate) async fn verify(
    spec: &VerificationSpec,
    workspace: &Path,
    images: &ImageService,
) -> Result<VerificationRecord, String> {
    let version = artifact_version(workspace)?;
    let mut record = VerificationRecord::not_run(&spec.goal);
    record.artifact_version = Some(version.clone());
    record.checked_at = Some(chrono::Utc::now().to_rfc3339());
    record.current = true;
    let required: Vec<_> = spec
        .required_files
        .iter()
        .map(|p| (p.clone(), "验收".into()))
        .collect();
    record.issues = super::delivery::missing_files(workspace, &required)
        .into_iter()
        .map(|p| format!("缺少必需文件：{p}"))
        .collect();
    record.issues.extend(super::delivery::content_issues(
        workspace, &required, &spec.goal,
    ));
    let plan = browser_plan(workspace);
    match plan {
        Err(error) => {
            record.issues.push(error);
            record.status = VerificationStatus::Failed;
        }
        Ok(plan) => {
            let root = match &plan.web_root {
                Some(root) => std::fs::canonicalize(workspace.join(root))
                    .map_err(|e| format!("验收 web_root 不可用：{e}"))?,
                None => std::fs::canonicalize(workspace).map_err(|e| e.to_string())?,
            };
            if !root.starts_with(std::fs::canonicalize(workspace).map_err(|e| e.to_string())?) {
                return Err("验收 web_root 必须位于任务工作区内".into());
            }
            if plan
                .entry
                .as_ref()
                .is_some_and(|entry| !root.join(entry).is_file())
            {
                record.issues.push("声明的浏览器验收入口不存在".into());
            }
            let entry = plan
                .entry
                .clone()
                .or_else(|| super::delivery::pick_single_file_entry(&root, &required));
            record.browser_required = entry.is_some() || plan.requires_browser();
            if entry.is_none() && record.browser_required {
                record.detail = "已声明浏览器检查，但没有可加载的 HTML 入口".into();
            }
            if let Some(entry) = entry.filter(|p| root.join(p).is_file()) {
                match super::browser_verify::probe(
                    &root,
                    &entry,
                    &spec.goal,
                    &plan.actions,
                    spec.screenshot,
                    images,
                )
                .await
                {
                    Ok(out) => {
                        let base = std::fs::canonicalize(workspace).map_err(|e| e.to_string())?;
                        for (path, hash) in out.resources {
                            let full = root.join(path);
                            let rel = full
                                .strip_prefix(&base)
                                .map_err(|e| e.to_string())?
                                .to_string_lossy()
                                .replace('\\', "/");
                            if file_hash(&full)?.0 != hash {
                                record.issues.push(format!("验证期间资源发生变化：{rel}"));
                            }
                            record.resources.insert(rel, hash);
                        }
                        record.checks = out.checks;
                        record.issues.extend(out.issues);
                        record.screenshots = out.screenshots;
                        record.status = if !record.issues.is_empty() {
                            VerificationStatus::Failed
                        } else if plan.actions.iter().any(|a| {
                            matches!(
                                a,
                                BrowserAction::AssertText { .. }
                                    | BrowserAction::AssertVisible { .. }
                            )
                        }) {
                            VerificationStatus::Passed
                        } else {
                            record.detail="入口加载完成，但没有需求断言；功能仍未验收，请声明与原始要求对应的检查".into();
                            VerificationStatus::NotRun
                        };
                    }
                    Err(error) => {
                        record.status = VerificationStatus::Unavailable;
                        record.detail = error;
                    }
                }
            }
        }
    }
    if !record.issues.is_empty() {
        record.status = VerificationStatus::Failed;
        record.detail = record.issues.join("\n");
    } else if record.status == VerificationStatus::Passed {
        record.detail = "入口加载与所列交互断言通过；未列出的要求和视觉效果仍须核对".into();
    }
    if artifact_version(workspace)? != version {
        record.status = VerificationStatus::NotRun;
        record.current = false;
        record.detail = "验证期间文件变化，本次证据无效".into();
    }
    record.artifact_version = Some(artifact_version_for(
        workspace,
        &record.resources.keys().cloned().collect::<Vec<_>>(),
    )?);
    if !resources_match(workspace, &record.resources)? {
        record.current = false;
        record.status = VerificationStatus::NotRun;
        record.detail = "验证期间受检资源改变，本次证据无效".into();
    }
    save(&spec.evidence_dir, &record)?;
    Ok(record)
}

/// 同一版本只反馈一次；修改后重验。反馈保留原目标，图片通过真实 image parts 下发。
#[derive(Default)]
pub(crate) struct LiveVerification {
    last_version: Option<String>,
    last_resources: Vec<String>,
    frozen_plan: Option<BrowserPlan>,
}
impl LiveVerification {
    pub(crate) async fn before_finish(
        &mut self,
        spec: &VerificationSpec,
        workspace: &Path,
        images: &ImageService,
        vision: bool,
    ) -> Result<Option<(VerificationStatus, LlmMessage)>, String> {
        let version = artifact_version_for(workspace, &self.last_resources)?;
        if self.last_version.as_ref() == Some(&version) {
            return Ok(None);
        }
        let plan = browser_plan(workspace);
        // 用户提供的契约跨续跑保留；null 表示最初未提供，模型生成的测试仍允许纠正定位等缺陷。
        if self.frozen_plan.is_none() {
            let bytes = std::fs::read(spec.evidence_dir.join("contract.json"))
                .map_err(|e| format!("读取执行前验收契约失败：{e}"))?;
            self.frozen_plan = serde_json::from_slice::<Option<BrowserPlan>>(&bytes)
                .map_err(|e| format!("冻结验收契约损坏：{e}"))?;
        }
        let changed_plan = matches!((&self.frozen_plan,&plan),(Some(frozen),Ok(plan)) if frozen!=plan)
            || self.frozen_plan.is_some() && plan.is_err();
        let verified = if changed_plan {
            let mut record = VerificationRecord::not_run(&spec.goal);
            record.artifact_version = Some(version.clone());
            record.current = true;
            record.status = VerificationStatus::Failed;
            record.detail =
                "修复期间修改了已冻结的验收契约，请修复实现并恢复原断言，不得降低验收条件".into();
            record.issues.push(record.detail.clone());
            record.checks.push(format!(
                "执行前冻结的验收步骤：{}",
                serde_json::to_string(&self.frozen_plan).map_err(|e| e.to_string())?
            ));
            save(&spec.evidence_dir, &record)?;
            Ok(record)
        } else {
            verify(spec, workspace, images).await
        };
        let record = match verified {
            Ok(record) => record,
            Err(error) => {
                let mut record = VerificationRecord::not_run(&spec.goal);
                record.browser_required = workspace.join("index.html").is_file()
                    || browser_plan(workspace).is_ok_and(|p| p.requires_browser());
                record.status = VerificationStatus::Unavailable;
                record.detail = error;
                save(&spec.evidence_dir, &record)?;
                record
            }
        };
        self.last_version = record.artifact_version.clone().or(Some(version));
        self.last_resources = record.resources.keys().cloned().collect();
        if record.status == VerificationStatus::NotRun
            && !record.browser_required
            && record.checks.is_empty()
        {
            return Ok(None);
        }
        let facts = crate::services::prompt_kit::untrusted_boundary_task(
            "独立验收的外部程序输出",
            &format!(
                "状态：{:?}\n产物版本：{}\n{}\n已执行检查：{}",
                record.status,
                record.artifact_version.as_deref().unwrap_or("未取得"),
                record.detail,
                record.checks.join("；")
            ),
        );
        // 不重放带步骤指令的原目标：模型会误以为用户重开任务。目标仍在首条消息中完整保留。
        let mut message=LlmMessage::plain("user",&format!("[独立验收证据]\n这是本轮准备交付后的宿主检查，不是新的任务；首条用户目标仍有效，已完成的步骤无需重复。\n{facts}\n核对证据覆盖范围。通过时按当前版本的实际结果收尾，不重复有效检查；失败时只修复具体问题并重验，不降低用户断言；环境不可用时列出待验项，不宣称通过。"));
        if vision {
            for mut image in record.screenshots {
                image.data_url = images.resolve_data_url(&image.id)?;
                message.images.push(image);
            }
        } else if !record.screenshots.is_empty() {
            message
                .content
                .push_str("\n当前执行连接无视觉能力，截图已持久化但视觉效果未验证。");
        }
        Ok(Some((record.status, message)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;
    #[test]
    fn changed_artifact_invalidates_passed_evidence() {
        let dir = TempDataDir::new("verify-version");
        let ws = dir.join("workspace");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(ws.join("app.js"), "old").unwrap();
        let evidence = dir.join("evidence");
        let mut record = VerificationRecord::not_run("原始目标");
        record.status = VerificationStatus::Passed;
        record.artifact_version = Some(artifact_version(&ws).unwrap());
        save(&evidence, &record).unwrap();
        assert_eq!(
            load_current(&evidence, &ws, "目标").status,
            VerificationStatus::Passed
        );
        std::fs::write(ws.join("app.js"), "new").unwrap();
        let stale = load_current(&evidence, &ws, "目标");
        assert_eq!(stale.status, VerificationStatus::NotRun);
        assert!(!stale.current);
        assert_eq!(stale.original_goal, "原始目标");
    }
    #[test]
    fn unreadable_evidence_is_not_passed() {
        let dir = TempDataDir::new("verify-corrupt");
        std::fs::write(dir.join("latest.json"), "broken").unwrap();
        assert_eq!(
            load_current(dir.path(), dir.path(), "目标").status,
            VerificationStatus::Unavailable
        );
    }
    #[tokio::test]
    async fn repairs_cannot_weaken_frozen_acceptance() {
        let dir = TempDataDir::new("verify-frozen");
        let ws = dir.join("ws");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(
            ws.join(".kedai-verify.json"),
            r##"{"actions":[{"action":"assert_text","selector":"#count","contains":"0"}]}"##,
        )
        .unwrap();
        let original = BrowserPlan {
            entry: None,
            web_root: None,
            actions: vec![BrowserAction::AssertText {
                selector: "#count".into(),
                contains: "1".into(),
            }],
        };
        let spec = VerificationSpec {
            evidence_dir: dir.join("evidence"),
            goal: "点击应加一".into(),
            required_files: vec![],
            screenshot: false,
        };
        crate::utils::fs_atomic::write_atomic(
            &spec.evidence_dir.join("contract.json"),
            &serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        let mut live = LiveVerification::default();
        let (_, message) = live
            .before_finish(&spec, &ws, &ImageService::new(dir.join("images")), false)
            .await
            .unwrap()
            .unwrap();
        assert!(message.content.contains("不得降低验收条件"));
        assert_eq!(
            load_current(&spec.evidence_dir, &ws, &spec.goal).status,
            VerificationStatus::Failed
        );
    }
    #[test]
    fn delivered_resources_under_cache_directories_are_versioned() {
        let dir = TempDataDir::new("verify-served-resource");
        let ws = dir.join("ws");
        std::fs::create_dir_all(ws.join("target/site")).unwrap();
        let app = ws.join("target/site/app.js");
        std::fs::write(&app, "old").unwrap();
        let mut record = VerificationRecord::not_run("网页");
        record.status = VerificationStatus::Passed;
        record.browser_required = true;
        record.resources.insert(
            "target/site/app.js".into(),
            format!("{:x}", Sha256::digest(b"old")),
        );
        record.artifact_version = Some(
            artifact_version_for(&ws, &record.resources.keys().cloned().collect::<Vec<_>>())
                .unwrap(),
        );
        let evidence = dir.join("evidence");
        save(&evidence, &record).unwrap();
        assert!(load_current(&evidence, &ws, "网页").current);
        std::fs::write(app, "new").unwrap();
        assert_eq!(
            load_current(&evidence, &ws, "网页").status,
            VerificationStatus::NotRun
        );
    }

    #[tokio::test]
    async fn model_authored_checks_remain_repairable_without_weakening_user_contracts() {
        let dir = TempDataDir::new("verify-authored-checks");
        let ws = dir.join("ws");
        std::fs::create_dir(&ws).unwrap();
        let spec = VerificationSpec {
            evidence_dir: dir.join("evidence"),
            goal: "保持用户功能要求，修复测试定位错误".into(),
            required_files: vec![],
            screenshot: false,
        };
        // 执行前没有用户验收文件，null 是已记录的事实，不等于允许覆盖已有用户契约。
        crate::utils::fs_atomic::write_atomic(&spec.evidence_dir.join("contract.json"), b"null")
            .unwrap();
        let images = ImageService::new(dir.join("images"));
        for selector in ["#typo", "#actual"] {
            std::fs::write(
                ws.join(".kedai-verify.json"),
                serde_json::json!({"actions":[{"action":"assert_visible","selector":selector}]})
                    .to_string(),
            )
            .unwrap();
            let mut live = LiveVerification::default();
            live.before_finish(&spec, &ws, &images, false)
                .await
                .unwrap();
            let record = load_current(&spec.evidence_dir, &ws, &spec.goal);
            assert_eq!(
                record.status,
                VerificationStatus::NotRun,
                "没有网页入口仍应待验，而不是禁止修正模型编写的验收脚本"
            );
        }
    }
}
