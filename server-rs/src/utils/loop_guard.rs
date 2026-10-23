// 通用循环熔断守卫(2026-09-14,P0-2)。
//
// 背景:系统里有两类「模型反复做同一件事」的死循环风险,此前只有一处有防护:
//   1. 契约多步变量路径 —— 已有熔断(`contracts/multi_step.rs`,基于 PatchOp 哈希);
//   2. **普通 agent 工具循环 —— 没有任何重复调用检测**,唯一终止条件是
//      `max_tool_rounds`。实测(2026-09-14)plan 模式任务在单步反复调用同一工具时,
//      7 分钟烧 150 万 prompt token 仍未收敛,必须人工 stop。
//
// 本模块提供与契约路径同口径的通用算法,供工具循环复用,避免再写一份:
// 把每步的「结构化指纹」写入近 N 步环形缓冲,同一指纹在窗口内出现 ≥K 次即判定熔断。
// 纯函数/无状态依赖(除自身缓冲),符合 utils 层(L1)纪律。
use std::collections::VecDeque;

/// 默认窗口大小(近 N 步历史),与契约路径 BREAK_WINDOW 同口径。
pub const DEFAULT_WINDOW: usize = 8;
/// 默认重复阈值(窗口内同一指纹出现 ≥K 次判定熔断),与契约路径同口径。
pub const DEFAULT_THRESHOLD: usize = 3;
/// 语义熔断默认窗口(HB-2):同一工具名近 W 次调用内达到 MIN_CALLS 且输出去重 ≤ K 即熔断。
/// 取 16 而非 12:允许窗口内夹入少量其它工具调用(真实空转常夹杂 read 之类的旁路),
/// 仍要求同工具累计 12 次。
pub const DEFAULT_SEMANTIC_WINDOW: usize = 16;
/// 语义熔断默认同工具调用次数下限(HB-2,保守:宁可放过不可误杀)
pub const DEFAULT_SEMANTIC_MIN_CALLS: usize = 12;
/// 语义熔断默认输出指纹去重上限(HB-2):≤2 视为「输出实质无变化」
pub const DEFAULT_SEMANTIC_MAX_DISTINCT: usize = 2;

/// 任务模式语义熔断的**上限**(D3,2026-09-26 实测):
/// 任务模式的工具循环是「一步一个 agent 循环」的短循环,16/12/2 的聊天侧默认太松——
/// 实测单步 10 条命令反复自检(重跑测试/pwd/ls/git status)、单轮往返 1.5~3 分钟,
/// 15 分钟不收敛。任务侧只**收紧**不放宽(见 [`clamp_for_task`])。
/// 取值依据:4 次同工具调用即可判别空转;「连续写多个不同文件」不会误伤——
/// `fs_write` 输出含相对路径(`已写入 {rel}(N 字节)`)、`fs_edit` 含替换处数,
/// 不同文件的输出指纹互不相同,去重数必然超过 max_distinct。
///
/// SKY-6 第 4 项(2026-10-23):本上限只约束**非纯读**工具。纯读工具走
/// [`SemanticGuard::record_classified`] 的整窗阈值,与 `min_calls` 无关——少量文件
/// 被反复读是核对/装配的常态,按 4 次判空转会把探索主预算烧光(全题复跑 4/8/2 实测)。
pub const TASK_MAX_SEMANTIC_WINDOW: usize = 8;
/// 任务模式同工具调用次数上限(见 [`TASK_MAX_SEMANTIC_WINDOW`];纯读工具不适用)
pub const TASK_MAX_SEMANTIC_MIN_CALLS: usize = 4;
/// 任务模式输出指纹去重上限(见 [`TASK_MAX_SEMANTIC_WINDOW`])
pub const TASK_MAX_SEMANTIC_MAX_DISTINCT: usize = 2;

/// 把语义熔断三值钳到任务侧上限(纯函数,便于边界单测)。
///
/// 纪律:任务侧**只收不放**——`min(v, 上限)`,故任务只会比聊天侧更早熔断,
/// 绝不把聊天侧的宽松值当许可。`min_calls == 0` 是「关闭本闸门」的既有开关语义
/// (`SemanticGuard::new` 的 0 = 关闭),钳制必须原样保持 0(详见 D3 口径:
/// 设置页上那三个扁平输入对任务依然生效,只是越不过上限)。
pub fn clamp_for_task(
    window: usize,
    min_calls: usize,
    max_distinct: usize,
) -> (usize, usize, usize) {
    (
        // clamp 的下界 1 与上界常量必须满足 min ≤ max(两个上限常量都 ≥1)
        window.clamp(1, TASK_MAX_SEMANTIC_WINDOW),
        if min_calls == 0 {
            0
        } else {
            min_calls.min(TASK_MAX_SEMANTIC_MIN_CALLS)
        },
        max_distinct.clamp(1, TASK_MAX_SEMANTIC_MAX_DISTINCT),
    )
}

