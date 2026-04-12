//! Fuzzy natural-language → local tools (offline).

use crate::tools::{
    change_dir_path, is_allowlisted_shell_program, read_file_path, run_allowlisted_shell,
    ListDirTool, MkdirTool, Tool, ToolResult,
    DEFAULT_READ_MAX_BYTES,
};

enum Action {
    List(String),
    Mkdir(String),
    Cd(String),
    Read(String),
    Shell(String),
}

const LIST_CN: &[&str] = &[
    "看看", "看下", "瞧瞧", "瞅瞅", "列一下", "列出", "列出来", "展示", "打开看看",
    "有啥", "有什么", "有哪些", "都有啥", "文件夹", "底下", "里面", "下边",
    "当前", "这里", "这边", "本目录", "浏览", "查下", "查一下", "看下目录", "列目录",
    "查询", "list", "ls", "dir",
];

const LIST_EN: &[&str] = &[
    "list", "show", "what's in", "whats in", "what is in", "directory", "folder",
    "files in", "look at", "see what's", "display",
];

const MKDIR_CN: &[&str] = &[
    "创建", "新建", "建立", "建个", "建一个", "弄一个", "搞个", "建目录", "建文件夹",
    "新建文件夹", "新建目录", "mkdir", "目录叫", "文件夹叫",
];

const MKDIR_EN: &[&str] = &[
    "mkdir", "create folder", "create directory", "make folder", "make directory",
    "new folder", "new directory",
];

fn score_list(s: &str) -> i32 {
    let lower = s.to_lowercase();
    let mut sc = 0;
    for kw in LIST_CN {
        if s.contains(kw) {
            sc += 2;
        }
    }
    for kw in LIST_EN {
        if lower.contains(kw) {
            sc += 2;
        }
    }
    if s.contains("文件") && !s.contains("读文件") && !s.contains("查看文件") {
        sc += 1;
    }
    if s.contains("显示") && !s.contains("显示内容") {
        sc += 2;
    }
    sc
}

fn score_mkdir(s: &str) -> i32 {
    let lower = s.to_lowercase();
    let mut sc = 0;
    for kw in MKDIR_CN {
        if s.contains(kw) {
            sc += 2;
        }
    }
    for kw in MKDIR_EN {
        if lower.contains(kw) {
            sc += 2;
        }
    }
    sc
}

fn score_cd(s: &str) -> i32 {
    let lower = s.to_lowercase();
    let mut sc = 0;
    if lower.starts_with("cd ") || lower == "cd" {
        sc += 6;
    } else if lower.contains(" cd ") {
        sc += 2;
    }
    if lower.starts_with("chdir ") {
        sc += 6;
    }
    if s.contains("切换到") {
        sc += 4;
    }
    if s.contains("进入") {
        sc += 3;
    }
    sc
}

fn score_shell(s: &str) -> i32 {
    let lower = s.to_lowercase();
    let mut sc = 0;
    if s.contains("日期")
        || (s.contains("时间")
            && (s.contains("几") || s.contains("现在") || s.contains("啥") || s.contains("什么")))
    {
        sc += 4;
    }
    if s.contains("现在几点") || s.contains("几点了") || s.contains("几点钟") {
        sc += 5;
    }
    if s.contains("今天") && (s.contains("几号") || s.contains("几月") || s.contains("哪天")) {
        sc += 5;
    }
    if s.contains("星期几") || s.contains("周几") {
        sc += 4;
    }
    if let Some(first) = lower.split_whitespace().next() {
        let bin = first.strip_prefix("./").unwrap_or(first);
        if is_allowlisted_shell_program(bin) {
            sc += 6;
        }
    }
    sc
}

fn score_read(s: &str) -> i32 {
    let lower = s.to_lowercase();
    let mut sc = 0;
    if lower.starts_with("cat ") {
        sc += 6;
    }
    if s.contains("查看文件") || s.contains("读文件") {
        sc += 5;
    }
    if s.contains("读一下") || s.contains("打开文件") || s.contains("显示内容") {
        sc += 3;
    }
    if s.contains("文件内容") {
        sc += 4;
    }
    if lower.starts_with("type ") {
        sc += 5;
    }
    if lower.contains("read file") || lower.contains("show content") {
        sc += 4;
    }
    sc
}

