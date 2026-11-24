// ============================================================
// llm.rs —— 统一 OpenAI 兼容模型客户端
//
// 本地 llama-server 与自定义云端 API 共用同一套 /chat/completions 协议：
//   - off    : 不调用任何模型，只走确定性规则
//   - local  : 连接本地 OpenAI 兼容服务（默认 http://127.0.0.1:18090/v1）
//   - custom : 连接自定义 OpenAI 兼容 API（Base URL + API Key + Model）
//
// 传输层说明：
//   复用 Windows 10/11 自带的 curl.exe（支持系统代理与 Schannel TLS），
//   免引入额外网络依赖、保持极速编译和小体积。
//   Authorization 头通过 stdin（-K -）传入，不进入进程命令行参数（argv）；
//   Windows 下附加 CREATE_NO_WINDOW，避免闪出黑框。
// ============================================================

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const STATUS_MARKER: &str = "\n__HTTP_STATUS__:";
static REQ_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LlmMode {
    Off,
    Local,
    Custom,
}

impl Default for LlmMode {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    #[serde(default)]
    pub mode: LlmMode,
    #[serde(default = "default_local_base_url")]
    pub local_base_url: String,
    #[serde(default = "default_local_model")]
    pub local_model: String,
    #[serde(default = "default_custom_base_url")]
    pub custom_base_url: String,
    #[serde(default)]
    pub custom_api_key: String,
    #[serde(default)]
    pub custom_model: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_disable_thinking")]
    pub disable_thinking: bool,
}

fn default_local_base_url() -> String {
    "http://127.0.0.1:18090/v1".into()
}

fn default_local_model() -> String {
    "local".into()
}

fn default_custom_base_url() -> String {
    "https://api.openai.com/v1".into()
}

fn default_timeout_secs() -> u64 {
    45
}

fn default_disable_thinking() -> bool {
    true
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            mode: LlmMode::Off,
            local_base_url: default_local_base_url(),
            local_model: default_local_model(),
            custom_base_url: default_custom_base_url(),
            custom_api_key: String::new(),
            custom_model: String::new(),
            timeout_secs: default_timeout_secs(),
            disable_thinking: default_disable_thinking(),
        }
    }
}

/// 返回给前端的配置视图：隐藏明文 API Key，只给掩码与状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfigView {
    pub mode: LlmMode,
    pub local_base_url: String,
    pub local_model: String,
    pub custom_base_url: String,
    pub custom_model: String,
    pub has_api_key: bool,
    pub api_key_masked: String,
    pub timeout_secs: u64,
    pub disable_thinking: bool,
}

/// 前端提交的配置更新：custom_api_key 为 None 时保留原 Key，为 Some("") 时清空。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfigInput {
    pub mode: LlmMode,
    pub local_base_url: String,
    pub local_model: String,
    pub custom_base_url: String,
    #[serde(default)]
    pub custom_api_key: Option<String>,
    pub custom_model: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub disable_thinking: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmReply {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if !base.trim().is_empty() {
            return PathBuf::from(base).join("local-ai-helper").join("llm.json");
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("llm.json")
}

pub fn load_config() -> LlmConfig {
    let path = config_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return LlmConfig::default();
    };
    serde_json::from_str::<LlmConfig>(&raw).unwrap_or_default()
}

pub fn save_config(cfg: &LlmConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let body = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败：{e}"))?;
    fs::write(&path, body).map_err(|e| format!("写入配置失败：{e}"))
}

pub fn to_view(cfg: &LlmConfig) -> LlmConfigView {
    let key = cfg.custom_api_key.trim();
    LlmConfigView {
        mode: cfg.mode.clone(),
        local_base_url: cfg.local_base_url.clone(),
        local_model: cfg.local_model.clone(),
        custom_base_url: cfg.custom_base_url.clone(),
        custom_model: cfg.custom_model.clone(),
        has_api_key: !key.is_empty(),
        api_key_masked: mask_api_key(key),
        timeout_secs: cfg.timeout_secs.clamp(5, 180),
        disable_thinking: cfg.disable_thinking,
    }
}

