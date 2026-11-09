// Kedai 桌面壳(Tauri 2)
// 职责:统一桌面数据目录 → 首次幂等迁移旧数据 → 启动内嵌后端
// → 健康检查通过后再导航并展示窗口。

use std::path::{Path, PathBuf};
use std::time::Duration;
// SystemTime/UNIX_EPOCH 仅用于便携版数据迁移的时间戳标记(桌面专属)
#[cfg(desktop)]
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::Manager;
// Rust→前端下发「用户点了窗口关闭」需要 Emitter trait 在作用域内(emit);
// 该事件仅桌面存在(Android 无 WindowEvent::CloseRequested,见 run() 的 RunEvent 分支)
#[cfg(desktop)]
use tauri::Emitter;

mod native_bridge;
#[cfg(desktop)]
pub mod process_owner;

const READY_TIMEOUT: Duration = Duration::from_secs(60);
const READY_INTERVAL: Duration = Duration::from_millis(250);

/// 关闭确认的二次关闭兜底标记(2026-09-16 批次 5)。
///
/// 为什么需要:关闭确认从「Tauri 原生对话框」改为「前端自绘弹窗」后,壳在
/// CloseRequested 里 prevent_close 并等前端响应。若前端崩溃、页面未加载完或弹窗
/// chunk 加载失败,确认框永远不会出现,窗口就再也关不掉——比没有确认框更糟。
/// 这枚标记让第二次点击关闭直接退出:第一次置位(交给前端弹),已是置位态即强退。
#[cfg(desktop)]
#[derive(Default)]
struct ExitGuard(std::sync::atomic::AtomicBool);

/// 登记一次关闭请求,返回「是否应当强制退出」。
///
/// 首次调用(false→true)返回 false:交给前端弹确认框;再次调用(已是 true)返回 true:
/// 前端没响应,不再阻止关闭。抽成纯函数是为了可单测——窗口关不掉是用户可感的硬故障。
#[cfg(desktop)]
fn register_close_request(flag: &std::sync::atomic::AtomicBool) -> bool {
    flag.swap(true, std::sync::atomic::Ordering::SeqCst)
}

/// 前端取消退出时复位兜底标记:否则「取消一次」之后,下一次单击关闭会因标记仍是
/// 置位态而跳过确认直接退出。
#[cfg(desktop)]
fn reset_close_request(flag: &std::sync::atomic::AtomicBool) {
    flag.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// app_data_dir() 失败时的回退目录。
/// - 桌面:沿用历史行为 %APPDATA%\com.kedai.app;
/// - Android:无 %APPDATA%,返回错误由启动流程记录并退出(宁可明确报错也不写错位置)。
#[cfg(desktop)]
fn fallback_app_data_dir() -> Result<PathBuf, String> {
    Ok(PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("com.kedai.app"))
}

#[cfg(mobile)]
fn fallback_app_data_dir() -> Result<PathBuf, String> {
    Err("无法解析 Android 应用数据目录(app_data_dir 调用失败)".into())
}

/// JNI 入口:Android 在 `System.loadLibrary("kedai_desktop_lib")` 时调用本函数,
/// 把 JavaVM 指针交给原生库。后端 API Key 加密与原生能力桥(外链/分享/保活)
/// 都依赖它(见 server-rs/src/services/jni_bridge.rs)。
///
/// 定义在此(cdylib 根)是为了保证符号被导出 —— 依赖库内部定义的 `JNI_OnLoad`
/// 可能被链接器丢弃。整个依赖栈中没有其它 `JNI_OnLoad`(已核查 tao/wry/tauri)。
///
/// # Safety
/// 由 JVM 调用,`vm` 为其传入的合法 `JavaVM*`;返回期望的 JNI 版本。
#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn JNI_OnLoad(
    vm: *mut std::ffi::c_void,
    _reserved: *mut std::ffi::c_void,
) -> i32 {
    kedai_server::services::jni_bridge::on_load(vm)
}

/// 前端请求退出应用的事件名(Android 返回键在无处可退时触发)。
///
/// 为什么用事件而不是自定义命令:
/// Tauri 2 的 IPC 对 **remote origin**(本应用页面来自 http://127.0.0.1:<port>/,
/// 而非 tauri:// 协议)会做 ACL 校验,而框架**不会**为 `#[tauri::command]` 生成权限条目
/// (已确认 gen/schemas 中无对应条目),实测调用自定义命令报
/// `exit_app not allowed. Plugin not found`;
/// `plugin:app|exit` 同样被拒(无 Rust 侧权限声明),
/// `WebviewWindow::close()` 在 Android 上又不结束 Activity(进程仍在且 Promise 悬挂)。
/// 而事件系统(`core:event:allow-emit`)已包含在 core:default 内,无需新增权限。
///
/// 语义与桌面版一致:后端与壳同进程,退出即整体结束,无残留服务。
const EXIT_APP_EVENT: &str = "kedai://exit-app";

