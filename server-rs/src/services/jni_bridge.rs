//! 共享 JNI 桥基础设施(仅 Android 编译)。
//!
//! 若干 Kotlin 静态方法需要被 Rust 侧按名反射调用(API Key 加解密、打开外链、
//! 分享文件、前台服务保活)。它们共用同一套 JNI 前置条件,故集中在此:
//!
//! ## 两个必须注意的 JNI 约束(踩坑记录,勿各自重写)
//!
//! 1. **类加载器**:在 `AttachCurrentThread` 附加的原生线程上调用 `FindClass` 会走系统
//!    类加载器,找不到应用类(`com.kedai.app.*`)。故必须在 `JNI_OnLoad` 期间
//!    (此时当前线程的类加载器可见应用类)取一次 `FindClass` 并缓存为 `GlobalRef`,
//!    后续所有线程复用该引用。
//! 2. **VM 获取**:Rust 侧没有 JavaVM 指针,只能由 JVM 在 `System.loadLibrary` 时通过
//!    `JNI_OnLoad` 传入。该函数定义在 cdylib 根(`src-tauri/src/lib.rs`,保证被导出),
//!    再回调 [`on_load`]。
//!
//! 调用方只需 [`call_string_static`](类名 + 方法名 + 单个 String 入参/返回)。
#![cfg(target_os = "android")]

use jni::objects::{GlobalRef, JString, JValue};
use jni::sys::JNI_VERSION_1_6;
use jni::{JNIEnv, JavaVM};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

/// 需要按名调用的桥接类(Kotlin object,静态方法入参/出参均为 String)。
///
/// **类名常量的单一出处**:调用方(`keystore_android` / `native_bridge_android` /
/// `exec/android` / `screen_capture_android`)一律 `use` 本处的常量,`BRIDGE_CLASSES`
/// 亦由同一批常量组装——
/// 「给类起名」与「登记缓存」因此是同一个动作,新增桥类时不可能只做一半。
///
/// 历史教训(2026-09-17 实测):`ShellExecutorBridge` 曾被漏登记,后果是 Android 端
/// 命令执行档位恒为 `disabled`(JNI 取不到类引用 → 探测失败 → 回退禁用),
/// bash 工具在 Android 上**永久不可用**,而错误文案指向「设置里没开开关」,
/// 把排查引向用户配置而非代码。现由 `tools/check-arch.mjs` 规则 L 三重锁定:
/// 调用方↔注册表、注册表↔Kotlin 源(含方法名)、注册表↔proguard keep。
pub const KEYSTORE_CLASS: &str = "com/kedai/app/KeystoreBridge";
pub const NATIVE_CLASS: &str = "com/kedai/app/KedaiNative";
pub const EXEC_CLASS: &str = "com/kedai/app/ShellExecutorBridge";
pub const SCREEN_CAPTURE_CLASS: &str = "com/kedai/app/ScreenCaptureBridge";

/// 全部桥接类(组装自上方常量,勿另起字面量)
const BRIDGE_CLASSES: &[&str] = &[
    KEYSTORE_CLASS,
    NATIVE_CLASS,
    EXEC_CLASS,
    SCREEN_CAPTURE_CLASS,
];

/// 桥接方法签名:入参 String,返回 String
const BRIDGE_SIG_STR_IN: &str = "(Ljava/lang/String;)Ljava/lang/String;";
/// 桥接方法签名:无入参,返回 String(Kotlin `fun f(): String`)
const BRIDGE_SIG_NO_ARG: &str = "()Ljava/lang/String;";

static VM: OnceLock<JavaVM> = OnceLock::new();
static BRIDGES: OnceLock<Mutex<HashMap<&'static str, GlobalRef>>> = OnceLock::new();