/// FNV-1a 64 位哈希:对多个字符串分段依次混入。用于把「工具名 + 参数」这类
/// 结构化指纹压成一个 u64,避免在缓冲里存完整参数字符串(参数可能极大)。
pub fn fnv1a_hash(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for part in parts {
        for b in part.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        // 分隔符:防止 ("ab","c") 与 ("a","bc") 撞哈希
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 循环熔断守卫:近 `window` 步内同一指纹出现 ≥ `threshold` 次即触发。
pub struct LoopGuard {
    window: usize,
    threshold: usize,
    history: VecDeque<u64>,
}

impl LoopGuard {
    pub fn new(window: usize, threshold: usize) -> Self {
        Self {
            window: window.max(1),
            threshold: threshold.max(2),
            history: VecDeque::new(),
        }
    }

    /// 使用默认口径(N=8, K=3)。
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_WINDOW, DEFAULT_THRESHOLD)
    }

    /// 记录一个指纹,返回是否应熔断(窗口内该指纹累计次数 ≥ threshold)。
    pub fn record(&mut self, fingerprint: u64) -> bool {
        self.history.push_back(fingerprint);
        while self.history.len() > self.window {
            self.history.pop_front();
        }
        self.history.iter().filter(|h| **h == fingerprint).count() >= self.threshold
    }

    /// 当前窗口内的指纹数量(诊断用)
    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    /// 窗口内某指纹的出现次数(日志/事件诊断用)
    pub fn count_of(&self, fingerprint: u64) -> usize {
        self.history.iter().filter(|h| **h == fingerprint).count()
    }
}

// ===================== 语义熔断(HB-2,2026-09-18) =====================
// 上面按「工具名 + 参数」指纹熔断,对「参数每次略变、输出实质无变化」的空转型长跑无效:
// 实测(2026-09-14)连续 54 轮 bash,命令各不相同(ls → cat a → cat b …),指纹互不相同,
// 熔断从未触发,上下文已收敛却仍烧 137 万 prompt token。
// 本判定改为看**输出的实质变化**:同一工具在窗口内被调用 ≥ min_calls 次,而其输出指纹
// 去重后 ≤ max_distinct 种,即认定在原地打转。
//
// 误杀代价高于收益(遗留 L22 的裁定),故:默认参数保守(min_calls=12)、只对「同工具」
// 生效、且输出先归一化噪声再取指纹——正常工作时「同一工具读不同目标」会因输出内容不同
// 而不触发。