/// 前端→壳:数据位置切换后重启应用。数据根在服务启动期绑定,运行期无法热切换;
/// 重启让新的 `KEDAI_DATA_ROOT` 生效(浏览器/独立 server 无壳,前端提示手动重启)。
const RESTART_APP_EVENT: &str = "kedai://restart-app";

/// 壳→前端下发「用户点了窗口关闭,请弹退出确认」(仅桌面)。
///
/// 为什么走事件而不是原生对话框:关闭确认要出前端至上主义样式,而 Tauri 的
/// `window.dialog()` 是 Windows MessageBox,外观不受前端 CSS 控制。
/// 方向与 EXIT_APP_EVENT 相反(Rust→前端),但同样复用事件系统权限
/// (core:default 内含 core:event:default = allow-listen/allow-emit),无需改 capabilities。
#[cfg(desktop)]
const CLOSE_REQUESTED_EVENT: &str = "kedai://close-requested";

/// 前端→壳:用户在确认框点了「取消」,复位二次关闭兜底标记(仅桌面)。
/// 没有它,取消一次之后的下一次关闭会被兜底标记直接放行。
#[cfg(desktop)]
const CLOSE_CANCELLED_EVENT: &str = "kedai://close-cancelled";

/// 内嵌后端与壳同属一个进程，退出只结束本实例。
#[cfg(desktop)]
fn exit_now(app: &tauri::AppHandle) {
    app.exit(0);
}

/// Android 进程退出必须绕过 `std::process::exit`。
///
/// Tauri/tao 在事件循环结束时调用 `std::process::exit`,该调用会执行 C++ 静态析构;
/// 退出时 hwui 的 `WebViewFunctorManager`(真机 Trichrome WebView)或 EGL display
/// (模拟器)析构会去锁一个已经被 WebView 销毁的互斥量,Android 14/15 的 FORTIFY
/// 直接 SIGABRT(`pthread_mutex_lock called on a destroyed mutex`)。
///
/// `_exit` 是内核级立即退出:不跑 atexit、不跑静态析构,也就不存在「WebView 已销毁
/// 但静态析构还没走完」的窗口。代价是不做 Rust/日志缓冲的收尾,因此调用前必须把
/// 需要留痕的错误同步写入文件(见 `write_start_error`)。
#[cfg(target_os = "android")]
fn hard_exit(code: i32) -> ! {
    // SAFETY:`_exit` 只接受退出码,不访问 Rust 内存;调用即终止本进程。
    unsafe { libc::_exit(code) }
}

/// Android:无端口残留清理流程(无 netstat/taskkill,且系统 launcher 保证单实例),
/// 后端与壳同进程,退出即整体结束。
///
/// 不能走 `AppHandle::exit`:它会触发 tao 的 `std::process::exit`,退出时静态析构
/// 撞上仍存活的 WebView(FORTIFY abort)。`hard_exit` 直接调用内核 `_exit`。
#[cfg(target_os = "android")]
fn exit_now(_app: &tauri::AppHandle) {
    hard_exit(0);
}

/// 其它移动端(iOS):保持 Tauri 原生退出语义。
#[cfg(all(mobile, not(target_os = "android")))]
fn exit_now(app: &tauri::AppHandle) {
    app.exit(0);
}

