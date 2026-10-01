// 任务模式工具策略:把 settings 里的策略档位编译为「下发工具定义 + 放行名单」。
//
// 背景:任务模式没有 UI 授权上下文,不能走「等待授权」(会空等超时)。因此任务侧
// 一律使用 ToolGate { no_ui_authorization: true } —— 名单内工具放行,名单外立即拒绝。
// 策略档位:
//   all            → 全量(剔除正文元工具),等价改造前的全放行行为
//   deny_dangerous → 全量 − 元工具 − 危险级 + 例外 bash/fs_write/fs_edit/fs_patch
//                    (默认;无人值守任务不默认放行写类操作,但保留命令执行与工作区写能力)
//   allowlist      → 仅配置白名单 ∩ 已注册(最严格)
// 三道正交闸门(先过闸门再套档位):platform_gate(平台/档位可用性)、
// workspace_gate(本任务是否绑定工作区)、coding_pack_gate(2026-10-01 Q6:包专属工具
// fs_patch 仅编码能力包开启时下发)。**2026-10-02 D4 起增第四道 vision_gate**:
// 视觉工具族(view_image/zoom_image/image_diff)仅生效连接开启「视觉输入」能力位时下发。
// 空集必须 fail-closed(docs/经验.md E56)。
//
// bash 例外说明:bash 在权限矩阵里恒为 Dangerous(见 tools/permissions.rs 的 default_risk),
// 若按风险级一刀切就会被默认策略整体剔除,任务模式连 `ls`/`echo` 都用不了。用户要求任务
// 模式具备命令执行能力,故这里按**工具名**开例外。注意「放行工具」≠「放行危险命令」:
// permissions::check 的命令级硬门在任何自动放行之前判定,破坏性/提权命令在任务模式下
// 仍被直接拒绝(无 UI 可确认),只有 safe/sensitive 命令经白名单授权放行。
//
// 工作区写工具例外说明(编码通道批次):`fs_write`/`fs_edit`/`fs_patch` 与 write/replace
// 同级归 Dangerous,同样按工具名开例外 —— 否则「绑定工作区」这件事在默认策略下只剩读能力,
// 模型改不了文件,本批的目标(结构化改代码,替代 sed/echo 拼命令)就落空。
// 例外的安全前提是**路径被 jail 在工作区内**(tools/workspace_guard.rs 逐段拒符号链接
// 与越界),而不是「工具不危险」;未绑定工作区的任务根本看不到这六个工具(见 workspace_gate)。
// fs_patch 还多一道编码包闸门:关包时它**先**在 coding_pack_gate 被剔除,例外只在开包时生效。
use crate::models::types::ToolDefinition;
// 工具风险词汇已下沉 L1(2026-09-14):L2 直连 models,不经 tools 转发。
use crate::models::tool_policy::ToolRisk;
use crate::tools::registry::ToolRegistry;
use crate::tools::tool_sets;

/// 编译结果。`allowed` 为放行名单(供 ToolGate 使用),需与 `defs` 同源:
/// 名单内 = 可被模型看到并执行;名单外工具即使被模型臆造调用也会被闸门拒绝。
pub(crate) struct TaskToolPolicy {
    pub defs: Vec<ToolDefinition>,
    pub allowed: Vec<String>,
}

impl TaskToolPolicy {
    /// 本策略对应的授权闸门:任务模式恒不等待授权(无 UI 上下文)。
    /// 注意:调用方若已取走 `defs`,应改用 `ToolGate::listed(&allowed)` 以免部分移动冲突。
    #[cfg(test)]
    pub(crate) fn gate(&self) -> crate::agents::engine::executor::ToolGate<'_> {
        crate::agents::engine::executor::ToolGate {
            whitelist: Some(&self.allowed),
            no_ui_authorization: true,
        }
    }
}

