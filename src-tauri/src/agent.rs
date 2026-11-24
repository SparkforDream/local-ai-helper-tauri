// ============================================================
// agent.rs —— 受控轻度 Agent（待办规划 + 权限闸门 + 逐步执行）
//
// 流程：
//   1. 模型收到自然语言请求与能力清单：
//      - 普通问答：直接返回人话文字；
//      - 本机操作：输出 JSON 待办计划 {"plan":[{title, capability, args}]}。
//   2. Rust 逐条用 capability::all() + validate() 核验，
//      风险等级强制以 Rust 注册表为准（绝不信任模型自报风险）。
//   3. 权限闸门：
//      - ReadOnly：直接执行；
//      - Reversible / Dangerous：
//          * review（请求审查，默认）：暂停在 WaitingConfirm，弹窗等待确认；
//          * auto（完全权限）：自动顺次执行（仍受 Rust 硬安全护栏约束）。
// ============================================================

use crate::capability::{self, Args, Risk};
use crate::llm::{self, ChatMessage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);
static AUDIT_SEQ: AtomicU64 = AtomicU64::new(1);

/// 允许 Agent 规划调用的能力白名单（不含安装配方，安装走独立四步状态机）。
const AGENT_CAPS: &[&str] = &[
    "fs.exists",
    "fs.mkdir",
    "fs.rmdir_empty",
    "fs.dir_size",
    "fs.biggest",
    "proc.top",
    "hw.all",
    "hw.diagnose",
    "net.check",
    "sys.open",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: u64,
    pub timestamp_ms: u64,
    pub capability_id: String,
    pub title: String,
    pub target: String,
    pub risk_label: String,
    pub ok: bool,
    pub detail: String,
}

static AUDIT_LOG: OnceLock<Mutex<Vec<AuditEntry>>> = OnceLock::new();

fn audit_store() -> &'static Mutex<Vec<AuditEntry>> {
    AUDIT_LOG.get_or_init(|| Mutex::new(Vec::new()))
}