/// 输出归一化:抹掉易变噪声,保留「实质内容」。
/// 抹掉的对象是那些与语义无关、每次都变的字节:时间戳/耗时/字节计数/PID/带数字的路径
/// (连续 **3 位及以上**的数字折叠为 `#`),同时折叠空白避免缩进差异。
/// 刻意保留非数字文本:读不同文件、返回不同条目都会留下可区分的内容。
///
/// TMR-5(2026-10-19)收窄:原实现把所有连续数字(含 1~2 位)都折叠成 `#`,导致
/// `NEXT=a02.txt` 与 `a03.txt` 指纹相同——任务侧 4 次链式 `fs_read` 就被误判
/// 「输出实质无变化」熔断(审计文档已取证)。改为只折叠 ≥3 位数字:时间戳/耗时/计数
/// 仍是噪声,而文件名/版本号里的短数字可区分。
///
/// TMR-L5(2026-10-20):3 位以上数字仍会折叠,`part-001.md` / `part-002.md` 依旧同构。
/// 本版在数字折叠**之前**先识别「文件名 token」:路径片段末尾是 `.` + 字母开头的
/// ≥2 位扩展名时,只把 basename 原样保留(目录部分丢弃)。于是分片序号可区分,
/// 而时间戳/耗时/字节计数仍按噪声折叠;`/tmp/proj-1234/out.txt` 这类只有目录在变的
/// 输出仍视为同构(只保护 basename,见遗留 TMR-L5 的新边界)。
fn normalize_tool_output(output: &str) -> String {
    /// 把一段连续数字写入输出:≥3 位折叠为 `#`,1~2 位原样保留。
    fn flush_digits(digits: &mut String, out: &mut String) {
        if digits.is_empty() {
            return;
        }
        if digits.len() >= 3 {
            out.push('#');
        } else {
            out.push_str(digits);
        }
        digits.clear();
    }

    /// 普通字符:数字进折叠缓冲,空白折叠为单空格,其余原样。
    fn push_char(ch: char, digits: &mut String, out: &mut String, prev_space: &mut bool) {
        if ch.is_ascii_digit() {
            digits.push(ch);
            return;
        }
        flush_digits(digits, out);
        if ch.is_whitespace() {
            if !*prev_space {
                out.push(' ');
                *prev_space = true;
            }
        } else {
            *prev_space = false;
            out.push(ch);
        }
    }

    /// 路径片段字符集:只收 ASCII,便于按字节切分且不会切断 UTF-8。
    fn is_path_run_byte(b: u8) -> bool {
        b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'/' | b'\\' | b':')
    }

    /// 命中文件名时返回 basename(保留在输出里,不参与数字折叠)。
    ///
    /// 判定:最后一个 `.` 之后是「字母开头、总长 ≥2、全字母数字」的扩展名,
    /// 且 `.` 前还有非空文件名。`1.23s` / `12.34` 这类耗时/小数不会被误保护。
    fn file_basename_token(run: &str) -> Option<&str> {
        let last_dot = run.rfind('.')?;
        let ext = &run[last_dot + 1..];
        if ext.len() < 2 {
            return None;
        }
        let mut ext_chars = ext.chars();
        if !ext_chars.next()?.is_ascii_alphabetic() {
            return None;
        }
        if !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let before = &run[..last_dot];
        if before.is_empty() {
            return None;
        }
        let base_start = before.rfind(['/', '\\']).map(|p| p + 1).unwrap_or(0);
        let basename = &run[base_start..];
        // `.md` 这种只有扩展名的片段不算文件名
        if basename.len() <= ext.len() + 1 {
            return None;
        }
        Some(basename)
    }

    let bytes = output.as_bytes();
    let mut out = String::with_capacity(output.len().min(4096));
    let mut digits = String::new();
    let mut prev_space = false;
    let mut i = 0usize;
    while i < bytes.len() {
        if is_path_run_byte(bytes[i]) {
            let start = i;
            while i < bytes.len() && is_path_run_byte(bytes[i]) {
                i += 1;
            }
            // 片段是纯 ASCII,按字节切片安全
            let run = &output[start..i];
            if let Some(basename) = file_basename_token(run) {
                flush_digits(&mut digits, &mut out);
                out.push_str("#f:");
                out.push_str(basename);
                prev_space = false;
            } else {
                for ch in run.chars() {
                    push_char(ch, &mut digits, &mut out, &mut prev_space);
                }
            }
        } else {
            let ch = output[i..].chars().next().expect("字节索引必在字符边界");
            push_char(ch, &mut digits, &mut out, &mut prev_space);
            i += ch.len_utf8();
        }
    }
    flush_digits(&mut digits, &mut out);
    out
}

/// 输出指纹(归一化后取「行数 + 前 512 字符 + 去重行集合哈希」)。
/// 三个分量各管一段:行数抓「结果规模变了」,头部抓「内容变了」,去重行集合抓
/// 「只是顺序/重复变了」——只比单一哈希会在噪声被归一化后过度聚合。
pub fn output_fingerprint(output: &str) -> u64 {
    let normalized = normalize_tool_output(output);
    let lines: Vec<&str> = normalized
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let head: String = normalized.chars().take(512).collect();
    let mut uniq = lines.clone();
    uniq.sort_unstable();
    uniq.dedup();
    let uniq_hash = fnv1a_hash(&uniq);
    fnv1a_hash(&[&lines.len().to_string(), &head, &uniq_hash.to_string()])
}

/// 文件改性工具:成功回执文本与「改了什么」无关,同一文件的多段编辑会得到相同
/// 输出。语义熔断若只看输出,会把正常的多段总装误判为空转——2026-10-23 空岛
/// team 实测即 4 次 `fs_edit` / 2 种输出被 HB-2 熔断,`index.html` 停在占位符骨架。
///
/// `bash` 刻意不在名单内:命令各不相同而输出恒定,正是 HB-2 要抓的病态形态
/// (见 `semantic_guard_breaks_on_constant_output_with_varying_args`),不能因为
/// 「参数变了」就放过。
pub fn is_file_mutation_tool(name: &str) -> bool {
    matches!(
        name,
        "fs_write" | "fs_edit" | "fs_patch" | "apply_patch" | "write" | "replace" | "create"
    )
}

