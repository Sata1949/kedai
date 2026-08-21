// 角色卡 / 导入导出 / 插件 / 技能 / 资源代理 / 任务 / 音频 / 脚本 / slash / 宏路由
// (自 api/mod.rs build_router 迁入)
use crate::api::app_state::AppState;
use crate::api::{
    audio, characters, import_export, macros, plugins, resource, skills, slash_commands, tasks,
    user_scripts,
};
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

pub(crate) fn misc_routes() -> Router<Arc<AppState>> {
    Router::new()
        // 角色卡
        .route("/api/characters", get(characters::list))
        .route("/api/characters/upload", post(characters::upload))
        .route(
            "/api/characters/{id}",
            get(characters::get)
                .put(characters::update)
                .delete(characters::delete),
        )
        // 导入导出
        .route("/api/export/chat", get(import_export::export_chat))
        .route("/api/import/chat", post(import_export::import_chat))
        // 插件(自定义工具)
        .route("/api/plugins/tools", get(plugins::list_tools))
        .route("/api/plugins/tools/reload", post(plugins::reload_tools))
        .route("/api/plugins/tools/upload", post(plugins::upload_tool))
        .route(
            "/api/plugins/tools/{name}",
            axum::routing::delete(plugins::delete_tool),
        )
        // 技能库(提示词技能)
        .route("/api/skills", get(skills::list).post(skills::import_skills))
        .route(
            "/api/skills/{id}",
            put(skills::update).delete(skills::delete),
        )
        // 角色卡远程资源界面代理(https + SSRF 防护)
        .route("/api/resource/proxy", get(resource::proxy))
        // 任务模式(task 工作台):列表/新建/详情/执行/停止/删除
        .route(
            "/api/tasks",
            get(tasks::list).post(tasks::create),
        )
        .route(
            "/api/tasks/{id}",
            get(tasks::get).delete(tasks::delete),
        )
        .route("/api/tasks/{id}/run", post(tasks::run))
        .route("/api/tasks/{id}/stop", post(tasks::stop))
        // 音频播放器(bgm/ambient 双通道):读取 / 设置 / 播放列表
        .route("/api/audio", get(audio::get))
        .route("/api/audio/settings", put(audio::update_settings))
        .route("/api/audio/playlist", put(audio::update_playlist))
        // 用户脚本(ScriptTree,阶段三):global / character 两级脚本树全量读写
        .route(
            "/api/scripts/tree",
            get(user_scripts::get_tree).put(user_scripts::save_tree),
        )
        // slash 命令清单(阶段四 4a):前端输入框联想
        .route("/api/slash/commands", get(slash_commands::list_commands))
        // 宏调试(阶段六 6b):纯扁平 vars 展开,供前端「宏调试」面板验证模板结果
        .route("/api/macros/expand", post(macros::expand))
}
