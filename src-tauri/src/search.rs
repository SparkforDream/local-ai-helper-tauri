// ============================================================
// search.rs —— 分层联网检索与小白安全过筛闸门
//
// 设计原则：
//   1. 双轨可切换（off / builtin / bocha / tavily），默认 builtin 零配置可用：
//      - Windows 报错/蓝屏/DLL 类优先直查微软官方 Learn Search JSON API；
//      - 通用问题走国内直连的 cn.bing.com 精简解析（跳过广告块）；
//      - 专业扩展轨支持博查（Bocha）与 Tavily，API Key 仅通过 stdin 传给 curl。
//   2. 强制安全过筛闸门：
//      - 任何引擎返回的结果都必须经过域名与关键词黑名单过滤，
//        下载站、低成本仿冒顶级域、高速下载器、破解/绿色版链接一律剥离；
//      - 仅把过滤后的前 N 条短摘要（总计 <= ~1100 字）注给小模型。
//   3. 失败静默降级：断网、超时或反爬时不报错阻塞对话。
// ============================================================

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static SEARCH_SEQ: AtomicU64 = AtomicU64::new(1);

const DESKTOP_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Off,
    Builtin,
    Bocha,
    Tavily,
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Builtin
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    #[serde(default)]
    pub mode: SearchMode,
    #[serde(default)]
    pub bocha_api_key: String,
    #[serde(default)]
    pub tavily_api_key: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_timeout_secs() -> u64 {
    8
}

fn default_max_results() -> usize {
    4
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            mode: SearchMode::Builtin,
            bocha_api_key: String::new(),
            tavily_api_key: String::new(),
            timeout_secs: default_timeout_secs(),
            max_results: default_max_results(),
        }
    }
}

