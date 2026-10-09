// 全局正则库服务(IMPHUB ⑥,2026-10-09):跨角色生效的**显示层**正则,持久化
// data/regex_scripts.json,结构 `{ "scripts": [RegexScript] }`。
//
// 与角色卡 `extensions.regex_scripts`(卡级)的关系:显示层把两者**合并**后应用,
// 全局在前、卡级在后(卡级更具体,后应用覆盖——顺序口径见 `docs/契约-协议与配置.md`)。
// 后端只负责全局库的整体读写与落盘;合并与渲染在前端(缓存键必须纳入全局库,
// 否则改了全局正则不会触发重渲染)。
//
// 条目**宽进**(对齐 prompt_inject_service 的 load 口径):入库/落盘的条目逐项经
// `normalize_script` 归一(酒馆 camelCase 与本机 snake_case 两种键名都收),
// 取不到查找模式的条目跳过并计数回报——正则**语法**非法不在写入侧拒绝
// (渲染层既有 try/catch 跳过),避免「库里有条目但界面打不开」这类写入期误杀。
use crate::parsing::regex_script::{normalize_script, RegexScript};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 全局正则库(整体读写;`{ "scripts": [...] }`)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegexScriptStore {
    #[serde(default)]
    pub scripts: Vec<RegexScript>,
}

impl RegexScriptStore {
    /// 从 data/regex_scripts.json 加载;缺失回退空库,损坏记录日志后回退空库(不静默归零)。
    /// 与 `PromptInjectConfig::load` 同一口径:坏文件不 panic、不半截生效,只留痕。
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("regex_scripts.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::error!(error = e.to_string(), "全局正则库读取失败,已回退空库");
                return Self::default();
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = e.to_string(), "全局正则库解析失败,已回退空库");
                return Self::default();
            }
        };
        let (store, skipped) = Self::from_value(value);
        if skipped > 0 {
            tracing::warn!(skipped, "全局正则库有不可识别条目,已跳过");
        }
        store
    }

    /// 把外部形状归一为库:收 `{scripts: [...]}` 与裸数组两种顶层形态,
    /// 逐项归一(见模块头「宽进」),返回 (库, 被跳过条数)。
    pub fn from_value(value: Value) -> (Self, usize) {
        let list = match value {
            Value::Object(mut obj) => obj.remove("scripts").unwrap_or(Value::Null),
            // 裸数组:导入件/手改文件都可能有,收下
            other => other,
        };
        let Value::Array(items) = list else {
            // 顶层既无 scripts 也不是数组:空库(不是错误——旧文件/初始状态)
            return (Self::default(), 0);
        };
        let mut scripts = Vec::with_capacity(items.len());
        let mut skipped = 0usize;
        for item in &items {
            match normalize_script(item) {
                Some(s) => scripts.push(s),
                None => {
                    skipped += 1;
                    tracing::warn!("全局正则库条目缺少查找模式,已跳过");
                }
            }
        }
        (RegexScriptStore { scripts }, skipped)
    }

    /// 持久化到 data/regex_scripts.json(原子写:崩溃不留半截 JSON)
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::utils::fs_atomic::write_atomic(&data_dir.join("regex_scripts.json"), text.as_bytes())
            .map_err(|e| e.to_string())
    }
}

/// 全局正则库服务:持有 data_dir,内存缓存库,整体读写
pub struct RegexScriptService {
    data_dir: PathBuf,
    store: RegexScriptStore,
}

impl RegexScriptService {
    pub fn new(data_dir: PathBuf) -> Self {
        let store = RegexScriptStore::load(&data_dir);
        RegexScriptService { data_dir, store }
    }

    pub fn get(&self) -> &RegexScriptStore {
        &self.store
    }