fn bridges() -> &'static Mutex<HashMap<&'static str, GlobalRef>> {
    BRIDGES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 由 `JNI_OnLoad` 调用:缓存 JavaVM 与全部桥接类的全局引用。
///
/// # Safety
/// `vm_raw` 必须是 JVM 传入的合法 `JavaVM*`,且仅在 `JNI_OnLoad` 期间调用一次。
/// 返回 JNI 版本号供 `JNI_OnLoad` 回传。
pub unsafe fn on_load(vm_raw: *mut c_void) -> i32 {
    let raw = vm_raw as *mut jni::sys::JavaVM;
    let vm = match JavaVM::from_raw(raw) {
        Ok(vm) => vm,
        Err(e) => {
            eprintln!("[jni] JavaVM 获取失败,原生桥功能不可用: {e}");
            return JNI_VERSION_1_6;
        }
    };

    // 此刻当前线程已由 JVM 附加,直接取 Env 即可(勿再 attach)
    match vm.get_env() {
        Ok(mut env) => {
            for class in BRIDGE_CLASSES {
                if let Err(e) = cache_class(&mut env, class) {
                    eprintln!("[jni] 缓存桥接类 {class} 失败: {e}");
                }
            }
        }
        Err(e) => eprintln!("[jni] 获取 JNIEnv 失败: {e}"),
    }

    let _ = VM.set(vm);
    JNI_VERSION_1_6
}

fn cache_class(env: &mut JNIEnv<'_>, class: &'static str) -> Result<(), String> {
    let found = env
        .find_class(class)
        .map_err(|e| format!("FindClass({class}) 失败: {e}"))?;
    let global = env
        .new_global_ref(found)
        .map_err(|e| format!("NewGlobalRef 失败: {e}"))?;
    bridges().lock().unwrap().insert(class, global);
    Ok(())
}

/// 调用指定桥接类的静态方法(入参 String,出参 String)。
///
/// `class` 必须是 [`BRIDGE_CLASSES`] 中的全限定名(`/` 分隔)。
pub fn call_string_static(
    class: &'static str,
    method: &str,
    input: &str,
) -> Result<String, String> {
    invoke(class, method, Some(input))
}

/// 调用指定桥接类的**无参**静态方法(Kotlin `fun f(): String`),出参 String。
///
/// 为什么必须有这个入口:JNI 的 `CallStaticMethod` 按**签名**查找方法,签名写错时
/// 报 `NoSuchMethodError` 而非「参数不匹配」,极易误判。2026-09-17 实测:此前统一用
/// `(Ljava/lang/String;)Ljava/lang/String;` 调所有桥接方法,而 `detectTier` /
/// `requestShizukuPermission` / `startKeepAlive` / `stopKeepAlive` 在 Kotlin 侧
/// **不接参数**,四个方法全部调用失败——其中 `detectTier` 失败使 Android 命令执行
/// 恒回退 `disabled`。该缺陷此前被「ShellExecutorBridge 未登记」掩盖(类都找不到,
/// 走不到方法查找这一层),修好登记后才暴露。规则 L 现已校验两侧方法名与参数个数一致。
pub fn call_string_static_no_arg(class: &'static str, method: &str) -> Result<String, String> {
    invoke(class, method, None)
}

/// 调用实现:按 `input` 是否给出选择 JNI 签名(两者出参均为 String)。
fn invoke(class: &'static str, method: &str, input: Option<&str>) -> Result<String, String> {
    let vm = VM
        .get()
        .ok_or_else(|| "Android JVM 未就绪(JNI_OnLoad 未执行)".to_string())?;
    // 先取全局引用再 attach:避免持锁跨 JNI 调用
    let global = {
        let map = bridges().lock().unwrap();
        map.get(class).cloned().ok_or_else(|| {
            // 该类的类引用只在 JNI_OnLoad 期缓存,故「不在表里」等价于「永远取不到」。
            // 历史案例:ShellExecutorBridge 漏登记致 Android 命令执行恒 disabled,
            // 而调用方错误文案指向设置项——此处必须把真因说清楚,别让排查走偏。
            format!(
                "{class} 类引用未缓存:该类未登记在 services/jni_bridge.rs 的 BRIDGE_CLASSES\
                 (JNI 类引用仅在 JNI_OnLoad 期缓存,未登记者永远取不到)"
            )
        })?
    };

    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("JNI 线程附加失败: {e}"))?;

    // 按签名分派:有入参传一个 String,无入参传空参数表。
    // 签名写错会得到 NoSuchMethodError(不含「参数不匹配」字样),故此处把两侧约定
    // 显式分开,并在错误信息里回显所用签名,便于对照 Kotlin 声明排查。
    let jstr = match input {
        Some(text) => Some(
            env.new_string(text)
                .map_err(|e| format!("构造 JNI 入参失败: {e}"))?,
        ),
        None => None,
    };
    let sig = if jstr.is_some() {
        BRIDGE_SIG_STR_IN
    } else {
        BRIDGE_SIG_NO_ARG
    };
    let args: Vec<JValue> = match &jstr {
        Some(s) => vec![JValue::Object(s)],
        None => Vec::new(),
    };

    // jni 0.21 为 &GlobalRef 实现了 Desc<JClass>,可直接作为类参数传入,
    // 无需手工转 JClass(避免了 from_raw 的所有权/释放问题)。
    let result = env.call_static_method(&global, method, sig, &args);

    let value = match result {
        Ok(v) => v,
        Err(e) => {
            // Kotlin 侧抛出的异常(如未知密钥、密文损坏)会以 PendingException 形式返回;
            // 取回描述并清除,避免异常悬挂影响后续 JNI 调用
            let detail = match env.exception_occurred() {
                Ok(ex) if !ex.is_null() => {
                    let _ = env.exception_describe();
                    let _ = env.exception_clear();
                    format!("{e}(Java 异常)")
                }
                _ => e.to_string(),
            };
            return Err(format!(
                "调用 {class}::{method} 失败(sig={sig}): {detail}\
                 ;若为 NoSuchMethodError 请核对 Kotlin 侧的参数个数与本签名是否一致"
            ));
        }
    };

    let obj = value
        .l()
        .map_err(|e| format!("{class}::{method} 返回类型异常: {e}"))?;
    if obj.is_null() {
        return Err(format!("{class}::{method} 返回 null"));
    }
    let jstr = JString::from(obj);
    let out: String = env
        .get_string(&jstr)
        .map_err(|e| format!("读取 {class}::{method} 返回字符串失败: {e}"))?
        .into();
    Ok(out)
}
