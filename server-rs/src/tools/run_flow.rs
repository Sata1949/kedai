// `run_flow` 内置工具(二维批次 7b 对比模式):把「本任务名单内的流程」作为**单一**
// 内置工具释放给自定义流程的宽松节点,模型在工具循环里自主调用、取回其成果。
//
// 为什么是单一工具而非「每流程一个工具定义」:`execute_call` 按名查全局工具注册表,
// 未注册直接拒绝(`agents/engine/executor.rs::execute_call`),而流程名是**用户运行期
// 数据**——运行期注册会失效 `definitions_cache` 并让并发任务互相踩缓存
//(`tools/registry.rs` 的失效点纪律)。故工具定义在启动时注册一次,「哪些流程可调用」
// 由**逐节点下发的描述**表达(`params.tools` 是逐次调用的定义列表,见
// `task_engine/custom.rs`),不碰全局注册表。
//
// 本模块**不认识**「任务 / 流程 / 快照」:它只按 `ToolContext.session_id` 查**调用点
// 注册表**,把参数交给注册方(custom 执行器)填进去的执行体。名单判定、预算、环与
// 深度守卫全在那一侧(见 `task_engine/flow_call.rs`)——tools 层保持领域无关,
// 也保证「查不到调用点」这种误用有一条明确的报错文案。
use crate::models::types::{ToolContext, ToolDefinition};
use crate::tools::registry::ToolRegistry;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 工具名(登记处引用同一常量,避免字符串各处手抄)
pub const TOOL_NAME: &str = "run_flow";

/// 调用点执行体:入参 `(流程名或 id, 传给该流程的输入文本)`,返回该流程的成果文本。
///
/// 装箱 + `'static` 是注册表形态的必然要求:调用点是**运行期**注册的每节点闭包,
/// 而工具处理器是启动期注册的 `Fn`,两者之间只能靠进程内注册表桥接。
pub type FlowInvoker =
    Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<String, String>> + Send + Sync>;

/// 进程内调用点注册表(key = `ToolContext.session_id`)。
///
/// key 用 session_id 是因为它是节点工具循环的**天然唯一标识**(形如
/// `task:{任务 id}:{phase}:{步序号}`,由 custom 执行器逐节点生成):并行节点、嵌套
/// 动态调用各自持有不同的 key,互不干扰;工具处理器手里也只有它,不需要额外协议。
fn call_sites() -> &'static Mutex<HashMap<String, FlowInvoker>> {
    static SITES: OnceLock<Mutex<HashMap<String, FlowInvoker>>> = OnceLock::new();
    SITES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 调用点注册守卫:**Drop 时注销**。
///
/// 用 RAII 而不是「循环后再删一行」:节点 future 可能被调度器丢弃(取消时 `drop(inflight)`),
/// 那种路径不会走到循环后的代码,漏注销会在进程内留下一个指向已结束任务的陈旧执行体
/// ——既泄漏内存,也让同一个 session_id 在重跑时命中旧条目。
pub struct CallSiteGuard {
    key: String,
}

impl Drop for CallSiteGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = call_sites().lock() {
            map.remove(&self.key);
        }
    }
}

/// 登记一个调用点(返回的守卫离开作用域即注销)。同 key 重复登记按覆盖处理——
/// 同一节点的工具循环只会登记一次,重跑时新条目覆盖旧的同 key 条目是正确行为。
pub fn register_call_site(key: &str, invoker: FlowInvoker) -> CallSiteGuard {
    if let Ok(mut map) = call_sites().lock() {
        map.insert(key.to_string(), invoker);
    }
    CallSiteGuard {
        key: key.to_string(),
    }
}

/// 按会话 id 取调用点(查不到 = 该会话不是「对比模式的流程节点」)
pub fn lookup_call_site(key: &str) -> Option<FlowInvoker> {
    call_sites()
        .lock()
        .ok()
        .and_then(|map| map.get(key).cloned())
}