/// 供 [`SemanticGuard`] 使用的语义指纹。
///
/// 普通工具只看输出;文件改性工具额外混入「工具名 + 参数」,使不同编辑目标/内容
/// 可区分。完全相同的参数 + 输出仍得到同一指纹,真正的原地重试照样会被熔断。
pub fn semantic_fingerprint(tool: &str, output: &str, arguments: &str) -> u64 {
    let out_fp = output_fingerprint(output);
    if is_file_mutation_tool(tool) {
        let args_fp = fnv1a_hash(&[tool, arguments]);
        fnv1a_hash(&[&out_fp.to_string(), &args_fp.to_string()])
    } else {
        out_fp
    }
}

/// 语义熔断守卫:近 `window` 次调用内,同一工具出现 ≥ `min_calls` 次且其输出指纹
/// 去重后 ≤ `max_distinct` 个 → 判定空转。
///
/// 纯读工具(`fs_read/fs_glob/fs_grep`)走 `record_classified(..., true)` 的**整窗**
/// 阈值,见该方法文档;其余工具维持 `min_calls` 原口径。
pub struct SemanticGuard {
    window: usize,
    min_calls: usize,
    max_distinct: usize,
    history: VecDeque<(String, u64)>,
}

impl SemanticGuard {
    pub fn new(window: usize, min_calls: usize, max_distinct: usize) -> Self {
        Self {
            window: window.max(1),
            // 0 = 关闭本闸门(与工具历史预算/单次预算同款开关语义);
            // 非零下限 3:两次同工具输出相同可能是巧合(如两次都失败),不构成熔断理由
            min_calls: if min_calls == 0 { 0 } else { min_calls.max(3) },
            max_distinct: max_distinct.max(1),
            history: VecDeque::new(),
        }
    }

    /// 默认口径(窗口 16 / 同工具 12 次 / 输出去重 ≤2)
    pub fn with_defaults() -> Self {
        Self::new(
            DEFAULT_SEMANTIC_WINDOW,
            DEFAULT_SEMANTIC_MIN_CALLS,
            DEFAULT_SEMANTIC_MAX_DISTINCT,
        )
    }

    /// 记录一次工具调用与其输出指纹;触发时返回 (工具名, 窗口内该工具次数, 去重指纹数)。
    ///
    /// 非纯读类口径(原样):窗口内同工具 ≥ `min_calls` 且输出去重 ≤ `max_distinct`。
    pub fn record(&mut self, tool: &str, output_fp: u64) -> Option<(String, usize, usize)> {
        self.record_classified(tool, output_fp, false)
    }

    /// 带工具分类的登记(SKY-6 第 4 项,2026-10-23)。
    ///
    /// `readonly_class = true` 表示纯读工具(`fs_read/fs_glob/fs_grep`):这类工具的
    /// 输出就是文件内容,少量文件被反复读(核对、装配前通读)是任务常态——2026-10-23
    /// 全题复跑实测「8 次调用中 4 次、去重 2 种输出」被误熔断,把预算烧在探索主上。
    /// 故纯读类的次数下限抬到**整窗**(`window.max(min_calls)`):必须最近一整窗都是
    /// 同一纯读工具、且输出去重 ≤ `max_distinct`,才判空转。读/写交替时同工具占不满
    /// 整窗,天然放行;集成主读 N 个不同分片时输出去重数远超上限,同样不会误伤。
    pub fn record_classified(
        &mut self,
        tool: &str,
        output_fp: u64,
        readonly_class: bool,
    ) -> Option<(String, usize, usize)> {
        if self.min_calls == 0 {
            return None; // 关闭态:不登记也不判定
        }
        self.history.push_back((tool.to_string(), output_fp));
        while self.history.len() > self.window {
            self.history.pop_front();
        }
        let min_calls = if readonly_class {
            self.window.max(self.min_calls)
        } else {
            self.min_calls
        };
        let mut calls = 0usize;
        let mut fps: Vec<u64> = Vec::new();
        for (t, fp) in &self.history {
            if t == tool {
                calls += 1;
                fps.push(*fp);
            }
        }
        if calls < min_calls {
            return None;
        }
        fps.sort_unstable();
        fps.dedup();
        if fps.len() <= self.max_distinct {
            Some((tool.to_string(), calls, fps.len()))
        } else {
            None
        }
    }

    /// 当前窗口内的调用数(诊断用)
    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_is_stable_and_separator_aware() {
        assert_eq!(fnv1a_hash(&["a", "b"]), fnv1a_hash(&["a", "b"]));
        assert_ne!(fnv1a_hash(&["a", "b"]), fnv1a_hash(&["b", "a"]));
        // 分隔符必须生效:("ab","c") != ("a","bc")
        assert_ne!(fnv1a_hash(&["ab", "c"]), fnv1a_hash(&["a", "bc"]));
    }

