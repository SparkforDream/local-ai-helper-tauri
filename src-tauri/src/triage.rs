// ============================================================
// triage.rs —— 小模型约束分诊台（GBNF 语法约束 + Richer Prompt）
//
// 依据实测结论（HANDOFF.md）：
//   1. 生成换分类：小模型从「自由答题」变「分诊台」，命中确定性意图后直接交给 Rust 能力；
//   2. GBNF 语法约束解码（本地 llama-server）+ 温度 0 + 种子 42，物理上无法输出枚举外内容；
//   3. Richer Prompt（定义 + 6 条规则 + 18 条口语化 few-shot），含 dont_know 体面退路。
// ============================================================

use crate::llm::{self, ChatMessage};
use serde::{Deserialize, Serialize};

pub const TRIAGE_GBNF: &str = r#"root ::= "{\"intent\":\"" intent "\"}"
intent ::= "lag_diagnose" | "mem_top" | "disk_biggest" | "net_check" | "hw_overview" | "official_url" | "install_software" | "fs_action" | "open_sys_tool" | "dont_know"
"#;

pub const VALID_INTENTS: &[&str] = &[
    "lag_diagnose",
    "mem_top",
    "disk_biggest",
    "net_check",
    "hw_overview",
    "official_url",
    "install_software",
    "fs_action",
    "open_sys_tool",
    "dont_know",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageResult {
    pub ok: bool,
    pub intent: String,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntentPayload {
    intent: String,
}

fn triage_system_prompt() -> String {
    String::from(
        "你是电脑助手的前置意图分诊器。把用户的一句中文口语分到下面 10 个枚举之一，只输出 JSON：{\"intent\":\"<枚举>\"}\n\
         \n\
         意图定义：\n\
         - lag_diagnose：说电脑卡、变慢、转圈、风扇狂转、发烫、打开东西半天没反应\n\
         - mem_top：问哪个软件在吃内存、后台什么程序占资源最多\n\
         - disk_biggest：问 C 盘/D 盘变红了、空间满了、哪个文件夹最大、什么占地方\n\
         - net_check：问是不是断网了、网页打不开、连不上网、测一下网络\n\
         - hw_overview：问自己电脑什么配置、几代 CPU、显卡/显存/内存/硬盘多大\n\
         - official_url：找某款软件的正版官网或下载地址、怀疑搜到假站\n\
         - install_software：明确要帮忙下载安装某款软件\n\
         - fs_action：明确要求创建、删除空文件夹或检查某个路径是否存在\n\
         - open_sys_tool：明确要求打开任务管理器、存储设置、网络设置或系统更新\n\
         - dont_know：知识问答、概念解释、报错分析、游戏能不能玩、闲聊或其他不在以上 9 类的问题\n\
         \n\
         6 条判断规则：\n\
         1. 只说「电脑卡/转圈/不流畅」选 lag_diagnose；明确问「谁占内存/后台程序」选 mem_top。\n\
         2. 说「盘红了/空间不够找大文件夹」选 disk_biggest；只问「电脑配置/硬盘多大」选 hw_overview。\n\
         3. 说「网页打不开/上不了网/断网」选 net_check，不要和卡顿混在一起。\n\
         4. 明确要「新建/删掉某文件夹」选 fs_action；问「某文件夹能不能删」属于知识判断选 dont_know。\n\
         5. 含具体软件且问地址/防假站选 official_url；明确说「帮我装」选 install_software。\n\
         6. 拿不准或超出以上 9 类一律选 dont_know，不要硬猜。\n\
         \n\
         示例（18 条）：\n\
         「我这破本子点啥都转半天圈圈」-> {\"intent\":\"lag_diagnose\"}\n\
         「最近开机特别慢，风扇呼呼响」-> {\"intent\":\"lag_diagnose\"}\n\
         「看看后台啥玩意最费内存」-> {\"intent\":\"mem_top\"}\n\
         「到底哪个程序把内存吃光了」-> {\"intent\":\"mem_top\"}\n\
         「我这C盘条都变红了，查查啥占的」-> {\"intent\":\"disk_biggest\"}\n\
         「D盘快塞满了，找找最大的几个文件夹」-> {\"intent\":\"disk_biggest\"}\n\
         「网页一直刷不出来是不是没网了」-> {\"intent\":\"net_check\"}\n\
         「帮我看看现在网通不通」-> {\"intent\":\"net_check\"}\n\
         「我这电脑啥配置，带独显吗」-> {\"intent\":\"hw_overview\"}\n\
         「看看我内存几G、硬盘剩多少」-> {\"intent\":\"hw_overview\"}\n\
         「微信正版从哪下，怕下到捆绑」-> {\"intent\":\"official_url\"}\n\
         「staem官网是哪个，搜出来一堆广告」-> {\"intent\":\"official_url\"}\n\
         「帮我把7zip装一下」-> {\"intent\":\"install_software\"}\n\
         「帮我在D盘根目录建个123空文件夹」-> {\"intent\":\"fs_action\"}\n\
         「把D盘那个123空目录删掉」-> {\"intent\":\"fs_action\"}\n\
         「帮我打开一下任务管理器」-> {\"intent\":\"open_sys_tool\"}\n\
         「什么是固态硬盘和机械硬盘」-> {\"intent\":\"dont_know\"}\n\
         「这个叫Temp的文件夹挺大的能不能删」-> {\"intent\":\"dont_know\"}",
    )
}

pub fn parse_intent(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Ok(p) = serde_json::from_str::<IntentPayload>(trimmed) {
        if VALID_INTENTS.contains(&p.intent.as_str()) {
            return p.intent;
        }
    }
    if let (Some(s), Some(e)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if e > s {
            if let Ok(p) = serde_json::from_str::<IntentPayload>(&trimmed[s..=e]) {
                if VALID_INTENTS.contains(&p.intent.as_str()) {
                    return p.intent;
                }
            }
        }
    }
    "dont_know".into()
}

pub async fn classify(query: String) -> TriageResult {
    let q = query.trim().to_string();
    if q.is_empty() {
        return TriageResult {
            ok: true,
            intent: "dont_know".into(),
            latency_ms: 0,
            error: None,
        };
    }

    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: triage_system_prompt(),
        },
        ChatMessage {
            role: "user".into(),
            content: q,
        },
    ];

    match llm::complete_constrained(messages, 24, Some(TRIAGE_GBNF.to_string())).await {
        Ok((text, _, latency_ms)) => TriageResult {
            ok: true,
            intent: parse_intent(&text),
            latency_ms,
            error: None,
        },
        Err(error) => TriageResult {
            ok: false,
            intent: "dont_know".into(),
            latency_ms: 0,
            error: Some(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gbnf_grammar_covers_every_valid_intent() {
        for intent in VALID_INTENTS {
            assert!(
                TRIAGE_GBNF.contains(&format!("\"{intent}\"")),
                "GBNF 缺少意图：{intent}"
            );
        }
    }

    #[test]
    fn parse_intent_validates_enum_and_falls_back_to_dont_know() {
        assert_eq!(parse_intent(r#"{"intent":"lag_diagnose"}"#), "lag_diagnose");
        assert_eq!(
            parse_intent("```json\n{\"intent\":\"disk_biggest\"}\n```"),
            "disk_biggest"
        );
        assert_eq!(parse_intent(r#"{"intent":"hack_nasa"}"#), "dont_know");
        assert_eq!(parse_intent("不知道呢"), "dont_know");
    }
}