fn record_audit(step: &TodoStep, ok: bool, detail: &str) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let target = step
        .args
        .get("path")
        .or_else(|| step.args.get("target"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let entry = AuditEntry {
        id: AUDIT_SEQ.fetch_add(1, Ordering::Relaxed),
        timestamp_ms: ts,
        capability_id: step.capability_id.clone(),
        title: step.title.clone(),
        target,
        risk_label: step.risk_label.clone(),
        ok,
        detail: detail.chars().take(180).collect(),
    };
    let mut guard = audit_store().lock().unwrap_or_else(|e| e.into_inner());
    guard.push(entry);
    if guard.len() > 100 {
        let drop_n = guard.len() - 100;
        guard.drain(0..drop_n);
    }
}

pub fn list_audit() -> Vec<AuditEntry> {
    let guard = audit_store().lock().unwrap_or_else(|e| e.into_inner());
    guard.iter().rev().cloned().collect()
}

pub fn clear_audit() {
    let mut guard = audit_store().lock().unwrap_or_else(|e| e.into_inner());
    guard.clear();
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Review,
    Auto,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::Review
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    WaitingConfirm,
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    WaitingConfirm,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoStep {
    pub id: usize,
    pub title: String,
    pub capability_id: String,
    pub args: Value,
    pub risk: Risk,
    pub risk_label: String,
    pub needs_confirm: bool,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub goal: String,
    pub permission: PermissionMode,
    pub steps: Vec<TodoStep>,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnResponse {
    pub ok: bool,
    /// "chat" | "task" | "error"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<AgentSession>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    plan: Vec<RawPlanStep>,
}

#[derive(Debug, Deserialize)]
struct RawPlanStep {
    #[serde(default)]
    title: String,
    #[serde(alias = "capability_id", alias = "tool", alias = "name")]
    capability: String,
    #[serde(default, alias = "arguments")]
    args: Value,
}

fn agent_system_prompt(hardware_summary: Option<&str>) -> String {
    let caps = capability::all();
    let mut cap_lines = String::new();
    for c in caps.iter().filter(|c| AGENT_CAPS.contains(&c.id)) {
        let params: Vec<String> = c
            .params
            .iter()
            .map(|p| format!("{}{}({})", p.name, if p.required { "*" } else { "" }, p.desc))
            .collect();
        cap_lines.push_str(&format!(
            "- {} [{}]：{} | 参数：{}\n",
            c.id,
            c.risk.label(),
            c.desc,
            if params.is_empty() {
                "无".to_string()
            } else {
                params.join("，")
            }
        ));
    }

    let mut prompt = format!(
        "你是一个面向电脑新手和长辈的本地电脑助手，具备受控任务执行能力。\n\
         \n\
         当前可调用的本机能力清单：\n\
         {cap_lines}\n\
         工作规则（严格遵守）：\n\
         1. 如果对方只是普通提问、聊天、咨询知识，不需要查看或操作电脑，直接用简短人话回答。\n\
         2. 如果对方明确要求操作电脑或检查某个路径（例如在 D 盘根目录新建/删除空文件夹、检查文件夹是否存在、算目录大小），\
            必须只输出一个 JSON 待办计划，不要加多余解释，也绝不能口头假装已经执行完成。\n\
         3. JSON 格式严格如下：\n\
            {{\"plan\":[{{\"title\":\"简洁步骤说明\",\"capability\":\"能力id\",\"args\":{{\"path\":\"D:\\\\123\"}}}}]}}\n\
         4. 单次最多规划 6 步；能力 id 必须严格从上面清单中选；Windows 路径在 JSON 里用双反斜杠，例如 \"D:\\\\123\"。\n\
         5. 若对方要删除非空文件夹、改注册表、格盘或超出清单的操作，直接用人话说明不在安全权限范围内，不输出 JSON。\n\
         \n\
         示例 1：对方说「在 D 盘根目录下建一个叫 123 的空文件夹」\n\
         输出：{{\"plan\":[{{\"title\":\"在 D 盘根目录创建文件夹 123\",\"capability\":\"fs.mkdir\",\"args\":{{\"path\":\"D:\\\\123\"}}}},{{\"title\":\"确认 D:\\\\123 已创建\",\"capability\":\"fs.exists\",\"args\":{{\"path\":\"D:\\\\123\"}}}}]}}\n\
         \n\
         示例 2：对方说「把 D 盘根目录那个 123 空文件夹删了」\n\
         输出：{{\"plan\":[{{\"title\":\"检查 D:\\\\123 是否存在且为空\",\"capability\":\"fs.exists\",\"args\":{{\"path\":\"D:\\\\123\"}}}},{{\"title\":\"删除空文件夹 D:\\\\123\",\"capability\":\"fs.rmdir_empty\",\"args\":{{\"path\":\"D:\\\\123\"}}}}]}}"
    );

    if let Some(hw) = hardware_summary {
        let hw = hw.trim();
        if !hw.is_empty() {
            prompt.push_str("\n\n当前这台电脑的实测配置（回答配置问题时严格以这些数字为准）：\n");
            prompt.push_str(hw);
        }
    }

    prompt
}

fn normalize_cap_id(raw: &str) -> String {
    let s = raw.trim();
    if AGENT_CAPS.contains(&s) {
        return s.to_string();
    }
    // 兼容把点写成下划线的情况（如 fs_mkdir -> fs.mkdir）
    for &id in AGENT_CAPS {
        if id.replace('.', "_") == s {
            return id.to_string();
        }
    }
    s.to_string()
}

/// 从模型回复文本中提取 JSON 待办计划（支持纯 JSON、```json 代码块、以及 <tool_call>）。
fn parse_plan_from_text(text: &str) -> Option<Vec<RawPlanStep>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. 直接就是 JSON 对象
    if let Ok(p) = serde_json::from_str::<RawPlan>(trimmed) {
        if !p.plan.is_empty() {
            return Some(p.plan);
        }
    }

    // 2. 提取 ```json ... ``` 或文本中的第一个包含 "plan" 的 JSON 对象
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                let candidate = &trimmed[start..=end];
                if let Ok(p) = serde_json::from_str::<RawPlan>(candidate) {
                    if !p.plan.is_empty() {
                        return Some(p.plan);
                    }
                }
            }
        }
    }

    // 3. 兼容 Qwen3 的 <tool_call>{...}</tool_call>
    let mut steps = Vec::new();
    let mut rest = trimmed;
    while let Some(s) = rest.find("<tool_call>") {
        let after = &rest[s + 11..];
        let Some(e) = after.find("</tool_call>") else {
            break;
        };
        let chunk = after[..e].trim();
        if let Ok(v) = serde_json::from_str::<Value>(chunk) {
            if let Some(arr) = v.get("plan").and_then(|x| x.as_array()) {
                for item in arr {
                    if let Ok(step) = serde_json::from_value::<RawPlanStep>(item.clone()) {
                        steps.push(step);
                    }
                }
            } else if let Some(name) = v
                .get("name")
                .or_else(|| v.get("capability"))
                .and_then(|x| x.as_str())
            {
                let args = v
                    .get("arguments")
                    .or_else(|| v.get("args"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                steps.push(RawPlanStep {
                    title: String::new(),
                    capability: name.to_string(),
                    args,
                });
            }
        }
        rest = &after[e + 12..];
    }

    if steps.is_empty() {
        None
    } else {
        Some(steps)
    }
}

fn default_step_title(cap_id: &str, args: &Value, desc: &str) -> String {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    match cap_id {
        "fs.mkdir" if !path.is_empty() => format!("创建文件夹 {path}"),
        "fs.rmdir_empty" if !path.is_empty() => format!("删除空文件夹 {path}"),
        "fs.exists" if !path.is_empty() => format!("检查路径 {path}"),
        "fs.dir_size" if !path.is_empty() => format!("查看目录大小 {path}"),
        "fs.biggest" if !path.is_empty() => format!("找出 {path} 下最大的项"),
        _ => desc.to_string(),
    }
}

fn build_session(
    goal: &str,
    raw_steps: Vec<RawPlanStep>,
    permission: PermissionMode,
) -> Result<AgentSession, String> {
    let all_caps = capability::all();
    let mut steps = Vec::new();

    for (idx, rs) in raw_steps.into_iter().take(6).enumerate() {
        let cap_id = normalize_cap_id(&rs.capability);
        if !AGENT_CAPS.contains(&cap_id.as_str()) {
            return Err(format!("模型请求的动作不在允许清单内：{cap_id}"));
        }
        let cap = all_caps
            .iter()
            .find(|c| c.id == cap_id)
            .ok_or_else(|| format!("未知能力：{cap_id}"))?;

        let args_val = if rs.args.is_object() {
            rs.args
        } else {
            json!({})
        };
        let args = Args::new(args_val.clone());
        capability::validate(cap, &args)
            .map_err(|e| format!("步骤「{}」参数校验未通过：{e}", cap.desc))?;

        let title = if rs.title.trim().is_empty() {
            default_step_title(&cap_id, &args_val, cap.desc)
        } else {
            rs.title.trim().to_string()
        };

        steps.push(TodoStep {
            id: idx + 1,
            title,
            capability_id: cap_id,
            args: args_val,
            risk: cap.risk,
            risk_label: cap.risk.label().to_string(),
            needs_confirm: cap.risk.needs_confirm(),
            status: StepStatus::Pending,
            output: None,
            error: None,
        });
    }

    if steps.is_empty() {
        return Err("待办计划为空".into());
    }

    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    Ok(AgentSession {
        id: format!("task-{seq}"),
        goal: goal.to_string(),
        permission,
        steps,
        status: SessionStatus::Running,
        summary: None,
    })
}

fn format_bytes_human(b: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if (b as f64) >= GB {
        format!("{:.2} GB", b as f64 / GB)
    } else if (b as f64) >= MB {
        format!("{:.1} MB", b as f64 / MB)
    } else if b >= 1024 {
        format!("{} KB", b / 1024)
    } else {
        format!("{b} B")
    }
}

fn format_step_outcome(cap_id: &str, data: &Value) -> String {
    match cap_id {
        "fs.mkdir" => {
            let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if data.get("alreadyExists").and_then(|v| v.as_bool()) == Some(true) {
                format!("{path} 已经存在，无需重复创建")
            } else {
                format!("已创建文件夹：{path}")
            }
        }
        "fs.rmdir_empty" => {
            let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("");
            format!("已删除空文件夹：{path}")
        }
        "fs.exists" => {
            let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let ex = data.get("exists").and_then(|v| v.as_bool()).unwrap_or(false);
            let is_dir = data.get("isDir").and_then(|v| v.as_bool()).unwrap_or(false);
            let is_empty = data.get("isEmpty").and_then(|v| v.as_bool()).unwrap_or(false);
            if !ex {
                format!("{path} 当前不存在")
            } else if is_dir && is_empty {
                format!("{path} 存在，是空文件夹")
            } else if is_dir {
                format!("{path} 存在，是非空文件夹")
            } else {
                format!("{path} 存在，是普通文件")
            }
        }
        "fs.biggest" => {
            let items: Vec<String> = data
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .take(5)
                        .map(|e| {
                            let name = e.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            let sz = e.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
                            format!("{name}（{}）", format_bytes_human(sz))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if items.is_empty() {
                "该目录下没有可统计的大项".into()
            } else {
                format!("最大的前几项：{}", items.join("、"))
            }
        }
        "fs.dir_size" => {
            let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let total = data.get("totalBytes").and_then(|v| v.as_u64()).unwrap_or(0);
            format!("{path} 统计约 {}", format_bytes_human(total))
        }
        "hw.diagnose" => {
            let findings = data
                .get("findings")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if findings.is_empty() {
                "五项卡顿硬指标目前都在正常范围内".into()
            } else {
                let titles: Vec<String> = findings
                    .iter()
                    .filter_map(|f| {
                        let t = f.get("title")?.as_str()?;
                        let d = f.get("detail").and_then(|v| v.as_str()).unwrap_or("");
                        Some(format!("{t}（{d}）"))
                    })
                    .collect();
                format!("检出 {} 项卡顿原因：{}", titles.len(), titles.join("；"))
            }
        }
        "net.check" => data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("网络检测完成")
            .to_string(),
        "sys.open" => {
            let label = data.get("label").and_then(|v| v.as_str()).unwrap_or("系统界面");
            format!("已打开 {label}")
        }
        "proc.top" => {
            let items: Vec<String> = data
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .take(4)
                        .map(|p| {
                            let n = p.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            let mb = p.get("memMB").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            format!("{n}({:.0}MB)", mb)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if items.is_empty() {
                "未读取到进程占用".into()
            } else {
                format!("内存前几名：{}", items.join("、"))
            }
        }
        _ => {
            let s = serde_json::to_string(data).unwrap_or_default();
            let short: String = s.chars().take(120).collect();
            short
        }
    }
}

fn run_single_step(step: &mut TodoStep) -> bool {
    let out = capability::execute(&step.capability_id, step.args.clone());
    if out.ok {
        let formatted = format_step_outcome(
            &step.capability_id,
            out.data.as_ref().unwrap_or(&Value::Null),
        );
        step.status = StepStatus::Done;
        step.output = Some(formatted.clone());
        step.error = None;
        record_audit(step, true, &formatted);
        true
    } else {
        let err = out.error.unwrap_or_else(|| "执行失败".into());
        step.status = StepStatus::Failed;
        step.error = Some(err.clone());
        record_audit(step, false, &err);
        false
    }
}

fn build_summary(session: &AgentSession) -> String {
    let done = session
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Done)
        .count();
    let total = session.steps.len();

    match session.status {
        SessionStatus::WaitingConfirm => {
            if let Some(step) = session
                .steps
                .iter()
                .find(|s| s.status == StepStatus::WaitingConfirm)
            {
                format!("待办已建好，第 {} 步「{}」需要你审查确认。", step.id, step.title)
            } else {
                "等待你确认下一步操作。".into()
            }
        }
        SessionStatus::Done => {
            let last_out = session
                .steps
                .iter()
                .rev()
                .find_map(|s| s.output.as_deref())
                .unwrap_or("全部步骤已完成");
            format!("任务完成（{done}/{total} 步）：{last_out}。")
        }
        SessionStatus::Failed => {
            let err = session
                .steps
                .iter()
                .find_map(|s| s.error.as_deref())
                .unwrap_or("执行失败");
            format!("任务在第 {} 步停下：{err}", done + 1)
        }
        SessionStatus::Cancelled => format!("已取消剩余操作（此前已完成 {done}/{total} 步）。"),
        SessionStatus::Running => "正在执行待办…".into(),
    }
}

/// 推进状态机：自动执行无需确认的步，遇到需审查的高风险步立刻暂停。
pub fn advance_session(session: &mut AgentSession) {
    let total = session.steps.len();
    for idx in 0..total {
        if session.steps[idx].status != StepStatus::Pending {
            continue;
        }

        let need_gate =
            session.steps[idx].needs_confirm && session.permission == PermissionMode::Review;
        if need_gate {
            session.steps[idx].status = StepStatus::WaitingConfirm;
            session.status = SessionStatus::WaitingConfirm;
            session.summary = Some(build_summary(session));
            return;
        }

        let ok = run_single_step(&mut session.steps[idx]);
        if !ok {
            for rem in (idx + 1)..total {
                if session.steps[rem].status == StepStatus::Pending {
                    session.steps[rem].status = StepStatus::Skipped;
                }
            }
            session.status = SessionStatus::Failed;
            session.summary = Some(build_summary(session));
            return;
        }
    }

    session.status = SessionStatus::Done;
    session.summary = Some(build_summary(session));
}

pub fn decide_step(
    mut session: AgentSession,
    decision: &str,
    permission: PermissionMode,
) -> AgentSession {
    session.permission = permission;

    let Some(idx) = session
        .steps
        .iter()
        .position(|s| s.status == StepStatus::WaitingConfirm)
    else {
        session.summary = Some(build_summary(&session));
        return session;
    };

    match decision {
        "approve" => {
            let ok = run_single_step(&mut session.steps[idx]);
            if !ok {
                for rem in (idx + 1)..session.steps.len() {
                    if session.steps[rem].status == StepStatus::Pending {
                        session.steps[rem].status = StepStatus::Skipped;
                    }
                }
                session.status = SessionStatus::Failed;
                session.summary = Some(build_summary(&session));
                return session;
            }
            advance_session(&mut session);
            session
        }
        "skip" => {
            session.steps[idx].status = StepStatus::Skipped;
            session.steps[idx].output = Some("已跳过".into());
            advance_session(&mut session);
            session
        }
        _ => {
            for rem in idx..session.steps.len() {
                if matches!(
                    session.steps[rem].status,
                    StepStatus::WaitingConfirm | StepStatus::Pending
                ) {
                    session.steps[rem].status = StepStatus::Skipped;
                }
            }
            session.status = SessionStatus::Cancelled;
            session.summary = Some(build_summary(&session));
            session
        }
    }
}

pub async fn run_turn(
    app: AppHandle,
    stream_id: String,
    history: Vec<ChatMessage>,
    hardware_summary: Option<String>,
    permission: PermissionMode,
) -> AgentTurnResponse {
    let goal = history
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim().to_string())
        .unwrap_or_default();

    let mut messages = Vec::with_capacity(history.len().min(12) + 1);
    messages.push(ChatMessage {
        role: "system".into(),
        content: agent_system_prompt(hardware_summary.as_deref()),
    });

    let start = history.len().saturating_sub(12);
    for m in history.into_iter().skip(start) {
        if m.role == "user" || m.role == "assistant" {
            let c = m.content.trim().to_string();
            if !c.is_empty() {
                messages.push(ChatMessage {
                    role: m.role,
                    content: c,
                });
            }
        }
    }

    let app_clone = app.clone();
    let sid = stream_id.clone();
    let stream_res = llm::complete_messages_stream(messages, 1024, move |kind, delta| {
        let _ = app_clone.emit(
            "llm-stream",
            json!({
                "streamId": sid,
                "kind": kind,
                "delta": delta,
            }),
        );
    })
    .await;

    let stream = match stream_res {
        Ok(v) => v,
        Err(error) => {
            return AgentTurnResponse {
                ok: false,
                kind: "error".into(),
                reply: None,
                thinking: None,
                model: None,
                latency_ms: 0,
                session: None,
                error: Some(error),
            };
        }
    };

    let thinking = if stream.thinking.is_empty() {
        None
    } else {
        Some(stream.thinking)
    };

    if let Some(raw_steps) = parse_plan_from_text(&stream.content) {
        match build_session(&goal, raw_steps, permission) {
            Ok(mut session) => {
                advance_session(&mut session);
                // 若只读观察步骤一次性跑完，且包含查询类能力，把实测结果回喂模型做 1 轮简洁人话总结（防死循环，最多 1 轮）
                let has_obs = session.status == SessionStatus::Done
                    && session.steps.iter().any(|s| {
                        matches!(
                            s.capability_id.as_str(),
                            "hw.diagnose" | "fs.biggest" | "fs.dir_size" | "net.check" | "proc.top" | "hw.all"
                        )
                    });
                if has_obs {
                    if let Some(synth) = synthesize_observation(&goal, &session).await {
                        session.summary = Some(synth);
                    }
                }
                let reply = session.summary.clone();
                return AgentTurnResponse {
                    ok: true,
                    kind: "task".into(),
                    reply,
                    thinking,
                    model: Some(stream.model),
                    latency_ms: stream.latency_ms,
                    session: Some(session),
                    error: None,
                };
            }
            Err(plan_err) => {
                return AgentTurnResponse {
                    ok: true,
                    kind: "chat".into(),
                    reply: Some(format!("这个操作被安全规则拦下了：{plan_err}")),
                    thinking,
                    model: Some(stream.model),
                    latency_ms: stream.latency_ms,
                    session: None,
                    error: None,
                };
            }
        }
    }

    AgentTurnResponse {
        ok: true,
        kind: "chat".into(),
        reply: Some(if stream.content.is_empty() {
            "模型没有返回文字内容。".into()
        } else {
            stream.content
        }),
        thinking,
        model: Some(stream.model),
        latency_ms: stream.latency_ms,
        session: None,
        error: None,
    }
}

async fn synthesize_observation(goal: &str, session: &AgentSession) -> Option<String> {
    let mut facts = String::new();
    for s in &session.steps {
        if let Some(out) = &s.output {
            facts.push_str(&format!("- {}: {}\n", s.title, out));
        }
    }
    if facts.trim().is_empty() {
        return None;
    }
    let msgs = vec![
        ChatMessage {
            role: "system".into(),
            content: "你是电脑助手。根据已经实测拿到的结果，用 2~3 句大白话直接给结论和下一步建议。严格使用给定数字，不编造、不说教、不输出 JSON。".into(),
        },
        ChatMessage {
            role: "user".into(),
            content: format!("原问题：{goal}\n\n实测结果：\n{facts}"),
        },
    ];
    let (reply, _, _) = llm::complete_messages(msgs, 256).await.ok()?;
    let trimmed = reply.trim().to_string();
    if trimmed.is_empty() || trimmed.starts_with('{') {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn 解析纯_json_和代码块和_tool_call() {
        let p1 = parse_plan_from_text(
            r#"{"plan":[{"title":"建123","capability":"fs.mkdir","args":{"path":"D:\\123"}}]}"#,
        )
        .unwrap();
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].capability, "fs.mkdir");

        let p2 = parse_plan_from_text(
            "好的，按步骤来：\n```json\n{\"plan\":[{\"title\":\"删123\",\"capability\":\"fs_rmdir_empty\",\"args\":{\"path\":\"D:\\\\123\"}}]}\n```",
        )
        .unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(normalize_cap_id(&p2[0].capability), "fs.rmdir_empty");

        let p3 = parse_plan_from_text(
            "<tool_call>{\"name\":\"fs.mkdir\",\"arguments\":{\"path\":\"D:\\\\123\"}}</tool_call>",
        )
        .unwrap();
        assert_eq!(p3.len(), 1);
    }

    #[test]
    fn 审查模式拦截建删空目录并在批准后完成() {
        let base = std::env::temp_dir().join(format!("local_ai_helper_agent_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let target = base.join("123");
        let t_str = target.to_string_lossy().to_string();

        let raw = vec![
            RawPlanStep {
                title: "创建 123".into(),
                capability: "fs.mkdir".into(),
                args: json!({ "path": t_str }),
            },
            RawPlanStep {
                title: "检查 123".into(),
                capability: "fs.exists".into(),
                args: json!({ "path": t_str }),
            },
            RawPlanStep {
                title: "删除空文件夹 123".into(),
                capability: "fs.rmdir_empty".into(),
                args: json!({ "path": t_str }),
            },
        ];

        let mut s = build_session("建完删 123", raw, PermissionMode::Review).unwrap();
        advance_session(&mut s);

        // 第 1 步是 Reversible，审查模式下必须停住，文件夹还没创建
        assert_eq!(s.status, SessionStatus::WaitingConfirm);
        assert_eq!(s.steps[0].status, StepStatus::WaitingConfirm);
        assert!(!target.exists());

        // 批准第 1 步 → 建好 → 第 2 步 ReadOnly 自动通过 → 停在第 3 步删除前
        let s = decide_step(s, "approve", PermissionMode::Review);
        assert!(target.exists());
        assert_eq!(s.steps[0].status, StepStatus::Done);
        assert_eq!(s.steps[1].status, StepStatus::Done);
        assert_eq!(s.steps[2].status, StepStatus::WaitingConfirm);
        assert_eq!(s.status, SessionStatus::WaitingConfirm);

        // 批准第 3 步 → 删除完成
        let s = decide_step(s, "approve", PermissionMode::Review);
        assert!(!target.exists());
        assert_eq!(s.status, SessionStatus::Done);
        assert_eq!(s.steps[2].status, StepStatus::Done);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn 完全权限模式一口气自动执行建和删() {
        let base = std::env::temp_dir().join(format!("local_ai_helper_agent_auto_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let target = base.join("123");
        let t_str = target.to_string_lossy().to_string();

        let raw = vec![
            RawPlanStep {
                title: "创建 123".into(),
                capability: "fs.mkdir".into(),
                args: json!({ "path": t_str }),
            },
            RawPlanStep {
                title: "删除空文件夹 123".into(),
                capability: "fs.rmdir_empty".into(),
                args: json!({ "path": t_str }),
            },
        ];

        let mut s = build_session("自动建删 123", raw, PermissionMode::Auto).unwrap();
        advance_session(&mut s);

        assert_eq!(s.status, SessionStatus::Done);
        assert_eq!(s.steps[0].status, StepStatus::Done);
        assert_eq!(s.steps[1].status, StepStatus::Done);
        assert!(!target.exists());

        let _ = fs::remove_dir_all(&base);
    }
}
