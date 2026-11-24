// 采集器：调 PowerShell 读硬件，返回 JSON 给前端。
//
// 纪律：
//   1. 只读。不写注册表、不装驱动、不提权。
//   2. 脚本找不到就返回结构化错误，不 panic。
//   3. PowerShell 调用放阻塞线程池，不卡 UI。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

pub mod agent;
pub mod capability;
pub mod download;
pub mod history;
pub mod llm;
pub mod runtime;
pub mod search;
pub mod triage;

#[derive(Debug, Serialize, Deserialize)]
pub struct CollectResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 开发时脚本在项目根的 assets，打包后在 exe 同级目录。
fn script_path() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|p| p.join("assets").join("hardware.ps1"))
            .unwrap_or_else(|| PathBuf::from("assets/hardware.ps1"))
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_default()
            .join("hardware.ps1")
    }
}

/// 依次尝试 pwsh（PS7）与 powershell（5.1），谁先成功用谁。
/// 小白机器上 PS7 常常没装，5.1 一定在。
fn run_ps(script: &PathBuf) -> Option<String> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;

    for exe in ["pwsh.exe", "powershell.exe"] {
        let mut cmd = Command::new(exe);
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);

        if let Ok(o) = cmd.output() {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if text.starts_with('{') {
                    return Some(text);
                }
            }
        }
    }
    None
}

#[tauri::command]
async fn collect_hardware() -> CollectResult {
    let script = script_path();

    if !script.exists() {
        return CollectResult {
            ok: false,
            profile: None,
            error: Some(format!("采集脚本不存在: {}", script.display())),
        };
    }

    let result = tauri::async_runtime::spawn_blocking(move || run_ps(&script))
        .await
        .ok()
        .flatten();

    match result {
        Some(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => CollectResult {
                ok: true,
                profile: Some(v),
                error: None,
            },
            Err(e) => CollectResult {
                ok: false,
                profile: None,
                error: Some(format!("JSON 解析失败: {e}")),
            },
        },
        None => CollectResult {
            ok: false,
            profile: None,
            error: Some("PowerShell 执行失败或被策略拦截".into()),
        },
    }
}

