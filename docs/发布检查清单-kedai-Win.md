# 发布检查清单 —— kedai-Win

> 本文件**只存在于 `kedai-Win` 分支**,是平台分支相对主干 `main` 的分叉内容之一。
> 分支模型见 [平台分支说明.md](平台分支说明.md)(在 `main` 上)。
> 适用读者:开发 agent 与维护者。禁止注入聊天模型。

## 一、分支角色

`kedai-Win` = `main` 主干 + Windows 发布元数据。**不在本分支改共享源码**;
共享改动落 `main` 后 `git merge --ff-only main` 吸收(见平台分支说明 §六)。

## 二、发布前检查

```powershell
# 1) 与主干同步(应输出 Already up to date 或线性快进)
git checkout kedai-Win
git merge --ff-only main

# 2) 共享门禁全绿(与 CI 同一套:lint + 契约 + 架构 + 双锁漂移 + cargo audit + npm audit)
powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-all.ps1

# 3) 版本号四处一致:package.json / src-tauri/tauri.conf.json /
#    launcher/Cargo.toml / server-rs/Cargo.toml(build.ps1 的 Assert-VersionConsistency 会校验)
```

> 本机裸 shell 无 cargo:先
> `call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"`,
> 再在同一 shell 执行;Git Bash 用 `cmd //c "<包装.bat>"` 嵌套。

## 三、构建正式产物

```powershell
.\build.ps1            # 默认双端同步:前端 + cargo build --release + 便携版 + 构建指纹 sidecar
.\build.ps1 -TestOnly  # 快速迭代:只产测试版(结尾会警告便携版未同步)
.\build.ps1 -Tauri     # 追加 NSIS 安装包(需 tauri CLI)

# 只出便携版(不重跑 release 链)
powershell -NoProfile -ExecutionPolicy Bypass -File tools/build-portable.ps1
```

**产物:**

| 产物 | 路径 | 说明 |
|---|---|---|
| 测试版 | `dist\kedai-server.exe` | 单二进制,内嵌 `web/dist` |
| 便携版 | `dist\Kedai-portable\Kedai.exe` | 交付物;含图形启动器 `Kedai.exe` |
| 图形启动器 | 项目根 `Kedai.exe` / `Kedai.lnk` | 源码 `launcher/`,启动前做过期/漂移检测 |
| 安装包(可选) | `src-tauri\target\release\bundle\nsis\*.exe` | `-Tauri` 时产出 |

> **只跑过 debug ≠ exe 已更新。** 交付前必须跑一次完整 `.\build.ps1`。
> 每个 dist 产物旁写有 `<exe>.build.json`(`{version, build_time, dist_hash}`)。

## 四、同步与验收

- [ ] 测试版与便携版由**同一次** `build.ps1` 产出;两端并排比较 `dist_hash` 一致
      (`tools/Write-BuildStamp.ps1` 算法)。
- [ ] `GET /api/health` 返回的 `build_id` / `build_time` 与 sidecar 相符;
      设置中心底部「版本 · 构建时间 · 指纹前 8 位」两端一致。
- [ ] 双击 `Kedai.lnk` 冷启动成功;启动器在指纹不一致时能自动触发双端重建。
- [ ] exe 放在 `server-rs/target/release/` 原路径,或其上级存在 `web` 目录(否则定位不到项目根)。

## 五、分发与安全

| 项 | 说明 |
|---|---|
| 分发方式 | 整目录拷贝,或 `exe + data + logs`;保持目录结构 |
| 代码签名 | **未做 Authenticode 签名**,SmartScreen 会拦;指引见 `docs/代码签名与SmartScreen说明.md` |
| 密钥 | API Key 经 Windows DPAPI 加密存于数据目录(**`%APPDATA%\com.kedai.app\` 或 `DATA_DIR`**),不入库、不随产物分发 |
| 提交红线 | 产物 `Kedai.exe` / `dist/` 已在 `.gitignore`,确认 `git status` 不误加 |

## 六、发布后记录

- [ ] 记录版本号、`dist_hash`、产物路径与 SHA-256。
- [ ] 在 `docs/` 对应变更说明文档中回写发布记录(参照已有各版「变更说明」末尾的发布段)。

## 七、回滚

- 代码:`git reset --hard backup/kedai-Win-0.2.1-b361fee`(或任一已发布 tag)。
- 产物:保留上一版 `dist\Kedai-portable\` 整目录,覆盖回去即可回退(数据目录不受影响)。