/// 应用入口。
///
/// `mobile_entry_point` 在 Android/iOS 上生成 JNI/FFI 入口符号(`Java_<pkg>_<cls>_...`),
/// 供 TauriActivity 在应用启动时回调;桌面端该属性被 cfg 排除,行为不变。
/// 缺此属性时 Android 能编译通过但启动即闪退(找不到原生入口)。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 导出保存对话框 + 文件写入(用户自由选择导出位置)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // 前端请求退出(Android 返回键在无处可退时发出)。
            // 用事件而非自定义命令的原因见 EXIT_APP_EVENT 的注释。
            {
                use tauri::Listener;
                let handle = app.handle().clone();
                app.listen(EXIT_APP_EVENT, move |_| {
                    tracing::info!("收到前端退出请求,结束进程");
                    // 走统一退出出口:清理端口残留后再退(此前事件路径漏了清理,
                    // 只有原生对话框回调清理,留下「点退出后 kedai-server 还在跑」的缺口)
                    exit_now(&handle);
                });

                // 数据位置切换后重启:让新的 KEDAI_DATA_ROOT/DATA_DIR 在启动期生效。
                {
                    let handle = app.handle().clone();
                    app.listen(RESTART_APP_EVENT, move |_| {
                        tracing::info!("收到重启请求,应用即将重启以切换数据位置");
                        #[cfg(target_os = "android")]
                        {
                            let _ = &handle;
                            // 原生 PendingIntent 重启:先安排 launcher Intent,再立即结束进程。
                            // 不能走 AppHandle::restart/Tauri process::restart —— 后者最终
                            // std::process::exit,在 WebView 存活时触发同一 FORTIFY abort。
                            match native_bridge::restart_app() {
                                Ok(()) => hard_exit(0),
                                Err(e) => {
                                    tracing::error!(error = e, "原生重启失败,改为直接退出");
                                    hard_exit(0);
                                }
                            }
                        }
                        #[cfg(not(target_os = "android"))]
                        handle.restart();
                    });
                }

                // 前端取消退出:复位二次关闭兜底标记,让下一次关闭仍走确认框。
                #[cfg(desktop)]
                {
                    let handle = app.handle().clone();
                    app.listen(CLOSE_CANCELLED_EVENT, move |_| {
                        if let Some(guard) = handle.try_state::<ExitGuard>() {
                            reset_close_request(&guard.0);
                        }
                    });
                }

                // 原生能力事件(外链/分享/保活):同样是「前端 emit → 原生执行」,
                // 绕开 remote origin 下的自定义命令 ACL 限制。
                // 载荷是 JSON 字符串(&str),解析失败仅记 warn 不 panic。
                app.listen(
                    native_bridge::OPEN_EXTERNAL_EVENT,
                    |event| match serde_json::from_str::<native_bridge::OpenExternalPayload>(
                        event.payload(),
                    ) {
                        Ok(p) => {
                            if let Err(e) = native_bridge::open_external(&p.url) {
                                tracing::warn!(url = p.url, error = e, "打开外链失败");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = e.to_string(), "open-external 载荷解析失败")
                        }
                    },
                );
                app.listen(
                    native_bridge::SHARE_FILE_EVENT,
                    |event| match serde_json::from_str::<native_bridge::ShareFilePayload>(
                        event.payload(),
                    ) {
                        Ok(p) => {
                            if let Err(e) = native_bridge::share_file(&p.name, &p.content) {
                                tracing::warn!(name = p.name, error = e, "分享导出失败");
                            }
                        }
                        Err(e) => tracing::warn!(error = e.to_string(), "share-file 载荷解析失败"),
                    },
                );
                app.listen(native_bridge::KEEPALIVE_START_EVENT, |_| {
                    if let Err(e) = native_bridge::keepalive_start() {
                        tracing::warn!(error = e, "启动前台服务保活失败");
                    }
                });
                app.listen(native_bridge::KEEPALIVE_STOP_EVENT, |_| {
                    if let Err(e) = native_bridge::keepalive_stop() {
                        tracing::warn!(error = e, "停止前台服务保活失败");
                    }
                });
                // 命令执行层的 Shizuku 授权请求(阶段 E):弹系统授权框
                app.listen(native_bridge::SHIZUKU_REQUEST_EVENT, |_| {
                    if let Err(e) = native_bridge::request_shizuku_permission() {
                        tracing::warn!(error = e, "请求 Shizuku 授权失败");
                    }
                });
                // 数据位置(Android):默认根在公共 Download 目录,需要「所有文件访问」权限。
                app.listen(native_bridge::ALL_FILES_ACCESS_EVENT, |_| {
                    if let Err(e) = native_bridge::request_all_files_access() {
                        tracing::warn!(error = e, "请求所有文件访问权限失败");
                    }
                });
            }

            // 数据目录:Android 上 app_data_dir() 映射到应用私有目录(/data/data/<pkg>/files),
            // 无 %APPDATA% 可回退,故回退实现按平台分离(见 fallback_app_data_dir)。
            let app_data = match app.path().app_data_dir() {
                Ok(dir) => dir,
                Err(_) => fallback_app_data_dir()?,
            };
            let data_dir = app_data.join("data");
            let log_dir = app_data.join("logs");

            std::fs::create_dir_all(&log_dir)?;
            // Android 没有 %APPDATA%:指针路径默认解析为 None,选择结果无法持久化。
            // 壳把指针放到应用私有目录(服务端 `data_location::pointer_path` 优先读该环境变量)。
            #[cfg(target_os = "android")]
            std::env::set_var(
                "KEDAI_DATA_LOCATION_POINTER",
                app_data.join("data-location.json"),
            );

            // 数据位置(2026-11 盘符选择):指针存在且 `<root>/data` 可用时以用户选择的根为准;
            // 缺失/不可用则沿用旧逻辑(`%APPDATA%/data`),由前端教程第二步/强制弹窗接管。
            // 旧版「项目 data → APPDATA」迁移只在未选择根时执行:选择根后数据已由服务端迁移。
            let chosen_root = kedai_server::data_location::read_pointer()
                .filter(|pointer| kedai_server::data_location::pointer_usable(&pointer.root))
                .map(|pointer| pointer.root);
            // Android 分支会写入 selected_root(默认公共根);桌面只读。
            #[cfg(target_os = "android")]
            let mut selected_root = chosen_root;
            #[cfg(not(target_os = "android"))]
            let selected_root = chosen_root;
            // Android 默认数据根(2026-11-06):无指针时应落到公共 Download/Kedai-Data。
            // 应用私有目录为空(全新安装/清过数据)且默认根可写 → 直接采用,不弹引导;
            // 已有私有数据 → 交给前端强制弹窗确认迁移(不会静默丢弃旧目录)。
            #[cfg(target_os = "android")]
            if selected_root.is_none() {
                if let Some(default_root) = kedai_server::data_location::suggested_root() {
                    let writable = kedai_server::data_location::probe_writable(&default_root);
                    let private_has_data = kedai_server::config::dir_has_user_data(&data_dir);
                    if writable && !private_has_data {
                        match kedai_server::data_location::ensure_root_layout(&default_root) {
                            Err(error) => {
                                eprintln!("[警告] 创建 Android 默认数据根失败: {error}");
                                std::env::set_var(
                                    "KEDAI_DATA_LOCATION_ERROR",
                                    format!("Android 默认数据根不可用: {error}"),
                                );
                            }
                            Ok(()) => {
                                // 公共目录「能写文件」不等于 SQLite 能用:scoped storage 下
                                // 隐藏探针可能成功、真实 kedai.db 却建不出来。用真实库路径
                                // 探针验证,失败则回退私有目录并按 pending 引导用户。
                                if let Err(error) = kedai_server::services::data_location_service::probe_sqlite_support(
                                    &default_root.join(kedai_server::data_location::DATA_DIR_NAME),
                                ) {
                                    eprintln!(
                                        "[警告] Android 默认数据根不能承载 SQLite: {error};回退应用私有目录"
                                    );
                                    std::env::set_var(
                                        "KEDAI_DATA_LOCATION_ERROR",
                                        format!("Android 默认数据根不可用于 SQLite: {error}"),
                                    );
                                } else {
                                    selected_root = Some(default_root);
                                }
                            }
                        }
                    }
                }
            }
            if let Some(root) = selected_root {
                std::env::set_var("KEDAI_DATA_ROOT", &root);
                eprintln!("[信息] KEDAI_DATA_ROOT={}", root.display());
            } else {
                // 便携版数据迁移仅桌面存在:靠 exe 同级向上找 data/,APK 内无此目录布局
                #[cfg(desktop)]
                if let Some(project_data) = find_project_data_dir() {
                    if let Err(error) = migrate_data_if_needed(&project_data, &data_dir) {
                        write_start_error(&log_dir, &format!("旧数据迁移失败: {error}"));
                    }
                }
                std::env::set_var("DATA_DIR", &data_dir);
                // Android:私有目录只是回退,数据位置仍待用户确认(默认 Download/Kedai-Data)。
                // 显式 DATA_DIR 在桌面/bench 表示「不要弹引导」,故用独立标记表达这一差异。
                #[cfg(target_os = "android")]
                std::env::set_var("KEDAI_DATA_LOCATION_PENDING", "1");
                eprintln!("[信息] DATA_DIR={}", data_dir.display());
            }
            std::env::set_var("LOG_DIR", &log_dir);

            // Android UI 迭代加速(仅 debug 构建):前端 dist 默认编译期内嵌进二进制
            // (server-rs/api/static_files.rs 的 include_dir),每次改样式都要重编 Rust。
            // 若应用私有目录下存在外置 dist,则经 KEDAI_WEB_DIST 覆盖为磁盘读取,
            // 迭代流程变成「npm build → adb push(tar 经 run-as 解包)→ 重启应用」,
            // 无需重编 Rust。
            //
            // 为什么用应用私有目录而非 /sdcard/Android/data/<pkg>/files:
            // 那个目录由 adb shell 创建时 owner 是 shell,Android 11+ 的 scoped storage
            // 会拒绝应用读取(即使 chmod 777 也不行,权限检查在 FUSE 层)。
            // 私有目录 owner 恒为应用自身,读写无限制;adb 侧用 `run-as` 以应用身份写入。
            #[cfg(all(mobile, debug_assertions))]
            {
                let dev_dist = data_dir
                    .parent()
                    .map(|p| p.join("kedai-dist"))
                    .unwrap_or_else(|| data_dir.join("kedai-dist"));
                if dev_dist.join("index.html").is_file() {
                    std::env::set_var("KEDAI_WEB_DIST", &dev_dist);
                    eprintln!("[信息] 使用设备外置前端目录: {}", dev_dist.display());
                }
            }

            let config = kedai_server::config::AppConfig::from_env();
            // 关闭确认的二次关闭兜底标记(仅桌面有 CloseRequested 事件)
            #[cfg(desktop)]
            app.manage(ExitGuard::default());
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) =
                    start_and_wait_ready(config, app_handle.clone(), log_dir.clone()).await
                {
                    write_start_error(&log_dir, &error);
                    eprintln!("[错误] {error}");
                    // Android 绝不能走 AppHandle::exit:它最终调用 std::process::exit,
                    // 在 WebView 仍存活时触发 hwui 的 destroyed mutex(FORTIFY abort)。
                    // 改为原生对话框展示失败原因;用户点「退出」时由 Kotlin 直接
                    // Process.killProcess(同样不跑静态析构)。
                    #[cfg(target_os = "android")]
                    {
                        let _ = &app_handle;
                        if let Err(e) = native_bridge::show_startup_error(&error) {
                            tracing::error!(error = e, "显示启动失败对话框失败,直接退出");
                            hard_exit(1);
                        }
                    }
                    #[cfg(not(target_os = "android"))]
                    app_handle.exit(1);
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建 Kedai 应用失败")
        .run(|app_handle, event| {
            // Android 不使用 app_handle(退出无需清理端口),显式忽略以避免未使用告警
            #[cfg(mobile)]
            let _ = &app_handle;
            // 关闭确认(实跑问题 8):拦截窗口关闭,下发事件让前端弹「确定退出」确认框;
            // 用户确认后前端发 EXIT_APP_EVENT,才真正退出。
            //
            // 为什么不用原生 window.dialog()(2026-09-16 批次 5 改动):它是 Windows
            // MessageBox,外观不受前端 CSS 影响,做不出至上主义样式;改为前端自绘弹窗。
            //
            // 平台差异:Android 没有 WindowEvent::CloseRequested(Activity 不会被「关闭」),
            // 退出由系统返回键/最近任务驱动(返回键无处可退时前端弹同一个确认框),
            // 故整段确认流程仅桌面编译。
            #[cfg(desktop)]
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } = event
            {
                api.prevent_close();
                // 二次关闭兜底:前端不响应(页面未加载完/弹窗 chunk 加载失败/脚本异常)
                // 时仍要能关掉窗口,否则比没有确认框更糟。
                let force = app_handle
                    .try_state::<ExitGuard>()
                    .map(|guard| register_close_request(&guard.0))
                    .unwrap_or(false);
                if force {
                    tracing::warn!("前端未响应关闭确认,第二次关闭直接退出");
                    exit_now(app_handle);
                } else if let Err(e) = app_handle.emit(CLOSE_REQUESTED_EVENT, ()) {
                    // 事件下发失败(无 webview 等)意味着确认框不可能出现:
                    // 用户点了关闭就必须关得掉,直接退出比挂住强。
                    tracing::warn!(error = e.to_string(), "关闭确认事件下发失败,直接退出");
                    exit_now(app_handle);
                }
            }

            // Android:任何 Tauri 退出请求都在这里收口,直接走 `_exit`,不给
            // `std::process::exit` 执行 WebView/EGL 静态析构的机会。
            // 这是最后一道安全网:即使某个第三方插件调用 AppHandle::exit,也不会崩。
            #[cfg(target_os = "android")]
            if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
                tracing::info!(code = ?code, "Kedai Android 进程退出请求");
                api.prevent_exit();
                if code == Some(tauri::RESTART_EXIT_CODE) {
                    if let Err(e) = native_bridge::restart_app() {
                        tracing::error!(error = e, "原生重启失败,改为直接退出");
                    }
                }
                hard_exit(code.unwrap_or(0));
            }

            // 其它移动端(iOS):保留观测点。
            #[cfg(all(mobile, not(target_os = "android")))]
            if let tauri::RunEvent::ExitRequested { .. } = event {
                tracing::info!("Kedai 进程退出请求");
            }
        });
}

