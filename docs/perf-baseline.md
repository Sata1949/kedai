# Kedai 性能基线(优化前)

> 记录日期:2026-08-21;基线 commit:`5f9a55a`(优化前基线)。
> 环境:Windows 11,debug 构建(`cargo test` 产物),数据目录为本机真实 `data/`。

## 测试基线

| 项 | 结果 | 耗时 |
|---|---|---|
| `cargo test`(vcvars64 环境) | **643 绿**(538 单测 + 105 集成,0 失败) | 单测 5.88s,集成合计约 100s |
| `npm test -w web`(Vitest) | **295 绿**(33 文件) | 2.70s |
| `npm run build -w web` | 通过 | 4.17s |

## 前端 bundle 基线(vite build)

| chunk | 体积 | gzip |
|---|---|---|
| `index-*.js`(全部应用代码,13 个弹窗 eager) | 373.85 kB | 111.26 kB |
| `vendor-*.js` | 182.11 kB | 68.99 kB |
| `content-rendering-*.js`(markdown-it + sanitize-html) | 102.88 kB | 44.62 kB |
| `vue-vendor-*.js` | 78.35 kB | 31.09 kB |
| `index-*.css` | 70.44 kB | 13.53 kB |

## 接口延迟基线(`tools/perf-baseline.mjs`,20 并发 × 100 请求)

| 端点 | p50 | p95 | avg | rps | errors |
|---|---|---|---|---|---|
| `GET /api/characters` | 9.9ms | 20.8ms | 11.4ms | 1618 | 0 |
| `GET /api/chat/history?session_id=…` | 7.2ms | 10.1ms | 6.9ms | 2692 | 0 |

注意:本机数据量小(消息表近空),绝对延迟低;阶段 1 验收以**同等工作负载**对比 p95 为准,
并补充「流式生成进行中并发读」场景。

## 压测脚本用法

```bash
# 先启动服务(默认 127.0.0.1:3001),再:
node tools/perf-baseline.mjs            # 20 并发 × 100 请求/端点
node tools/perf-baseline.mjs -c 50 -n 200
```

脚本自动从 `/api/bootstrap` 取 token;自动发现首个有会话的角色做 history 压测,
无会话时跳过该端点。

## Rust 编译环境备忘

`cargo` 命令需在 vcvars64 环境执行。已新增包装脚本:

```bash
cmd //c "C:\Users\LENOVO\Desktop\kedai\tools\cargo-vcvars.cmd cargo test"
cmd //c "C:\Users\LENOVO\Desktop\kedai\tools\cargo-vcvars.cmd cargo clippy -- -D warnings"
```