/// 按策略档位编译工具集。
/// `policy`:all / deny_dangerous / allowlist(非法值按 deny_dangerous 处理,与加载侧钳制一致)。
/// `allowlist`:仅 allowlist 档位使用。
/// `has_workspace`:本任务是否**有执行作用域**(绑定的工作区,或 D1 起的任务 scratch)——
/// 决定 fs_* 族是否下发,见 [`workspace_gate`]。
/// `coding_pack_enabled`:编码能力包开关(**task 合并值**,与提示词缺省值同源口径)——
/// 决定包专属工具(`fs_patch`)是否下发,见 [`coding_pack_gate`]。
/// `vision_enabled`:生效连接的「视觉输入」能力位(取值口径单一出处
/// `settings_service::vision_enabled`;默认连接口径)——决定视觉三件是否下发,见 [`vision_gate`]。
pub(crate) fn compile(
    policy: &str,
    allowlist: &[String],
    registry: &ToolRegistry,
    has_workspace: bool,
    coding_pack_enabled: bool,
    vision_enabled: bool,
) -> TaskToolPolicy {
    let all = coding_pack_gate(
        vision_gate(
            workspace_gate(
                platform_gate(tool_sets::exclude_meta(registry.list_definitions())),
                has_workspace,
            ),
            vision_enabled,
        ),
        coding_pack_enabled,
    );
    let selected: Vec<ToolDefinition> = match policy {
        "all" => all,
        "allowlist" => tool_sets::filter_by_names(all, allowlist),
        // 默认(含非法值回退):拒绝危险级;bash 与工作区写/改/补丁工具按工具名开例外
        //(见文件头「bash 例外说明」与「工作区写工具例外说明」),危险动作仍由
        // permissions 的命令级硬门/路径闸门拦截,不因本例外放行。
        _ => {
            let risk = registry.permissions();
            all.into_iter()
                .filter(|d| {
                    d.name == crate::tools::bash::TOOL_NAME
                        || d.name == crate::tools::agent_tools_fs::WRITE_TOOL
                        || d.name == crate::tools::agent_tools_fs::EDIT_TOOL
                        || d.name == crate::tools::agent_tools_fs::PATCH_TOOL
                        || risk.risk_for(&d.name) != ToolRisk::Dangerous
                })
                .collect()
        }
    };
    let allowed = selected.iter().map(|d| d.name.clone()).collect();
    TaskToolPolicy {
        defs: selected,
        allowed,
    }
}

/// 平台/档位可见性闸门(2026-09-17)。
///
/// 与风险级过滤正交:本函数按**运行环境**决定某些工具该不该出现在模型面前。
/// 当前唯一受限的是 `submit`(产物提交):
///   - 它只在 Android 且执行档位为**沙箱**(非 ROOT / 非 Shizuku)时有意义——
///     那两档下 `bash` 本就能写任意路径,再给一个 submit 只会让模型在两条等价通道间摇摆;
///   - 非 Android 平台它必然失败(桌面有保存对话框与 write 工具),下发纯属噪声。
///
/// 为什么在策略层而不是注册层过滤:工具始终注册,权限面板与契约类型才有一致的
/// 工具面;过滤只影响「本轮下发给模型什么」。档位在**每轮编译时现探**(带进程内
/// 缓存),故用户在任务执行途中改授权档位,下一轮的工具清单即随之变化。
fn platform_gate(defs: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    if crate::services::artifact_submit::is_available() {
        return defs;
    }
    defs.into_iter()
        .filter(|d| d.name != crate::tools::submit::TOOL_NAME)
        .collect()
}

