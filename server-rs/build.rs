// 构建脚本:让 cargo 感知前端 web/dist 的变化。
// kedai-server 用 include_dir! 在编译期把 web/dist 整体嵌入二进制(见 src/api/mod.rs),
// 但 include_dir 宏本身不输出 rerun-if-changed,cargo 增量编译只看 .rs 文件,
// 导致改完前端后 kedai-server 被跳过重编、新 dist 进不了二进制。
// 这里显式声明 dist 目录变化即触发 build script 重跑与 crate 重编。
fn main() {
    println!("cargo:rerun-if-changed=../web/dist");
}