    /// 同指纹连续 3 次熔断;2 次不熔断
    #[test]
    fn breaks_after_threshold_repeats() {
        let mut g = LoopGuard::new(8, 3);
        let h = fnv1a_hash(&["read", "{\"path\":\"/a\"}"]);
        assert!(!g.record(h));
        assert!(!g.record(h));
        assert!(g.record(h), "窗口内第 3 次同指纹应熔断");
    }

    /// 不同指纹交替出现不误判(正常工作的特征:每次调用参数不同,如读不同文件)
    #[test]
    fn distinct_fingerprints_do_not_break() {
        let mut g = LoopGuard::new(8, 3);
        // 合法场景:对多个不同目标依次读-改,每次指纹都不同
        for i in 0..30 {
            let read = fnv1a_hash(&["read", &format!("{{\"path\":\"/file{i}\"}}")]);
            let write = fnv1a_hash(&["replace", &format!("{{\"path\":\"/file{i}\"}}")]);
            assert!(!g.record(read), "第 {i} 轮 read 不应误判");
            assert!(!g.record(write), "第 {i} 轮 replace 不应误判");
        }
    }

    /// 同参数交替重复**属于死循环**(模型在两个相同动作间来回打转,无新信息),
    /// 这是有意熔断的场景——与「参数不同」的合法交替区别在此。
    #[test]
    fn same_args_alternation_is_treated_as_loop() {
        let mut g = LoopGuard::new(8, 3);
        let a = fnv1a_hash(&["bash", "{\"cmd\":\"ls\"}"]);
        let b = fnv1a_hash(&["read", "{\"path\":\"/same\"}"]);
        let mut broke = false;
        for _ in 0..10 {
            if g.record(a) || g.record(b) {
                broke = true;
                break;
            }
        }
        assert!(broke, "同参数在两个动作间来回应判定循环并中止");
    }

    /// 窗口滑动:指纹被挤出窗口后不再计数
    #[test]
    fn old_fingerprints_fall_out_of_window() {
        let mut g = LoopGuard::new(3, 3);
        let h = fnv1a_hash(&["x"]);
        assert!(!g.record(h));
        assert!(!g.record(h));
        // 插入 3 个其它指纹,把前两次 h 挤出窗口
        g.record(fnv1a_hash(&["y1"]));
        g.record(fnv1a_hash(&["y2"]));
        g.record(fnv1a_hash(&["y3"]));
        assert_eq!(g.count_of(h), 0, "h 应已被挤出窗口");
        assert!(!g.record(h), "挤出后重新计数,不应触发");
    }

    /// threshold 下限保护:配置 1 会被钳为 2(避免「首次调用即熔断」)
    #[test]
    fn threshold_is_clamped_to_at_least_two() {
        let mut g = LoopGuard::new(8, 1);
        let h = fnv1a_hash(&["x"]);
        assert!(!g.record(h), "首次不应熔断");
        assert!(g.record(h), "第二次应熔断(阈值被钳为 2)");
    }

    // ===== 语义熔断(HB-2) =====

    /// 病态形态(2026-09-14 实测):54 轮 bash 命令各不相同、输出恒定 → 必须熔断,
    /// 而「工具名+参数」指纹对其无效(每轮参数不同)。
    #[test]
    fn semantic_guard_breaks_on_constant_output_with_varying_args() {
        let mut g = SemanticGuard::with_defaults();
        let mut tripped = None;
        for i in 0..54 {
            // 参数每轮都不同(ls / cat a / cat b …),输出逐字相同
            let _args = format!("cmd-{i}");
            if let Some(hit) = g.record(
                "bash",
                output_fingerprint(
                    "total 0
drwxr-xr-x 2 user
",
                ),
            ) {
                tripped = Some(hit);
                break;
            }
        }
        let (tool, calls, distinct) = tripped.expect("输出恒定的空转必须熔断");
        assert_eq!(tool, "bash");
        assert_eq!(
            calls, DEFAULT_SEMANTIC_MIN_CALLS,
            "恰在第 N 次同工具调用触发"
        );
        assert_eq!(distinct, 1, "输出只有一种");
    }

    /// 2026-10-23 空岛 team 实测回归:同一文件的多段 `fs_edit` 成功回执恒同,
    /// 但编辑参数各不相同——这属于正常总装,不得被 HB-2 熔断。
    #[test]
    fn semantic_fingerprint_keeps_edit_targets_distinct() {
        let mut g = SemanticGuard::new(8, 4, 2);
        for i in 0..6 {
            let args =
                format!("{{\"path\":\"index.html\",\"old\":\"/*@@slot{i}@@*/\",\"new\":\"body\"}}");
            let fp = semantic_fingerprint("fs_edit", "已编辑 index.html(1 处替换)", &args);
            assert!(
                g.record("fs_edit", fp).is_none(),
                "不同编辑参数不得熔断(第 {i} 次)"
            );
        }
    }

