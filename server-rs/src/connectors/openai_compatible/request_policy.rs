//! 按连接选定的方言组装请求；执行轮控制不能改变端点、权限或生成预算。
use super::{
    build_anthropic_body, build_chat_body, build_responses_body, provider_host, ApiStyle,
    OpenAiCompatibleConnector,
};
use crate::models::types::{GenerationParams, LlmMessage};
use serde_json::{json, Value};

impl OpenAiCompatibleConnector {
    /// 各方言沿用同一组能力位；仅支持图像且有实际图像时把内容组装成图文 parts。
    pub(super) fn build_request_body(
        &self,
        messages: &[LlmMessage],
        params: &GenerationParams,
    ) -> Value {
        match self.style {
            ApiStyle::ChatCompletions => {
                let mut body = build_chat_body(&self.model, messages, params, self.capabilities);
                if let Some(effort) = self.reasoning_effort(params) {
                    body["reasoning_effort"] = json!(effort);
                }
                body
            }
            ApiStyle::Responses => {
                let mut body =
                    build_responses_body(&self.model, messages, params, self.capabilities);
                if let Some(effort) = self.reasoning_effort(params) {
                    body["reasoning"] = json!({"effort": effort});
                }
                body
            }
            ApiStyle::Anthropic => {
                build_anthropic_body(&self.model, messages, params, self.capabilities)
            }
        }
    }

    /// 推理控制依赖实际路由和方言，而非模型名。Command Code 的 Responses 已验证
    /// 能用 `none` 发出无推理的工具调用；Chat 方言不能用同样的值，不替用户切换接口。
    pub(super) fn reasoning_effort(&self, params: &GenerationParams) -> Option<&'static str> {
        if !params.is_focused_action_round() {
            return None;
        }
        match (
            self.style,
            provider_host(&self.base_url).as_str(),
            self.model.as_str(),
        ) {
            (
                ApiStyle::ChatCompletions,
                "api.deepseek.com",
                "deepseek-flash" | "deepseek-v4-pro",
            )
            | (ApiStyle::ChatCompletions, "api.commandcode.ai", "deepseek/deepseek-v4.1-flash") => {
                Some("low")
            }
            (ApiStyle::Responses, "api.commandcode.ai", "deepseek/deepseek-v4.1-flash") => {
                Some("none")
            }
            _ => None,
        }
    }
}