/// 端口当前是否可绑定(用于确认旧实例已真正释放端口/端口是否被占用)。
/// 直接试绑一次:成功即空闲;失败说明仍被占用。
async fn port_bindable(config: &kedai_server::config::AppConfig) -> bool {
    match tokio::net::TcpListener::bind((config.host.as_str(), config.port)).await {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(_) => false,
    }
}

async fn start_and_wait_ready(
    config: kedai_server::config::AppConfig,
    app: tauri::AppHandle,
    log_dir: PathBuf,
) -> Result<(), String> {
    // Android:端口被非 Kedai 进程占用时改用系统分配的空闲端口。
    // 不能让「端口冲突」变成启动失败 → 退出:Android 上退出会触发 WebView 析构
    // (FORTIFY abort)。桌面端的接管逻辑在下方单独处理。
    #[cfg(target_os = "android")]
    let config = {
        let mut config = config;
        if !port_bindable(&config).await && !health_ok(&config).await {
            let listener = tokio::net::TcpListener::bind((config.host.as_str(), 0))
                .await
                .map_err(|e| format!("无法分配空闲端口: {e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("读取空闲端口失败: {e}"))?
                .port();
            drop(listener);
            tracing::warn!(from = config.port, to = port, "端口被占用,改用空闲端口");
            eprintln!("[警告] 端口 {} 被占用,改用 {}", config.port, port);
            config.port = port;
        }
        config
    };
    let service_url = service_url(&config);

    // 只有同安装目录、同数据目录的 TCP 实例可以接管；退出不再扫描其他进程。
    #[cfg(windows)]
    if !port_bindable(&config).await {
        let process = verified_other_instance(&config).await?;
        process
            .terminate()
            .map_err(|error| format!("结束已核验的旧实例失败：{error}"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
        while !port_bindable(&config).await {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!("旧实例已结束，但端口 {} 未释放", config.port));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    #[cfg(all(desktop, not(windows)))]
    if !port_bindable(&config).await {
        return Err(format!(
            "端口 {} 已被占用，请先关闭旧实例后启动",
            config.port
        ));
    }

    let (server_error_tx, mut server_error_rx) = tokio::sync::oneshot::channel();
    let server_config = config.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = kedai_server::run_server(server_config).await {
            let _ = server_error_tx.send(error);
        }
    });

    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if let Ok(error) = server_error_rx.try_recv() {
            // 双启动时只复用完整路径和数据目录都已核验的实例。
            #[cfg(windows)]
            if verified_other_instance(&config).await.is_ok() {
                show_main_window(&app, &service_url)?;
                clear_start_error(&log_dir);
                return Ok(());
            }
            return Err(format!("Kedai 后端启动失败: {error}"));
        }
        if health_ok(&config).await {
            #[cfg(windows)]
            if !process_owner::listener_owned_by(service_endpoint(&config)?, std::process::id())
                .map_err(|error| format!("核对当前实例监听进程失败：{error}"))?
            {
                tokio::time::sleep(READY_INTERVAL).await;
                continue;
            }
            show_main_window(&app, &service_url)?;
            // 启动成功:清掉历史失败留下的陈旧错误日志,避免误导排查
            clear_start_error(&log_dir);
            return Ok(());
        }
        tokio::time::sleep(READY_INTERVAL).await;
    }

    Err(format!(
        "Kedai 后端在 {} 秒内未就绪: {service_url}",
        READY_TIMEOUT.as_secs()
    ))
}

/// 导航到服务地址并展示/聚焦主窗口(启动成功与复用已有实例两条路径共用)。
fn show_main_window(app: &tauri::AppHandle, service_url: &str) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "找不到主窗口".to_string())?;
    window
        .navigate(url::Url::parse(service_url).map_err(|e| format!("服务地址非法: {e}"))?)
        .map_err(|e| format!("主窗口导航失败: {e}"))?;
    window.show().map_err(|e| format!("主窗口显示失败: {e}"))?;
    window
        .set_focus()
        .map_err(|e| format!("主窗口聚焦失败: {e}"))?;
    Ok(())
}

