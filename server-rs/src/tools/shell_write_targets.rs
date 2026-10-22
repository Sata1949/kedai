// bash 命令写入目标解析(TMR3-L6,2026-10-22)。
//
// 背景:team 主 agent 的文件所有权(TMR-4)此前只拦 `fs_write`/`fs_edit`/`fs_patch`;
// bash 里的重定向、`tee`、`cp`/`mv`/`rm` 等同样能改工作区文件,却完全绕过所有权,
// 多主并行时仍是 last-writer-wins。本模块把命令文本里**能静态识别的写目标**抽出来,
// 交给 `ExecScope::reserve_write` 做「检查 + 认领」(调用方负责解析成工作区路径)。
//
// 边界(刻意保守,宁可不拦也不误拦):
//   - 只认 shell 层的显式写法;内联脚本(`node -e`/`python -c`/`powershell -Command`)
//     里调用的写 API 无法静态解析,仍不受所有权约束——这是 TMR3-L6 的已知边界;
//   - 含变量/通配符/命令替换/设备名的目标一律跳过;
//   - 这里只做**文本解析**;越界判定与规范化仍由 `workspace_guard` +
//     `ExecScope::reserve_write` 承担,不复制第二套路径规则。

/// 词法单元:普通词或 shell 运算符。
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Op(&'static str),
}

