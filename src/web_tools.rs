//! web_search and web_fetch: the engine's own read-only web tools, which the Bots' research
//! rules rely on. Public http(s) only: a host that resolves to a loopback, private,
//! link-local or otherwise internal address is refused, and the connection is pinned to the
//! address that was checked, so the dotbot server and the local network stay out of reach.
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use pi_agent_core::{Tool, ToolResult};
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

pub const SEARCH: &str = "web_search";
pub const FETCH: &str = "web_fetch";
const AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const MAX_BODY: usize = 5 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

pub fn definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: SEARCH.into(),
            description: "Search the public web. Returns titles, links and snippets; open a result with web_fetch before relying on it.".into(),
            parameters: json!({"type":"object","properties":{
                "query":{"type":"string","description":"The search query, in the language the sources are likely written in"},
                "max_results":{"type":"integer","description":"1-10, default 8"}
            },"required":["query"]}),
            sequential: false,
        },
        Tool {
            name: FETCH.into(),
            description: "Read a public web page or text/JSON resource as plain text, with its title and main links. Long pages come in parts: pass start to continue where the previous part ended.".into(),
            parameters: json!({"type":"object","properties":{
                "url":{"type":"string","description":"An http(s) address"},
                "start":{"type":"integer","description":"Character offset to continue from, default 0"},
                "max_chars":{"type":"integer","description":"1000-60000, default 20000"}
            },"required":["url"]}),
            sequential: false,
        },
    ]
}

pub async fn run(name: &str, args: &Value) -> ToolResult {
    let result = match name {
        SEARCH => search(args).await,
        _ => fetch(args).await,
    };
    match result {
        Ok(text) => ToolResult { content: vec![json!({"type":"text","text":text})], is_error: false, details: Value::Null, terminate: false },
        Err(error) => ToolResult::error(format!("{error:#}")),
    }
}

fn global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_documentation() || v4.is_multicast() || a == 0 || (a == 100 && (64..128).contains(&b)) || (a == 198 && (18..20).contains(&b)) || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return global(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// A client bound to one checked public address of the URL's host.
async fn pinned(url: &reqwest::Url) -> Result<reqwest::Client> {
    ensure!(matches!(url.scheme(), "http" | "https"), "只能访问 http(s) 地址");
    ensure!(url.username().is_empty() && url.password().is_none(), "地址不能包含凭据");
    let host = url.host_str().context("地址缺少主机名")?.trim_matches(['[', ']']).to_string();
    let port = url.port_or_known_default().context("地址缺少端口")?;
    let addresses: Vec<SocketAddr> = tokio::time::timeout(Duration::from_secs(10), tokio::net::lookup_host((host.as_str(), port)))
        .await
        .context("域名解析超时")?
        .context("域名无法解析")?
        .collect();
    ensure!(!addresses.is_empty(), "域名没有地址");
    ensure!(addresses.iter().all(|a| global(a.ip())), "不能访问本机、局域网或内部地址");
    Ok(reqwest::Client::builder()
        .resolve(&host, addresses[0])
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent(AGENT)
        .build()?)
}

/// GET with redirects followed one hop at a time, each hop checked again.
async fn get(url: &str, form: Option<&[(&str, &str)]>) -> Result<(reqwest::Url, String, String)> {
    let mut url = reqwest::Url::parse(url.trim()).context("地址无效")?;
    for _ in 0..=MAX_REDIRECTS {
        let client = pinned(&url).await?;
        let request = match form {
            Some(form) => client.post(url.clone()).form(form),
            None => client.get(url.clone()),
        };
        let mut response = request.header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8").send().await.with_context(|| format!("无法连接 {}", url.host_str().unwrap_or_default()))?;
        if response.status().is_redirection() {
            let location = response.headers().get("location").and_then(|v| v.to_str().ok()).context("重定向缺少目标")?;
            url = url.join(location).context("重定向目标无效")?;
            continue;
        }
        ensure!(response.status().is_success(), "网站返回 HTTP {}", response.status());
        let kind = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.context("读取网页中断")? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_BODY {
                body.truncate(MAX_BODY);
                break;
            }
        }
        return Ok((url, kind, String::from_utf8_lossy(&body).into_owned()));
    }
    bail!("重定向次数过多")
}