    /// 同参数 + 同输出的 `fs_edit` 仍是真空转:第 4 次必须熔断。
    #[test]
    fn semantic_fingerprint_still_breaks_identical_edits() {
        let mut g = SemanticGuard::new(8, 4, 2);
        let args = "{\"path\":\"index.html\",\"old\":\"a\",\"new\":\"b\"}";
        let fp = semantic_fingerprint("fs_edit", "已编辑 index.html(1 处替换)", args);
        for i in 0..3 {
            assert!(g.record("fs_edit", fp).is_none(), "第 {i} 次不应熔断");
        }
        assert!(
            g.record("fs_edit", fp).is_some(),
            "同参数同输出第 4 次应熔断"
        );
    }

    /// `bash` 保持输出侧判定:命令每轮不同但输出恒定,仍须熔断(病态空转的原始形态)。
    #[test]
    fn semantic_fingerprint_keeps_bash_output_only() {
        let mut g = SemanticGuard::new(8, 4, 2);
        for i in 0..4 {
            let fp = semantic_fingerprint("bash", "total 0", &format!("cmd-{i}"));
            let hit = g.record("bash", fp);
            if i == 3 {
                assert!(hit.is_some(), "bash 命令不同、输出恒定应熔断");
            } else {
                assert!(hit.is_none(), "第 {i} 次不应熔断");
            }
        }
    }

    /// 正常形态:同一工具读不同文件(输出文本不同)→ 不得误杀。
    ///
    /// 注意归一化的**已知取舍**:数字统一折叠为 `#`,故「只差数值」的输出会被视作同构
    /// (这是刻意的——时间戳/耗时/计数正是要抹掉的那类噪声)。真实文件内容差异在文本上,
    /// 本例按真实形态构造。
    #[test]
    fn semantic_guard_keeps_distinct_outputs() {
        let mut g = SemanticGuard::with_defaults();
        let topics = [
            "开篇设定",
            "人物关系",
            "世界观",
            "伏笔一",
            "伏笔二",
            "结局走向",
            "支线甲",
            "支线乙",
            "时间线",
            "地点设定",
            "道具设定",
            "势力设定",
            "冲突设计",
            "节奏安排",
            "叙事视角",
            "语言风格",
            "意象",
            "隐喻",
            "对白设计",
            "场景切换",
        ];
        for (i, topic) in topics.iter().enumerate() {
            let out = format!("文件 {i}.md 的内容:关于「{topic}」的说明,与其它文件不同。");
            assert!(
                g.record("read", output_fingerprint(&out)).is_none(),
                "输出文本各不相同不得熔断(第 {i} 次:{topic})"
            );
        }
    }

    /// 噪声(时间戳/耗时/字节数/带数字路径)被归一化抹掉 → 仍判空转
    #[test]
    fn semantic_guard_normalizes_volatile_noise() {
        let mut g = SemanticGuard::with_defaults();
        let mut tripped = false;
        for i in 0..20 {
            let out = format!(
                "耗时 12{}ms  字节数 409{}  路径 /home/user/project/{}/out.txt  状态 ok
",
                i,
                i,
                1000 + i
            );
            if g.record("bash", output_fingerprint(&out)).is_some() {
                tripped = true;
                break;
            }
        }
        assert!(
            tripped,
            "只有数字在变(时间戳/耗时/计数)应视为输出无实质变化"
        );
    }

    /// 同工具调用不足 min_calls 不触发;去重指纹超过 max_distinct 不触发
    #[test]
    fn semantic_guard_requires_enough_calls_and_few_distinct_outputs() {
        let mut g = SemanticGuard::new(16, 12, 2);
        for _ in 0..11 {
            assert!(g.record("bash", 7).is_none(), "11 次不足 12 次下限");
        }
        assert!(g.record("bash", 7).is_some(), "第 12 次触发");

        let mut g2 = SemanticGuard::new(16, 5, 2);
        for i in 0..10 {
            let fp = if i % 3 == 0 {
                1
            } else if i % 3 == 1 {
                2
            } else {
                3
            };
            assert!(
                g2.record("bash", fp).is_none(),
                "去重 3 种 > 上限 2,不得熔断(第 {i} 次)"
            );
        }

        // 窗口外滑出后计数回落(不再累计)
        let mut g3 = SemanticGuard::new(4, 12, 2);
        for _ in 0..4 {
            assert!(g3.record("bash", 1).is_none());
        }
        assert_eq!(g3.len(), 4, "窗口上限 4");
    }

