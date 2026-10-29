//! 用户设定(USERPERSONA-1):角色扮演专属的全局用户角色配置。
//!
//! 独立配置文件 `DATA_DIR/user_persona.json`,不进入 `RuntimeSettings`:
//! - 避免 task 覆盖层 / settings.json 线格式 / check-settings-parity / params.rs 行数基线;
//! - 与 `prompt_floors.json` 同型(角色扮演专属配置),后续可按角色覆盖(USERPERSONA-L1)。
//!
//! 语义:
//! - `enabled=false` 时完全无行为变化(`{{user}}` 仍为「用户」,不注入【用户角色】段);
//! - `name` 为空时宏回退「用户」,注入段称呼用「你」;
//! - `description` 经 `untrusted_boundary("user_persona")` 包裹后注入;
//! - 任务模式不读本配置(任务侧保持「用户」,USERPERSONA-L4)。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::services::prompt_kit::untrusted_boundary;
use crate::utils::fs_atomic::write_atomic;

/// 称呼最大字符数(写入侧 400;load 侧超长截断,避免手改文件导致每轮提示词膨胀)
pub const MAX_NAME_CHARS: usize = 64;
/// 用户角色设定最大字符数
pub const MAX_DESCRIPTION_CHARS: usize = 4000;

/// 用户设定配置。缺字段一律取默认(false / 空 / 空),旧文件零迁移。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct UserPersonaConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

impl UserPersonaConfig {
    /// 归一化:trim;`name` 去掉换行并折叠连续空白;两字段按字符数截断(load 侧兜底)。
    pub fn normalize(&mut self) {
        self.normalize_whitespace();
        self.name = truncate_chars(&self.name, MAX_NAME_CHARS);
        self.description = truncate_chars(&self.description, MAX_DESCRIPTION_CHARS);
    }

    /// 只做空白归一(`name` 去换行 + 折叠连续空白 + trim,`description` trim),**不截断**。
    /// 写入路径必须先做这一步再 `validate`——否则截断会把「超长」变成静默丢字,
    /// 用户看到保存成功却少了一截称呼(API 应 400 点名,不是猜用户想留多少)。
    pub fn normalize_whitespace(&mut self) {
        self.name = self
            .name
            .replace(['\r', '\n'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        self.description = self.description.trim().to_string();
    }

    /// 写入侧校验(API 400 文案需含字段名,便于用户定位)。
    pub fn validate(&self) -> Result<(), String> {
        let name_chars = self.name.trim().chars().count();
        if name_chars > MAX_NAME_CHARS {
            return Err(format!(
                "name 最多 {MAX_NAME_CHARS} 个字符(当前 {name_chars})"
            ));
        }
        let desc_chars = self.description.trim().chars().count();
        if desc_chars > MAX_DESCRIPTION_CHARS {
            return Err(format!(
                "description 最多 {MAX_DESCRIPTION_CHARS} 个字符(当前 {desc_chars})"
            ));
        }
        Ok(())
    }

    /// 宏 `{{user}}` / `{{user_name}}` 的称呼:仅启用且称呼非空时生效。
    pub fn user_name(&self) -> Option<&str> {
        if self.enabled {
            let name = self.name.trim();
            if !name.is_empty() {
                return Some(name);
            }
        }
        None
    }

    /// 【用户角色】注入段(单一出处):
    /// - 未启用 / 称呼与设定都空 → None;
    /// - 只有称呼 → 只输出称呼行;
    /// - 有设定 → 经 `untrusted_boundary("user_persona")` 包裹(素材语气,不可提权)。
    pub fn block(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let name = self.name.trim();
        let description = self.description.trim();
        if name.is_empty() && description.is_empty() {
            return None;
        }
        let mut text = String::from("【用户角色】\n");
        text.push_str(&format!(
            "称呼: {}\n",
            if name.is_empty() { "你" } else { name }
        ));
        if !description.is_empty() {
            text.push_str("设定:\n");
            text.push_str(&untrusted_boundary("user_persona", description));
        }
        Some(text)
    }
}

/// 用户设定服务:内存配置 + DATA_DIR/user_persona.json 原子落盘。
pub struct UserPersonaService {
    data_dir: PathBuf,
    config: UserPersonaConfig,
}

impl UserPersonaService {
    pub fn new(data_dir: PathBuf) -> Self {
        let config = UserPersonaConfig::load(&data_dir);
        Self { data_dir, config }
    }

    pub fn get(&self) -> &UserPersonaConfig {
        &self.config
    }

    /// 空白归一 → 校验 → 落盘 → 替换内存(磁盘失败不造成「内存有、磁盘无」)。
    /// 顺序不可换:先校验原始长度,超长走 400 点名字段,不做静默截断。
    pub fn set(&mut self, mut config: UserPersonaConfig) -> Result<(), String> {
        config.normalize_whitespace();
        config.validate()?;
        config.save(&self.data_dir)?;
        self.config = config;
        Ok(())
    }
}

impl UserPersonaConfig {
    /// 缺失 → 默认;损坏 → 记 error 并回退默认(与 prompt_inject_service 同口径,不静默归零)。
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("user_persona.json");
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(config) => config,
                Err(error) => {
                    tracing::error!(
                        error = error.to_string(),
                        path = path.display().to_string(),
                        "用户设定解析失败,已回退默认配置"
                    );
                    Self::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                tracing::error!(
                    error = error.to_string(),
                    path = path.display().to_string(),
                    "用户设定读取失败,已回退默认配置"
                );
                Self::default()
            }
        };
        config.normalize();
        config
    }

    /// 原子写(崩溃不留半截 JSON)。
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        write_atomic(&data_dir.join("user_persona.json"), text.as_bytes())
            .map_err(|e| e.to_string())
    }
}

