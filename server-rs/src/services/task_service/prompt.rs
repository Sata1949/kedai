// 提示词组装:任务模式有效设置读取(task_settings)、世界书常驻段落(world_context)、
// Agent 系统提示词占位符渲染(render_agent_prompt)。
// 注:提示词注入文本(inject_text)与执行者人设参考(persona_style)已随 TM-SET-2
// 退役——任务 system 恒不注入 prompt_floors.json,执行者身份段只认执行者指令。
// 自 task_service.rs 拆分迁入,纯代码移动,逻辑不变;依赖经 `use super::*` 取自 mod.rs。
use super::*;

// ===== 三层固定提示词(单一来源)=====
// 规划器/执行者/汇总者内置指令:执行器(executor.rs)与设置预览(api/settings.rs)
// 共用同一份文本,改动只改这里,预览即真实下发。内置指令不经 untrusted 包裹
//(docs/契约-协议与配置.md 第三节)。
// 批次 B.3 依赖倒置:常量本体已机械搬迁至 task_core::prompt_consts
//(使 task_engine 侧可直接引用而不经 task_service);此处按名再导出宿主侧
// 既有使用者所需的三层固定提示词(executor.rs / api/settings.rs / 本文件),
// 调用方零改动。team/custom 专属常量由 task_engine 侧直接从 task_core 引用。
pub(crate) use crate::services::task_core::prompt_consts::{
    EXECUTOR_PROMPT, EXECUTOR_TOOL_DISCIPLINE, PLANNER_PROMPT, PLANNER_REVISE_GUIDANCE,
    SUMMARIZER_PROMPT, VISION_VERIFY_DISCIPLINE,
};