    /// 0 = 关闭:任何形态都不熔断(误杀风险高的闸门必须可关)
    #[test]
    fn semantic_guard_disabled_by_zero_min_calls() {
        let mut g = SemanticGuard::new(16, 0, 2);
        for _ in 0..100 {
            assert!(g.record("bash", 42).is_none(), "关闭态不得熔断");
        }
        assert!(g.is_empty(), "关闭态不登记历史");
    }

    /// TMR-5:短数字标识符必须可区分——原实现把 1~2 位数字也折叠成 `#`,导致
    /// `NEXT=a02.txt` / `a03.txt` 被语义熔断误判为同一种输出(任务侧 4 次链式
    /// `fs_read` 即误杀)。3 位以上仍按噪声折叠,见 `output_fingerprint_is_noise_insensitive`。
    #[test]
    fn output_fingerprint_keeps_short_numeric_identifiers() {
        assert_ne!(
            output_fingerprint("NEXT=a02.txt"),
            output_fingerprint("NEXT=a03.txt"),
            "两位文件名序号必须可区分"
        );
        assert_ne!(
            output_fingerprint("NEXT=a2.txt"),
            output_fingerprint("NEXT=a3.txt"),
            "一位文件名序号必须可区分"
        );
        assert_ne!(
            output_fingerprint("v1.2.3"),
            output_fingerprint("v1.2.4"),
            "短版本号必须可区分"
        );
        // 反向钉住:3 位以上的时间戳/计数仍折叠(该取舍不因本修复丢失)
        assert_eq!(
            output_fingerprint("耗时 120ms"),
            output_fingerprint("耗时 980ms"),
            "三位耗时仍属噪声"
        );
    }

    /// TMR-L5:3 位以上分片序号必须可区分——原实现把 `part-001.md` / `part-002.md`
    /// 的数字统一折叠,任务侧链式读取 4 个分片即被语义熔断误杀。
    #[test]
    fn output_fingerprint_keeps_long_file_sequence_numbers() {
        assert_ne!(
            output_fingerprint("part-001.md (共 10 行)\nNEXT=part-002.md"),
            output_fingerprint("part-002.md (共 10 行)\nNEXT=part-003.md"),
            "三位分片序号必须可区分"
        );
        assert_ne!(
            output_fingerprint("C:\\ws\\part-001.md"),
            output_fingerprint("C:\\ws\\part-002.md"),
            "Windows 路径下的分片序号同样必须可区分"
        );
        // 目录噪声仍被抹掉:只保护 basename
        assert_eq!(
            output_fingerprint("/tmp/proj-1234/out.txt"),
            output_fingerprint("/tmp/proj-5678/out.txt"),
            "只有目录中的数字在变时仍视为同构"
        );
        // 已知边界:同 basename 不同目录会同构(遗留 TMR-L5 新边界)
        assert_eq!(
            output_fingerprint("src/a/x.js"),
            output_fingerprint("src/b/x.js"),
            "basename-only 保护下同文件名不同目录视为同构"
        );
    }

    /// TMR-L5:任务侧收紧参数(窗口 8 / 同工具 4 / 去重 2)下,链式读取
    /// `part-001`…`part-006` 不得被误熔断。
    #[test]
    fn semantic_guard_keeps_part_sequence_reads() {
        let mut g = SemanticGuard::new(8, 4, 2);
        for i in 1..=6 {
            let out = format!("part-{i:03}.md (共 10 行)\nNEXT=part-{:03}.md", i + 1);
            assert!(
                g.record_classified("fs_read", output_fingerprint(&out), true)
                    .is_none(),
                "第 {i} 个分片的读取不得被误熔断"
            );
        }
    }

    // ===== SKY-6 第 4 项:纯读工具的整窗阈值(2026-10-23) =====

    /// 纯读工具必须占满整窗:7 次同输出不熔断,第 8 次才熔断(任务侧窗口 8)。
    #[test]
    fn semantic_guard_readonly_class_needs_full_window() {
        let mut g = SemanticGuard::new(8, 4, 2);
        for i in 1..8 {
            assert!(
                g.record_classified("fs_read", 7, true).is_none(),
                "第 {i} 次同输出纯读未占满整窗,不得熔断"
            );
        }
        assert_eq!(
            g.record_classified("fs_read", 7, true),
            Some(("fs_read".to_string(), 8, 1)),
            "第 8 次占满整窗且输出去重 ≤2 种必须熔断"
        );
    }