/// 工作区绑定闸门(编码通道批次;任务模式 D1 起语义不变、调用方取值口径变了)。
///
/// 与平台闸门正交:本函数按「本任务是否有执行作用域」决定工作区文件工具族该不该出现在
/// 模型面前。**无作用域即整族剔除**——族里三个是安全级,只靠风险级过滤会漏进模型视野,
/// 而它们没有作用域时必然报「未绑定工作区」,只会浪费轮次并诱导模型反复重试。
/// 有作用域时整族保留;`fs_write`/`fs_edit` 在 deny_dangerous 档的放行由 `compile` 的按名
/// 例外承担(剔除顺序:先闸门后策略,故例外只会作用于已绑定的任务)。
///
/// **调用方取值口径(D1)**:任务自 D1 起恒有作用域——绑定了工作区的用工作区,未绑定的由
/// `task_engine::run_inner` 建任务级 scratch 并绑定,所以本闸门对任务是「恒开」的;
/// `has_workspace` 仍按 `ctx.scope.is_some()` 求值而不写死 true,是为守住
/// 「没有作用域就不给文件工具」这条不变量(聊天路径无 scope,将来若有任务路径拿不到
/// 作用域,也不会下发必然报错的工具)。「未绑定工作区」与「无作用域」自 D1 起不再等价。
///
/// 为什么在策略层而不是注册层过滤:与 platform_gate 同理——工具始终注册,权限面板与
/// 契约类型才有一致的工具面;过滤只影响「本轮下发给模型什么」。作用域在**每轮编译时**
/// 由调用方按任务记录求值(任务运行途中绑定不会变,口径仍是当轮为准)。
fn workspace_gate(defs: Vec<ToolDefinition>, has_workspace: bool) -> Vec<ToolDefinition> {
    if has_workspace {
        return defs;
    }
    // 剔除口径与聊天路径同源(单一出处),避免两处各写一份名单
    tool_sets::exclude_workspace(defs)
}

/// 编码能力包闸门(2026-10-01,Q6「首个包专属工具」)。
///
/// 与平台/工作区闸门正交:本函数按**编码能力包开关**(`task_coding_bundle_enabled` 的
/// task 合并值)决定包专属工具(`fs_patch`)该不该出现在模型面前。关包(默认)整族剔除
/// ——「包开关 = 包专属工具的总闸」;开包才进后续档位过滤(默认档下再由按名例外放行)。
/// 其余工具不受影响:包是**只加不锁**的——关包不改变任何既有工具面。
///
/// 为什么在策略层而不是注册层过滤:与 platform_gate 同理——工具始终注册,权限面板与
/// 契约类型才有一致的工具面;过滤只影响「本轮下发给模型什么」。开关在**每轮编译时**
/// 由调用方按任务有效设置求值(执行途中开/关包,下一轮的工具清单即随之变化)。
fn coding_pack_gate(defs: Vec<ToolDefinition>, coding_pack_enabled: bool) -> Vec<ToolDefinition> {
    if coding_pack_enabled {
        return defs;
    }
    defs.into_iter()
        .filter(|d| d.name != crate::tools::agent_tools_fs::PATCH_TOOL)
        .collect()
}

