// yxpil · BIT
// 幻觉防护：分词 + 重复检测。
// 分词管线与参考 JS 实现对齐：剔除符号 → 按空白分片 → 含中文的片段走 jieba
// 搜索粒度切词（cut_for_search），英文/数字整块保留 → 长度≥2 → 去重 → 长词在前。
// 检测目标：模型幻觉循环（同一个词刷屏）与工具死循环（重复调用工具 N 次）。
use jieba_rs::Jieba;
use std::collections::HashMap;
use std::sync::OnceLock;

fn jieba() -> &'static Jieba {
    static J: OnceLock<Jieba> = OnceLock::new();
    J.get_or_init(Jieba::new)
}

/// 去掉代码围栏内容：正规代码里重复关键字（function/def/if...）是常态，
/// 不剥离会把正常长代码误判为幻觉循环
fn strip_code_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 内部分词（保留重复项，供词频统计）：符号剔除 → 空白分片 → 中文片段切词/英文整块 → 长度≥2
fn tokenize(text: &str) -> Vec<String> {
    let cleaned: String = strip_code_fences(text)
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let mut out: Vec<String> = Vec::new();
    for frag in cleaned.split_whitespace() {
        if frag.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
            for w in jieba().cut_for_search(frag, true) {
                out.push(w.to_string());
            }
        } else {
            out.push(frag.to_string());
        }
    }
    out.into_iter().filter(|w| w.chars().count() >= 2).collect()
}

/// 搜索专用纯净分词：去重 + 长词在前（与参考实现 splitWords 等价）
#[cfg(test)]
pub fn split_words(text: &str) -> Vec<String> {
    let mut uniq: Vec<String> = tokenize(text);
    uniq.sort();
    uniq.dedup();
    uniq.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    uniq
}

/// 词频表：词 → 出现次数（含重复项）
pub fn word_counts(text: &str) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for w in tokenize(text) {
        *counts.entry(w).or_insert(0) += 1;
    }
    counts
}

/// 幻觉式重复检测：某词出现次数 ≥ max 即命中（max=0 关闭）。
/// 返回次数最多的 (词, 次数)；并列时取词更长（更反常）的那个。
pub fn find_repeat(text: &str, max: u32) -> Option<(String, usize)> {
    if max == 0 {
        return None;
    }
    word_counts(text)
        .into_iter()
        .filter(|(_, n)| *n >= max as usize)
        .max_by_key(|(w, n)| (*n, w.chars().count()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_words_basics() {
        // 符号剔除 + 中英混合 + 长词在前
        let ws = split_words("你好，世界！ hello world 123\n测试环境。");
        assert!(ws.contains(&"你好".to_string()) || ws.contains(&"世界".to_string()));
        assert!(ws.iter().all(|w| w.chars().count() >= 2));
        assert!(ws.contains(&"hello".to_string()));
        assert!(ws.contains(&"world".to_string()));
        assert!(ws.contains(&"123".to_string()));
        // 无重复且降序
        let sorted: Vec<usize> = ws.iter().map(|w| w.chars().count()).collect();
        let mut s2 = sorted.clone();
        s2.sort_by_key(|&x| std::cmp::Reverse(x));
        assert_eq!(sorted, s2);
        assert_eq!(ws.len(), ws.iter().collect::<std::collections::HashSet<_>>().len());
        // 单字被过滤
        assert!(!ws.iter().any(|w| w.chars().count() < 2));
    }

    #[test]
    fn split_words_empty_and_symbols() {
        assert!(split_words("").is_empty());
        assert!(split_words("。，！？!!!???").is_empty());
        assert!(split_words("a b c").is_empty()); // 全部单字符
    }

    #[test]
    fn find_repeat_detects_loop() {
        let text = "测试".repeat(25);
        let (w, n) = find_repeat(&text, 20).unwrap();
        assert_eq!(w, "测试");
        assert!(n >= 20);
    }

    #[test]
    fn find_repeat_normal_text_ok() {
        let text = "这是一段正常的回复，讨论了项目的架构设计、性能优化与测试方案。".repeat(3);
        assert!(find_repeat(&text, 20).is_none());
    }

    #[test]
    fn find_repeat_disabled_at_zero() {
        let text = "循环".repeat(100);
        assert!(find_repeat(&text, 0).is_none());
    }

    #[test]
    fn find_repeat_ignores_code_fences() {
        // 围栏内 30 个 function 属正常代码，不应命中
        let code = format!("```js\n{}```", "function a() {}\n".repeat(30));
        assert!(find_repeat(&code, 20).is_none());
    }

    #[test]
    fn find_repeat_counts_outside_fences() {
        let text = format!("说明开始。{}", "测试".repeat(25));
        let (w, n) = find_repeat(&text, 20).unwrap();
        assert_eq!(w, "测试");
        assert!(n >= 20);
    }

    #[test]
    fn word_counts_english_blocks() {
        let c = word_counts("the the the the cat sat");
        assert_eq!(c.get("the"), Some(&4));
        assert_eq!(c.get("cat"), Some(&1));
    }
}
