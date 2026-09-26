// Windows 进程派生常量(单一出处)。
//
// 为什么需要这个模块(2026-09-18 实测):
// 桌面版(Kedai.exe / kedai-portable.exe)入口带 `windows_subsystem = "windows"`,
// 进程**没有控制台**。Windows 下由无控制台的父进程派生控制台程序(如 `cmd /C ...`)
// 时,系统会为该子进程**新建一个可见控制台窗口**——stdio 重定向(piped)并不能阻止
// 窗口创建。任务模式每次 bash 工具调用都走 `cmd /C`,于是模型每轮调工具就闪一个
// 黑窗(实测日志:70 秒内 15+ 次 bash 调用)。
//
// 关键陷阱:debug 构建入口没有 `windows_subsystem` 属性(见 src-tauri/src/main.rs),
// 自身带控制台,子进程继承父控制台,**开发期完全看不到弹窗**——只在交付的
// release/便携版复现。故此处纪律为硬性的:
//   **本仓库任何派生控制台程序的代码路径,都必须显式带 CREATE_NO_WINDOW。**
// 检查手段:`git grep -n "Command::new"` 逐处核对(量少且集中在 exec/mcp/壳三处)。
//
// 例外:`launcher` 是零依赖 crate(不依赖本模块),且其中一处是**故意**要可见窗口
// (CREATE_NEW_CONSOLE 跑构建脚本),常量就地内联,注释标明「故意」。
// 代际: L1(老层·稳)——纯常量,零依赖。

/// 子进程以无控制台窗口方式运行(CreateProcess 的 dwCreationFlags 位)。
///
/// 语义:进程**有**控制台(可正常写 CONOUT$/读 stdin 重定向),但没有可见窗口。
/// 这正是「后台跑命令、不打扰用户」所需的形态;不要用 DETACHED_PROCESS——
/// 那会让子进程完全没有控制台,部分命令(需要 CONOUT$ 的)行为会变。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
