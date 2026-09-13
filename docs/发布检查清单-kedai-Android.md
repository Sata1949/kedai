# 发布检查清单 —— kedai-Android

> 本文件**只存在于 `kedai-Android` 分支**,是平台分支相对主干 `main` 的分叉内容之一。
> 分支模型见 [平台分支说明.md](平台分支说明.md)(在 `main` 上)。
> 适用读者:开发 agent 与维护者。禁止注入聊天模型。

## 一、分支角色

`kedai-Android` = `main` 主干 + Android 发布元数据。**不在本分支改共享源码**;
共享改动落 `main` 后 `git merge --ff-only main` 吸收(见平台分支说明 §六)。

## 二、发布前检查

```bash
# 1) 与主干同步(应输出 "Already up to date." 或线性快进)
git checkout kedai-Android
git merge --ff-only main

# 2) 共享门禁全绿(与 CI 同一套;Android 交叉编译不开的段可用 -SkipRust)
powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-all.ps1

# 3) 版本号三处一致:package.json / src-tauri/tauri.conf.json / launcher/Cargo.toml
#    (如发布新版本,在 main 用 tools/bump-version.ps1 统一改,再同步下来)
```

## 三、构建正式包(单 ABI)

```bash
# arm64 真机
D:\kedai-android\bin\ata.bat android build --apk --target aarch64
# x86_64 模拟器
D:\kedai-android\bin\ata.bat android build --apk --target x86_64

# UI 快速迭代(仅 debug 包,约 20 秒,无需重编 Rust)
bash D:\kedai-android\bin\deploy-ui.sh
```

> `--target` 取 Rust 三元组风格:`aarch64` / `x86_64` / `armv7` / `i686`;
> 传 `arm64` 会报 invalid value。`gradle.properties` 的 `abiList` 对 tauri CLI 不生效。

## 四、签名与安全

| 项 | 值 / 位置 |
|---|---|
| 包名 | `com.kedai.app` |
| minSdk / targetSdk | 24 / 36(编译 36) |
| keystore | `D:\kedai-android\keystores\kedai-release.jks`(别名 `kedai`,有效期 10000 天) |
| 口令文件 | `src-tauri/gen/android/keystore.properties`(**已被 `gen/android/.gitignore` 忽略,严禁入库**) |
| 证书 SHA-256 | `19d79a89d87473d286b1ab4d5a45defceae70efe65fa2de0369c1de25f2f062f` |
| 产物目录 | `D:\kedai-android\artifacts\` |

**安全红线:**
1. 提交前确认 `git status` 不含 `keystore.properties` / `*.jks` / `*.keystore`。
2. 凡被 Rust 经 JNI 按名调用的 Kotlin 类/方法,`proguard-rules.pro` 必须**整类保留 `{ *; }`**,
   并用 dex 字节搜索验证**方法名**(不是只看类名)——详见 `docs/android-port-plan.md` §五之五。
3. release 包 `android:debuggable=false`,无法 `run-as`;排查走应用内 API(前端经 CDP)或 logcat。

## 五、发布后验证

- [ ] 安装包能装到目标 ABI 设备/模拟器,冷启动进入 UI(WebView 加载 `http://127.0.0.1:<port>/`)。
- [ ] 关键路径实测:设置页写入 API Key → 200,`has_api_key: true`(验 Keystore 桥未被混淆)。
- [ ] 命令执行三档(ROOT/Shizuku/沙箱)探测结果与授权弹窗在真机表现正常。
- [ ] 关于页版本号与构建信息正确。
- [ ] 记录产物路径与哈希,更新 `docs/android-port-plan.md` 的发布信息表。

## 六、回滚

- 代码:`git reset --hard backup/kedai-Android-0.3.0-beta-b4bd12e`(或任一已发布 tag)。
- 应用:同一 keystore 重新签旧版本号 APK 覆盖安装(签名一致才可覆盖,否则需卸载)。
