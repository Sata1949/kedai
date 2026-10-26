// sandboxBroadcast 测试（C-2F-3）：存活沙箱注册表与 MVU/卡级事件广播。
// 覆盖：注册→投递、注销后不再投递、空注册表 no-op、1MB 上限跳过、payload 克隆语义。
import { afterEach, describe, expect, it } from 'vitest';
import {
  broadcastCardEvent,
  broadcastMvuUpdate,
  registerSandboxSink,
  unregisterSandboxSink,
  type SandboxSink,
} from './sandboxBroadcast';

interface Posted {
  message: Record<string, unknown>;
  target: string;
}

/** 最小 contentWindow 桩：只实现 postMessage，记录收到的消息与 targetOrigin。 */
function sink(): { win: SandboxSink; posted: Posted[] } {
  const posted: Posted[] = [];
  const win: SandboxSink = {
    postMessage: (message: unknown, target: string) => {
      posted.push({ message: message as Record<string, unknown>, target });
    },
  };
  return { win, posted };
}

const registered: string[] = [];
function register(nonce: string, win: SandboxSink): void {
  registerSandboxSink(nonce, win);
  registered.push(nonce);
}

afterEach(() => {
  for (const nonce of registered.splice(0)) unregisterSandboxSink(nonce);
});

describe('sandboxBroadcast', () => {
  it('向全部已登记沙箱投递 MVU 更新（channel/nonce/type 与 payload 形状不变）', () => {
    const a = sink();
    const b = sink();
    register('n-a', a.win);
    register('n-b', b.win);

    broadcastMvuUpdate({
      stat_data: { hp: 5 },
      display_data: { label: '雨' },
    });

    expect(a.posted).toHaveLength(1);
    expect(b.posted).toHaveLength(1);
    const { message, target } = a.posted[0];
    expect(target).toBe('*');
    expect(message).toMatchObject({
      channel: 'kedai-character-script-v1',
      nonce: 'n-a',
      type: 'mvu-event',
      event: 'mag_variable_updated',
    });
    expect(message.payload).toEqual({ stat_data: { hp: 5 }, display_data: { label: '雨' } });
    // variables 与 payload 同值（沙箱内两套字段名兼容）
    expect(message.variables).toEqual(message.payload);
  });

  it('payload 是深拷贝：广播后修改源变量不影响已投递消息', () => {
    const a = sink();
    register('n-clone', a.win);
    const variables = { stat_data: { hp: 5 }, display_data: {} };
    broadcastMvuUpdate(variables);
    variables.stat_data.hp = 999;
    expect((a.posted[0].message.payload as { stat_data: { hp: number } }).stat_data.hp).toBe(5);
  });

  it('注销后不再投递；空注册表是 no-op', () => {
    const a = sink();
    register('n-remove', a.win);
    unregisterSandboxSink('n-remove');
    broadcastMvuUpdate({ stat_data: {}, display_data: {} });
    expect(a.posted).toHaveLength(0);

    // 未登记任何沙箱时调用不抛错
    expect(() => broadcastCardEvent('message_swiped', 1)).not.toThrow();
  });

  it('超过 1MB 的 MVU 变量树跳过广播（1MB 上限语义）', () => {
    const a = sink();
    register('n-big', a.win);
    broadcastMvuUpdate({
      stat_data: { blob: 'x'.repeat(1024 * 1024) },
      display_data: {},
    });
    expect(a.posted).toHaveLength(0);
  });

  it('卡级事件携带 name 与 payload，空 name 不投递', () => {
    const a = sink();
    register('n-card', a.win);
    const payload = { floor: 1, nested: { ok: true } };
    broadcastCardEvent('message_swiped', payload);
    payload.nested.ok = false;

    expect(a.posted).toHaveLength(1);
    expect(a.posted[0].message).toMatchObject({
      channel: 'kedai-character-script-v1',
      nonce: 'n-card',
      type: 'card-event',
      name: 'message_swiped',
    });
    // 克隆语义：广播后修改源 payload 不影响已投递消息
    expect(a.posted[0].message.payload).toEqual({ floor: 1, nested: { ok: true } });

    broadcastCardEvent('', payload);
    expect(a.posted).toHaveLength(1);
  });

  it('单个沙箱 postMessage 抛错不影响其他沙箱', () => {
    const broken: SandboxSink = {
      postMessage: () => {
        throw new Error('已销毁');
      },
    };
    const ok = sink();
    register('n-broken', broken);
    register('n-ok', ok.win);

    expect(() => broadcastCardEvent('message_swiped', 0)).not.toThrow();
    expect(ok.posted).toHaveLength(1);
  });
});