/// 删除陈旧的启动错误日志(仅在启动成功/复用成功后调用;失败路径由 write_start_error 重写)。
fn clear_start_error(log_dir: &Path) {
    let _ = std::fs::remove_file(log_dir.join("tauri-start-error.log"));
}

fn service_url(config: &kedai_server::config::AppConfig) -> String {
    let host = match config.host.as_str() {
        "0.0.0.0" => "127.0.0.1",
        "::" => "[::1]",
        host => host,
    };
    format!("http://{host}:{}/", config.port)
}

async fn health_ok(config: &kedai_server::config::AppConfig) -> bool {
    let url = format!("{}api/health", service_url(config));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(1200))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => matches!(
            response.json::<serde_json::Value>().await,
            Ok(body) if body.get("ok").and_then(|value| value.as_bool()).unwrap_or(false)
        ),
        _ => false,
    }
}

/// 将健康请求指向的地址解析成一个 TCP 端点，进程核验与 HTTP 必须看同一地址。
#[cfg(windows)]
fn service_endpoint(
    config: &kedai_server::config::AppConfig,
) -> Result<std::net::SocketAddr, String> {
    use std::net::ToSocketAddrs;
    let host = match config.host.as_str() {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        host => host,
    };
    (host, config.port)
        .to_socket_addrs()
        .map_err(|error| format!("解析服务地址失败：{error}"))?
        .next()
        .ok_or_else(|| "服务地址没有对应的 TCP 端点".to_string())
}

