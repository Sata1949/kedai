//! TQUAL-15：任务进展按实际读取/验证事实计量，不把“调用了写工具”当作进展。
use serde_json::Value;
use std::collections::VecDeque;

use crate::utils::loop_guard::{fnv1a_hash, output_fingerprint};

/// 只保存最近事实的指纹，不驻留工具正文，不随长任务无限增长。
#[derive(Default)]
pub(super) struct TaskProgress {
    observations: VecDeque<u64>,
}

/// 文件读取结果包含真实路径和行号，必须保留路径/数字差异；命令输出沿用噪声归一化。
pub(super) fn result_fingerprint(tool: &str, output: &Value) -> u64 {
    let text = semantic_receipt(tool, output);
    if crate::tools::tool_sets::is_workspace_readonly_tool(tool) {
        fnv1a_hash(&[tool, &text])
    } else {
        fnv1a_hash(&[tool, &output_fingerprint(&text).to_string()])
    }
}

/// 证据句柄只是同一事实的存放位置；新 UUID 不能把重复失败伪装成进展。
fn semantic_receipt(tool: &str, output: &Value) -> String {
    let Some(raw) = output.as_str() else {
        return output.to_string();
    };
    let parts = match tool {
        "bash" => raw
            .rsplit_once("\n[exec-meta] ")
            .map(|(body, meta)| (body, meta, "")),
        "exec_log_read" => raw
            .strip_prefix("[exec-log] ")
            .and_then(|s| s.split_once('\n'))
            .map(|(meta, body)| ("", meta, body)),
        _ => None,
    };
    if let Some((head, meta, tail)) = parts {
        if let Ok(Value::Object(mut metadata)) = serde_json::from_str(meta) {
            metadata.remove("log_handle");
            metadata.remove("handle");
            return serde_json::json!(format!("{head}\n{}\n{tail}", Value::Object(metadata)))
                .to_string();
        }
    }
    output.to_string()
}

impl TaskProgress {
    pub(super) fn observe(&mut self, tool: &str, output: &Value) -> bool {
        // 写入由内容变更台账判定；拒绝/参数错误不是新证据，不能靠失败写入清零。
        if !matches!(
            tool,
            "fs_read"
                | "fs_glob"
                | "fs_grep"
                | "read"
                | "search"
                | "memory_read"
                | "bash"
                | "exec_log_read"
        ) || output.is_null()
            || output.get("error").is_some()
        {
            return false;
        }
        let fingerprint = result_fingerprint(tool, output);
        if self.observations.contains(&fingerprint) {
            return false;
        }
        self.observations.push_back(fingerprint);
        if self.observations.len() > 128 {
            self.observations.pop_front();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn log_handles_do_not_masquerade_as_new_evidence() {
        let a=json!("$ node check.cjs\nFAIL missing feature\n[exec-meta] {\"exit_code\":1,\"log_handle\":\"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\"}");
        let b=json!("$ node check.cjs\nFAIL missing feature\n[exec-meta] {\"exit_code\":1,\"log_handle\":\"bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb\"}");
        assert_eq!(
            result_fingerprint("bash", &a),
            result_fingerprint("bash", &b)
        );
        let mut progress = TaskProgress::default();
        assert!(progress.observe("bash", &a));
        assert!(!progress.observe("bash", &b));
    }

    #[test]
    fn task_quality_p0_distinct_file_facts_are_progress() {
        let mut progress = TaskProgress::default();
        for n in 0..20 {
            assert!(progress.observe(
                "fs_read",
                &json!(format!("src/{n}/index.ts\n1 interface Item {{}}"))
            ));
        }
        assert!(!progress.observe("fs_read", &json!("src/19/index.ts\n1 interface Item {}")));
    }

    #[test]
    fn task_quality_p0_write_receipts_and_errors_are_not_progress() {
        let mut progress = TaskProgress::default();
        assert!(!progress.observe("fs_write", &json!("已写入 out.txt")));
        assert!(!progress.observe("fs_edit", &json!({"error":"未授权"})));
        assert!(!progress.observe("bash", &json!({"error":"参数错误"})));
        assert!(progress.observe("bash", &json!("FAIL: expected 3, got 2")));
        assert!(!progress.observe("bash", &json!("FAIL: expected 3, got 2")));
        assert!(progress.observe("bash", &json!("PASS")));
    }

    #[test]
    fn task_quality_p0_evidence_history_is_bounded() {
        let mut progress = TaskProgress::default();
        for n in 0..300 {
            progress.observe("fs_read", &json!(format!("part-{n}.rs")));
        }
        assert_eq!(progress.observations.len(), 128);
    }
}
