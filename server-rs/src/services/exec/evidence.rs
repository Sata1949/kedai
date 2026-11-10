// 命令证据（TQUAL-18）：完整双流落盘，模型只取诊断窗口或有界行区间。
// handle 不含本机路径；目录按任务/会话身份隔离，不能拿另一任务的 handle 越权读取。
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub const MAX_READ_LINES: usize = 200;
// 为工具结果头与执行说明预留空间，最终回读文本小于 16000 UTF-8 字节。
pub const MAX_READ_BYTES: usize = 12_000;
const MAX_LINE_BYTES: usize = 4_096;
const EDGE_LINE_BYTES: usize = 320;
const SMALL_PREVIEW_BYTES: usize = 3_000;

pub fn owner_from_session(session_id: &str) -> String {
    match crate::services::task_change_service::task_id_of(session_id) {
        Some(task) => format!("task:{task}"),
        None => format!("session:{session_id}"),
    }
}

#[derive(Debug, Clone)]
pub struct ExecEvidence {
    pub handle: String,
    dir: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogState {
    Running,
    Completed,
    TimedOut,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogMeta {
    pub handle: String,
    /// 本次授权执行的命令预览；审计不靠作者转述，也不把大段命令灌回上下文。
    pub command: String,
    pub state: LogState,
    pub exit_code: Option<i32>,
    pub created_at: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
}

impl LogStream {
    pub fn filename(self) -> &'static str {
        match self {
            Self::Stdout => "stdout.log",
            Self::Stderr => "stderr.log",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LogPage {
    pub handle: String,
    pub stream: LogStream,
    pub start_line: usize,
    pub end_line: usize,
    pub next_line: Option<usize>,
    pub truncated: bool,
    pub text: String,
    pub execution: LogMeta,
}

fn owner_dir(data_dir: &Path, owner: &str) -> Result<PathBuf, String> {
    let root = data_dir.join("evidence").join("exec");
    std::fs::create_dir_all(&root).map_err(|e| format!("创建命令证据目录失败: {e}"))?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("解析命令证据目录失败: {e}"))?;
    let dir = root.join(format!("{:x}", Sha256::digest(owner.as_bytes())));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建任务证据目录失败: {e}"))?;
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("解析任务证据目录失败: {e}"))?;
    if !dir.starts_with(&root) {
        return Err("任务证据目录越出数据目录".into());
    }
    Ok(dir)
}

impl ExecEvidence {
    pub fn create(data_dir: &Path, owner: &str, command: &str) -> Result<Self, String> {
        let handle = uuid::Uuid::new_v4().to_string();
        let dir = owner_dir(data_dir, owner)?.join(&handle);
        std::fs::create_dir(&dir).map_err(|e| format!("创建命令日志失败: {e}"))?;
        let evidence = Self { handle, dir };
        for stream in [LogStream::Stdout, LogStream::Stderr] {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(evidence.path(stream))
                .map_err(|e| format!("创建命令输出文件失败: {e}"))?;
        }
        // 控制符改为空格，长度按 UTF-8 截断；20 条命令预览连同 JSON 转义也低于回读预算。
        let prefix = utf8_prefix(command, 256);
        let mut preview: String = prefix
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        if prefix.len() < command.len() {
            preview.push('…');
        }
        evidence.write_meta(&LogMeta {
            handle: evidence.handle.clone(),
            command: preview,
            state: LogState::Running,
            exit_code: None,
            created_at: crate::models::db::now_iso(),
            detail: String::new(),
        })?;
        Ok(evidence)
    }

    pub fn open(data_dir: &Path, owner: &str, handle: &str) -> Result<Self, String> {
        let id =
            uuid::Uuid::parse_str(handle).map_err(|_| "日志 handle 必须是有效 UUID".to_string())?;
        if id.to_string() != handle {
            return Err("日志 handle 必须是标准小写 UUID".into());
        }
        let root = owner_dir(data_dir, owner)?;
        let dir = root
            .join(handle)
            .canonicalize()
            .map_err(|_| "本任务不存在该命令日志".to_string())?;
        if !dir.starts_with(&root) {
            return Err("命令日志越出本任务证据目录".into());
        }
        Ok(Self {
            handle: handle.into(),
            dir,
        })
    }

    pub fn path(&self, stream: LogStream) -> PathBuf {
        self.dir.join(stream.filename())
    }
    pub fn cancel_path(&self) -> PathBuf {
        self.dir.join("cancelled")
    }
    pub fn directory(&self) -> &Path {
        &self.dir
    }

    pub fn metadata(&self) -> Result<LogMeta, String> {
        let path = self.checked_file("meta.json")?;
        let file = std::fs::File::open(path).map_err(|e| format!("读取命令证据状态失败: {e}"))?;
        serde_json::from_reader(file.take(8_192)).map_err(|e| format!("命令证据状态无效: {e}"))
    }

    fn write_meta(&self, meta: &LogMeta) -> Result<(), String> {
        let bytes = serde_json::to_vec(meta).map_err(|e| format!("编码命令证据状态失败: {e}"))?;
        crate::utils::fs_atomic::write_atomic(&self.dir.join("meta.json"), &bytes)
            .map_err(|e| format!("保存命令证据状态失败: {e}"))
    }

    pub fn finish(
        &self,
        state: LogState,
        exit_code: Option<i32>,
        detail: &str,
    ) -> Result<(), String> {
        let mut meta = self.metadata()?;
        meta.state = state;
        meta.exit_code = exit_code;
        let mut end = detail.len().min(2_048);
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        meta.detail = detail[..end].into();
        self.write_meta(&meta)
    }

    pub fn cancel(&self) -> Result<(), String> {
        // Android 同一 app UID 的进程桥在 waitFor 间隙观察本标记，不要求提权进程访问 DATA_DIR。
        if let Err(error) = std::fs::write(self.cancel_path(), b"cancelled") {
            let detail = format!("写入命令取消标记失败，无法保证 Android 命令已终止: {error}");
            self.finish(LogState::Failed, None, &detail)?;
            return Err(detail);
        }
        self.finish(
            LogState::Cancelled,
            None,
            "执行被取消；保留取消前已收到的输出",
        )
    }

    fn checked_file(&self, filename: &str) -> Result<PathBuf, String> {
        let path = self
            .dir
            .join(filename)
            .canonicalize()
            .map_err(|e| format!("读取命令日志失败: {e}"))?;
        if !path.starts_with(&self.dir) {
            return Err("命令日志文件越出本次证据目录".into());
        }
        Ok(path)
    }

    pub fn read_range(
        &self,
        stream: LogStream,
        start: usize,
        limit: usize,
    ) -> Result<LogPage, String> {
        if start == 0 || !(1..=MAX_READ_LINES).contains(&limit) {
            return Err(format!(
                "start_line 必须从 1 起，limit 必须在 1..={MAX_READ_LINES}"
            ));
        }
        let file = std::fs::File::open(self.checked_file(stream.filename())?)
            .map_err(|e| format!("打开命令日志失败: {e}"))?;
        let mut reader = BufReader::new(file);
        let mut number = 0;
        let mut text = String::new();
        let mut end = start.saturating_sub(1);
        let mut truncated = false;
        let mut next = None;
        while let Some((line, cut)) = bounded_line(&mut reader)? {
            number += 1;
            if number < start {
                continue;
            }
            let line = super::strip_ansi(&String::from_utf8_lossy(&line));
            let row = format!(
                "{number}: {}{}\n",
                line.trim_end_matches('\r'),
                if cut { " …[长行截断]" } else { "" }
            );
            if number - start >= limit || text.len() + row.len() > MAX_READ_BYTES {
                truncated = true;
                next = Some(number);
                break;
            }
            text.push_str(&row);
            truncated |= cut;
            end = number;
        }
        Ok(LogPage {
            handle: self.handle.clone(),
            stream,
            start_line: start,
            end_line: end,
            next_line: next,
            truncated,
            text,
            execution: self.metadata()?,
        })
    }

    /// 失败线索窗口是诊断提示，不以匹配词代替真实退出码。
    pub fn diagnostic_window(&self, stream: LogStream) -> Result<String, String> {
        let file = std::fs::File::open(self.checked_file(stream.filename())?)
            .map_err(|e| format!("打开命令诊断日志失败: {e}"))?;
        let mut reader = BufReader::new(file);
        let mut previous: VecDeque<String> = VecDeque::new();
        let mut result = String::new();
        let mut after = 0;
        let mut number = 0;
        while let Some((line, cut)) = bounded_line(&mut reader)? {
            number += 1;
            let line = super::strip_ansi(&String::from_utf8_lossy(&line));
            let row = format!(
                "{number}: {}{}\n",
                line.trim_end_matches('\r'),
                if cut { " …[长行截断]" } else { "" }
            );
            let hit = failure_hint(&line);
            if result.is_empty() && hit {
                result.push_str("[诊断线索窗口；完整输出可按行回读]\n");
                for row in &previous {
                    append_budgeted(&mut result, row);
                }
                append_budgeted(&mut result, &row);
                after = 4;
            } else if after > 0 {
                append_budgeted(&mut result, &row);
                after -= 1;
                if after == 0 {
                    break;
                }
            }
            previous.push_back(row);
            if previous.len() > 3 {
                previous.pop_front();
            }
        }
        Ok(result)
    }

    /// 完整输出已有持久副本时，32K 平台预览只作采集缓冲，不再整段回灌模型。
    /// 小输出逐字保留；长输出优先给中段失败窗口，未命中才给带行号的短首尾。
    pub fn model_preview(
        &self,
        stream: LogStream,
        original: &str,
        truncated: bool,
    ) -> Result<String, String> {
        if !truncated || original.len() <= SMALL_PREVIEW_BYTES {
            return Ok(original.to_string());
        }
        let diagnostic = self.diagnostic_window(stream)?;
        if !diagnostic.is_empty() {
            return Ok(diagnostic);
        }
        let file = std::fs::File::open(self.checked_file(stream.filename())?)
            .map_err(|e| format!("打开命令首尾日志失败: {e}"))?;
        let mut reader = BufReader::new(file);
        let mut first = Vec::new();
        let mut last = VecDeque::new();
        let mut number = 0;
        while let Some((line, cut)) = bounded_line(&mut reader)? {
            number += 1;
            let line = super::strip_ansi(&String::from_utf8_lossy(&line));
            let excerpt = utf8_prefix(line.trim_end_matches('\r'), EDGE_LINE_BYTES);
            let row = format!(
                "{number}: {excerpt}{}\n",
                if cut || excerpt.len() < line.trim_end_matches('\r').len() {
                    " …[长行省略]"
                } else {
                    ""
                }
            );
            if first.len() < 3 {
                first.push(row.clone());
            }
            last.push_back((number, row));
            if last.len() > 3 {
                last.pop_front();
            }
        }
        if number == 0 {
            return Ok(String::new());
        }
        let mut result =
            "[首尾窗口；未匹配失败线索，成功与否仍以退出码为准；完整输出可按行回读]\n".to_string();
        result.push_str(&first.concat());
        if number > first.len() {
            result.push_str("…（中间行已省略）\n");
            for (_, row) in last.into_iter().filter(|(line, _)| *line > first.len()) {
                result.push_str(&row);
            }
        }
        Ok(result)
    }
}

/// 归档已回灌的命令正文时只带失败窗口；支持平台窗口已经附加的日志行号。
pub(crate) fn diagnostic_excerpt(text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let Some(hit) = lines.iter().position(|line| {
        let body = line
            .split_once(": ")
            .filter(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
            .map_or(*line, |(_, body)| body);
        failure_hint(body)
    }) else {
        return String::new();
    };
    let mut result = "[保留的失败线索；完整输出用 exec_log_read 回读]\n".to_string();
    for line in &lines[hit.saturating_sub(3)..(hit + 5).min(lines.len())] {
        append_budgeted(&mut result, line);
        append_budgeted(&mut result, "\n");
    }
    result
}

/// 非零/未知退出码但没有通用错误词时仍保留状态和短预览；完整原因由日志句柄回读。
pub(crate) fn fallback_excerpt(text: &str) -> String {
    if text.len() <= SMALL_PREVIEW_BYTES {
        return text.to_string();
    }
    let first = text
        .lines()
        .take(3)
        .map(|line| utf8_prefix(line, EDGE_LINE_BYTES))
        .collect::<Vec<_>>();
    let mut last = text
        .lines()
        .rev()
        .take(3)
        .map(|line| utf8_prefix(line, EDGE_LINE_BYTES))
        .collect::<Vec<_>>();
    last.reverse();
    format!(
        "[历史返回预览首尾；完整错误按日志句柄回读]\n{}\n…\n{}",
        first.join("\n"),
        last.join("\n")
    )
}

fn utf8_prefix(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub(super) fn failure_hint(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("error:")
        || lower.contains("error[")
        || lower.starts_with("error ")
        || lower.contains("panicked at")
        || lower.contains("exception:")
        || lower.contains("assertionerror")
        || lower.trim_end().ends_with("... failed")
        || lower.contains("[error]")
        || lower.contains(" fail ")
        || lower.starts_with("failures:")
        || lower.starts_with("错误")
        || lower.starts_with("失败")
}

fn append_budgeted(out: &mut String, value: &str) {
    let mut end = value.len().min(MAX_READ_BYTES.saturating_sub(out.len()));
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&value[..end]);
}

/// 一行再长也只驻留 4KiB；超出部分仍排空，原始字节始终保存在日志文件里。
fn bounded_line(reader: &mut impl BufRead) -> Result<Option<(Vec<u8>, bool)>, String> {
    let mut line = Vec::new();
    let mut seen = false;
    let mut cut = false;
    loop {
        let buf = reader
            .fill_buf()
            .map_err(|e| format!("读取命令日志失败: {e}"))?;
        if buf.is_empty() {
            return Ok(seen.then_some((line, cut)));
        }
        seen = true;
        let newline = buf.iter().position(|b| *b == b'\n');
        let consumed = newline.map_or(buf.len(), |n| n + 1);
        let content = newline.unwrap_or(buf.len());
        let take = content.min(MAX_LINE_BYTES - line.len());
        line.extend_from_slice(&buf[..take]);
        cut |= content > take;
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some((line, cut)));
        }
    }
}

pub fn recent(data_dir: &Path, owner: &str) -> Result<Vec<LogMeta>, String> {
    let root = owner_dir(data_dir, owner)?;
    let mut logs = Vec::new();
    for item in std::fs::read_dir(root).map_err(|e| format!("列举命令证据失败: {e}"))? {
        let item = item.map_err(|e| format!("读取命令证据目录失败: {e}"))?;
        let handle = item.file_name().to_string_lossy().into_owned();
        logs.push(ExecEvidence::open(data_dir, owner, &handle)?.metadata()?);
        logs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.handle.cmp(&a.handle))
        });
        logs.truncate(20);
    }
    Ok(logs)
}

/// future 取消仍保留日志与可查询状态；正常结束必须显式 finish 后解除守卫。
pub(crate) struct EvidenceRun(pub Option<ExecEvidence>);
impl Drop for EvidenceRun {
    fn drop(&mut self) {
        if let Some(log) = &self.0 {
            if let Err(error) = log.cancel() {
                tracing::error!(handle = %log.handle, %error, "命令取消证据保存失败");
            }
        }
    }
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