pub fn merge_config(existing: &LlmConfig, input: LlmConfigInput) -> LlmConfig {
    let next_key = match input.custom_api_key {
        None => existing.custom_api_key.clone(),
        Some(k) => k.trim().to_string(),
    };
    LlmConfig {
        mode: input.mode,
        local_base_url: {
            let s = input.local_base_url.trim();
            if s.is_empty() {
                default_local_base_url()
            } else {
                s.to_string()
            }
        },
        local_model: {
            let s = input.local_model.trim();
            if s.is_empty() {
                default_local_model()
            } else {
                s.to_string()
            }
        },
        custom_base_url: input.custom_base_url.trim().to_string(),
        custom_api_key: next_key,
        custom_model: input.custom_model.trim().to_string(),
        timeout_secs: input
            .timeout_secs
            .unwrap_or(existing.timeout_secs)
            .clamp(5, 180),
        disable_thinking: input.disable_thinking.unwrap_or(existing.disable_thinking),
    }
}

pub fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}****{tail}")
}

/// 拼出标准 OpenAI `/chat/completions` 接口地址。
pub fn build_chat_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("请填写接口地址（Base URL）".into());
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("接口地址必须以 http:// 或 https:// 开头".into());
    }
    if trimmed.contains('\r') || trimmed.contains('\n') || trimmed.contains('\0') {
        return Err("接口地址包含非法字符".into());
    }
    if trimmed.ends_with("/chat/completions") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("{trimmed}/chat/completions"))
    }
}

/// 去掉模型可能吐出的 <think>...</think> 思考块，只留给普通人看的正文。
pub fn strip_think_blocks(raw: &str) -> String {
    let mut s = raw.to_string();
    while let Some(start) = s.find("<think>") {
        if let Some(rel_end) = s[start + 7..].find("</think>") {
            let end = start + 7 + rel_end + 8;
            s.replace_range(start..end, "");
        } else {
            s.truncate(start);
            break;
        }
    }
    s.trim().to_string()
}