/// 打开并持有候选进程，再核对健康响应的数据目录及监听归属；只有全部一致才返回可接管句柄。
#[cfg(windows)]
async fn verified_other_instance(
    config: &kedai_server::config::AppConfig,
) -> Result<process_owner::VerifiedProcess, String> {
    let endpoint = service_endpoint(config)?;
    let owners = process_owner::listening_pids(endpoint)
        .map_err(|error| format!("读取端口监听进程失败：{error}"))?;
    let [pid] = owners.as_slice() else {
        return Err(format!(
            "端口 {} 的监听归属不唯一或已变化，请先关闭占用者",
            config.port
        ));
    };
    if *pid == std::process::id() {
        return Err("当前实例已持有服务端口，不接管自身".into());
    }
    let executable =
        std::env::current_exe().map_err(|error| format!("读取当前程序路径失败：{error}"))?;
    let process =
        process_owner::VerifiedProcess::open(*pid, &process_owner::trusted_images(&executable))
            .map_err(|error| format!("端口 {} 的进程身份核验失败：{error}", config.port))?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1200))
        .build()
        .map_err(|error| format!("创建实例核验请求失败：{error}"))?;
    let response = client
        .get(format!("http://{endpoint}/api/health"))
        .send()
        .await
        .map_err(|error| format!("读取旧实例健康信息失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "旧实例健康检查返回 {}，未执行接管",
            response.status()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("旧实例健康信息格式错误：{error}"))?;
    verify_instance_data_dir(&body, &config.data_dir)?;
    if !process_owner::listener_owned_by(endpoint, process.pid)
        .map_err(|error| format!("再次核对监听归属失败：{error}"))?
    {
        return Err("实例核验期间端口归属已变化，未执行接管".into());
    }
    Ok(process)
}

