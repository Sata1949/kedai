// resourceDomains.ts — 资源卡「首次遇新域名一次性确认」的域名记忆(FE-5 A,2026-10-06)。
//
// 背景(FRONTEND-REPORT P1-3):一条消息文本里的 `$('body').load('https://…')` 会让
// 宿主自动经代理拉取第三方页面并在沙箱 iframe 里执行其 JS——这是**唯一**「一条消息
// 文本即可绕过 renderHtml 开关与逐卡脚本授权两道闸门」的路径。`遗留.md` T1 的既有
// 裁决(知情接受:作者页外联 + TavernHelper.generate 生成能力)不被推翻:本模块只把
// 「一条消息文本即自动装载」改成「域名级一次性确认后自动装载」(2026-10-06 R1 裁决
// = A+C;逐卡授权 B 会让既有卡直接失效,本轮不做)。
//
// 记忆落 localStorage(与 renderHtmlOverrides 同族的本机偏好,不进 settings.json,
// 不新增设置字段);键值:域名(host,含端口)的 JSON 数组,小写。
const STORAGE_KEY = 'kedai.resource-allowed-domains';

/** 取 URL 的域名(host,含端口,小写);非法 URL 返回 null(调用方按未授权处理)。 */
export function resourceDomainOf(url: string): string | null {
  try {
    const host = new URL(url).host.toLowerCase();
    return host || null;
  } catch {
    return null;
  }
}

/** 已确认域名清单(读取失败——隐私模式/存量脏数据——按空数组处理)。 */
export function allowedResourceDomains(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]') as unknown;
    if (!Array.isArray(raw)) return [];
    return raw.filter((d): d is string => typeof d === 'string' && d.length > 0);
  } catch {
    return [];
  }
}

/** 该 URL 的域名此前是否已被用户确认(非法 URL 恒 false)。 */
export function isResourceDomainAllowed(url: string): boolean {
  const host = resourceDomainOf(url);
  return host !== null && allowedResourceDomains().includes(host);
}

/** 记录用户对该 URL 域名的确认(幂等;写失败静默——最坏结果是下次仍需确认)。 */
export function allowResourceDomain(url: string): void {
  const host = resourceDomainOf(url);
  if (!host) return;
  const list = allowedResourceDomains();
  if (list.includes(host)) return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...list, host]));
  } catch {
    /* 忽略(隐私模式/配额满) */
  }
}