fn decode_entities(text: &str) -> String {
    let named = regex::Regex::new(r"&(amp|lt|gt|quot|apos|nbsp|#39|#x[0-9a-fA-F]+|#[0-9]+);").unwrap();
    named
        .replace_all(text, |c: &regex::Captures| match &c[1] {
            "amp" => "&".to_string(),
            "lt" => "<".into(),
            "gt" => ">".into(),
            "quot" => "\"".into(),
            "apos" | "#39" => "'".into(),
            "nbsp" => " ".into(),
            code if code.starts_with("#x") => u32::from_str_radix(&code[2..], 16).ok().and_then(char::from_u32).map(String::from).unwrap_or_default(),
            code => code[1..].parse().ok().and_then(char::from_u32).map(String::from).unwrap_or_default(),
        })
        .into_owned()
}

fn strip_tags(html: &str) -> String {
    let tags = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    decode_entities(&tags.replace_all(html, "")).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Readable text of an HTML page: title, text with block structure, and its main links.
pub fn html_text(html: &str, base: &reqwest::Url) -> (String, String, Vec<(String, String)>) {
    let title = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap().captures(html).map(|c| strip_tags(&c[1])).unwrap_or_default();
    let html = regex::Regex::new(r"(?is)<(script|style|noscript|svg|template|iframe|head)\b.*?</(script|style|noscript|svg|template|iframe|head)>|<!--.*?-->").unwrap().replace_all(html, " ");
    let mut links = Vec::new();
    for c in regex::Regex::new(r#"(?is)<a\b[^>]*href\s*=\s*["']([^"'#]+)["'][^>]*>(.*?)</a>"#).unwrap().captures_iter(&html) {
        let text = strip_tags(&c[2]);
        if text.chars().count() < 2 || links.len() >= 40 {
            continue;
        }
        if let Ok(url) = base.join(&decode_entities(&c[1])) && matches!(url.scheme(), "http" | "https") && !links.iter().any(|(_, u): &(String, String)| u == url.as_str()) {
            links.push((text, url.to_string()));
        }
    }
    let html = regex::Regex::new(r"(?i)<li\b[^>]*>").unwrap().replace_all(&html, "\n- ");
    let html = regex::Regex::new(r"(?i)<(br|/p|/div|/h[1-6]|/tr|/li|/section|/article|/header|/footer|/pre|/blockquote|/table)\b[^>]*>").unwrap().replace_all(&html, "\n");
    let html = regex::Regex::new(r"(?i)<(h[1-6])\b[^>]*>").unwrap().replace_all(&html, "\n## ");
    let html = regex::Regex::new(r"(?i)</t[dh]>").unwrap().replace_all(&html, " | ");
    let text = decode_entities(&regex::Regex::new(r"(?s)<[^>]*>").unwrap().replace_all(&html, ""));
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() || line == "-" {
            if lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(line);
        }
    }
    (title, lines.join("\n").trim().to_string(), links)
}