fn escape_curl_val(s: &str) -> Result<String, String> {
    if s.contains('\r') || s.contains('\n') || s.contains('\0') {
        return Err("配置字段包含非法换行符".into());
    }
    Ok(s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn system_prompt(hardware_summary: Option<&str>) -> String {
    let mut prompt = String::from(
        "你是一个面向电脑新手和长辈的本地电脑助手。\n\
         回答纪律：\n\
         1. 说人话，短句分段，少用英文缩写和术语；必须提操作步骤时按 1. 2. 3. 写清。\n\
         2. 不说教、不评论拼写错误、不写免责声明。\n\
         3. 遇到拿不准的问题直接说不确定，不编造网址、版本号或操作结果。\n\
         4. 绝不建议格式化磁盘、关闭杀毒软件、修改未知注册表或去第三方下载站下软件。",
    );
    if let Some(hw) = hardware_summary {
        let hw = hw.trim();
        if !hw.is_empty() {
            prompt.push_str("\n\n当前这台电脑的实测配置如下（回答相关问题时严格以这些数字为准，不得改写）：\n");
            prompt.push_str(hw);
        }
    }
    prompt
}

fn resolve_endpoint(cfg: &LlmConfig) -> Result<(String, String, Option<String>, bool), String> {
    match cfg.mode {
        LlmMode::Off => Err("模型未开启，请在设置中选择本地模型或自定义 API".into()),
        LlmMode::Local => {
            let url = build_chat_url(&cfg.local_base_url)?;
            let model = if cfg.local_model.trim().is_empty() {
                "local".to_string()
            } else {
                cfg.local_model.trim().to_string()
            };
            Ok((url, model, None, cfg.disable_thinking))
        }
        LlmMode::Custom => {
            let url = build_chat_url(&cfg.custom_base_url)?;
            let model = cfg.custom_model.trim().to_string();
            if model.is_empty() {
                return Err("请填写模型名（例如 deepseek-chat / MiniMax-M2.7）".into());
            }
            let key = cfg.custom_api_key.trim().to_string();
            let auth = if key.is_empty() { None } else { Some(key) };
            Ok((url, model, auth, false))
        }
    }
}

fn format_http_error(status: u16, body: &str, is_local: bool) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message").or(Some(e)))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();

    let hint = match status {
        401 | 403 => "API Key 无效、过期或没有权限",
        404 => "接口地址或模型名不存在",
        429 => "请求太频繁或额度已用完（429 限流）",
        500..=599 => {
            if is_local {
                "本地模型服务内部报错"
            } else {
                "远端接口暂时出错，请稍后再试"
            }
        }
        _ => "接口返回异常",
    };

    if detail.is_empty() {
        format!("{hint}（HTTP {status}）")
    } else {
        let short: String = detail.chars().take(160).collect();
        format!("{hint}（HTTP {status}：{short}）")
    }
}

fn split_curl_output(raw: &str) -> Result<(&str, u16), String> {
    let idx = raw
        .rfind(STATUS_MARKER)
        .ok_or_else(|| "未能解析 HTTP 状态码".to_string())?;
    let body = &raw[..idx];
    let code_str = raw[idx + STATUS_MARKER.len()..].trim();
    let status = code_str
        .parse::<u16>()
        .map_err(|_| format!("异常 HTTP 状态码：{code_str}"))?;
    Ok((body, status))
}

fn send_completion_with_options(
    cfg: &LlmConfig,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f64,
    seed: Option<u32>,
    grammar: Option<&str>,
    force_disable_thinking: bool,
) -> Result<(String, String, u64), String> {
    let (url, model, api_key, disable_thinking) = resolve_endpoint(cfg)?;
    let is_local = cfg.mode == LlmMode::Local;
    let timeout_secs = cfg.timeout_secs.clamp(5, 180);

    let mut payload = json!({
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": false,
    });

    if let Some(s) = seed {
        payload["seed"] = json!(s);
    }
    if disable_thinking || (force_disable_thinking && is_local) {
        payload["chat_template_kwargs"] = json!({ "enable_thinking": false });
    }
    if is_local {
        if let Some(g) = grammar {
            payload["grammar"] = json!(g);
        }
    }

    let body_bytes =
        serde_json::to_vec(&payload).map_err(|e| format!("构造请求体失败：{e}"))?;

    let seq = REQ_SEQ.fetch_add(1, Ordering::Relaxed);
    let body_path = std::env::temp_dir().join(format!(
        "local_ai_helper_req_{}_{}.json",
        std::process::id(),
        seq
    ));
    fs::write(&body_path, &body_bytes).map_err(|e| format!("写入临时请求失败：{e}"))?;
    let _guard = TempFileGuard(body_path.clone());

    let url_esc = escape_curl_val(&url)?;
    let path_esc = escape_curl_val(&body_path.to_string_lossy())?;

    let mut curl_cfg = format!(
        "url = \"{url_esc}\"\n\
         request = \"POST\"\n\
         header = \"Content-Type: application/json\"\n\
         data-binary = \"@{path_esc}\"\n\
         connect-timeout = 10\n\
         max-time = {timeout_secs}\n\
         write-out = \"\\n__HTTP_STATUS__:%{{http_code}}\"\n"
    );
    if let Some(key) = api_key {
        let key_esc = escape_curl_val(&key)?;
        curl_cfg.push_str(&format!("header = \"Authorization: Bearer {key_esc}\"\n"));
    }

    let mut cmd = Command::new("curl.exe");
    cmd.args(["-sS", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let t0 = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法调用系统 curl.exe：{e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(curl_cfg.as_bytes())
            .map_err(|e| format!("写入请求配置失败：{e}"))?;
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("等待网络响应失败：{e}"))?;

    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(match code {
            6 => "找不到该域名，请检查 Base URL 或网络连接".into(),
            7 => {
                if is_local {
                    format!(
                        "连不上本地服务（{}），请先启动 llama-server",
                        cfg.local_base_url
                    )
                } else {
                    "连不上接口地址，请检查网络或 Base URL".into()
                }
            }
            28 => format!("请求超时（超过 {timeout_secs} 秒）"),
            35 | 60 => "HTTPS 安全连接失败，请检查代理或系统时间".into(),
            _ => {
                if stderr.is_empty() {
                    format!("网络请求失败（退出码 {code}）")
                } else {
                    let short: String = stderr.chars().take(160).collect();
                    format!("网络请求失败：{short}")
                }
            }
        });
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let (body, status) = split_curl_output(&stdout)?;

    if !(200..300).contains(&status) {
        return Err(format_http_error(status, body, is_local));
    }

    let parsed: Value =
        serde_json::from_str(body).map_err(|e| format!("接口返回的不是有效 JSON：{e}"))?;

    let raw_content = parsed
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "接口返回缺少 choices[0].message.content".to_string())?;

    let clean = strip_think_blocks(raw_content);
    let reply_model = parsed
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or(&model)
        .to_string();

    Ok((clean, reply_model, t0.elapsed().as_millis() as u64))
}

fn send_completion_blocking(
    cfg: &LlmConfig,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
) -> Result<(String, String, u64), String> {
    send_completion_with_options(cfg, messages, max_tokens, 0.2, None, None, false)
}

/// 供分诊模块使用：温度 0 + 固定种子 42 + 本地 llama-server GBNF 语法硬约束。
pub async fn complete_constrained(
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    grammar: Option<String>,
) -> Result<(String, String, u64), String> {
    let cfg = load_config();
    if cfg.mode == LlmMode::Off {
        return Err("模型未开启".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        send_completion_with_options(
            &cfg,
            messages,
            max_tokens,
            0.0,
            Some(42),
            grammar.as_deref(),
            true,
        )
    })
    .await
    .map_err(|e| format!("分诊线程异常：{e}"))
    .and_then(|inner| inner)
}

#[derive(Debug, Clone)]
pub struct StreamResult {
    pub content: String,
    pub thinking: String,
    pub model: String,
    pub latency_ms: u64,
}

/// 流式 <think>...</think> 拆分器：
/// 即使标签被网络分片拆断（例如 "<th" + "ink>"），也能准确把思考链和正文分离。
pub struct ThinkStreamSplitter {
    in_think: bool,
    buf: String,
}

impl ThinkStreamSplitter {
    pub fn new() -> Self {
        Self {
            in_think: false,
            buf: String::new(),
        }
    }

    pub fn push<F>(&mut self, chunk: &str, mut emit: F)
    where
        F: FnMut(&str, &str),
    {
        self.buf.push_str(chunk);
        loop {
            if !self.in_think {
                if let Some(pos) = self.buf.find("<think>") {
                    if pos > 0 {
                        let out = self.buf[..pos].to_string();
                        emit("content", &out);
                    }
                    self.buf.drain(..pos + 7);
                    self.in_think = true;
                    continue;
                }
                let keep = partial_tag_suffix_len(&self.buf, "<think>");
                let emit_len = self.buf.len() - keep;
                if emit_len > 0 {
                    let out = self.buf[..emit_len].to_string();
                    self.buf.drain(..emit_len);
                    emit("content", &out);
                }
                break;
            } else {
                if let Some(pos) = self.buf.find("</think>") {
                    if pos > 0 {
                        let out = self.buf[..pos].to_string();
                        emit("thinking", &out);
                    }
                    self.buf.drain(..pos + 8);
                    self.in_think = false;
                    continue;
                }
                let keep = partial_tag_suffix_len(&self.buf, "</think>");
                let emit_len = self.buf.len() - keep;
                if emit_len > 0 {
                    let out = self.buf[..emit_len].to_string();
                    self.buf.drain(..emit_len);
                    emit("thinking", &out);
                }
                break;
            }
        }
    }

    pub fn finish<F>(&mut self, mut emit: F)
    where
        F: FnMut(&str, &str),
    {
        if !self.buf.is_empty() {
            let rem = std::mem::take(&mut self.buf);
            emit(if self.in_think { "thinking" } else { "content" }, &rem);
        }
    }
}

fn partial_tag_suffix_len(s: &str, tag: &str) -> usize {
    let max_check = (tag.len() - 1).min(s.len());
    for len in (1..=max_check).rev() {
        let start = s.len() - len;
        if s.is_char_boundary(start) && tag.starts_with(&s[start..]) {
            return len;
        }
    }
    0
}

fn parse_last_http_status(headers_raw: &str) -> Option<u16> {
    let mut last = None;
    for line in headers_raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("HTTP/") {
            if let Some(code_str) = trimmed.split_whitespace().nth(1) {
                if let Ok(code) = code_str.parse::<u16>() {
                    last = Some(code);
                }
            }
        }
    }
    last
}

fn send_stream_blocking<F>(
    cfg: &LlmConfig,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    mut on_chunk: F,
) -> Result<StreamResult, String>
where
    F: FnMut(&str, &str),
{
    let (url, model, api_key, disable_thinking) = resolve_endpoint(cfg)?;
    let is_local = cfg.mode == LlmMode::Local;
    let timeout_secs = cfg.timeout_secs.clamp(5, 180);

    let mut payload = json!({
        "model": model,
        "messages": messages,
        "temperature": 0.2,
        "max_tokens": max_tokens,
        "stream": true,
    });

    if disable_thinking {
        payload["chat_template_kwargs"] = json!({ "enable_thinking": false });
    }

    let body_bytes =
        serde_json::to_vec(&payload).map_err(|e| format!("构造请求体失败：{e}"))?;

    let seq = REQ_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let body_path = std::env::temp_dir().join(format!("local_ai_helper_sreq_{pid}_{seq}.json"));
    let hdr_path = std::env::temp_dir().join(format!("local_ai_helper_shdr_{pid}_{seq}.txt"));
    fs::write(&body_path, &body_bytes).map_err(|e| format!("写入临时请求失败：{e}"))?;
    let _body_guard = TempFileGuard(body_path.clone());
    let _hdr_guard = TempFileGuard(hdr_path.clone());

    let url_esc = escape_curl_val(&url)?;
    let path_esc = escape_curl_val(&body_path.to_string_lossy())?;
    let hdr_esc = escape_curl_val(&hdr_path.to_string_lossy())?;

    let mut curl_cfg = format!(
        "url = \"{url_esc}\"\n\
         request = \"POST\"\n\
         header = \"Content-Type: application/json\"\n\
         header = \"Accept: text/event-stream\"\n\
         data-binary = \"@{path_esc}\"\n\
         dump-header = \"{hdr_esc}\"\n\
         connect-timeout = 10\n\
         max-time = {timeout_secs}\n"
    );
    if let Some(key) = api_key {
        let key_esc = escape_curl_val(&key)?;
        curl_cfg.push_str(&format!("header = \"Authorization: Bearer {key_esc}\"\n"));
    }

    let mut cmd = Command::new("curl.exe");
    cmd.args(["-sS", "-N", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let t0 = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法调用系统 curl.exe：{e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(curl_cfg.as_bytes())
            .map_err(|e| format!("写入请求配置失败：{e}"))?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法读取流式响应通道".to_string())?;
    let mut reader = BufReader::new(stdout);

    let mut splitter = ThinkStreamSplitter::new();
    let mut full_content = String::new();
    let mut full_thinking = String::new();
    let mut raw_non_sse = String::new();
    let mut reply_model = model.clone();
    let mut saw_sse = false;

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("读取流式数据失败：{e}"))?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(':') {
            continue;
        }
        if let Some(data_part) = trimmed.strip_prefix("data:") {
            saw_sse = true;
            let data = data_part.trim();
            if data == "[DONE]" {
                break;
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(m) = v.get("model").and_then(|x| x.as_str()) {
                    if !m.is_empty() {
                        reply_model = m.to_string();
                    }
                }
                if let Some(delta) = v.pointer("/choices/0/delta") {
                    if let Some(r) = delta
                        .get("reasoning_content")
                        .or_else(|| delta.get("reasoning"))
                        .and_then(|x| x.as_str())
                    {
                        if !r.is_empty() {
                            full_thinking.push_str(r);
                            on_chunk("thinking", r);
                        }
                    }
                    if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
                        if !c.is_empty() {
                            splitter.push(c, |kind, seg| {
                                if kind == "thinking" {
                                    full_thinking.push_str(seg);
                                } else {
                                    full_content.push_str(seg);
                                }
                                on_chunk(kind, seg);
                            });
                        }
                    }
                }
            }
        } else {
            raw_non_sse.push_str(&line);
        }
    }

    splitter.finish(|kind, seg| {
        if kind == "thinking" {
            full_thinking.push_str(seg);
        } else {
            full_content.push_str(seg);
        }
        on_chunk(kind, seg);
    });

    let out = child
        .wait_with_output()
        .map_err(|e| format!("等待网络结束失败：{e}"))?;

    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(match code {
            6 => "找不到该域名，请检查 Base URL 或网络连接".into(),
            7 => {
                if is_local {
                    format!(
                        "连不上本地服务（{}），请先启动 llama-server",
                        cfg.local_base_url
                    )
                } else {
                    "连不上接口地址，请检查网络或 Base URL".into()
                }
            }
            28 => format!("请求超时（超过 {timeout_secs} 秒）"),
            35 | 60 => "HTTPS 安全连接失败，请检查代理或系统时间".into(),
            _ => {
                if stderr.is_empty() {
                    format!("网络请求失败（退出码 {code}）")
                } else {
                    let short: String = stderr.chars().take(160).collect();
                    format!("网络请求失败：{short}")
                }
            }
        });
    }

    let status = fs::read_to_string(&hdr_path)
        .ok()
        .and_then(|h| parse_last_http_status(&h))
        .unwrap_or(200);

    if !(200..300).contains(&status) {
        return Err(format_http_error(status, &raw_non_sse, is_local));
    }

    // 兼容极少数忽略 stream:true 而直接返回非流式 JSON 的接口
    if !saw_sse && !raw_non_sse.trim().is_empty() {
        if let Ok(parsed) = serde_json::from_str::<Value>(&raw_non_sse) {
            if let Some(raw_c) = parsed
                .pointer("/choices/0/message/content")
                .and_then(|v| v.as_str())
            {
                let mut sp = ThinkStreamSplitter::new();
                sp.push(raw_c, |kind, seg| {
                    if kind == "thinking" {
                        full_thinking.push_str(seg);
                    } else {
                        full_content.push_str(seg);
                    }
                    on_chunk(kind, seg);
                });
                sp.finish(|kind, seg| {
                    if kind == "thinking" {
                        full_thinking.push_str(seg);
                    } else {
                        full_content.push_str(seg);
                    }
                    on_chunk(kind, seg);
                });
            }
        }
    }

    Ok(StreamResult {
        content: full_content.trim().to_string(),
        thinking: full_thinking.trim().to_string(),
        model: reply_model,
        latency_ms: t0.elapsed().as_millis() as u64,
    })
}