/// 从命令文本抽取「可能被写入的路径」(原样,未解析)。
///
/// 覆盖:输出重定向(`>`/`>>`/`&>`)、`tee`、`cp`/`copy`/`mv`/`move`/`install` 的目标、
/// `rm`/`del`/`erase`/`rmdir` 的删除目标、`sed -i` 的原地文件、`dd of=`、
/// `curl -o`/`wget -O` 的落盘文件。返回去重后的出现顺序;空 = 无可识别的写目标。
pub(crate) fn extract_write_targets(command: &str) -> Vec<String> {
    let tokens = strip_heredocs(tokenize(command));
    let mut targets = Vec::new();
    // 1) 重定向目标:操作符后紧跟的词。`dup`(2>&1)与 `<` 系列不是写目标。
    let mut i = 0;
    while i < tokens.len() {
        if matches!(tokens[i], Tok::Op(">") | Tok::Op(">>")) {
            if let Some(Tok::Word(w)) = tokens.get(i + 1) {
                push_target(&mut targets, w);
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    // 2) 命令参数里的写目标(按 `&&`/`||`/`|`/`;`/换行分段)
    for segment in segments(&tokens) {
        collect_command_targets(&segment, &mut targets);
    }
    targets
}

/// 词法扫描:引号内的 `>`/`|`/`;` 是普通字符;反斜杠只在双引号内转义。
///
/// 为什么不追求完整 shell 语法:本函数只服务所有权预检,解析失败的方向是
/// 「少拦一个目标」(既有行为),而不是「误杀合法命令」。
fn tokenize(command: &str) -> Vec<Tok> {
    #[derive(PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote = Quote::None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Quote::Single => {
                if c == '\'' {
                    quote = Quote::None;
                } else {
                    cur.push(c);
                }
            }
            Quote::Double => {
                if c == '"' {
                    quote = Quote::None;
                } else if c == '\\' {
                    if let Some(next) = chars.next() {
                        cur.push(next);
                    }
                } else {
                    cur.push(c);
                }
            }
            Quote::None => match c {
                '\'' => {
                    flush_word(&mut out, &mut cur);
                    quote = Quote::Single;
                }
                '"' => {
                    flush_word(&mut out, &mut cur);
                    quote = Quote::Double;
                }
                ' ' | '\t' | '\r' => flush_word(&mut out, &mut cur),
                '\n' => {
                    flush_word(&mut out, &mut cur);
                    out.push(Tok::Op("\n"));
                }
                '&' => {
                    flush_word(&mut out, &mut cur);
                    if chars.peek() == Some(&'&') {
                        chars.next();
                        out.push(Tok::Op("&&"));
                    } else {
                        out.push(Tok::Op("&"));
                    }
                }
                '|' => {
                    flush_word(&mut out, &mut cur);
                    if chars.peek() == Some(&'|') {
                        chars.next();
                        out.push(Tok::Op("||"));
                    } else {
                        out.push(Tok::Op("|"));
                    }
                }
                ';' => {
                    flush_word(&mut out, &mut cur);
                    out.push(Tok::Op(";"));
                }
                '>' => {
                    flush_word(&mut out, &mut cur);
                    if chars.peek() == Some(&'&') {
                        // `2>&1` / `>&2`:fd 复制,不是文件目标
                        chars.next();
                        while matches!(chars.peek(), Some(d) if d.is_ascii_digit()) {
                            chars.next();
                        }
                        out.push(Tok::Op("dup"));
                    } else {
                        if chars.peek() == Some(&'>') {
                            chars.next();
                            out.push(Tok::Op(">>"));
                        } else {
                            out.push(Tok::Op(">"));
                        }
                        // `>| file` 的 `|` 属于重定向符,不能当管道分隔
                        if chars.peek() == Some(&'|') {
                            chars.next();
                        }
                    }
                }
                '<' => {
                    flush_word(&mut out, &mut cur);
                    if chars.peek() == Some(&'<') {
                        chars.next();
                        if chars.peek() == Some(&'<') {
                            chars.next();
                            out.push(Tok::Op("<<<"));
                        } else {
                            out.push(Tok::Op("<<"));
                        }
                    } else {
                        out.push(Tok::Op("<"));
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    flush_word(&mut out, &mut cur);
    out
}

fn flush_word(out: &mut Vec<Tok>, cur: &mut String) {
    if !cur.is_empty() {
        out.push(Tok::Word(std::mem::take(cur)));
    }
}

/// 丢弃 heredoc 正文:正文里的 `rm x` 只是要写入文件的内容,不是命令。
///
/// 终止行按整个词相等判定(与 shell 的 `<<DELIM` 语义一致);heredoc 与输出重定向
/// 可以同处一行(`cat <<EOF > a.txt`),故分隔符**之后、本行换行之前**的 token
/// (如 `> a.txt`)必须保留,只丢正文。终止行后补一个换行 token,保证
/// `cat > a <<EOF ... EOF\necho b > c` 两段命令不被粘连。
fn strip_heredocs(tokens: Vec<Tok>) -> Vec<Tok> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == Tok::Op("<<") {
            // 找分隔符(跳过 `<<` 与分隔符之间无意义的 token)
            let mut j = i + 1;
            let mut delim: Option<String> = None;
            while j < tokens.len() {
                match &tokens[j] {
                    Tok::Word(w) => {
                        delim = Some(w.trim_start_matches('-').to_string());
                        break;
                    }
                    Tok::Op("\n") => break,
                    _ => j += 1,
                }
            }
            let Some(delim) = delim else {
                i = j + 1;
                continue;
            };
            // 本行剩余 token(分隔符之后到换行)是同一命令的其它重定向,必须保留
            let line_end = tokens[j + 1..]
                .iter()
                .position(|t| *t == Tok::Op("\n"))
                .map(|p| j + 1 + p);
            let body_start = match line_end {
                Some(e) => {
                    out.extend_from_slice(&tokens[j + 1..e]);
                    out.push(Tok::Op("\n"));
                    e + 1
                }
                // 没有换行 = 没有正文(非法/截断输入),按普通 token 处理
                None => {
                    out.extend_from_slice(&tokens[j + 1..]);
                    break;
                }
            };
            // 找终止行(整词等于分隔符,且后面是换行/结束)
            let mut k = body_start;
            while k < tokens.len() {
                if let Tok::Word(w) = &tokens[k] {
                    if w == &delim && matches!(tokens.get(k + 1), None | Some(Tok::Op("\n"))) {
                        k += 1;
                        if tokens.get(k) == Some(&Tok::Op("\n")) {
                            k += 1;
                        }
                        break;
                    }
                }
                k += 1;
            }
            out.push(Tok::Op("\n"));
            i = k;
            continue;
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    out
}

/// 按命令分隔符切段;重定向操作符与其目标词不进参数列表。
fn segments(tokens: &[Tok]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Tok::Word(w) => {
                // `2>file` 的 fd 数字不是参数
                let is_fd = matches!(w.as_str(), "1" | "2");
                let next_redirect = matches!(
                    tokens.get(i + 1),
                    Some(Tok::Op(">")) | Some(Tok::Op(">>")) | Some(Tok::Op("dup"))
                );
                if !(is_fd && next_redirect) {
                    cur.push(w.clone());
                }
                i += 1;
            }
            Tok::Op(op) if is_redirect(op) => {
                // 跳过操作符与紧随的目标词(`<` 的目标是输入文件,同样不属于参数)
                i += if matches!(tokens.get(i + 1), Some(Tok::Word(_))) {
                    2
                } else {
                    1
                };
            }
            Tok::Op(op) if is_separator(op) => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                i += 1;
            }
            Tok::Op(_) => i += 1,
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_redirect(op: &str) -> bool {
    matches!(op, ">" | ">>" | "dup" | "<" | "<<" | "<<<")
}

fn is_separator(op: &str) -> bool {
    matches!(op, "&&" | "||" | "|" | ";" | "&" | "\n")
}

/// 每个命令段的写目标(命令名大小写不敏感,Windows 的 `.exe` 后缀剥掉)。
fn collect_command_targets(words: &[String], out: &mut Vec<String>) {
    let Some(head) = words.first() else {
        return;
    };
    let base = head
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(head)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    let args = &words[1..];
    // Windows 原生命令用 `/flag`;POSIX 命令用 `-flag`。两种都按平台语义过滤,
    // 不混用(否则 `rm /tmp/x` 的绝对路径会被当成 flag 丢掉)。
    let win_flags = matches!(
        base.as_str(),
        "copy" | "move" | "del" | "erase" | "rd" | "rmdir"
    );
    let is_flag = |s: &String| s.starts_with('-') || (win_flags && s.starts_with('/'));
    let non_flags: Vec<&String> = args.iter().filter(|a| !is_flag(a)).collect();
    match base.as_str() {
        "tee" => {
            for a in non_flags {
                push_target(out, a);
            }
        }
        // 目标 = 最后一个非 flag 参数;多源复制时其余是输入,不该拦
        "cp" | "copy" | "mv" | "move" | "install" => {
            if let Some(dst) = non_flags.last() {
                push_target(out, dst);
            }
        }
        "rm" | "del" | "erase" | "unlink" | "truncate" | "touch" => {
            for a in non_flags {
                push_target(out, a);
            }
        }
        "rd" | "rmdir" => {
            for a in non_flags {
                push_target(out, a);
            }
        }
        "sed" => {
            // 只有 `-i`(含 `-i.bak`)才是原地写;普通 sed 是只读
            let in_place = args
                .iter()
                .any(|a| a == "-i" || a.starts_with("-i.") || a.starts_with("--in-place"));
            if in_place {
                if let Some(file) = non_flags.last() {
                    push_target(out, file);
                }
            }
        }
        "dd" => {
            for a in args {
                if let Some(raw) = a.strip_prefix("of=") {
                    push_target(out, raw);
                }
            }
        }
        "curl" | "wget" => {
            // 落盘参数:`curl -o f` / `wget -O f` / `--output f`
            let mut i = 0;
            while i < args.len() {
                let a = &args[i];
                if matches!(a.as_str(), "-o" | "-O" | "--output") {
                    if let Some(v) = args.get(i + 1) {
                        push_target(out, v);
                    }
                    i += 2;
                    continue;
                }
                i += 1;
            }
        }
        _ => {}
    }
}

/// 过滤明显不是「工作区文件目标」的词,并去重。
fn push_target(out: &mut Vec<String>, raw: &str) {
    let t = raw.trim();
    if t.is_empty() {
        return;
    }
    let lower = t.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "nul" | "con" | "/dev/null" | "&1" | "&2" | "-" | "--"
    ) {
        return;
    }
    // 变量/通配符/命令替换/花括号展开:文本解析不可靠,跳过
    if t.chars()
        .any(|c| matches!(c, '$' | '%' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '~'))
    {
        return;
    }
    // URL 不是路径;目录目标(`cp a dir/`)无法映射成文件键
    if t.contains("://") || t.ends_with('/') || t.ends_with('\\') {
        return;
    }
    if !out.iter().any(|x| x == t) {
        out.push(t.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::extract_write_targets;

    fn targets(command: &str) -> Vec<String> {
        extract_write_targets(command)
    }

    #[test]
    fn redirects_and_quotes() {
        assert_eq!(targets("echo hi > out.txt"), vec!["out.txt"]);
        assert_eq!(targets("echo hi>>'dir/out.txt'"), vec!["dir/out.txt"]);
        assert_eq!(
            targets("cmd1 && cmd2 > a.txt"),
            vec!["a.txt"],
            "分段不丢重定向目标"
        );
        assert_eq!(
            targets("echo \"a > b\" > c.txt"),
            vec!["c.txt"],
            "引号里的 > 不是重定向"
        );
        assert!(
            targets("cmd 2>&1 > out.log").contains(&"out.log".to_string()),
            "fd 复制不应污染目标"
        );
    }

    #[test]
    fn heredoc_body_is_not_a_command() {
        let cmd = "cat > a.txt <<'EOF'\nrm b.txt\nEOF\necho tail > c.txt\n";
        assert_eq!(
            targets(cmd),
            vec!["a.txt", "c.txt"],
            "heredoc 正文里的 rm 不是命令;终止行后命令仍要拆开"
        );
        // heredoc 与输出重定向同处一行:分隔符之后的 `> a.txt` 必须保留(另一常见写法)
        assert_eq!(
            targets("cat <<EOF > a.txt\nbody > x.txt\nEOF\n"),
            vec!["a.txt"],
            "heredoc 同行重定向不能被正文跳过逻辑吞掉"
        );
    }

    #[test]
    fn common_writers() {
        assert_eq!(targets("tee -a log.txt"), vec!["log.txt"]);
        assert_eq!(targets("cp -r src a.txt"), vec!["a.txt"]);
        assert_eq!(
            targets("cp a.txt b.txt"),
            vec!["b.txt"],
            "多源复制的目标是最后一个参数"
        );
        assert_eq!(targets("rm -rf tmp.txt"), vec!["tmp.txt"]);
        assert_eq!(
            targets("sed -i 's/a/b/' file.txt"),
            vec!["file.txt"],
            "sed -i 是原地写"
        );
        assert!(targets("sed 's/a/b/' file.txt").is_empty(), "普通 sed 只读");
        assert_eq!(targets("dd if=a of=out.bin"), vec!["out.bin"]);
        assert_eq!(targets("curl -o out.zip https://x/y"), vec!["out.zip"]);
    }

    #[test]
    fn skips_unreliable_targets() {
        assert!(targets("echo hi > $OUT").is_empty(), "变量目标跳过");
        assert!(targets("echo hi > *.log").is_empty(), "通配目标跳过");
        assert!(targets("echo hi > nul").is_empty(), "空设备跳过");
        assert!(targets("cp a dir/").is_empty(), "目录目标跳过");
        assert!(targets("git status").is_empty());
        assert!(targets("node -e \"fs.writeFileSync('a.txt')\"").is_empty());
    }

    #[test]
    fn windows_copy_flags_are_not_targets() {
        assert_eq!(targets("copy /y a.txt b.txt"), vec!["b.txt"]);
        assert_eq!(targets("del /q a.txt"), vec!["a.txt"]);
    }
}
