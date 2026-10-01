// Base URL 端点后缀剥离与规范化(接口方言批次,2026-10-01)。
//
// 用例与后端 `settings_service::strip_endpoint_suffix` / `normalize_base_url` 的单测
// 逐例对齐:双端归一化规则必须一致,否则用户在前端失焦修正过的地址保存后又被后端改形。
import { describe, expect, it } from 'vitest';
import { normalizeUrl, stripEndpointSuffix } from './useApiSettings';

describe('stripEndpointSuffix(端点后缀剥离)', () => {
  it('剥离完整端点并给出方言推断(百炼 workspace 实测形态)', () => {
    expect(
      stripEndpointSuffix(
        'https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions',
      ),
    ).toEqual({
      url: 'https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1',
      style: 'chat-completions',
    });
    expect(
      stripEndpointSuffix(
        'https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/responses',
      ),
    ).toEqual({
      url: 'https://ws-x.cn-beijing.maas.aliyuncs.com/compatible-mode/v1',
      style: 'responses',
    });
    // /v1/messages 必须先于 /messages 命中(claude-code-proxy 形态)
    expect(
      stripEndpointSuffix('https://ws-x/api/v2/apps/claude-code-proxy/v1/messages'),
    ).toEqual({ url: 'https://ws-x/api/v2/apps/claude-code-proxy', style: 'anthropic' });
    expect(stripEndpointSuffix('https://api.anthropic.com/v1/messages')).toEqual({
      url: 'https://api.anthropic.com',
      style: 'anthropic',
    });
    // /models 只剥地址、不改方言
    expect(stripEndpointSuffix('https://api.example.com/v1/models')).toEqual({
      url: 'https://api.example.com/v1',
      style: null,
    });
    // 尾部斜杠照剥;普通 base 原样
    expect(stripEndpointSuffix('https://api.example.com/v1/chat/completions/')).toEqual({
      url: 'https://api.example.com/v1',
      style: 'chat-completions',
    });
    expect(stripEndpointSuffix('https://api.deepseek.com/v1/')).toEqual({
      url: 'https://api.deepseek.com/v1',
      style: null,
    });
  });
});

describe('normalizeUrl(与后端 normalize_base_url 同口径)', () => {
  it('端点后缀剥离先于补协议与补 /v1', () => {
    expect(normalizeUrl('https://api.openai.com/v1/chat/completions')).toBe(
      'https://api.openai.com/v1',
    );
    expect(normalizeUrl('api.deepseek.com/chat/completions')).toBe('https://api.deepseek.com/v1');
  });

  it('既有规则不回退:协议补全、/v1 补全、本机 http', () => {
    expect(normalizeUrl('api.openai.com')).toBe('https://api.openai.com/v1');
    expect(normalizeUrl('https://api.deepseek.com/v1/')).toBe('https://api.deepseek.com/v1');
    expect(normalizeUrl('localhost:11434')).toBe('http://localhost:11434/v1');
    expect(normalizeUrl('http://127.0.0.1:1234')).toBe('http://127.0.0.1:1234/v1');
    expect(normalizeUrl('')).toBe('');
  });
});