fn extract_quoted(s: &str) -> Option<String> {
    let t = s.trim();
    if t.len() >= 2 {
        let ch0 = t.chars().next()?;
        let ch1 = t.chars().last()?;
        if (ch0 == '"' && ch1 == '"') || (ch0 == '\'' && ch1 == '\'') {
            let inner: String = t.chars().skip(1).take(t.chars().count().saturating_sub(2)).collect();
            let inner = inner.trim();
            if !inner.is_empty() {
                return Some(inner.into());
            }
        }
        if t.starts_with('「') && t.ends_with('」') && t.len() > 2 {
            return Some(t.chars().skip(1).take(t.chars().count() - 2).collect());
        }
    }
    None
}

fn extract_list_path(s: &str) -> String {
    if let Some(q) = extract_quoted(s) {
        return q;
    }
    let no_slash = !s.contains('/');
    if no_slash
        && (s.contains("当前")
            || s.contains("这里")
            || s.contains("这边")
            || s.contains("本目录")
            || s.contains("有啥")
            || s.contains("有什么")
            || s.contains("有哪些")
            || s.contains("底下")
            || s.contains("里面"))
    {
        return ".".into();
    }
    if let Some(pos) = s.find('/') {
        let rest = s[pos..]
            .split_whitespace()
            .next()
            .unwrap_or(".")
            .trim_end_matches(|c: char| "，。！？".contains(c));
        if !rest.is_empty() {
            return rest.into();
        }
    }
    if let Some(idx) = s.find('在') {
        let after = s[idx + '在'.len_utf8()..].trim();
        let first: String = after
            .chars()
            .take_while(|c| !c.is_whitespace() && !"里中下的下面".contains(*c))
            .collect();
        let first = first.trim_end_matches(|c: char| "，。！？".contains(c));
        if !first.is_empty() && first.len() < 512 {
            return first.into();
        }
    }
    let lower = s.to_lowercase();
    let rest = s.trim();
    for prefix in ["ls ", "list ", "dir "] {
        if lower.starts_with(prefix) {
            let tail = rest[prefix.len()..].trim();
            if !tail.is_empty() {
                return tail.split_whitespace().next().unwrap_or(".").into();
            }
            return ".".into();
        }
    }
    if lower == "ls" || lower == "list" || lower == "dir" {
        return ".".into();
    }
    let tokens: Vec<&str> = s.split_whitespace().collect();
    for t in tokens {
        let t = t.trim_end_matches(|c: char| "，。！？".contains(c));
        if t == "." || t == ".." {
            return t.into();
        }
        let ascii_pathish = !t.is_empty()
            && t.len() <= 255
            && t.chars()
                .all(|c| c.is_ascii() && (c.is_alphanumeric() || c == '_' || c == '-' || c == '.'));
        if ascii_pathish && score_list(s) > score_mkdir(s) {
            return t.into();
        }
    }
    ".".into()
}

fn trim_path_token(p: &str) -> &str {
    p.trim_end_matches(|c: char| "，。！？吧呢的".contains(c))
}

fn extract_cd_path(s: &str) -> Option<String> {
    if let Some(q) = extract_quoted(s) {
        return Some(q);
    }
    let lower = s.to_lowercase();
    let rest = s.trim();
    if lower.starts_with("cd ") {
        let tail = rest["cd ".len()..].trim();
        let p = tail.split_whitespace().next()?;
        let p = trim_path_token(p);
        return (!p.is_empty()).then(|| p.into());
    }
    if lower.starts_with("chdir ") {
        let tail = rest["chdir ".len()..].trim();
        let p = tail.split_whitespace().next()?;
        let p = trim_path_token(p);
        return (!p.is_empty()).then(|| p.into());
    }
    for key in ["切换到", "进入"] {
        if let Some(i) = s.find(key) {
            let after = s[i + key.len()..].trim();
            let p = after.split_whitespace().next()?;
            let p = trim_path_token(p);
            if !p.is_empty() {
                return Some(p.into());
            }
        }
    }
    None
}

fn extract_read_path(s: &str) -> Option<String> {
    if let Some(q) = extract_quoted(s) {
        return Some(q);
    }
    let lower = s.to_lowercase();
    let rest = s.trim();
    if lower.starts_with("cat ") {
        let tail = rest["cat ".len()..].trim();
        let p = tail.split_whitespace().next()?;
        let p = trim_path_token(p);
        return (!p.is_empty()).then(|| p.into());
    }
    if lower.starts_with("type ") {
        let tail = rest["type ".len()..].trim();
        let p = tail.split_whitespace().next()?;
        let p = trim_path_token(p);
        return (!p.is_empty()).then(|| p.into());
    }
    for key in ["查看文件", "读文件", "读一下", "打开文件", "文件内容"] {
        if let Some(i) = s.find(key) {
            let after = s[i + key.len()..].trim();
            if let Some(p) = after.split_whitespace().next() {
                let p = trim_path_token(p);
                if !p.is_empty() {
                    return Some(p.into());
                }
            }
        }
    }
    None
}

