// @vitest-environment jsdom
// FE-5 A(2026-10-06 前端修复线收口批;R1 裁决 = A+C):资源卡域名确认记忆。
// 背景见 resourceDomains.ts 头部注释(P1-3:一条消息文本即可绕过两道授权闸门)。
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  allowResourceDomain,
  allowedResourceDomains,
  isResourceDomainAllowed,
  resourceDomainOf,
} from './resourceDomains';

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => {
  localStorage.clear();
});

describe('resourceDomains(域名确认记忆)', () => {
  it('resourceDomainOf:取 host(含端口)并小写;非法 URL 返回 null', () => {
    expect(resourceDomainOf('https://Example.COM/game/index.html')).toBe('example.com');
    expect(resourceDomainOf('https://card.test:8443/x')).toBe('card.test:8443');
    expect(resourceDomainOf('not-a-url')).toBeNull();
    expect(resourceDomainOf('')).toBeNull();
  });

  it('默认未确认;确认后按域名记忆且幂等', () => {
    expect(isResourceDomainAllowed('https://card-a.test/a.html')).toBe(false);
    allowResourceDomain('https://card-a.test/a.html');
    expect(isResourceDomainAllowed('https://card-a.test/b.html')).toBe(true);
    expect(allowedResourceDomains()).toEqual(['card-a.test']);
    allowResourceDomain('https://card-a.test/c.html');
    expect(allowedResourceDomains()).toEqual(['card-a.test']);
  });

  it('域名隔离:确认 A 不放行 B;端口不同视为不同域名', () => {
    allowResourceDomain('https://card-a.test/a.html');
    expect(isResourceDomainAllowed('https://card-b.test/a.html')).toBe(false);
    expect(isResourceDomainAllowed('https://card-a.test:8443/a.html')).toBe(false);
  });

  it('脏数据/非法 URL 按未授权处理(不抛)', () => {
    localStorage.setItem('kedai.resource-allowed-domains', '{oops');
    expect(allowedResourceDomains()).toEqual([]);
    expect(isResourceDomainAllowed('https://card-a.test/a.html')).toBe(false);
    localStorage.setItem('kedai.resource-allowed-domains', '[1,"card-a.test",null]');
    expect(allowedResourceDomains()).toEqual(['card-a.test']);
    expect(isResourceDomainAllowed('not-a-url')).toBe(false);
  });
});
