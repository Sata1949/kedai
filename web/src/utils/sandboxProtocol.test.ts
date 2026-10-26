// sandboxProtocol 纯函数测试（C-2F-3）：常量值、深拷贝隔离与 UTF-8 字节口径。
import { describe, expect, it } from 'vitest';
import {
  CHANNEL,
  MAX_MESSAGE_BYTES,
  cloneData,
  utf8Bytes,
} from './sandboxProtocol';

describe('sandboxProtocol', () => {
  it('频道名与 1MB 上限是协议冻结值', () => {
    expect(CHANNEL).toBe('kedai-character-script-v1');
    expect(MAX_MESSAGE_BYTES).toBe(1024 * 1024);
  });

  it('cloneData 返回与源对象不共享引用的深拷贝', () => {
    const src = { stat_data: { hp: 1, tags: ['a'] } };
    const cloned = cloneData(src);
    expect(cloned).toEqual(src);
    expect(cloned).not.toBe(src);
    cloned.stat_data.hp = 99;
    cloned.stat_data.tags.push('b');
    expect(src.stat_data).toEqual({ hp: 1, tags: ['a'] });
  });

  it('utf8Bytes 按 UTF-8 编码计字节（中文 3 字节）', () => {
    expect(utf8Bytes('雨')).toBe(5); // "雨" = 3 字节 + 两个引号
    expect(utf8Bytes({ a: 1 })).toBe(new TextEncoder().encode('{"a":1}').byteLength);
  });
});