/// 视觉能力闸门(视觉能力包 D4,2026-10-02)。
///
/// 与其余闸门正交:本函数按**生效连接的「视觉输入」能力位**决定视觉工具族
/// (`view_image`/`zoom_image`/`image_diff`)该不该出现在模型面前。关位整族剔除——
/// 能力位没开时,模型即使调用,图像也无法随请求下发(连接器序列化会丢弃),只会
/// 浪费轮次;执行侧同样有兜底(解析不出 data URL 时引用被跳过)。
///
/// 取值口径:`settings_service::vision_enabled`(默认连接的 supports_vision,与聊天
/// 贴图闸门同源)。**已知简化**:任务的工具清单按运行编译一次,不跨节点重编,
/// 故节点级显式连接的视觉能力差异不细判(按默认连接口径;在 docs/遗留.md 登记)。
///
/// 为什么在策略层而不是注册层过滤:与 platform_gate 同理——工具始终注册,权限面板与
/// 契约类型才有一致的工具面;过滤只影响「本轮下发给模型什么」。能力位在**每轮编译时**
/// 由调用方按当前设置求值(勾选/取消能力位,下一轮的工具清单即随之变化)。
fn vision_gate(defs: Vec<ToolDefinition>, vision_enabled: bool) -> Vec<ToolDefinition> {
    if vision_enabled {
        return defs;
    }
    defs.into_iter()
        .filter(|d| !tool_sets::VISION_TOOLS.contains(&d.name.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::ToolRegistry;
    use serde_json::json;

    /// 注册若干代表性工具(含元工具与各风险级),用于策略断言
    fn registry_with_tools() -> ToolRegistry {
        let reg = ToolRegistry::new();
        let noop: crate::tools::registry::ToolExecutor =
            std::sync::Arc::new(|_a, _c| Box::pin(async { Ok("{}".to_string()) }));
        for name in [
            "read",
            "search",
            "todo",
            "agentgo",
            "agentend",
            "write",
            "replace",
            "create",
            "memory_write",
            "update_variables",
            "get_state",
            "apply_patch",
            "bash",
            // 名字含 "bash" 的其它危险工具:锁死「按工具名精确匹配」——
            // 若实现改成前缀/包含匹配,这两个会被误放行,泄漏测试即失败。
            "bash2",
            "mcp_x_bash",
            // 产物提交:敏感级,但受平台/档位闸门限制(见 platform_gate)
            "submit",
            // 工作区文件工具族(编码通道批次):可见性由 workspace_gate 决定;
            // fs_write2 是「名字含 fs_write」的对照项,锁死例外按精确名匹配
            "fs_read",
            "fs_write",
            "fs_edit",
            "fs_glob",
            "fs_grep",
            "fs_write2",
            // 包专属工具(Q6):fs_patch2 是对照项,锁死包闸门与例外一律按精确名匹配
            "fs_patch",
            "fs_patch2",
            // 视觉工具族(D4):view_image2 是「名字含 view_image」的对照项,
            // 锁死视觉闸门按精确名匹配
            "view_image",
            "zoom_image",
            "image_diff",
            "view_image2",
        ] {
            reg.register(
                ToolDefinition {
                    name: name.into(),
                    description: name.into(),
                    parameters: json!({ "type": "object" }),
                },
                noop.clone(),
            );
        }
        reg
    }

    #[test]
    fn deny_dangerous_excludes_dangerous_and_meta() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg, false, false, false);
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        for banned in [
            "write",
            "replace",
            "create",
            "memory_write",
            "update_variables",
        ] {
            assert!(!names.contains(&banned), "危险工具 {banned} 应被拒绝");
        }
        for meta in ["get_state", "apply_patch"] {
            assert!(!names.contains(&meta), "元工具 {meta} 不应下发");
        }
        assert!(names.contains(&"read"));
        assert!(names.contains(&"agentend"), "agentend 属敏感级,应保留");
        // bash 恒为危险级但按工具名开例外:任务模式需具备命令执行能力
        // (危险命令由 permissions 的命令级硬门拦截,与工具是否下发无关)
        assert!(
            names.contains(&"bash"),
            "bash 应在 deny_dangerous 下下发(任务模式命令执行能力)"
        );
    }

    /// bash 例外只针对 bash 本身:其它危险工具在 deny_dangerous 下仍不得下发
    #[test]
    fn deny_dangerous_bash_exception_does_not_leak_to_other_dangerous() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg, false, false, false);
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        // 精确匹配护栏:名字包含 "bash" 的其它危险工具不得被例外带出
        // (若实现改为前缀/包含匹配,下面两条断言即失败)
        for lookalike in ["bash2", "mcp_x_bash"] {
            assert!(
                !names.contains(&lookalike),
                "{lookalike} 名字含 bash 但非 bash 本体,deny_dangerous 下不得下发"
            );
        }
        let dangerous: Vec<&str> = p
            .defs
            .iter()
            .map(|d| d.name.as_str())
            .filter(|n| *n != "bash")
            .filter(|n| reg.permissions().risk_for(n) == ToolRisk::Dangerous)
            .collect();
        assert!(
            dangerous.is_empty(),
            "除 bash 外不得放行危险工具,实际:{}",
            dangerous.join(",")
        );
    }

    #[test]
    fn allowlist_only_keeps_configured_and_registered() {
        let reg = registry_with_tools();
        let p = compile(
            "allowlist",
            &["read".to_string(), "not_registered".to_string()],
            &reg,
            false,
            false,
            false,
        );
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read"]);
    }

    #[test]
    fn all_keeps_everything_except_meta() {
        let reg = registry_with_tools();
        let p = compile("all", &[], &reg, false, false, false);
        let names: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"write"));
        assert!(!names.contains(&"get_state"));
    }

    #[test]
    fn allowed_names_match_defs() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg, false, false, false);
        let defs: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        let allowed: Vec<&str> = p.allowed.iter().map(|s| s.as_str()).collect();
        assert_eq!(defs, allowed);
    }

    /// 平台/档位闸门(2026-09-17):submit 只在 Android 沙箱档可见。
    ///
    /// 单测跑在桌面,故此处断言的是「不可用环境下必须剔除」这一侧;Android 侧的
    /// 反向断言(沙箱档下发、ROOT/Shizuku 档不下发)由 services::artifact_submit
    /// 的 is_available 语义 + Kotlin 档位探测共同保证,规则 L 守桥接方法登记。
    /// 关键点:**defs 与 allowed 必须同时剔除**——只剔 defs 会让模型臆造调用时报
    /// 「未注册」而非「不可用」,只剔 allowed 则模型看得见却调不动。
    #[test]
    fn submit_filtered_out_when_unavailable() {
        let reg = registry_with_tools();
        let p = compile("all", &[], &reg, false, false, false);
        let defs: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            !defs.contains(&"submit"),
            "不可用环境下 submit 不得下发: {defs:?}"
        );
        assert!(
            !p.allowed.iter().any(|n| n == "submit"),
            "allowed 与 defs 必须同源剔除: {:?}",
            p.allowed
        );
        // 其它工具不受影响(闸门只作用于 submit)
        assert!(defs.contains(&"read"), "闸门不得误伤其它工具: {defs:?}");
        assert!(defs.contains(&"bash"), "闸门不得误伤 bash: {defs:?}");
    }

    #[test]
    fn gate_never_waits_for_authorization() {
        let reg = registry_with_tools();
        let p = compile("deny_dangerous", &[], &reg, false, false, false);
        assert!(p.gate().no_ui_authorization, "任务模式闸门不得等待授权");
    }

    /// 工作区闸门(编码通道批次):未绑定工作区时,工作区工具族**一个都不下发**——
    /// 三档策略各验一次。族里 `fs_read`/`fs_glob`/`fs_grep` 是安全级,只靠风险级过滤
    /// 会漏进模型视野,而它们没有工作区时必然报错(浪费轮次并诱导反复重试)。
    #[test]
    fn workspace_tools_hidden_without_binding_across_all_policies() {
        let reg = registry_with_tools();
        let all_names: Vec<String> = ["fs_read", "fs_write", "fs_edit", "fs_glob", "fs_grep"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for (policy, names) in [
            ("all", Vec::new()),
            ("deny_dangerous", Vec::new()),
            ("allowlist", all_names),
        ] {
            // 包开(true):关包的剔除路径由 coding_pack_gate_hides_fs_patch_only_when_disabled
            // 单独覆盖;本用例盯的是工作区闸门本身。视觉闸门关闭(false)不干扰本断言
            // (无工作区时整族先被剔除)。
            let p = compile(policy, &names, &reg, false, true, false);
            let got: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
            for banned in crate::tools::tool_sets::WORKSPACE_TOOLS {
                assert!(
                    !got.contains(banned),
                    "{policy} 档未绑定工作区时不得下发 {banned}:{got:?}"
                );
            }
        }
    }

    /// 绑定了工作区时:整族下发(包开,fs_patch 在列),`fs_write`/`fs_edit`/`fs_patch`
    /// 在 deny_dangerous 档按名开例外,且 `allowed` 与 `defs` 同源(否则模型看得见却调不动)。
    #[test]
    fn workspace_tools_visible_and_writes_excepted_when_bound() {
        let reg = registry_with_tools();
        // 视觉能力位开(true):本用例断言「整族下发」,视觉三件现属工作区族
        let p = compile("deny_dangerous", &[], &reg, true, true, true);
        let defs: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        for name in crate::tools::tool_sets::WORKSPACE_TOOLS {
            assert!(defs.contains(name), "绑定工作区后应下发 {name}");
        }
        // 例外只按精确名:对照项 fs_write2 / fs_patch2 仍属危险级且不在族内 → 必须被剔除
        assert!(
            !defs.contains(&"fs_write2"),
            "例外不得按前缀/包含匹配放行 fs_write2:{defs:?}"
        );
        assert!(
            !defs.contains(&"fs_patch2"),
            "例外/包闸门不得按前缀/包含匹配放行 fs_patch2:{defs:?}"
        );
        let allowed: Vec<&str> = p.allowed.iter().map(|s| s.as_str()).collect();
        assert_eq!(defs, allowed, "allowed 与 defs 必须同源");
    }

    /// 编码能力包闸门(2026-10-01,Q6「首个包专属工具」):
    /// `fs_patch` 仅当包开启时下发——关包时三档策略全不可见、`allowed` 同源剔除;
    /// 开包后 deny_dangerous 由按名例外放行;其余工作区族成员不受包开关影响(包「只加不锁」)。
    #[test]
    fn coding_pack_gate_hides_fs_patch_only_when_disabled() {
        let reg = registry_with_tools();
        for (policy, names) in [
            ("all", Vec::new()),
            ("deny_dangerous", Vec::new()),
            (
                "allowlist",
                vec!["fs_patch".to_string(), "fs_read".to_string()],
            ),
        ] {
            let p = compile(policy, &names, &reg, true, false, false);
            let got: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
            assert!(
                !got.contains(&"fs_patch"),
                "{policy} 档关包时不得下发 fs_patch:{got:?}"
            );
            assert!(
                !p.allowed.iter().any(|n| n == "fs_patch"),
                "{policy} 档 allowed 与 defs 必须同源剔除"
            );
            assert!(
                got.contains(&"fs_read"),
                "{policy} 档关包不得误伤工作区族其余成员:{got:?}"
            );
        }
        // 开包:deny_dangerous 下由按名例外放行;allowed 同源(视觉位开:过视觉闸门)
        let p = compile("deny_dangerous", &[], &reg, true, true, true);
        let got: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            got.contains(&"fs_patch"),
            "开包 + deny_dangerous 应下发 fs_patch:{got:?}"
        );
        assert!(
            p.allowed.iter().any(|n| n == "fs_patch"),
            "defs 与 allowed 必须同源"
        );
    }

    /// 视觉闸门(2026-10-02,D4):生效连接未开「视觉输入」→ 视觉三件整族剔除
    /// (defs 与 allowed 同源),工作区族其余成员与对照项不受影响;开位后正常下发。
    #[test]
    fn vision_gate_hides_vision_tools_without_capability() {
        let reg = registry_with_tools();
        // 绑定工作区 + 包开,仅视觉位关:三件必须不可见,其余照常
        let p = compile("deny_dangerous", &[], &reg, true, true, false);
        let got: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        for banned in crate::tools::tool_sets::VISION_TOOLS {
            assert!(
                !got.contains(banned),
                "未开视觉位时不得下发 {banned}:{got:?}"
            );
            assert!(
                !p.allowed.iter().any(|n| n == banned),
                "allowed 与 defs 必须同源剔除:{banned}"
            );
        }
        assert!(got.contains(&"fs_read"), "闸门不得误伤工作区族:{got:?}");
        // 对照项 view_image2:用 all 档验证(deny_dangerous 会按风险级剔除未登记的
        // 危险名,那是风险过滤的另一条路径,与本闸门无关)——精确名匹配下它必须仍在
        let p_all = compile("all", &[], &reg, true, true, false);
        let got_all: Vec<&str> = p_all.defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            got_all.contains(&"view_image2"),
            "闸门只按精确名匹配,不得误伤对照项:{got_all:?}"
        );
        // 开位:三件下发
        let p = compile("deny_dangerous", &[], &reg, true, true, true);
        let got: Vec<&str> = p.defs.iter().map(|d| d.name.as_str()).collect();
        for name in crate::tools::tool_sets::VISION_TOOLS {
            assert!(got.contains(name), "开位后应下发 {name}:{got:?}");
        }
    }
}