/// 工具定义**单一出处**:注册(默认描述)与逐节点下发(描述按名单改写)共用同一份
/// 参数 schema,避免「注册的定义」与「下发的定义」在参数上漂移。
fn definition(description: &str) -> ToolDefinition {
    ToolDefinition {
        name: TOOL_NAME.into(),
        description: description.into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "flow": {
                    "type": "string",
                    "description": "要调用的流程名或流程 id(以上列列举为准)"
                },
                "input": {
                    "type": "string",
                    "description": "传给该流程的输入文本;省略 = 任务目标"
                }
            },
            "required": ["flow"]
        }),
    }
}

/// 逐节点下发的定义(描述由 custom 执行器按任务名单生成;params 与注册版一致)
pub fn definition_with_description(description: String) -> ToolDefinition {
    definition(&description)
}

/// 启动时注册(在 `tools::register_builtin_tools` 内调用)。
/// 默认描述面向**人类可读面**(工具权限面板、规划预览的工具清单)——模型面由
/// custom 执行器逐节点改写,那段文案才是模型真正看到的那份。
pub fn register_run_flow_tool(registry: &ToolRegistry) {
    registry.register(
        definition(
            "调用本任务流程名单内的一套预置流程,并取回其成果(仅自定义流程的对比模式节点可用)",
        ),
        Arc::new(|args: Value, ctx: ToolContext| {
            Box::pin(async move {
                let Some(invoker) = lookup_call_site(&ctx.session_id) else {
                    return Err(format!("{TOOL_NAME} 只能在自定义流程的对比模式节点内调用"));
                };
                let flow = args
                    .get("flow")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if flow.is_empty() {
                    return Err("缺少参数 flow:请给出要调用的流程名或 id".into());
                }
                let input = args
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                invoker(flow, input).await
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoker(tag: &'static str) -> FlowInvoker {
        Arc::new(move |flow: String, input: String| {
            Box::pin(async move { Ok(format!("{tag}|{flow}|{input}")) })
        })
    }

    #[tokio::test]
    async fn call_site_roundtrip_and_lookup_miss() {
        assert!(lookup_call_site("sess-none").is_none(), "未登记即查不到");
        let _guard = register_call_site("sess-1", invoker("A"));
        let f = lookup_call_site("sess-1").expect("登记后应查得到");
        assert_eq!(f("B".into(), "in".into()).await.unwrap(), "A|B|in");
        assert!(lookup_call_site("sess-2").is_none(), "别人的 key 互不干扰");
    }

    /// 守卫 Drop 即注销:节点 future 被丢弃(取消)时不留陈旧条目
    #[tokio::test]
    async fn guard_drop_unregisters() {
        {
            let _guard = register_call_site("sess-drop", invoker("A"));
            assert!(lookup_call_site("sess-drop").is_some());
        }
        assert!(
            lookup_call_site("sess-drop").is_none(),
            "守卫离开作用域后必须注销"
        );
    }

    /// 同名 key 重复登记 = 覆盖(重跑同一任务时的正确行为)
    #[tokio::test]
    async fn register_overwrites_same_key() {
        let _g1 = register_call_site("sess-dup", invoker("旧"));
        let _g2 = register_call_site("sess-dup", invoker("新"));
        let f = lookup_call_site("sess-dup").unwrap();
        assert_eq!(f("x".into(), String::new()).await.unwrap(), "新|x|");
    }

    /// 工具定义只此一份参数 schema:注册版与逐节点下发版必须逐字节一致(除描述)
    #[test]
    fn definition_parameters_are_shared() {
        let a = definition("d1");
        let b = definition_with_description("d2".into());
        assert_eq!(a.name, TOOL_NAME);
        assert_eq!(b.name, TOOL_NAME);
        assert_eq!(a.parameters, b.parameters, "参数 schema 不得两处各写一份");
        assert_eq!(a.description, "d1");
        assert_eq!(b.description, "d2");
    }
}