async fn fetch(args: &Value) -> Result<String> {
    let url = args["url"].as_str().context("缺少 url")?;
    let start = args["start"].as_u64().unwrap_or(0) as usize;
    let max = args["max_chars"].as_u64().unwrap_or(20_000).clamp(1_000, 60_000) as usize;
    let (url, kind, body) = get(url, None).await?;
    let (title, text, links) = if kind.contains("html") || (kind.is_empty() && body.trim_start().starts_with('<')) {
        html_text(&body, &url)
    } else if kind.starts_with("text/") || kind.contains("json") || kind.contains("xml") || kind.contains("javascript") {
        (String::new(), body, vec![])
    } else {
        bail!("不支持的内容类型 {kind}：需要时用浏览器或桌面下载后处理");
    };
    let total = text.chars().count();
    let part: String = text.chars().skip(start).take(max).collect();
    let end = (start + max).min(total);
    let mut out = format!("URL: {url}\n");
    if !title.is_empty() {
        out.push_str(&format!("标题: {title}\n"));
    }
    out.push_str(&format!("内容（第 {start}–{end} 字，共 {total} 字）:\n{part}"));
    if end < total {
        out.push_str(&format!("\n\n[未完：用 start={end} 继续读取]"));
    }
    if start == 0 && !links.is_empty() {
        out.push_str("\n\n链接:\n");
        for (text, link) in links.iter().take(25) {
            out.push_str(&format!("- {} {link}\n", text.chars().take(80).collect::<String>()));
        }
    }
    Ok(out)
}

/// Bing wraps result links as /ck/a?…&u=a1<base64url of the address>.
fn bing_target(href: &str) -> String {
    let href = decode_entities(href);
    reqwest::Url::parse(&href)
        .ok()
        .filter(|u| u.host_str().is_some_and(|h| h.ends_with("bing.com")) && u.path().starts_with("/ck/"))
        .and_then(|u| u.query_pairs().find(|(k, _)| k == "u").map(|(_, v)| v.into_owned()))
        .and_then(|v| v.strip_prefix("a1").map(str::to_string))
        .and_then(|v| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(v.trim_end_matches('=')).ok())
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or(href)
}