/// 给 agent 模块复用的流式调用：边收边通过回调向外推送 thinking/content 增量。
pub async fn complete_messages_stream<F>(
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    on_chunk: F,
) -> Result<StreamResult, String>
where
    F: FnMut(&str, &str) + Send + 'static,
{
    let cfg = load_config();
    if cfg.mode == LlmMode::Off {
        return Err("模型未开启".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        send_stream_blocking(&cfg, messages, max_tokens, on_chunk)
    })
    .await
    .map_err(|e| format!("后台流式线程异常：{e}"))
    .and_then(|inner| inner)
}

/// 给 agent 模块复用的底层调用：传入完整消息列表（含自定义 system prompt），返回正文、模型名与耗时。
pub async fn complete_messages(
    messages: Vec<ChatMessage>,
    max_tokens: u32,
) -> Result<(String, String, u64), String> {
    let cfg = load_config();
    if cfg.mode == LlmMode::Off {
        return Err("模型未开启".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        send_completion_blocking(&cfg, messages, max_tokens)
    })
    .await
    .map_err(|e| format!("后台请求线程异常：{e}"))
    .and_then(|inner| inner)
}

pub async fn chat(
    history: Vec<ChatMessage>,
    hardware_summary: Option<String>,
) -> LlmReply {
    let cfg = load_config();
    if cfg.mode == LlmMode::Off {
        return LlmReply {
            ok: false,
            reply: None,
            model: None,
            latency_ms: 0,
            error: Some("模型未开启".into()),
        };
    }

    let mut messages = Vec::with_capacity(history.len().min(12) + 1);
    messages.push(ChatMessage {
        role: "system".into(),
        content: system_prompt(hardware_summary.as_deref()),
    });

    // 只保留最近 12 条消息，防止上下文过长拖慢小模型或浪费费用
    let start = history.len().saturating_sub(12);
    for m in history.into_iter().skip(start) {
        let role = match m.role.as_str() {
            "user" | "assistant" => m.role,
            _ => continue,
        };
        let content = m.content.trim().to_string();
        if !content.is_empty() {
            messages.push(ChatMessage { role, content });
        }
    }

    let res = tauri::async_runtime::spawn_blocking(move || {
        send_completion_blocking(&cfg, messages, 1024)
    })
    .await
    .map_err(|e| format!("后台请求线程异常：{e}"))
    .and_then(|inner| inner);

    match res {
        Ok((reply, model, latency_ms)) => LlmReply {
            ok: true,
            reply: Some(if reply.is_empty() {
                "模型没有返回文字内容。".into()
            } else {
                reply
            }),
            model: Some(model),
            latency_ms,
            error: None,
        },
        Err(error) => LlmReply {
            ok: false,
            reply: None,
            model: None,
            latency_ms: 0,
            error: Some(error),
        },
    }
}