/// 返回给前端的配置视图：隐藏明文 Key，仅暴露掩码。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfigView {
    pub mode: SearchMode,
    pub has_bocha_key: bool,
    pub bocha_key_masked: String,
    pub has_tavily_key: bool,
    pub tavily_key_masked: String,
    pub timeout_secs: u64,
    pub max_results: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfigInput {
    pub mode: SearchMode,
    /// None 保留旧 Key；Some("") 清空；Some(val) 覆盖
    #[serde(default)]
    pub bocha_api_key: Option<String>,
    #[serde(default)]
    pub tavily_api_key: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub title: String,
    pub url: String,
    pub domain: String,
    pub snippet: String,
    pub trust: String,
    pub engine: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOutcome {
    pub ok: bool,
    pub engine: String,
    pub items: Vec<SearchItem>,
    pub blocked_count: usize,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn mask_key(k: &str) -> String {
    let t = k.trim();
    if t.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = t.chars().collect();
    if chars.len() <= 8 {
        return "****".into();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}****{tail}")
}

pub fn to_view(cfg: &SearchConfig) -> SearchConfigView {
    SearchConfigView {
        mode: cfg.mode,
        has_bocha_key: !cfg.bocha_api_key.trim().is_empty(),
        bocha_key_masked: mask_key(&cfg.bocha_api_key),
        has_tavily_key: !cfg.tavily_api_key.trim().is_empty(),
        tavily_key_masked: mask_key(&cfg.tavily_api_key),
        timeout_secs: cfg.timeout_secs.clamp(3, 20),
        max_results: cfg.max_results.clamp(1, 6),
    }
}

pub fn merge_config(existing: &SearchConfig, input: SearchConfigInput) -> SearchConfig {
    let bocha_api_key = match input.bocha_api_key {
        None => existing.bocha_api_key.clone(),
        Some(v) => v.trim().to_string(),
    };
    let tavily_api_key = match input.tavily_api_key {
        None => existing.tavily_api_key.clone(),
        Some(v) => v.trim().to_string(),
    };
    SearchConfig {
        mode: input.mode,
        bocha_api_key,
        tavily_api_key,
        timeout_secs: input.timeout_secs.clamp(3, 20),
        max_results: input.max_results.clamp(1, 6),
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("local-ai-helper").join("search.json")
}

pub fn load_config() -> SearchConfig {
    let p = config_path();
    let Ok(raw) = fs::read_to_string(&p) else {
        return SearchConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_config(cfg: &SearchConfig) -> Result<(), String> {
    let p = config_path();
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化失败：{e}"))?;
    fs::write(&p, json).map_err(|e| format!("写入检索配置失败：{e}"))
}

// ============================================================
// 安全过筛闸门（对齐 trust.js 与 official.yaml）
// ============================================================

const BLOCKED_DOMAINS: &[&str] = &[
    "onlinedown.net",
    "pc6.com",
    "crsky.com",
    "downza.cn",
    "skycn.com",
    "mydown.com",
    "softonic.com",
    "zol.com.cn",
    "pconline.com.cn",
    "xitongzhijia.net",
    "xitongtiandi.net",
    "51xiazai.cn",
    "downcc.com",
    "cr173.com",
    "uzzf.com",
    "ddooo.com",
    "xpgod.com",
    "9ht.com",
    "kkx.net",
    "pcsoft.com.cn",
    "2345.com",
    "360doc.com",
];

const ABUSED_TLDS: &[&str] = &[
    "top", "xyz", "buzz", "click", "download", "site", "online", "store", "shop", "vip", "work",
    "live", "fun", "icu", "cyou", "rest", "biz", "info", "link", "host", "press", "space",
    "website", "tech", "life", "world", "today", "sale", "deal", "bid",
];

const SUSPECT_HOST_SUBSTR: &[&str] = &[
    "xiazai",
    "guanwang",
    "zhengban",
    "zhongwen",
    "mianfei",
    "pojie",
    "crack",
    "keygen",
    "pcsoft",
];

const BLOCKED_TEXT_CUES: &[&str] = &[
    "高速下载器",
    "破解版",
    "绿色版",
    "去广告版",
    "全家桶",
    "一键重装",
    "驱动精灵",
    "驱动人生",
    "免费激活",
    "下载站",
];

const AUTHORITATIVE_DOMAINS: &[&str] = &[
    "learn.microsoft.com",
    "support.microsoft.com",
    "microsoft.com",
    "github.com",
    "nvidia.cn",
    "nvidia.com",
    "amd.com",
    "intel.cn",
    "intel.com",
    "huorong.cn",
    "7-zip.org",
    "store.steampowered.com",
    "wps.cn",
    "weixin.qq.com",
    "im.qq.com",
    "dingtalk.com",
    "feishu.cn",
];

pub fn extract_host(url: &str) -> String {
    let trimmed = url.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host_port = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    host_port.split(':').next().unwrap_or("").to_string()
}

fn host_matches_list(host: &str, list: &[&str]) -> bool {
    list.iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// 检查单条检索结果是否安全可用；命中高危下载站/广告跳转/破解关键词返回 Err(原因)。
pub fn check_result_safety(title: &str, url: &str, snippet: &str) -> Result<(String, String), &'static str> {
    let u = url.trim();
    if !(u.starts_with("https://") || u.starts_with("http://")) {
        return Err("非 HTTP 链接");
    }
    let u_low = u.to_lowercase();
    if u_low.contains("bing.com/aclick")
        || u_low.contains("baidu.com/baidu.php")
        || u_low.contains("googleadservices.com")
    {
        return Err("广告跳转链接");
    }

    let host = extract_host(u);
    if host.is_empty() || !host.contains('.') {
        return Err("无效域名");
    }
    if host_matches_list(&host, BLOCKED_DOMAINS) {
        return Err("黑名单下载站");
    }

    let parts: Vec<&str> = host.split('.').collect();
    let tld = parts.last().copied().unwrap_or("");
    let sld = if parts.len() >= 2 {
        parts[parts.len() - 2]
    } else {
        host.as_str()
    };

    if ABUSED_TLDS.contains(&tld) {
        return Err("低成本高风险顶级域");
    }
    if SUSPECT_HOST_SUBSTR.iter().any(|s| sld.contains(s)) {
        return Err("域名含高风险下载/仿冒词");
    }

    let combined = format!("{title} {snippet}");
    if BLOCKED_TEXT_CUES.iter().any(|c| combined.contains(c)) {
        return Err("标题或摘要含高危捆绑/破解词");
    }

    let trust = if host_matches_list(&host, AUTHORITATIVE_DOMAINS)
        || tld == "gov"
        || tld == "edu"
        || host.ends_with(".gov.cn")
        || host.ends_with(".edu.cn")
    {
        "官方/权威".to_string()
    } else {
        "普通来源".to_string()
    };

    Ok((host, trust))
}

// ============================================================
// 按需触发判断（避免对本地操作指令或短寒暄无谓联网）
// ============================================================

pub fn should_search_for_goal(goal: &str) -> bool {
    let t = goal.trim();
    if t.chars().count() < 4 {
        return false;
    }
    let low = t.to_lowercase();
    // 明确本机文件/系统操作指令不联网
    let local_cmd_cues = [
        "建一个", "建个", "新建", "创建文件夹", "创建目录", "删掉", "删除空", "删了",
        "打开任务管理器", "打开存储设置", "打开网络设置", "打开系统更新",
        "存在吗", "还在吗", "有多大", "占多大",
    ];
    if local_cmd_cues.iter().any(|c| low.contains(c)) {
        return false;
    }
    true
}

fn is_windows_tech_query(q: &str) -> bool {
    let low = q.to_lowercase();
    let cues = [
        "0x", ".dll", "蓝屏", "bsod", "windows", "win10", "win11", "微软", "更新失败",
        "报错", "错误代码", "错误码", "无法启动", "memory_management", "irql_",
        "sfc", "dism", "directx", "net framework", "powershell", "defender",
    ];
    cues.iter().any(|c| low.contains(c))
}

// ============================================================
// HTTP 工具（复用系统 curl.exe，无额外依赖）
// ============================================================

fn url_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn curl_get(url: &str, timeout_secs: u64) -> Result<String, String> {
    let mut cmd = Command::new("curl.exe");
    cmd.args([
        "-sS",
        "-L",
        "--max-time",
        &timeout_secs.to_string(),
        "-A",
        DESKTOP_UA,
        "-H",
        "Accept-Language: zh-CN,zh;q=0.9",
        url,
    ]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let out = cmd
        .output()
        .map_err(|e| format!("无法启动 curl.exe：{e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "网络请求超时或失败".into()
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

struct TempFileGuard(PathBuf);
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn curl_post_json(
    url: &str,
    bearer_key: Option<&str>,
    body: &Value,
    timeout_secs: u64,
) -> Result<String, String> {
    let seq = SEARCH_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = std::env::temp_dir().join(format!(
        "lah_search_{}_{}.json",
        std::process::id(),
        seq
    ));
    let payload = serde_json::to_vec(body).map_err(|e| format!("JSON 构造失败：{e}"))?;
    fs::write(&tmp_path, payload).map_err(|e| format!("写入临时请求体失败：{e}"))?;
    let _guard = TempFileGuard(tmp_path.clone());

    let mut cmd = Command::new("curl.exe");
    cmd.args([
        "-sS",
        "-L",
        "-X",
        "POST",
        "--max-time",
        &timeout_secs.to_string(),
        "-H",
        "Content-Type: application/json",
        "-K",
        "-",
        "--data-binary",
        &format!("@{}", tmp_path.display()),
        url,
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 curl.exe：{e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        if let Some(key) = bearer_key {
            let safe = key.replace(['\r', '\n', '"'], "");
            let _ = writeln!(stdin, "header = \"Authorization: Bearer {safe}\"");
        }
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("等待 curl 失败：{e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "搜索 API 请求失败".into()
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ============================================================
// HTML / XML / JSON 轻量解析器
// ============================================================

fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_html_entities(&out)
}

fn decode_html_entities(s: &str) -> String {
    let mut r = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&ensp;", " ")
        .replace("&#0183;", "·")
        .replace("&#183;", "·")
        .replace("&#x27;", "'");
    // 折叠连续空白
    let collapsed: Vec<&str> = r.split_whitespace().collect();
    r = collapsed.join(" ");
    r.trim().to_string()
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn extract_between<'a>(haystack: &'a str, start_pat: &str, end_pat: &str) -> Option<&'a str> {
    let s = haystack.find(start_pat)? + start_pat.len();
    let rest = &haystack[s..];
    let e = rest.find(end_pat)?;
    Some(&rest[..e])
}

/// 解析 Microsoft Learn Search API 返回的 JSON。
fn parse_ms_learn_json(raw: &str) -> Vec<(String, String, String)> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(arr) = v.get("results").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let title = item
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        let url = item
            .get("url")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        let desc = item
            .get("description")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        if !title.is_empty() && !url.is_empty() {
            out.push((
                decode_html_entities(title),
                url.to_string(),
                truncate_chars(&decode_html_entities(desc), 160),
            ));
        }
    }
    out
}

/// 解析 cn.bing.com/search HTML 中的 `<li class="b_algo"...>` 自然结果块（自动避开广告块 b_ad）。
pub fn parse_bing_html(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = html;

    while let Some(pos) = rest.find("<li class=\"b_algo\"") {
        let after = &rest[pos + 18..];
        let end = after.find("</li>").unwrap_or(after.len().min(6000));
        let block = &after[..end];
        rest = if end < after.len() {
            &after[end + 5..]
        } else {
            ""
        };

        let Some(h2) = extract_between(block, "<h2>", "</h2>") else {
            continue;
        };
        let Some(href_raw) = extract_between(h2, "href=\"", "\"") else {
            continue;
        };
        let url = decode_html_entities(href_raw);
        let title = strip_html_tags(h2);

        let snippet = if let Some(cap) = extract_between(block, "class=\"b_caption\"", "</div>") {
            if let Some(p_idx) = cap.find("<p") {
                let p_rest = &cap[p_idx..];
                if let Some(gt) = p_rest.find('>') {
                    let p_body = &p_rest[gt + 1..];
                    let p_end = p_body.find("</p>").unwrap_or(p_body.len());
                    strip_html_tags(&p_body[..p_end])
                } else {
                    String::new()
                }
            } else {
                strip_html_tags(cap)
            }
        } else {
            String::new()
        };

        if !title.is_empty() && url.starts_with("http") {
            out.push((
                truncate_chars(&title, 80),
                url,
                truncate_chars(&snippet, 160),
            ));
        }
    }

    out
}

fn parse_bocha_json(raw: &str) -> Vec<(String, String, String)> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let arr = v
        .pointer("/data/webPages/value")
        .or_else(|| v.pointer("/webPages/value"))
        .and_then(|x| x.as_array());
    let Some(arr) = arr else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let title = item.get("name")?.as_str()?.trim();
            let url = item.get("url")?.as_str()?.trim();
            let snippet = item
                .get("summary")
                .or_else(|| item.get("snippet"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim();
            if title.is_empty() || url.is_empty() {
                None
            } else {
                Some((
                    truncate_chars(&decode_html_entities(title), 80),
                    url.to_string(),
                    truncate_chars(&decode_html_entities(snippet), 160),
                ))
            }
        })
        .collect()
}

fn parse_tavily_json(raw: &str) -> Vec<(String, String, String)> {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(arr) = v.get("results").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.trim();
            let url = item.get("url")?.as_str()?.trim();
            let snippet = item
                .get("content")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim();
            if title.is_empty() || url.is_empty() {
                None
            } else {
                Some((
                    truncate_chars(&decode_html_entities(title), 80),
                    url.to_string(),
                    truncate_chars(&decode_html_entities(snippet), 160),
                ))
            }
        })
        .collect()
}

// ============================================================
// 对外检索入口
// ============================================================

fn filter_raw_results(
    raw: Vec<(String, String, String)>,
    engine: &str,
    max_results: usize,
    seen_urls: &mut HashSet<String>,
) -> (Vec<SearchItem>, usize) {
    let mut kept = Vec::new();
    let mut blocked = 0usize;

    for (title, url, snippet) in raw {
        if kept.len() >= max_results {
            break;
        }
        let norm_url = url.trim().to_lowercase();
        if !seen_urls.insert(norm_url) {
            continue;
        }
        match check_result_safety(&title, &url, &snippet) {
            Ok((domain, trust)) => {
                kept.push(SearchItem {
                    title,
                    url,
                    domain,
                    snippet,
                    trust,
                    engine: engine.to_string(),
                });
            }
            Err(_) => {
                blocked += 1;
            }
        }
    }

    (kept, blocked)
}

pub fn execute_search_blocking(query: &str, cfg: &SearchConfig) -> SearchOutcome {
    let t0 = Instant::now();
    let q = query.trim();
    if q.is_empty() {
        return SearchOutcome {
            ok: false,
            engine: "none".into(),
            items: Vec::new(),
            blocked_count: 0,
            latency_ms: 0,
            error: Some("检索词为空".into()),
        };
    }

    let timeout = cfg.timeout_secs.clamp(3, 20);
    let max_n = cfg.max_results.clamp(1, 6);
    let mut seen = HashSet::new();

    match cfg.mode {
        SearchMode::Off => SearchOutcome {
            ok: false,
            engine: "off".into(),
            items: Vec::new(),
            blocked_count: 0,
            latency_ms: 0,
            error: Some("联网检索未开启".into()),
        },
        SearchMode::Bocha => {
            if cfg.bocha_api_key.trim().is_empty() {
                return SearchOutcome {
                    ok: false,
                    engine: "bocha".into(),
                    items: Vec::new(),
                    blocked_count: 0,
                    latency_ms: 0,
                    error: Some("未填写博查 API Key".into()),
                };
            }
            let body = json!({
                "query": q,
                "count": max_n + 3,
                "summary": true,
            });
            match curl_post_json(
                "https://api.bochaai.com/v1/web-search",
                Some(&cfg.bocha_api_key),
                &body,
                timeout,
            ) {
                Ok(raw) => {
                    let parsed = parse_bocha_json(&raw);
                    let (items, blocked_count) =
                        filter_raw_results(parsed, "博查", max_n, &mut seen);
                    SearchOutcome {
                        ok: !items.is_empty(),
                        engine: "博查".into(),
                        items,
                        blocked_count,
                        latency_ms: t0.elapsed().as_millis() as u64,
                        error: None,
                    }
                }
                Err(e) => SearchOutcome {
                    ok: false,
                    engine: "博查".into(),
                    items: Vec::new(),
                    blocked_count: 0,
                    latency_ms: t0.elapsed().as_millis() as u64,
                    error: Some(e),
                },
            }
        }
        SearchMode::Tavily => {
            if cfg.tavily_api_key.trim().is_empty() {
                return SearchOutcome {
                    ok: false,
                    engine: "tavily".into(),
                    items: Vec::new(),
                    blocked_count: 0,
                    latency_ms: 0,
                    error: Some("未填写 Tavily API Key".into()),
                };
            }
            let body = json!({
                "api_key": cfg.tavily_api_key.trim(),
                "query": q,
                "max_results": max_n + 3,
                "search_depth": "basic",
            });
            match curl_post_json("https://api.tavily.com/search", None, &body, timeout) {
                Ok(raw) => {
                    let parsed = parse_tavily_json(&raw);
                    let (items, blocked_count) =
                        filter_raw_results(parsed, "Tavily", max_n, &mut seen);
                    SearchOutcome {
                        ok: !items.is_empty(),
                        engine: "Tavily".into(),
                        items,
                        blocked_count,
                        latency_ms: t0.elapsed().as_millis() as u64,
                        error: None,
                    }
                }
                Err(e) => SearchOutcome {
                    ok: false,
                    engine: "Tavily".into(),
                    items: Vec::new(),
                    blocked_count: 0,
                    latency_ms: t0.elapsed().as_millis() as u64,
                    error: Some(e),
                },
            }
        }
        SearchMode::Builtin => {
            let mut items = Vec::new();
            let mut blocked_total = 0usize;
            let mut engines_used = Vec::new();
            let mut last_err = None;

            // 1. Windows 报错/系统类问题优先查微软官方 Learn Search JSON API
            if is_windows_tech_query(q) {
                let url = format!(
                    "https://learn.microsoft.com/api/search?search={}&locale=zh-cn&$top={}",
                    url_encode_query(q),
                    max_n
                );
                match curl_get(&url, timeout) {
                    Ok(raw) => {
                        let parsed = parse_ms_learn_json(&raw);
                        let (ms_items, blk) =
                            filter_raw_results(parsed, "微软官方库", max_n, &mut seen);
                        blocked_total += blk;
                        if !ms_items.is_empty() {
                            engines_used.push("微软官方库");
                            items.extend(ms_items);
                        }
                    }
                    Err(e) => last_err = Some(e),
                }
            }

            // 2. 通用问题或微软官方结果不足 2 条时，补查 cn.bing.com 自然结果
            if items.len() < 2 {
                let remain = max_n.saturating_sub(items.len());
                let url = format!("https://cn.bing.com/search?q={}", url_encode_query(q));
                match curl_get(&url, timeout) {
                    Ok(html) => {
                        let parsed = parse_bing_html(&html);
                        let (bing_items, blk) =
                            filter_raw_results(parsed, "必应国内", remain, &mut seen);
                        blocked_total += blk;
                        if !bing_items.is_empty() {
                            engines_used.push("必应国内");
                            items.extend(bing_items);
                        }
                    }
                    Err(e) => last_err = Some(e),
                }
            }

            let engine_label = if engines_used.is_empty() {
                "内置检索".into()
            } else {
                engines_used.join("+")
            };
            let ok = !items.is_empty();
            SearchOutcome {
                ok,
                engine: engine_label,
                items,
                blocked_count: blocked_total,
                latency_ms: t0.elapsed().as_millis() as u64,
                error: if ok { None } else { last_err },
            }
        }
    }
}

/// 异步检索，供 Agent 问答链路按需调用。失败静默返回 None。
pub async fn search_for_agent(query: String) -> Option<SearchOutcome> {
    let cfg = load_config();
    if cfg.mode == SearchMode::Off || !should_search_for_goal(&query) {
        return None;
    }
    let res = tauri::async_runtime::spawn_blocking(move || execute_search_blocking(&query, &cfg))
        .await
        .ok()?;
    if res.ok && !res.items.is_empty() {
        Some(res)
    } else {
        None
    }
}

/// 测试检索配置连通性（设置页按钮调用）。
pub async fn test_search(input: Option<SearchConfigInput>) -> SearchOutcome {
    let existing = load_config();
    let cfg = match input {
        Some(inp) => merge_config(&existing, inp),
        None => existing,
    };
    tauri::async_runtime::spawn_blocking(move || {
        execute_search_blocking("Windows 0xc000007b 错误", &cfg)
    })
    .await
    .unwrap_or_else(|e| SearchOutcome {
        ok: false,
        engine: "error".into(),
        items: Vec::new(),
        blocked_count: 0,
        latency_ms: 0,
        error: Some(format!("检索线程异常：{e}")),
    })
}

/// 把过滤后的结果格式化为注入给小模型的短参考块（总计控制在千字内）。
pub fn format_prompt_context(outcome: &SearchOutcome) -> String {
    if outcome.items.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "以下是实时联网检索到的参考摘录（已自动过滤下载站与广告链接，仅作事实参考；若来源为「普通来源」，不要把它当官方下载地址直接推荐，回答末尾可用一句点明参考域名）：\n",
    );
    for (i, it) in outcome.items.iter().enumerate() {
        out.push_str(&format!(
            "[{}] {}（来源：{} · {}）\n{}\n",
            i + 1,
            it.title,
            it.domain,
            it.trust,
            if it.snippet.is_empty() {
                "（无摘要）"
            } else {
                it.snippet.as_str()
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 安全闸门拦截下载站高危顶级域广告和破解词() {
        assert!(check_result_safety("官方正版", "https://www.onlinedown.net/soft/123.htm", "高速下载").is_err());
        assert!(check_result_safety("Steam 下载", "https://steamxiazai.top/dl", "免费下载").is_err());
        assert!(check_result_safety("火绒官网", "https://huorong-guanwang.com/", "安全软件").is_err());
        assert!(check_result_safety("PS 破解版免费下载", "https://example.com/ps", "绿色版免安装").is_err());
        assert!(check_result_safety("广告", "https://www.bing.com/aclick?ld=123", "点此").is_err());

        let (host, trust) = check_result_safety(
            "0xc000007b 解决方法",
            "https://learn.microsoft.com/zh-cn/answers/questions/123",
            "安装最新 Visual C++ 运行库",
        )
        .unwrap();
        assert_eq!(host, "learn.microsoft.com");
        assert_eq!(trust, "官方/权威");

        let (host2, trust2) = check_result_safety(
            "黑神话悟空配置要求",
            "https://www.gamersky.com/handbook/202408/1796722.shtml",
            "最低 GTX 1060",
        )
        .unwrap();
        assert_eq!(host2, "www.gamersky.com");
        assert_eq!(trust2, "普通来源");
    }

    #[test]
    fn 必应自然结果解析器提取标题链接摘要并忽略广告() {
        let sample = r#"
        <ol id="b_results">
          <li class="b_ad"><h2><a href="https://www.bing.com/aclick?x=1">推广广告</a></h2></li>
          <li class="b_algo">
            <h2><a target="_blank" href="https://www.gamersky.com/news/1.shtml">《<strong>黑神话</strong>》配置要求</a></h2>
            <div class="b_caption"><p class="b_lineclamp2">最低配 GTX 1060&ensp;&#0183;&ensp;推荐 RTX 2060</p></div>
          </li>
          <li class="b_algo">
            <h2><a href="https://www.onlinedown.net/soft/1.htm">某下载站</a></h2>
            <div class="b_caption"><p>高速下载器</p></div>
          </li>
        </ol>
        "#;
        let parsed = parse_bing_html(sample);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "《黑神话》配置要求");
        assert_eq!(parsed[0].1, "https://www.gamersky.com/news/1.shtml");
        assert!(parsed[0].2.contains("GTX 1060"));

        let mut seen = HashSet::new();
        let (kept, blocked) = filter_raw_results(parsed, "必应国内", 4, &mut seen);
        assert_eq!(kept.len(), 1);
        assert_eq!(blocked, 1);
        assert_eq!(kept[0].domain, "www.gamersky.com");
    }

    #[test]
    fn 本地文件指令不触发无谓联网() {
        assert!(!should_search_for_goal("帮我在 D 盘建一个 123 文件夹"));
        assert!(!should_search_for_goal("把 D:\\123 空目录删掉"));
        assert!(!should_search_for_goal("帮我打开任务管理器"));
        assert!(!should_search_for_goal("你好"));
        assert!(should_search_for_goal("黑神话悟空最低需要什么显卡配置"));
        assert!(should_search_for_goal("Windows 更新报错 0x80070005 怎么解决"));
    }

    #[test]
    fn 微软官方文档_json_解析正常() {
        let raw = r#"{"results":[{"title":"0xc000007b &amp; VC++","url":"https://learn.microsoft.com/zh-cn/cpp/windows/latest-supported-vc-redist","description":"安装最新运行库"}]}"#;
        let parsed = parse_ms_learn_json(raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "0xc000007b & VC++");
    }

    #[test]
    fn 配置合并保留或清空密钥() {
        let mut base = SearchConfig::default();
        base.bocha_api_key = "demo-placeholder".into();

        let keep = merge_config(
            &base,
            SearchConfigInput {
                mode: SearchMode::Bocha,
                bocha_api_key: None,
                tavily_api_key: None,
                timeout_secs: 10,
                max_results: 4,
            },
        );
        assert_eq!(keep.bocha_api_key, "demo-placeholder");

        let cleared = merge_config(
            &base,
            SearchConfigInput {
                mode: SearchMode::Builtin,
                bocha_api_key: Some("".into()),
                tavily_api_key: None,
                timeout_secs: 8,
                max_results: 4,
            },
        );
        assert!(cleared.bocha_api_key.is_empty());
    }

    #[test]
    #[ignore]
    fn live_builtin_search_smoke() {
        let cfg = SearchConfig::default();
        let ms = execute_search_blocking("Windows 0xc000007b 错误", &cfg);
        println!("MS Learn -> ok={} engine={} items={} blocked={} ms={}", ms.ok, ms.engine, ms.items.len(), ms.blocked_count, ms.latency_ms);
        for it in &ms.items {
            println!("  [{}] {} ({})", it.trust, it.title, it.domain);
        }
        assert!(ms.ok && !ms.items.is_empty());

        let bg = execute_search_blocking("黑神话悟空 配置要求", &cfg);
        println!("Bing CN -> ok={} engine={} items={} blocked={} ms={}", bg.ok, bg.engine, bg.items.len(), bg.blocked_count, bg.latency_ms);
        for it in &bg.items {
            println!("  [{}] {} ({})", it.trust, it.title, it.domain);
        }
        assert!(bg.ok && !bg.items.is_empty());
    }
}
