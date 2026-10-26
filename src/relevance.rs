//! Local recall combines bilingual task concepts and normalized text. It is a
//! bounded fallback, not an embedding service or evidence of task correctness.
use std::collections::HashSet;
pub fn terms(text: &str) -> HashSet<String> {
    let lower = text.to_lowercase();
    let mut out: HashSet<String> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.chars().count() > 1)
        .map(str::to_string)
        .collect();
    let chars: Vec<_> = lower.chars().collect();
    for pair in chars.windows(2) {
        if pair.iter().all(|c| ('\u{3400}'..='\u{9fff}').contains(c)) {
            out.insert(pair.iter().collect());
        }
    }
    for (concept, aliases) in [
        (
            "table",
            vec!["表格", "电子表", "excel", "spreadsheet", "xlsx", "csv"],
        ),
        ("export", vec!["导出", "输出文件", "export", "download"]),
        (
            "verify",
            vec![
                "核对",
                "校验",
                "验收",
                "验证",
                "检查",
                "verify",
                "validation",
                "check",
            ],
        ),
        (
            "document",
            vec!["文档", "报告", "document", "report", "word", "docx", "pdf"],
        ),
        (
            "research",
            vec!["调研", "研究", "资料检索", "research", "investigate"],
        ),
        (
            "image",
            vec!["生图", "图片", "图像", "image", "illustration"],
        ),
        ("video", vec!["视频", "影片", "video", "movie"]),
        (
            "code",
            vec![
                "编程", "代码", "开发", "程序", "code", "coding", "software", "develop",
            ],
        ),
        (
            "debug",
            vec!["排错", "修复", "报错", "故障", "debug", "fix", "error"],
        ),
        ("test", vec!["测试", "回归", "test", "regression"]),
        (
            "count",
            vec!["数量", "条数", "行数", "计数", "counts", "count", "rows"],
        ),
        (
            "web",
            vec![
                "网页",
                "页面",
                "浏览器",
                "网站",
                "browser",
                "website",
                "webpage",
            ],
        ),
        ("mail", vec!["邮件", "邮箱", "email", "gmail"]),
    ] {
        if aliases.iter().any(|a| {
            if a.is_ascii() {
                lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == *a)
            } else {
                lower.contains(a)
            }
        }) {
            out.insert(format!("@{concept}"));
        }
    }
    out
}
pub fn similarity(a: &str, b: &str) -> f64 {
    let a = terms(a);
    let b = terms(b);
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let weight = |s: &String| if s.starts_with('@') { 2.0 } else { 1.0 };
    let shared: f64 = a.intersection(&b).map(weight).sum();
    shared / (a.iter().map(weight).sum::<f64>() * b.iter().map(weight).sum::<f64>()).sqrt()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bilingual_concepts_recall_without_matching_words() {
        assert!(similarity("校验电子表导出条数", "verify spreadsheet export counts") > 0.45);
        assert_eq!(
            similarity("写一首诗", "verify spreadsheet export counts"),
            0.0
        );
    }
}