    /// 全题复跑的 4/8/2 形态:窗内 4 次 `fs_read`、2 种输出,其余为其它工具。
    /// 未分档时第 4 次即熔断;整窗阈值下同工具只占 4/8,恒不触发。
    #[test]
    fn semantic_guard_readonly_class_ignores_interleaved_reads() {
        let mut g = SemanticGuard::new(8, 4, 2);
        for round in 0..4 {
            assert!(g.record_classified("fs_read", 100, true).is_none());
            assert!(g.record_classified("bash", 900 + round, false).is_none());
            assert!(g.record_classified("fs_read", 200, true).is_none());
            assert!(g.record_classified("bash", 950 + round, false).is_none());
        }
    }

    /// 整窗阈值不是「关掉」:整窗 8 次 + 2 种输出仍熔断(去重上限仍是 2)。
    #[test]
    fn semantic_guard_readonly_class_breaks_on_full_window_two_outputs() {
        let mut g = SemanticGuard::new(8, 4, 2);
        for i in 0..7 {
            let fp = if i % 2 == 0 { 100 } else { 200 };
            assert!(g.record_classified("fs_read", fp, true).is_none());
        }
        let hit = g
            .record_classified("fs_read", 100, true)
            .expect("整窗 8 次、2 种输出仍须熔断");
        assert_eq!((hit.1, hit.2), (8, 2));
    }

    /// 非纯读工具维持原口径:同工具 4 次同输出即熔断(bash 抓「输出恒定」的病态)。
    #[test]
    fn semantic_guard_default_record_keeps_threshold_for_other_tools() {
        let mut g = SemanticGuard::new(8, 4, 2);
        for i in 1..4 {
            assert!(g.record("bash", 7).is_none(), "第 {i} 次不应熔断");
        }
        assert!(
            g.record("bash", 7).is_some(),
            "非纯读工具仍按同工具 4 次熔断"
        );
    }

    /// 归一化/指纹的稳定性:同内容(含噪声差异)指纹相同,不同内容指纹不同
    #[test]
    fn output_fingerprint_is_noise_insensitive() {
        let a = output_fingerprint(
            "耗时 120ms, 共 3 行
A
B
",
        );
        let b = output_fingerprint(
            "耗时 980ms, 共 3 行
A
B
",
        );
        assert_eq!(a, b, "只有数字变化时指纹必须相同");
        let c = output_fingerprint(
            "耗时 120ms, 共 3 行
A
C
",
        );
        assert_ne!(a, c, "内容变化必须改变指纹");
        // 行数变化也要能区分(短输出可能头部相同)
        let d = output_fingerprint(
            "耗时 120ms, 共 3 行
A
B
B
",
        );
        assert_ne!(a, d, "行数变化应改变指纹");
    }

    // ===== 任务侧钳制(提交 3 · D3) =====

    /// 钳制只收不放:三值超上限即压到上限,上限内原样保留
    #[test]
    fn clamp_for_task_caps_but_never_loosens() {
        assert_eq!(
            clamp_for_task(64, 12, 4),
            (TASK_MAX_SEMANTIC_WINDOW, TASK_MAX_SEMANTIC_MIN_CALLS, 2),
            "聊天侧宽松值必须被压到任务上限"
        );
        assert_eq!(
            clamp_for_task(16, 12, 2),
            (8, 4, 2),
            "聊天侧默认值在任务侧同样收紧"
        );
        assert_eq!(clamp_for_task(4, 4, 1), (4, 4, 1), "上限内原样保留");
    }

    /// `min_calls == 0` 是「关闭本闸门」的既有开关语义:钳制必须原样保持 0。
    /// 若这里被改成 `max(上限)` 之类的写法,「关掉熔断」的用户设置会被静默推翻。
    #[test]
    fn clamp_for_task_keeps_zero_as_off() {
        assert_eq!(clamp_for_task(64, 0, 4).1, 0, "0 = 关闭,不得被抬起");
        let (w, m, d) = clamp_for_task(0, 0, 0);
        assert_eq!(
            (w, m, d),
            (1, 0, 1),
            "下界兜底:窗口/去重至少 1,关闭位保持 0"
        );
    }

    /// 钳制后的值喂给 SemanticGuard 必须仍然可用(接线一致性):
    /// 4 次输出相同的同工具调用即熔断,而聊天侧的 12 次门槛在任务侧不可达。
    #[test]
    fn clamped_values_break_early_in_semantic_guard() {
        let (w, m, d) = clamp_for_task(64, 12, 4);
        let mut g = SemanticGuard::new(w, m, d);
        for i in 1..m {
            assert!(g.record("bash", 7).is_none(), "第 {i} 次不应熔断");
        }
        assert!(
            g.record("bash", 7).is_some(),
            "第 {m} 次同工具同输出必须熔断(未钳制时聊天侧要 12 次)"
        );
    }
}