    /// 全量替换并持久化(逐个归一见 [`RegexScriptStore::from_value`]);返回 (落盘条数, 跳过条数)。
    /// 落盘失败返回 Err 且**内存不变**(不出现「内存有、磁盘无」的幻影条目)。
    pub fn set(&mut self, value: Value) -> Result<(usize, usize), String> {
        let (store, skipped) = RegexScriptStore::from_value(value);
        store.save(&self.data_dir)?;
        let n = store.scripts.len();
        self.store = store;
        Ok((n, skipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;
    use serde_json::json;

    fn script(id: &str, find: &str, enabled: bool) -> Value {
        json!({
            "id": id,
            "script_name": format!("脚本{id}"),
            "find_regex": find,
            "replace_string": "<b>替换</b>",
            "markdown_only": true,
            "enabled": enabled,
        })
    }

    /// 往返:落盘后重读,条目逐字段一致(含停用态与深度限制)
    #[test]
    fn roundtrip_persists_entries() {
        let dir = TempDataDir::new("regex-rt");
        let mut svc = RegexScriptService::new(dir.to_path_buf());
        assert!(svc.get().scripts.is_empty(), "初始应为空库");

        let (n, skipped) = svc
            .set(json!({ "scripts": [
                script("a", "/<Status\\/>/g", true),
                { "id": "b", "script_name": "带深度", "find_regex": "A", "replace_string": "B",
                  "markdown_only": false, "enabled": false, "min_depth": 2 }
            ]}))
            .unwrap();
        assert_eq!((n, skipped), (2, 0));

        let reloaded = RegexScriptService::new(dir.to_path_buf());
        let scripts = &reloaded.get().scripts;
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].id, "a");
        assert!(scripts[0].enabled);
        assert!(!scripts[1].enabled, "停用条目往返后不得复活");
        assert_eq!(scripts[1].min_depth, Some(2));
        assert_eq!(scripts[1].replace_string, "B");
    }

    /// 键名兼容:酒馆 camelCase 形态(以及 `disabled` 取反)也收
    #[test]
    fn accepts_tavern_camel_case_entries() {
        let dir = TempDataDir::new("regex-camel");
        let mut svc = RegexScriptService::new(dir.to_path_buf());
        let (n, skipped) = svc
            .set(json!({ "scripts": [
                { "id": "st1", "scriptName": "状态栏", "findRegex": "/<P\\/>/g",
                  "replaceString": "<i>状态</i>", "markdownOnly": true, "disabled": false }
            ]}))
            .unwrap();
        assert_eq!((n, skipped), (1, 0));
        let s = &svc.get().scripts[0];
        assert_eq!(s.script_name, "状态栏");
        assert_eq!(s.find_regex, "/<P\\/>/g");
        assert!(s.enabled);
    }

    /// 宽进:取不到查找模式的条目跳过并计数,其余条目照常入库(不整单拒绝)
    #[test]
    fn skips_entries_without_pattern_and_counts() {
        let dir = TempDataDir::new("regex-skip");
        let mut svc = RegexScriptService::new(dir.to_path_buf());
        let (n, skipped) = svc
            .set(json!({ "scripts": [
                script("ok", "A", true),
                { "id": "bad", "replace_string": "只有替换体" },
                { "id": "bad2" }
            ]}))
            .unwrap();
        assert_eq!((n, skipped), (1, 2));
        assert_eq!(svc.get().scripts.len(), 1);
        // 裸数组同样可收
        let (n2, _) = svc.set(json!([script("arr", "B", true)])).unwrap();
        assert_eq!(n2, 1);
    }

    /// 空替换串的条目合法保留(「纯删除」类脚本);库不替渲染层做语义裁剪
    #[test]
    fn keeps_empty_replace_string() {
        let dir = TempDataDir::new("regex-empty-repl");
        let mut svc = RegexScriptService::new(dir.to_path_buf());
        let (n, skipped) = svc
            .set(json!({ "scripts": [
                { "id": "del", "script_name": "删标记", "find_regex": "<X>[\\s\\S]*?</X>",
                  "replace_string": "", "markdown_only": true, "enabled": true }
            ]}))
            .unwrap();
        assert_eq!((n, skipped), (1, 0));
        assert_eq!(svc.get().scripts[0].replace_string, "");
    }

    /// 缺失文件 → 空库;损坏文件 → 空库(不 panic、不半截生效)
    #[test]
    fn missing_and_corrupt_file_fall_back_to_empty() {
        let dir = TempDataDir::new("regex-bad");
        let svc = RegexScriptService::new(dir.to_path_buf());
        assert!(svc.get().scripts.is_empty());

        std::fs::write(dir.join("regex_scripts.json"), "{{{ not json").unwrap();
        let svc2 = RegexScriptService::new(dir.to_path_buf());
        assert!(svc2.get().scripts.is_empty());
    }
}