#[tauri::command]
fn open_external(app: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    // 只允许 https，防止被拼出本地路径
    if !url.starts_with("https://") {
        return Err("只允许 https 链接".into());
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// ══════════════════════════════════════════════
// 能力清单接口
//
// 模型和前端都靠这三个命令工作：
//   capability_list   —— 读清单，知道「能做什么」
//   capability_call   —— 执行一个能力
//   capability_check  —— 只校验不执行，用来在执行前给用户看
// ══════════════════════════════════════════════

#[tauri::command]
fn capability_list() -> serde_json::Value {
    serde_json::json!(capability::all())
}

#[tauri::command]
fn capability_call(
    id: String,
    args: serde_json::Value,
) -> capability::Outcome {
    capability::execute(&id, args)
}

/// 执行前预检：这个能力存在吗、参数对不对、要不要用户确认。
/// 不做任何实际操作。
#[tauri::command]
fn capability_check(
    id: String,
    args: serde_json::Value,
) -> serde_json::Value {
    let caps = capability::all();
    let cap = caps.iter().find(|c| c.id == id);

    match cap {
        None => serde_json::json!({
            "known": false,
            "error": format!("未知能力：{id}"),
        }),
        Some(c) => {
            let a = capability::Args::new(args);
            let valid = capability::validate(c, &a);
            serde_json::json!({
                "known": true,
                "id": c.id,
                "desc": c.desc,
                "risk": c.risk,
                "riskLabel": c.risk.label(),
                "needsConfirm": c.risk.needs_confirm(),
                "argsValid": valid.is_ok(),
                "argsError": valid.err(),
            })
        }
    }
}

// ══════════════════════════════════════════════
// 四步编排接口
//
// 前端持有 FlowState，每次调用把状态传回来，
// 后端根据当前阶段决定下一步 —— 状态机在 Rust 侧，
// 前端只负责显示和收集用户点击。
// ══════════════════════════════════════════════

#[tauri::command]
fn flow_start(recipe: String) -> serde_json::Value {
    serde_json::json!(capability::flow::start(&recipe))
}

#[tauri::command]
fn flow_advance(
    state: capability::flow::FlowState,
    action: String,
) -> serde_json::Value {
    serde_json::json!(capability::flow::advance(&state, &action))
}

/// 列出支持一句话安装的软件
#[tauri::command]
fn flow_recipes() -> serde_json::Value {
    serde_json::json!(capability::recipe::RECIPES
        .iter()
        .map(|r| serde_json::json!({
            "id": r.id,
            "name": r.name,
            "sizeMB": (r.size as f64 / 1_048_576.0 * 10.0).round() / 10.0,
            "silent": r.install_args.is_some(),
        }))
        .collect::<Vec<_>>())
}

// ══════════════════════════════════════════════
// 统一模型接口（本地 llama-server / 自定义 OpenAI API）
// ══════════════════════════════════════════════

#[tauri::command]
fn llm_config_get() -> llm::LlmConfigView {
    llm::to_view(&llm::load_config())
}

#[tauri::command]
fn llm_config_save(input: llm::LlmConfigInput) -> Result<llm::LlmConfigView, String> {
    let existing = llm::load_config();
    let merged = llm::merge_config(&existing, input);
    llm::save_config(&merged)?;
    Ok(llm::to_view(&merged))
}

#[tauri::command]
async fn llm_test(input: Option<llm::LlmConfigInput>) -> llm::TestResult {
    llm::test_connection(input).await
}

#[tauri::command]
async fn llm_chat(
    messages: Vec<llm::ChatMessage>,
    hardware_summary: Option<String>,
) -> llm::LlmReply {
    llm::chat(messages, hardware_summary).await
}

#[tauri::command]
fn llm_cancel_stream(stream_id: String) -> bool {
    llm::cancel_stream(&stream_id)
}

// ══════════════════════════════════════════════
// 受控 Agent 接口（待办规划 + 权限闸门 + 逐项执行）
// ══════════════════════════════════════════════

#[tauri::command]
async fn agent_turn(
    app: tauri::AppHandle,
    stream_id: String,
    messages: Vec<llm::ChatMessage>,
    hardware_summary: Option<String>,
    permission: agent::PermissionMode,
    web_search: Option<bool>,
) -> agent::AgentTurnResponse {
    agent::run_turn(
        app,
        stream_id,
        messages,
        hardware_summary,
        permission,
        web_search.unwrap_or(true),
    )
    .await
}

#[tauri::command]
fn agent_decide(
    session: agent::AgentSession,
    decision: String,
    permission: agent::PermissionMode,
) -> agent::AgentSession {
    agent::decide_step(session, &decision, permission)
}

#[tauri::command]
fn agent_audit_list() -> Vec<agent::AuditEntry> {
    agent::list_audit()
}

#[tauri::command]
fn agent_audit_clear() {
    agent::clear_audit();
}

// ══════════════════════════════════════════════
// 本地模型下载与下载源切换
// ══════════════════════════════════════════════

#[tauri::command]
fn download_config_get() -> download::DownloadConfig {
    download::load_config()
}

#[tauri::command]
fn download_config_save(config: download::DownloadConfig) -> Result<download::DownloadConfig, String> {
    download::save_config(config)
}

#[tauri::command]
fn download_list_status() -> Vec<download::ModelStatus> {
    download::list_status()
}

#[tauri::command]
fn download_model_start(app: tauri::AppHandle, tier: String) -> Result<String, String> {
    download::start_download(app, tier)
}

#[tauri::command]
fn download_model_cancel(tier: String) -> bool {
    download::cancel_download(&tier)
}

#[tauri::command]
fn download_import_gguf(path: String) -> Result<download::DownloadConfig, String> {
    download::import_local_gguf(&path)
}

#[tauri::command]
fn download_clear_imported() -> Result<download::DownloadConfig, String> {
    download::clear_imported_gguf()
}

#[tauri::command]
async fn download_pick_gguf() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(download::pick_gguf_dialog)
        .await
        .map_err(|e| format!("文件选择线程异常：{e}"))?
}

// ══════════════════════════════════════════════
// 目录授权白名单（P2-a）与小模型约束分诊（P1-a）
// ══════════════════════════════════════════════

#[tauri::command]
fn fs_policy_get() -> capability::fs::FsPolicy {
    capability::fs::load_policy()
}

#[tauri::command]
fn fs_policy_save(policy: capability::fs::FsPolicy) -> Result<capability::fs::FsPolicy, String> {
    capability::fs::save_policy(policy)
}

#[tauri::command]
async fn fs_pick_folder() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(capability::fs::pick_folder_dialog)
        .await
        .map_err(|e| format!("目录选择线程异常：{e}"))?
}