impl TaskService {
    /// 读取任务模式合并后的有效设置(生成参数覆盖项已应用,连接信息共享)。
    /// pub(crate):任务引擎(task_engine)构造 TaskRunContext 设置快照用。
    pub(crate) fn task_settings(&self) -> RuntimeSettings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .for_mode(AppMode::Task)
    }

    /// 执行者角色卡读取(solo 提示词组装用;无执行者/角色不存在返回 None)。
    /// **兼容路径**:仅供旧任务(带 character_id 且无 executor_id)回退使用。
    /// 新任务一律走 [`Self::executor_for`] 的独立执行者库。
    pub(crate) fn character_for(&self, character_id: Option<&str>) -> Option<CharacterRecord> {
        character_id.and_then(|cid| self.characters.get(cid))
    }

    /// 独立执行者读取(执行者库命中返回配置;未指定/已删除返回 None)。
    /// None 的调用方语义 = 通用执行者(不注入任何身份段)。
    pub(crate) fn executor_for(
        &self,
        executor_id: Option<&str>,
    ) -> Option<crate::services::executor_service::TaskExecutorConfig> {
        let id = executor_id.map(str::trim).filter(|s| !s.is_empty())?;
        self.executors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// 执行者「建议温度」(TM-GEN-1):执行者库命中且配置了温度时返回该值,否则 None。
    /// 调用方语义 = `执行者温度 ?? 任务有效缺省温度`(与前端占位「留空 = 沿用任务模式
    /// 默认温度」一致);单点解析式对线程内外同源(`solo::run_agent_loop` 与
    /// `generate_step` 两处调用点,勿在任一侧复制判定)。
    pub(crate) fn executor_temperature(&self, executor_id: Option<&str>) -> Option<f64> {
        self.executor_for(executor_id).and_then(|e| e.temperature)
    }

    /// 执行者 system 提示词组装(单一实现):内置执行者指令 → 执行者身份段 → 世界书 →
    /// 提示词注入 → 用户可编辑 Agent 提示词,外部来源段落逐一 untrusted 边界包裹,
    /// 内置指令不包裹(WP7 纪律)。
    ///
    /// 身份段的两种来源(优先级见下,2026-09-17 执行者库批次):
    ///   ① `executor_id` 命中执行者库 → 注入该执行者的指令段为「执行者职责」,
    ///      **不再**注入任何角色卡人设(执行者与角色扮演资产彻底解耦);
    ///   ② 否则 `character_id` 命中角色卡 → 旧行为逐字节不变(旧任务重跑零回归);
    ///   ③ 都无 → 通用执行者(不注入身份段)。
    ///
    /// 世界书随身份来源取:走执行者库时只取全局世界书(执行者与角色卡无关联关系),
    /// 走角色卡时按该角色过滤(旧语义)。
    ///
    /// 调用方:legacy 步骤生成(task_service::generate_step_with)与任务引擎主 agent
    /// 循环(task_engine::solo::run_agent_loop)——两处此前各有一份逐行同构实现,
    /// 差异仅在角色取用方式(characters.get vs character_for,本实现统一经
    /// character_for,两者等价),合并后消除漂移风险。
    /// `user_goal` 供 {{lastUserMessage}} 占位符渲染(步骤生成传任务目标,主 agent 传 goal)。
    /// `has_tools`(提交 3 · D3-c):本轮是否真的下发了工具——true 时在内置执行者指令后
    /// 追加工具使用纪律段(单一出处 `EXECUTOR_TOOL_DISCIPLINE`);legacy(无工具)传 false。
    /// `has_vision_tools`(视觉能力包 D4):本轮工具面是否含视觉三件——true 时再追加
    /// 视觉验证纪律段(单一出处 `VISION_VERIFY_DISCIPLINE`);无视觉工具时该段不出现。
    pub(crate) fn assemble_executor_system_prompt(
        &self,
        settings: &RuntimeSettings,
        executor_id: Option<&str>,
        character_id: Option<&str>,
        user_goal: &str,
        has_tools: bool,
        has_vision_tools: bool,
    ) -> String {
        let executor = self.executor_for(executor_id);
        // 执行者库命中即独占身份段:角色卡不参与(含世界书过滤口径)
        let character = if executor.is_some() {
            None
        } else {
            self.character_for(character_id)
        };

        let mut sys = String::from(EXECUTOR_PROMPT);
        // 工具纪律段紧随内置指令(属内置块,不经 untrusted 包裹);仅在真有工具时追加
        if has_tools {
            sys.push_str(&format!(
                "\n\n{}",
                crate::services::task_core::prompt_consts::EXECUTOR_TOOL_DISCIPLINE
            ));
        }
        // 视觉验证纪律段(D4):仅当工具面含视觉三件时追加(条件段,与工具纪律同族)
        if has_vision_tools {
            sys.push_str(&format!(
                "\n\n{}",
                crate::services::task_core::prompt_consts::VISION_VERIFY_DISCIPLINE
            ));
        }
        if let Some(e) = &executor {
            let instruction = e.instruction.trim();
            if !instruction.is_empty() {
                sys.push_str(&format!(
                    "\n\n执行者职责(执行者「{}」):\n{}",
                    e.name,
                    crate::services::prompt_kit::untrusted_boundary("task_executor", instruction)
                ));
            }
        }
        // 角色卡人设段已随「执行者人设档位」一并退役(TM-SET-2):执行者身份段只认执行者
        // 指令;旧任务(仅读侧可达,含 character_id)不再注入「写作风格参考」。character
        // 仍参与下方世界书过滤口径与占位符渲染({{char}} 等),那两项维持现状。
        // 世界书过滤口径与身份来源一致:执行者库路径只取全局(传 None,
        // 执行者与角色卡无关联关系),角色卡路径按该角色过滤(旧语义:原样传 character_id,
        // 即使该角色已被删除也与改造前一致);无身份时也是全局
        let world_char_id = if executor.is_some() {
            None
        } else {
            character_id
        };
        let world = self.world_context(world_char_id);
        if !world.is_empty() {
            sys.push_str(&format!(
                "\n\n{}",
                crate::services::prompt_kit::untrusted_boundary("world_book", &world)
            ));
        }
        // 提示词注入**恒隔离**(TM-SET-2):原 task_prompt_inject_enabled 开关退役后固定为
        // 不继承 prompt_floors.json(2026-09-10 F2 实跑修复的默认侧)。solo/multi/team 主
        // agent 与 followup 续跑共用本函数,行为不再有恢复通道。
        // settings 为 for_mode(Task) 合并值;agent_system_prompt 字段类型 RoleplayPromptConfig
        // (RuntimeSettings 共用成员),此处内容已是 task 有效值(覆盖层计算结果),.0 取字符串
        if !settings.agent_system_prompt.0.trim().is_empty() {
            let rendered = self.render_agent_prompt(
                &settings.agent_system_prompt.0,
                character.as_ref(),
                &world,
                user_goal,
            );
            sys.push_str(&format!(
                "\n\n{}",
                crate::services::prompt_kit::untrusted_boundary("agent_prompt", &rendered)
            ));
        }
        sys
    }

    /// 世界书常驻条目文本:过滤 enabled && constant 且内容非空,按 position→order→id 排序,
    /// 拼成「世界书设定」段落。无执行者时传空 id 仅取全局世界书。
    /// 过滤/排序/格式化逻辑下沉 prompt_kit::constant_world_text(WP7),与引擎侧共用口径。
    /// pub(crate):任务引擎 solo 提示词组装复用(勿复制实现)。
    pub(crate) fn world_context(&self, character_id: Option<&str>) -> String {
        let cid = character_id.unwrap_or("");
        let entries = self.world_books.collect_entries_for_character(cid);
        crate::services::prompt_kit::constant_world_text(&entries)
    }

    /// 渲染任务模式 Agent 系统提示词占位符(与角色扮演同款占位符语义):
    /// {{char}}/{{character_name}}/{{character_description}}/{{personality}}/{{scenario}}/
    /// {{world_info}}/{{user}}/{{lastUserMessage}}(任务模式取任务目标)。
    /// 无执行者时角色类占位符一律置空,避免宏原文泄漏进 LLM 上下文。
    /// 替换链下沉 prompt_kit::render_character_placeholders(WP7),模式专属对经 extra 传入。
    /// pub(crate):任务引擎 solo 提示词组装复用。
    pub(crate) fn render_agent_prompt(
        &self,
        prompt: &str,
        character: Option<&CharacterRecord>,
        world_text: &str,
        user_goal: &str,
    ) -> String {
        crate::services::prompt_kit::render_character_placeholders(
            prompt,
            character,
            world_text,
            &[
                ("{{user}}".to_string(), "用户".to_string()),
                ("{{lastUserMessage}}".to_string(), user_goal.to_string()),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 提示词契约加固(审计项 G):mock 钩子 [[empty_if:任务执行者]] /
    /// [[reply_if:任务规划器]] 依赖的独特子串不得丢失;新增契约纪律须就位。
    /// 本用例是硬约束的回归锁:改文案时若删掉这两个子串,集成测试会连锁失败。
    #[test]
    fn builtin_prompts_keep_mock_hook_substrings_and_contract_disciplines() {
        assert!(
            EXECUTOR_PROMPT.contains("任务执行者"),
            "EXECUTOR_PROMPT 必须含「任务执行者」(mock [[empty_if:]] 钩子依赖)"
        );
        assert!(
            PLANNER_PROMPT.contains("任务规划器"),
            "PLANNER_PROMPT 必须含「任务规划器」(mock [[reply_if:]] 钩子依赖)"
        );
        // 规划器:契约先行(交付物 + 可判是验收判据 + 权威版本唯一 + 字段名唯一来源)
        for needle in ["交付物", "验收判据", "权威版本", "唯一来源"] {
            assert!(
                PLANNER_PROMPT.contains(needle),
                "PLANNER_PROMPT 应含契约先行要素「{needle}」: {PLANNER_PROMPT}"
            );
        }
        // 执行者:取代对象声明 + 审计只回填结论
        for needle in ["取代对象", "验证/审计"] {
            assert!(
                EXECUTOR_PROMPT.contains(needle),
                "EXECUTOR_PROMPT 应含纪律「{needle}」: {EXECUTOR_PROMPT}"
            );
        }
        // END 只属于子任务指令(agentgo description),不得进 EXECUTOR_PROMPT 污染用户可见正文
        assert!(
            !EXECUTOR_PROMPT.contains("END"),
            "EXECUTOR_PROMPT 不得含 END 收尾(属子任务指令模板): {EXECUTOR_PROMPT}"
        );
    }
}