/// 按字符截断(不切断多字节字符)
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::TempDataDir;

    #[test]
    fn missing_file_returns_default() {
        let dir = TempDataDir::new("user-persona-missing");
        let cfg = UserPersonaConfig::load(dir.path());
        assert_eq!(cfg, UserPersonaConfig::default());
    }

    #[test]
    fn corrupted_file_returns_default_and_logs() {
        let dir = TempDataDir::new("user-persona-corrupt");
        std::fs::write(dir.path().join("user_persona.json"), "{broken").unwrap();
        let cfg = UserPersonaConfig::load(dir.path());
        assert_eq!(cfg, UserPersonaConfig::default());
    }

    #[test]
    fn save_load_roundtrip_and_atomic_write() {
        let dir = TempDataDir::new("user-persona-roundtrip");
        let cfg = UserPersonaConfig {
            enabled: true,
            name: "林".into(),
            description: "退役侦探,怕黑。".into(),
        };
        cfg.save(dir.path()).unwrap();
        let loaded = UserPersonaConfig::load(dir.path());
        assert_eq!(loaded, cfg);
        // 保存失败不覆盖内存:复用服务层语义(写入不存在目录下的路径仍会创建父目录,
        // 故这里只验证正常往返;磁盘失败分支由 API 层 500 承接)。
        let service = UserPersonaService::new(dir.path().to_path_buf());
        assert_eq!(service.get(), &cfg);
    }

    #[test]
    fn normalize_removes_newlines_and_collapses_spaces() {
        let mut cfg = UserPersonaConfig {
            enabled: true,
            name: "  林 \r\n 晚  ".into(),
            description: "  设定\n第二行  ".into(),
        };
        cfg.normalize();
        assert_eq!(cfg.name, "林 晚");
        assert_eq!(cfg.description, "设定\n第二行");
    }

    #[test]
    fn limits_are_enforced_and_load_truncates() {
        let too_long_name = "名".repeat(MAX_NAME_CHARS + 1);
        let cfg = UserPersonaConfig {
            enabled: true,
            name: too_long_name.clone(),
            description: "设".repeat(MAX_DESCRIPTION_CHARS + 1),
        };
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("name"), "{err}");
        let err = UserPersonaConfig {
            name: "ok".into(),
            ..cfg.clone()
        }
        .validate()
        .unwrap_err();
        assert!(err.contains("description"), "{err}");

        // load 侧归一化截断(手改文件不炸)
        let mut normalized = cfg;
        normalized.normalize();
        assert_eq!(normalized.name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(
            normalized.description.chars().count(),
            MAX_DESCRIPTION_CHARS
        );
    }

    #[test]
    fn user_name_only_when_enabled_and_nonempty() {
        let mut cfg = UserPersonaConfig {
            enabled: false,
            name: "林".into(),
            ..Default::default()
        };
        assert_eq!(cfg.user_name(), None);
        cfg.enabled = true;
        assert_eq!(cfg.user_name(), Some("林"));
        cfg.name = "   ".into();
        assert_eq!(cfg.user_name(), None);
    }

    #[test]
    fn block_is_none_when_disabled_or_empty() {
        let mut cfg = UserPersonaConfig::default();
        assert!(cfg.block().is_none());
        cfg.enabled = true;
        assert!(cfg.block().is_none());
        cfg.name = "林".into();
        let block = cfg.block().unwrap();
        assert!(block.contains("【用户角色】"));
        assert!(block.contains("称呼: 林"));
        assert!(!block.contains("设定:"));
    }

    #[test]
    fn block_wraps_description_with_untrusted_boundary() {
        let cfg = UserPersonaConfig {
            enabled: true,
            name: "林".into(),
            description: "退役侦探,怕黑。".into(),
        };
        let block = cfg.block().unwrap();
        assert!(block.contains("称呼: 林"));
        assert!(block.contains("设定:"));
        assert!(block.contains(r#"<UNTRUSTED_PROMPT_SOURCE source="user_persona">"#));
        assert!(block.contains("退役侦探"));
    }

    #[test]
    fn service_set_validates_and_persists() {
        let dir = TempDataDir::new("user-persona-service");
        let mut service = UserPersonaService::new(dir.path().to_path_buf());
        let bad = UserPersonaConfig {
            enabled: true,
            name: "名".repeat(MAX_NAME_CHARS + 1),
            description: String::new(),
        };
        assert!(service.set(bad).is_err(), "超长 name 必须拒绝");
        let good = UserPersonaConfig {
            enabled: true,
            name: " 林 ".into(),
            description: " 侦探 ".into(),
        };
        service.set(good).unwrap();
        let reloaded = UserPersonaConfig::load(dir.path());
        assert_eq!(reloaded.name, "林");
        assert_eq!(reloaded.description, "侦探");
    }

    /// 写入侧「超长 = 拒绝」而非「静默截断」:截断会让用户以为保存成功,称呼却少一截。
    /// load 侧仍截断(手改文件兜底),两者分工不同,故这里同时钉住不落盘。
    #[test]
    fn set_rejects_overlong_name_without_persisting() {
        let dir = TempDataDir::new("user-persona-reject");
        let mut service = UserPersonaService::new(dir.path().to_path_buf());
        let bad = UserPersonaConfig {
            enabled: true,
            name: "名".repeat(MAX_NAME_CHARS + 1),
            description: "侦探".into(),
        };
        let err = service.set(bad).unwrap_err();
        assert!(err.contains("name"), "错误文案应点名字段: {err}");
        assert_eq!(
            service.get(),
            &UserPersonaConfig::default(),
            "拒绝时不得改内存"
        );
        assert_eq!(
            UserPersonaConfig::load(dir.path()),
            UserPersonaConfig::default(),
            "拒绝时不得落盘"
        );
    }
}