fn extract_mkdir_name(s: &str) -> Option<String> {
    if let Some(q) = extract_quoted(s) {
        return sanitize_name(&q);
    }
    let patterns_zh = ["叫做", "名为", "名字叫", "叫", "名：", "文件夹", "目录"];
    for pat in patterns_zh {
        if let Some(i) = s.find(pat) {
            let after = s[i + pat.len()..].trim();
            if let Some(tok) = after.split_whitespace().next() {
                let tok = tok.trim_end_matches(|c: char| "，。！？的".contains(c));
                if let Some(n) = sanitize_name(tok) {
                    return Some(n);
                }
            }
        }
    }
    let lower = s.to_lowercase();
    for prefix in ["mkdir ", "create folder ", "make folder ", "new folder "] {
        if lower.starts_with(prefix) {
            let tail = s[prefix.len()..].trim();
            if let Some(tok) = tail.split_whitespace().next() {
                if let Some(n) = sanitize_name(tok) {
                    return Some(n);
                }
            }
        }
    }
    for prefix in ["创建", "新建", "建立", "建个", "建一个", "弄一个", "搞个"] {
        if let Some(i) = s.find(prefix) {
            let after = s[i + prefix.len()..].trim();
            if let Some(tok) = after.split_whitespace().next() {
                let tok = tok.trim_end_matches(|c: char| "，。！？吧呢的".contains(c));
                if let Some(n) = sanitize_name(tok) {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn sanitize_name(raw: &str) -> Option<String> {
    let t = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    if t.is_empty() || t.contains('/') || t.contains("..") {
        return None;
    }
    if !t.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return None;
    }
    Some(t.into())
}

fn extract_shell_command(s: &str) -> Option<String> {
    let t = s.trim();
    let lower = t.to_lowercase();
    if let Some(first) = lower.split_whitespace().next() {
        let bin = first.strip_prefix("./").unwrap_or(first);
        if is_allowlisted_shell_program(bin) {
            return Some(t.to_string());
        }
    }
    if t.contains("日期")
        || t.contains("现在几点")
        || t.contains("几点了")
        || t.contains("几点钟")
        || (t.contains("今天") && (t.contains("几号") || t.contains("几月") || t.contains("哪天")))
        || t.contains("星期几")
        || t.contains("周几")
        || (t.contains("时间")
            && (t.contains("几") || t.contains("现在") || t.contains("啥") || t.contains("什么")))
    {
        return Some("date".into());
    }
    None
}

fn classify(line: &str) -> Option<Action> {
    let s = line.trim();
    if s.is_empty() {
        return None;
    }
    let mut scored = [
        ("list", score_list(s)),
        ("mkdir", score_mkdir(s)),
        ("cd", score_cd(s)),
        ("read", score_read(s)),
        ("shell", score_shell(s)),
    ];
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    if scored[0].1 < 2 {
        return None;
    }
    if scored.len() > 1 && scored[1].1 >= 2 && scored[0].1 <= scored[1].1 + 1 {
        return None;
    }
    match scored[0].0 {
        "list" => Some(Action::List(extract_list_path(s))),
        "mkdir" => {
            if let Some(name) = extract_mkdir_name(s) {
                return Some(Action::Mkdir(name));
            }
            if let Some(last) = s.split_whitespace().last() {
                let last = last.trim_end_matches(|c: char| "，。！？吧呢的".contains(c));
                if let Some(n) = sanitize_name(last) {
                    return Some(Action::Mkdir(n));
                }
            }
            None
        }
        "cd" => extract_cd_path(s).map(Action::Cd),
        "read" => extract_read_path(s).map(Action::Read),
        "shell" => extract_shell_command(s).map(Action::Shell),
        _ => None,
    }
}

/// Offline: fuzzy match → tool output, or `None` if not understood.
pub fn dispatch_fuzzy_offline(line: &str) -> Option<String> {
    let action = classify(line)?;
    let list = ListDirTool;
    let mkdir = MkdirTool;
    let r: ToolResult = match action {
        Action::List(p) => list.execute(&p),
        Action::Mkdir(n) => mkdir.execute(&n),
        Action::Cd(p) => change_dir_path(&p),
        Action::Read(p) => read_file_path(&p, DEFAULT_READ_MAX_BYTES),
        Action::Shell(cmd) => run_allowlisted_shell(&cmd),
    };
    Some(r.to_tool_message_content())
}