pub fn parse_bing(html: &str) -> Vec<(String, String, String)> {
    let link = regex::Regex::new(r#"(?is)<h2[^>]*>\s*<a\b[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let snippet = regex::Regex::new(r"(?is)<p\b[^>]*>(.*?)</p>").unwrap();
    html.split(r#"class="b_algo""#)
        .skip(1)
        .filter_map(|block| {
            let block = block.split("</li>").next().unwrap_or(block);
            let c = link.captures(block)?;
            Some((strip_tags(&c[2]), bing_target(&c[1]), snippet.captures(block).map(|s| strip_tags(&s[1])).unwrap_or_default()))
        })
        .collect()
}

pub fn parse_duckduckgo(html: &str) -> Vec<(String, String, String)> {
    let link = regex::Regex::new(r#"(?is)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
    let snippet = regex::Regex::new(r#"(?is)class="result__snippet"[^>]*>(.*?)</a>"#).unwrap();
    html.split(r#"class="result__body"#)
        .skip(1)
        .filter_map(|block| {
            let c = link.captures(block)?;
            let href = decode_entities(&c[1]);
            let href = reqwest::Url::parse(&format!("https:{}", href.trim_start_matches("https:")))
                .ok()
                .and_then(|u| u.query_pairs().find(|(k, _)| k == "uddg").map(|(_, v)| v.into_owned()))
                .unwrap_or(href);
            Some((strip_tags(&c[2]), href, snippet.captures(block).map(|s| strip_tags(&s[1])).unwrap_or_default()))
        })
        .collect()
}

async fn search(args: &Value) -> Result<String> {
    let query = args["query"].as_str().map(str::trim).filter(|q| !q.is_empty()).context("缺少 query")?;
    let limit = args["max_results"].as_u64().unwrap_or(8).clamp(1, 10) as usize;
    let mut failures = Vec::new();
    let mut results = Vec::new();
    // A self-hosted SearXNG (DOTBOT_SEARXNG_URL) comes first when configured.
    if let Ok(base) = std::env::var("DOTBOT_SEARXNG_URL") && !base.trim().is_empty() {
        let url = format!("{}/search?format=json&q={}", base.trim_end_matches('/'), urlencoding::encode(query));
        match reqwest::Client::builder().timeout(Duration::from_secs(20)).build()?.get(url).send().await.and_then(|r| r.error_for_status()) {
            Ok(response) => {
                let value: Value = response.json().await.unwrap_or_default();
                results = value["results"].as_array().into_iter().flatten().map(|r| (r["title"].as_str().unwrap_or_default().to_string(), r["url"].as_str().unwrap_or_default().to_string(), r["content"].as_str().unwrap_or_default().to_string())).collect();
            }
            Err(error) => failures.push(format!("SearXNG: {error}")),
        }
    }
    // Bing now and then serves a page without the usual result markup; its other host often does.
    for host in ["www.bing.com", "cn.bing.com"] {
        if !results.is_empty() {
            break;
        }
        match get(&format!("https://{host}/search?q={}&count=10", urlencoding::encode(query)), None).await {
            Ok((_, _, html)) => results = parse_bing(&html),
            Err(error) => failures.push(format!("{host}: {error:#}")),
        }
    }
    if results.is_empty() {
        match get("https://html.duckduckgo.com/html/", Some(&[("q", query)])).await {
            Ok((_, _, html)) => results = parse_duckduckgo(&html),
            Err(error) => failures.push(format!("DuckDuckGo: {error:#}")),
        }
    }
    results.retain(|(_, url, _)| url.starts_with("http"));
    if results.is_empty() {
        bail!("搜索没有返回结果{}。换个关键词再试一次；仍然不行时改用已连接的搜索服务、浏览器或桌面。", if failures.is_empty() { String::new() } else { format!("（{}）", failures.join("；")) });
    }
    Ok(results
        .into_iter()
        .take(limit)
        .enumerate()
        .map(|(i, (title, url, snippet))| format!("{}. {title}\n   {url}\n   {snippet}", i + 1))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_addresses_are_refused() {
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "172.16.0.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1"] {
            assert!(!global(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "93.184.216.34", "2606:4700::1111"] {
            assert!(global(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn fetching_the_local_server_is_refused_before_connecting() {
        let error = fetch(&json!({"url":"http://127.0.0.1:4870/internal/tool"})).await.err().unwrap().to_string();
        assert!(error.contains("内部地址"), "{error}");
        let error = fetch(&json!({"url":"http://localhost/"})).await.err().unwrap().to_string();
        assert!(error.contains("内部地址"), "{error}");
        assert!(fetch(&json!({"url":"file:///etc/passwd"})).await.is_err());
    }

    #[test]
    fn pages_become_readable_text_with_links() {
        let base = reqwest::Url::parse("https://example.com/a/").unwrap();
        let (title, text, links) = html_text("<html><head><title>T &amp; U</title><style>x{}</style></head><body><h1>Head</h1><script>bad()</script><p>One&nbsp;<b>two</b></p><ul><li>a</li><li>b</li></ul><a href=\"/x\">Next page</a></body></html>", &base);
        assert_eq!(title, "T & U");
        assert!(text.contains("## Head") && text.contains("One two") && text.contains("- a") && !text.contains("bad()"), "{text}");
        assert_eq!(links, vec![("Next page".to_string(), "https://example.com/x".to_string())]);
    }

    #[test]
    fn search_results_parse_and_unwrap_redirects() {
        let target = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("https://rust-lang.org/");
        let bing = format!(r#"<li class="b_algo"><h2><a href="https://www.bing.com/ck/a?!&amp;p=1&amp;u=a1{target}&amp;ntb=1"><strong>Rust</strong> Language</a></h2><div class="b_caption"><p class="x">Fast &amp; safe</p></div></li>"#);
        assert_eq!(parse_bing(&bing), vec![("Rust Language".into(), "https://rust-lang.org/".into(), "Fast & safe".into())]);
        let ddg = r#"<div class="result__body"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&amp;rut=x">Tokio</a><a class="result__snippet" href="x">An async runtime</a></div>"#;
        assert_eq!(parse_duckduckgo(ddg), vec![("Tokio".into(), "https://tokio.rs/".into(), "An async runtime".into())]);
    }
}
