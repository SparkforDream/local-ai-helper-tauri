// ============================================================
// history.rs —— 多会话对话历史本地持久化
//
// 保存路径：%LOCALAPPDATA%\local-ai-helper\chat_history.json
// 上限：最多保留最近 30 个会话，每个会话最多 80 条消息，避免文件无限膨胀。
// ============================================================

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const MAX_THREADS: usize = 30;
const MAX_MSGS_PER_THREAD: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMsg {
    pub who: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatThread {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    #[serde(default)]
    pub messages: Vec<StoredMsg>,
}

fn history_path() -> PathBuf {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if !base.trim().is_empty() {
            return PathBuf::from(base)
                .join("local-ai-helper")
                .join("chat_history.json");
        }
    }
    PathBuf::from("chat_history.json")
}

pub fn normalize_threads(mut threads: Vec<ChatThread>) -> Vec<ChatThread> {
    threads.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    threads.truncate(MAX_THREADS);
    for t in &mut threads {
        if t.messages.len() > MAX_MSGS_PER_THREAD {
            let drop_n = t.messages.len() - MAX_MSGS_PER_THREAD;
            t.messages.drain(0..drop_n);
        }
    }
    threads
}

pub fn load() -> Vec<ChatThread> {
    let Ok(raw) = fs::read_to_string(history_path()) else {
        return Vec::new();
    };
    let parsed = serde_json::from_str::<Vec<ChatThread>>(&raw).unwrap_or_default();
    normalize_threads(parsed)
}

pub fn save(threads: Vec<ChatThread>) -> Result<(), String> {
    let cleaned = normalize_threads(threads);
    let path = history_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建历史目录失败：{e}"))?;
    }
    let body = serde_json::to_string_pretty(&cleaned).map_err(|e| format!("序列化历史失败：{e}"))?;
    fs::write(&path, body).map_err(|e| format!("保存对话历史失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_sorts_and_caps_threads_and_messages() {
        let mut threads = Vec::new();
        for i in 0..35 {
            let mut msgs = Vec::new();
            for m in 0..90 {
                msgs.push(StoredMsg {
                    who: "user".into(),
                    text: format!("m{m}"),
                    thinking: None,
                    meta: None,
                    agent_session: None,
                });
            }
            threads.push(ChatThread {
                id: format!("t{i}"),
                title: format!("会话{i}"),
                updated_at: i as u64,
                messages: msgs,
            });
        }
        let out = normalize_threads(threads);
        assert_eq!(out.len(), MAX_THREADS);
        assert_eq!(out[0].id, "t34");
        assert_eq!(out[0].messages.len(), MAX_MSGS_PER_THREAD);
        assert_eq!(out[0].messages[0].text, "m10");
    }
}