pub async fn test_connection(input: Option<LlmConfigInput>) -> TestResult {
    let existing = load_config();
    let cfg = match input {
        Some(i) => merge_config(&existing, i),
        None => existing,
    };

    if cfg.mode == LlmMode::Off {
        return TestResult {
            ok: false,
            latency_ms: 0,
            model: None,
            message: None,
            error: Some("当前模式是「关闭」，请先选「本地模型」或「自定义 API」".into()),
        };
    }

    let probe = vec![ChatMessage {
        role: "user".into(),
        content: "只回复：OK".into(),
    }];

    let res = tauri::async_runtime::spawn_blocking(move || {
        send_completion_blocking(&cfg, probe, 16)
    })
    .await
    .map_err(|e| format!("后台测试线程异常：{e}"))
    .and_then(|inner| inner);

    match res {
        Ok((reply, model, latency_ms)) => TestResult {
            ok: true,
            latency_ms,
            model: Some(model.clone()),
            message: Some(format!("连接正常（{model} · {latency_ms}ms · 回复：{reply}）")),
            error: None,
        },
        Err(error) => TestResult {
            ok: false,
            latency_ms: 0,
            model: None,
            message: None,
            error: Some(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalization_handles_standard_and_full_paths() {
        assert_eq!(
            build_chat_url("https://api.minimaxi.com/v1").unwrap(),
            "https://api.minimaxi.com/v1/chat/completions"
        );
        assert_eq!(
            build_chat_url("http://127.0.0.1:18090/v1/").unwrap(),
            "http://127.0.0.1:18090/v1/chat/completions"
        );
        assert_eq!(
            build_chat_url("https://api.openai.com/v1/chat/completions").unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert!(build_chat_url("").is_err());
        assert!(build_chat_url("ftp://example.com/v1").is_err());
        assert!(build_chat_url("https://a.com/v1\nheader=x").is_err());
    }

    #[test]
    fn api_key_masking_never_exposes_middle() {
        assert_eq!(mask_api_key(""), "");
        assert_eq!(mask_api_key("12345678"), "********");
        assert_eq!(mask_api_key("demo-1234567890abcd"), "demo****abcd");
    }

    #[test]
    fn think_block_stripping_works() {
        assert_eq!(
            strip_think_blocks("<think>推理过程</think>\n你好，这是答案。"),
            "你好，这是答案。"
        );
        assert_eq!(strip_think_blocks("<think>没闭合的思考"), "");
        assert_eq!(strip_think_blocks("纯正文"), "纯正文");
    }

    #[test]
    fn split_curl_output_extracts_body_and_status() {
        let raw = "{\"ok\":true}\n__HTTP_STATUS__:200";
        let (body, status) = split_curl_output(raw).unwrap();
        assert_eq!(body, "{\"ok\":true}");
        assert_eq!(status, 200);
    }

    #[test]
    fn merge_config_preserves_and_clears_key_properly() {
        let mut base = LlmConfig::default();
        base.custom_api_key = "demo-placeholder".into();

        let kept = merge_config(
            &base,
            LlmConfigInput {
                mode: LlmMode::Custom,
                local_base_url: "http://127.0.0.1:18090/v1".into(),
                local_model: "local".into(),
                custom_base_url: "https://api.minimaxi.com/v1".into(),
                custom_api_key: None,
                custom_model: "MiniMax-M2.7".into(),
                timeout_secs: Some(60),
                disable_thinking: Some(true),
            },
        );
        assert_eq!(kept.custom_api_key, "demo-placeholder");

        let cleared = merge_config(
            &kept,
            LlmConfigInput {
                mode: LlmMode::Custom,
                local_base_url: "http://127.0.0.1:18090/v1".into(),
                local_model: "local".into(),
                custom_base_url: "https://api.minimaxi.com/v1".into(),
                custom_api_key: Some("".into()),
                custom_model: "MiniMax-M2.7".into(),
                timeout_secs: None,
                disable_thinking: None,
            },
        );
        assert_eq!(cleared.custom_api_key, "");
    }

    #[test]
    fn send_completion_roundtrip_over_real_local_http() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().unwrap().port();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();

            assert!(req.contains("POST /v1/chat/completions HTTP/1.1"));
            assert!(req.to_lowercase().contains("authorization: bearer demo-roundtrip"));

            let body = r#"{"model":"MiniMax-M2.7","choices":[{"message":{"content":"<think>内心的想法</think>你好，收到。"}}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let cfg = LlmConfig {
            mode: LlmMode::Custom,
            custom_base_url: format!("http://127.0.0.1:{port}/v1"),
            custom_api_key: "demo-roundtrip".into(),
            custom_model: "MiniMax-M2.7".into(),
            timeout_secs: 10,
            ..LlmConfig::default()
        };

        let (reply, model, _) = send_completion_blocking(
            &cfg,
            vec![ChatMessage {
                role: "user".into(),
                content: "测试".into(),
            }],
            32,
        )
        .expect("roundtrip should succeed");

        server.join().unwrap();
        assert_eq!(model, "MiniMax-M2.7");
        assert_eq!(reply, "你好，收到。");
    }

    #[test]
    fn send_completion_reports_401_clearly() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().unwrap().port();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = r#"{"error":{"message":"invalid api key"}}"#;
            let resp = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let cfg = LlmConfig {
            mode: LlmMode::Custom,
            custom_base_url: format!("http://127.0.0.1:{port}/v1"),
            custom_api_key: "demo-bad".into(),
            custom_model: "MiniMax-M2.7".into(),
            timeout_secs: 10,
            ..LlmConfig::default()
        };

        let err = send_completion_blocking(
            &cfg,
            vec![ChatMessage {
                role: "user".into(),
                content: "测试".into(),
            }],
            16,
        )
        .unwrap_err();

        server.join().unwrap();
        assert!(err.contains("401"));
        assert!(err.contains("API Key"));
    }

    #[test]
    fn think_stream_splitter_handles_split_tags_across_chunks() {
        let mut sp = ThinkStreamSplitter::new();
        let mut think = String::new();
        let mut body = String::new();

        for part in ["<th", "ink>先想", "一步</thi", "nk>最终", "回答"] {
            sp.push(part, |k, s| {
                if k == "thinking" {
                    think.push_str(s);
                } else {
                    body.push_str(s);
                }
            });
        }
        sp.finish(|k, s| {
            if k == "thinking" {
                think.push_str(s);
            } else {
                body.push_str(s);
            }
        });

        assert_eq!(think, "先想一步");
        assert_eq!(body, "最终回答");
    }

    #[test]
    fn send_stream_roundtrip_over_real_local_sse() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind sse mock");
        let port = listener.local_addr().unwrap().port();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);

            let sse = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/event-stream\r\n",
                "Connection: close\r\n\r\n",
                "data: {\"model\":\"Qwen3-8B\",\"choices\":[{\"delta\":{\"reasoning_content\":\"分析一下\"}}]}\n\n",
                "data: {\"model\":\"Qwen3-8B\",\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n\n",
                "data: {\"model\":\"Qwen3-8B\",\"choices\":[{\"delta\":{\"content\":\"，世界\"}}]}\n\n",
                "data: [DONE]\n\n"
            );
            stream.write_all(sse.as_bytes()).unwrap();
        });

        let cfg = LlmConfig {
            mode: LlmMode::Local,
            local_base_url: format!("http://127.0.0.1:{port}/v1"),
            local_model: "Qwen3-8B".into(),
            timeout_secs: 10,
            ..LlmConfig::default()
        };

        let mut events: Vec<(String, String)> = Vec::new();
        let res = send_stream_blocking(
            &cfg,
            vec![ChatMessage {
                role: "user".into(),
                content: "你好".into(),
            }],
            64,
            |k, s| events.push((k.to_string(), s.to_string())),
        )
        .expect("sse roundtrip should succeed");

        server.join().unwrap();
        assert_eq!(res.model, "Qwen3-8B");
        assert_eq!(res.thinking, "分析一下");
        assert_eq!(res.content, "你好，世界");
        assert_eq!(events.len(), 3);
    }
}