/// 健康字段必须完整，且指向当前准备启动的同一数据目录。
#[cfg(windows)]
fn verify_instance_data_dir(body: &serde_json::Value, expected: &Path) -> Result<(), String> {
    let directory = body
        .get("data_dir")
        .and_then(serde_json::Value::as_str)
        .filter(|_| body.get("ok").and_then(serde_json::Value::as_bool) == Some(true))
        .ok_or_else(|| "旧实例缺少有效健康状态或数据目录，未执行接管".to_string())?;
    if !process_owner::same_path(Path::new(directory), expected)
        .map_err(|error| format!("核对旧实例数据目录失败：{error}"))?
    {
        return Err("运行中实例使用另一数据目录，请先关闭该实例或更换端口；未执行接管".into());
    }
    Ok(())
}

/// 以下便携版数据迁移一组函数仅桌面存在:
/// 依赖 exe 同级向上查找 data/ 与 web/ 的发行目录布局,APK 内无此结构。
/// Android 的数据目录由系统分配,首次安装即为空,无需迁移。
#[cfg(desktop)]
fn find_project_data_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut current = exe.parent()?.to_path_buf();
    for _ in 0..6 {
        let data = current.join("data");
        if data.is_dir() && (current.join("web").is_dir() || current.join("Kedai.exe").is_file()) {
            return Some(data);
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// 仅当目标没有任何用户数据时执行复制。源目录始终保留,迁移可重复调用且不会覆盖目标。
#[cfg(desktop)]
fn migrate_data_if_needed(source: &Path, target: &Path) -> Result<usize, String> {
    if source == target || !has_migratable_data(source) || !target_is_pristine(target) {
        return Ok(0);
    }

    std::fs::create_dir_all(target).map_err(|e| format!("创建数据目录失败: {e}"))?;
    let copied = copy_tree(source, target).map_err(|e| format!("复制数据失败: {e}"))?;
    let source_db = source.join("kedai.db");
    if source_db.is_file() {
        kedai_server::migration::snapshot_database(&source_db, &target.join("kedai.db"))?;
    }
    rewrite_avatar_paths(target, source)?;

    let marker = format!(
        "source={}\nmigrated_at={}\nfiles={}\n",
        source.display(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0),
        copied
    );
    std::fs::write(target.join(".migration-v1"), marker)
        .map_err(|e| format!("写入迁移标记失败: {e}"))?;
    Ok(copied)
}

#[cfg(desktop)]
fn has_migratable_data(path: &Path) -> bool {
    path.join("kedai.db").is_file()
        || path.join("settings.json").is_file()
        || path.join("characters").is_dir()
        || path.join("avatars").is_dir()
}

#[cfg(desktop)]
fn target_is_pristine(path: &Path) -> bool {
    if !path.exists() {
        return true;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        if entry.file_name() == "builtin-system.json" {
            continue;
        }
        return false;
    }
    true
}

#[cfg(desktop)]
fn copy_tree(source: &Path, target: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(target)?;
    let mut copied = 0;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text == "kedai.db"
            || name_text.ends_with("-wal")
            || name_text.ends_with("-shm")
            || name_text.ends_with(".log")
            || name_text.contains(".bak-")
            || name_text.starts_with(".verify")
        {
            continue;
        }
        let destination = target.join(name);
        if entry.path().is_dir() {
            copied += copy_tree(&entry.path(), &destination)?;
        } else {
            std::fs::copy(entry.path(), destination)?;
            copied += 1;
        }
    }
    Ok(copied)
}

#[cfg(desktop)]
fn rewrite_avatar_paths(target: &Path, old_data_dir: &Path) -> Result<(), String> {
    let database = target.join("kedai.db");
    if !database.is_file() {
        return Ok(());
    }
    let connection = rusqlite::Connection::open(&database)
        .map_err(|e| format!("打开迁移后的数据库失败: {e}"))?;
    let has_characters = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='characters')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| format!("检查角色表失败: {e}"))?;
    if !has_characters {
        return Ok(());
    }
    let old_prefix = old_data_dir.to_string_lossy();
    let new_prefix = target.to_string_lossy();
    connection
        .execute(
            "UPDATE characters SET avatar_path = replace(avatar_path, ?1, ?2) WHERE avatar_path LIKE ?3",
            rusqlite::params![old_prefix.as_ref(), new_prefix.as_ref(), format!("{}%", old_prefix)],
        )
        .map_err(|e| format!("修正头像路径失败: {e}"))?;
    Ok(())
}

