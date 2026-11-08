//! 交付摘要的一次请求内文件事实收集。
//!
//! 只在单次 `summarize` 调用内复用事实：不把文件状态缓存到进程或数据库，
//! 因此下一次详情请求仍会看到外部删除、改写和替换。

use std::collections::HashMap;
use std::fs::Metadata;
use std::path::Path;

#[cfg(test)]
use std::cell::Cell;

const MAX_INSPECTABLE_BYTES: u64 = 512 * 1024;
const TEXT_EXTENSIONS: &[&str] = &[
    "html", "htm", "js", "mjs", "cjs", "css", "json", "svg", "md", "txt", "xml",
];

#[cfg(test)]
thread_local! {
    static COLLECT_CALLS: Cell<usize> = const { Cell::new(0) };
}

#[derive(Debug, Clone, Default)]
pub(super) struct FileFact {
    pub(super) is_file: bool,
    pub(super) len: u64,
    pub(super) text: Option<String>,
}

pub(super) fn collect_file_facts(
    workspace: &Path,
    required: &[(String, String)],
) -> HashMap<String, FileFact> {
    #[cfg(test)]
    COLLECT_CALLS.with(|calls| calls.set(calls.get() + 1));
    collect_file_facts_with(
        workspace,
        required,
        |path| std::fs::metadata(path).ok(),
        read_text_with_metadata,
    )
}

#[cfg(test)]
pub(super) fn reset_collect_calls() {
    COLLECT_CALLS.with(|calls| calls.set(0));
}

#[cfg(test)]
pub(super) fn collect_calls() -> usize {
    COLLECT_CALLS.with(Cell::get)
}

/// 元数据与正文读取均经注入闭包，单测可以证明「每条声明路径一次 stat，
/// 可检查文本一次 read」，而生产实现仍直接使用标准文件系统调用。
fn collect_file_facts_with<M, R>(
    workspace: &Path,
    required: &[(String, String)],
    mut metadata: M,
    mut read_text: R,
) -> HashMap<String, FileFact>
where
    M: FnMut(&Path) -> Option<Metadata>,
    R: FnMut(&Path, &Metadata) -> Option<String>,
{
    if !workspace.is_dir() {
        return HashMap::new();
    }
    required
        .iter()
        .map(|(path, _)| {
            let abs = workspace.join(path);
            let fact = metadata(&abs)
                .map(|meta| FileFact {
                    is_file: meta.is_file(),
                    len: meta.len(),
                    text: meta.is_file().then(|| read_text(&abs, &meta)).flatten(),
                })
                .unwrap_or_default();
            (path.clone(), fact)
        })
        .collect()
}

pub(super) fn missing_from_facts(
    workspace_available: bool,
    required: &[(String, String)],
    facts: &HashMap<String, FileFact>,
) -> Vec<String> {
    if !workspace_available {
        return Vec::new();
    }
    required
        .iter()
        .filter(|(path, _)| !facts.get(path).is_some_and(|fact| fact.is_file))
        .map(|(path, _)| path.clone())
        .collect()
}

pub(super) fn basic_content_issues(
    required: &[(String, String)],
    facts: &HashMap<String, FileFact>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (path, _) in required {
        let Some(fact) = facts.get(path) else {
            continue;
        };
        if !fact.is_file {
            continue;
        }
        if fact.len == 0 {
            out.push(format!("{path}:文件为空(0B)"));
            continue;
        }
        let Some(text) = &fact.text else {
            continue;
        };
        let lines = text.lines().filter(|line| line.contains("@@")).count();
        if lines > 0 {
            out.push(format!("{path}:含 {lines} 行 @@ 未替换槽位"));
        } else if text.trim().is_empty() {
            out.push(format!("{path}:文件仅含空白"));
        }
    }
    out
}

fn read_text_with_metadata(path: &Path, meta: &Metadata) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !TEXT_EXTENSIONS.contains(&ext.as_str()) || meta.len() > MAX_INSPECTABLE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;
    use std::cell::Cell;

    #[test]
    fn facts_collect_each_metadata_and_text_once() {
        let dir = TempDataDir::new("delivery-facts-count");
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("a.txt"), "ready\n").unwrap();
        std::fs::write(ws.join("b.bin"), [0u8, 1, 2]).unwrap();
        std::fs::create_dir(ws.join("folder")).unwrap();
        let required = vec![
            ("a.txt".to_string(), String::new()),
            ("b.bin".to_string(), String::new()),
            ("folder".to_string(), String::new()),
            ("missing.txt".to_string(), String::new()),
        ];
        let metadata_calls = Cell::new(0usize);
        let text_calls = Cell::new(0usize);
        let facts = collect_file_facts_with(
            &ws,
            &required,
            |path| {
                metadata_calls.set(metadata_calls.get() + 1);
                std::fs::metadata(path).ok()
            },
            |path, meta| {
                text_calls.set(text_calls.get() + 1);
                read_text_with_metadata(path, meta)
            },
        );
        assert_eq!(metadata_calls.get(), required.len());
        assert_eq!(text_calls.get(), 2, "仅普通文件应进入正文判定");
        assert!(facts["a.txt"]
            .text
            .as_deref()
            .is_some_and(|s| s == "ready\n"));
        assert!(facts["b.bin"].is_file);
        assert!(!facts["folder"].is_file);
        assert!(!facts["missing.txt"].is_file);
        assert_eq!(
            missing_from_facts(true, &required, &facts),
            vec!["folder".to_string(), "missing.txt".to_string()]
        );
        assert!(basic_content_issues(&required, &facts).is_empty());
    }

    #[test]
    fn facts_are_request_local_and_reflect_external_rewrite() {
        let dir = TempDataDir::new("delivery-facts-refresh");
        let ws = dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let required = vec![("report.txt".to_string(), String::new())];
        std::fs::write(ws.join("report.txt"), "ready\n").unwrap();
        let first = collect_file_facts(&ws, &required);
        assert!(basic_content_issues(&required, &first).is_empty());
        std::fs::write(ws.join("report.txt"), "@@TODO@@\n").unwrap();
        let second = collect_file_facts(&ws, &required);
        assert_eq!(
            basic_content_issues(&required, &second),
            vec!["report.txt:含 1 行 @@ 未替换槽位"]
        );
    }
}