#[tauri::command]
async fn triage_classify(query: String) -> triage::TriageResult {
    triage::classify(query).await
}

// ══════════════════════════════════════════════
// 分层联网检索与安全过筛
// ══════════════════════════════════════════════

#[tauri::command]
fn search_config_get() -> search::SearchConfigView {
    search::to_view(&search::load_config())
}

#[tauri::command]
fn search_config_save(input: search::SearchConfigInput) -> Result<search::SearchConfigView, String> {
    let existing = search::load_config();
    let merged = search::merge_config(&existing, input);
    search::save_config(&merged)?;
    Ok(search::to_view(&merged))
}

#[tauri::command]
async fn search_test(input: Option<search::SearchConfigInput>) -> search::SearchOutcome {
    search::test_search(input).await
}

// ══════════════════════════════════════════════
// 多会话对话历史持久化
// ══════════════════════════════════════════════

#[tauri::command]
fn history_load() -> Vec<history::ChatThread> {
    history::load()
}

#[tauri::command]
fn history_save(threads: Vec<history::ChatThread>) -> Result<(), String> {
    history::save(threads)
}

// ══════════════════════════════════════════════
// 本地推理引擎托管（自动启动 / 停止 / 状态）
// ══════════════════════════════════════════════

#[tauri::command]
async fn runtime_status() -> runtime::RuntimeStatus {
    tauri::async_runtime::spawn_blocking(runtime::status)
        .await
        .unwrap_or_else(|_| runtime::RuntimeStatus {
            running: false,
            loading: false,
            managed: false,
            server_found: false,
            model_found: false,
            model_name: None,
            model_path: None,
            endpoint: "http://127.0.0.1:18090/v1".into(),
            pid: None,
            message: "状态检测线程异常".into(),
        })
}

#[tauri::command]
async fn runtime_start(tier: Option<String>) -> Result<runtime::RuntimeStatus, String> {
    tauri::async_runtime::spawn_blocking(move || runtime::start(tier))
        .await
        .map_err(|e| format!("启动线程异常：{e}"))?
}

#[tauri::command]
fn runtime_stop() -> bool {
    runtime::stop()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            collect_hardware,
            open_external,
            capability_list,
            capability_call,
            capability_check,
            flow_start,
            flow_advance,
            flow_recipes,
            llm_config_get,
            llm_config_save,
            llm_test,
            llm_chat,
            llm_cancel_stream,
            agent_turn,
            agent_decide,
            agent_audit_list,
            agent_audit_clear,
            download_config_get,
            download_config_save,
            download_list_status,
            download_model_start,
            download_model_cancel,
            download_import_gguf,
            download_clear_imported,
            download_pick_gguf,
            fs_policy_get,
            fs_policy_save,
            fs_pick_folder,
            triage_classify,
            search_config_get,
            search_config_save,
            search_test,
            history_load,
            history_save,
            runtime_status,
            runtime_start,
            runtime_stop
        ])
        .build(tauri::generate_context!())
        .expect("Tauri 初始化失败");

    app.run(|_app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            runtime::stop();
        }
    });
}