fn write_start_error(log_dir: &Path, message: &str) {
    let _ = std::fs::create_dir_all(log_dir);
    let _ = std::fs::write(
        log_dir.join("tauri-start-error.log"),
        format!("{message}\n"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(desktop)]
    fn temp_dir(tag: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("kedai-desktop-{tag}-{stamp}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn migration_copies_once_and_keeps_source() {
        let source = temp_dir("source");
        let target = temp_dir("target");
        std::fs::write(source.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(source.join("characters")).unwrap();
        std::fs::write(source.join("characters/card.json"), "{}").unwrap();
        let source_db = source.join("kedai.db");
        let connection = rusqlite::Connection::open(&source_db).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE snapshot_test(value TEXT); INSERT INTO snapshot_test VALUES('WAL 中的数据');")
            .unwrap();

        assert_eq!(migrate_data_if_needed(&source, &target).unwrap(), 2);
        assert!(source.join("settings.json").is_file(), "迁移不得删除源数据");
        assert!(target.join("settings.json").is_file());
        assert!(target.join("characters/card.json").is_file());
        let migrated = rusqlite::Connection::open(target.join("kedai.db")).unwrap();
        assert_eq!(
            migrated
                .query_row("SELECT COUNT(*) FROM snapshot_test", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1,
            "迁移必须通过 backup API 包含尚在 WAL 中的数据"
        );
        assert!(target.join(".migration-v1").is_file());

        std::fs::write(source.join("new.json"), "new").unwrap();
        assert_eq!(migrate_data_if_needed(&source, &target).unwrap(), 0);
        assert!(
            !target.join("new.json").exists(),
            "重复运行不得覆盖或追加目标数据"
        );

        std::fs::remove_dir_all(source).ok();
        std::fs::remove_dir_all(target).ok();
    }

    #[test]
    fn migration_never_overwrites_existing_target_data() {
        let source = temp_dir("used-source");
        let target = temp_dir("used-target");
        std::fs::write(source.join("settings.json"), "source").unwrap();
        std::fs::write(target.join("settings.json"), "target").unwrap();

        assert_eq!(migrate_data_if_needed(&source, &target).unwrap(), 0);
        assert_eq!(
            std::fs::read_to_string(target.join("settings.json")).unwrap(),
            "target"
        );

        std::fs::remove_dir_all(source).ok();
        std::fs::remove_dir_all(target).ok();
    }

    #[test]
    fn service_url_uses_loopback_for_unspecified_bind_host() {
        let mut config = kedai_server::config::AppConfig::from_env();
        config.host = "0.0.0.0".into();
        config.port = 4321;
        assert_eq!(service_url(&config), "http://127.0.0.1:4321/");
    }

    /// 二次关闭兜底(批次 5):关闭确认改前端弹窗后,前端不响应时窗口也必须关得掉。
    /// 首次关闭交给前端弹确认框;第二次直接强退;用户取消后复位,回到确认流程。
    #[cfg(desktop)]
    #[test]
    fn close_request_guard_forces_exit_only_on_second_request() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = AtomicBool::new(false);
        assert!(
            !register_close_request(&flag),
            "首次关闭应交给前端弹确认框,不得直接退出"
        );
        assert!(
            register_close_request(&flag),
            "前端未响应时,第二次关闭必须强制退出(否则窗口关不掉)"
        );
        // 用户点了取消:复位后应恢复「先弹确认框」的行为,
        // 不得因上一轮的置位标记把下一次关闭直接放行
        reset_close_request(&flag);
        assert!(
            !register_close_request(&flag),
            "取消退出后应回到确认流程,而不是下一次点击就退出"
        );
        assert!(flag.load(Ordering::SeqCst), "复位后应被本次请求重新置位");
    }

    #[cfg(windows)]
    #[test]
    fn takeover_requires_same_data_directory_and_explicit_health() {
        let expected = std::env::current_dir().unwrap();
        let valid = serde_json::json!({"ok": true, "data_dir": expected.join(".")});
        assert!(verify_instance_data_dir(&valid, &expected).is_ok());
        let other = serde_json::json!({"ok": true, "data_dir": expected.parent().unwrap()});
        assert!(verify_instance_data_dir(&other, &expected)
            .unwrap_err()
            .contains("另一数据目录"));
        assert!(verify_instance_data_dir(&serde_json::json!({"ok": true}), &expected).is_err());
        assert!(verify_instance_data_dir(
            &serde_json::json!({"ok": false, "data_dir": expected}),
            &expected
        )
        .is_err());
    }
}
