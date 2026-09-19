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
/// (数字统一折叠为 `#`),同时折叠空白避免缩进差异。
/// 刻意保留非数字文本:读不同文件、返回不同条目都会留下可区分的内容。
fn normalize_tool_output(output: &str) -> String {
    let mut out = String::with_capacity(output.len().min(4096));
    let mut prev_digit = false;
    let mut prev_space = false;
    for ch in output.chars() {
        if ch.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
            prev_space = false;
            continue;
        }
        prev_digit = false;
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            prev_space = false;
            out.push(ch);
        }
    }
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

/// 语义熔断守卫:近 `window` 次调用内,同一工具出现 ≥ `min_calls` 次且其输出指纹
/// 去重后 ≤ `max_distinct` 个 → 判定空转。
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
    pub fn record(&mut self, tool: &str, output_fp: u64) -> Option<(String, usize, usize)> {
        if self.min_calls == 0 {
            return None; // 关闭态:不登记也不判定
        }
        self.history.push_back((tool.to_string(), output_fp));
        while self.history.len() > self.window {
            self.history.pop_front();
        }
        let mut calls = 0usize;
        let mut fps: Vec<u64> = Vec::new();
        for (t, fp) in &self.history {
            if t == tool {
                calls += 1;
                fps.push(*fp);
            }
        }
        if calls < self.min_calls {
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
            if let Some(hit) = g.record("bash", output_fingerprint("total 0
drwxr-xr-x 2 user
")) {
                tripped = Some(hit);
                break;
            }
        }
        let (tool, calls, distinct) = tripped.expect("输出恒定的空转必须熔断");
        assert_eq!(tool, "bash");
        assert_eq!(calls, DEFAULT_SEMANTIC_MIN_CALLS, "恰在第 N 次同工具调用触发");
        assert_eq!(distinct, 1, "输出只有一种");
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
            "开篇设定", "人物关系", "世界观", "伏笔一", "伏笔二", "结局走向", "支线甲", "支线乙",
            "时间线", "地点设定", "道具设定", "势力设定", "冲突设计", "节奏安排", "叙事视角",
            "语言风格", "意象", "隐喻", "对白设计", "场景切换",
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
                i, i, 1000 + i
            );
            if g.record("bash", output_fingerprint(&out)).is_some() {
                tripped = true;
                break;
            }
        }
        assert!(tripped, "只有数字在变(时间戳/耗时/计数)应视为输出无实质变化");
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
            let fp = if i % 3 == 0 { 1 } else if i % 3 == 1 { 2 } else { 3 };
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

    /// 归一化/指纹的稳定性:同内容(含噪声差异)指纹相同,不同内容指纹不同
    #[test]
    fn output_fingerprint_is_noise_insensitive() {
        let a = output_fingerprint("耗时 120ms, 共 3 行
A
B
");
        let b = output_fingerprint("耗时 980ms, 共 3 行
A
B
");
        assert_eq!(a, b, "只有数字变化时指纹必须相同");
        let c = output_fingerprint("耗时 120ms, 共 3 行
A
C
");
        assert_ne!(a, c, "内容变化必须改变指纹");
        // 行数变化也要能区分(短输出可能头部相同)
        let d = output_fingerprint("耗时 120ms, 共 3 行
A
B
B
");
        assert_ne!(a, d, "行数变化应改变指纹");
    }
}
