// ============================================================
// capability/proc.rs —— 进程能力
//
// 用 PowerShell 采集而非 Win32 API：跨 PowerShell 5.1/7 都稳，
// 且不需要 unsafe。代价是每次调用约 200-400ms，可接受。
//
// 全部只读。结束进程放在后续版本，且必须是 Reversible 以上风险。
// ============================================================

use super::{Args, Capability, Param, ParamType, Risk};
use serde_json::{json, Value};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn capabilities() -> Vec<Capability> {
    vec![
        Capability {
            id: "proc.top",
            desc: "列出占用内存最多的进程",
            when: "用户说电脑卡、慢、风扇响，需要找出是谁在吃资源",
            params: vec![
                Param {
                    name: "n",
                    desc: "要几个，默认 8",
                    ty: ParamType::Int { min: Some(1), max: Some(30) },
                    required: false,
                    default: Some(json!(8)),
                },
                Param {
                    name: "by",
                    desc: "排序依据：mem（内存）或 cpu（CPU）。默认 mem",
                    ty: ParamType::Str { max_len: Some(8) },
                    required: false,
                    default: Some(json!("mem")),
                },
            ],
            risk: Risk::ReadOnly,
            returns: "数组，每项含 name / memMB / pid",
        },
        Capability {
            id: "proc.list",
            desc: "列出所有正在运行的程序名",
            when: "用户问「我开了哪些程序」「某软件是不是在跑」",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "去重后的程序名数组",
        },
    ]
}

fn run_ps(script: &str) -> Result<String, String> {
    for exe in ["pwsh.exe", "powershell.exe"] {
        let mut cmd = Command::new(exe);
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ]);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd.output();

        if let Ok(o) = out {
            if o.status.success() {
                let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !t.is_empty() {
                    return Ok(t);
                }
            }
        }
    }
    Err("PowerShell 执行失败".into())
}

pub fn top(args: &Args) -> Result<Value, String> {
    let n = args.int_or("n", 8, 1, 30);
    let by = args.str("by").unwrap_or("mem");
    // 白名单校验，不允许拼进命令
    let sort = match by {
        "cpu" => "CPU",
        _ => "WorkingSet64",
    };

    // 用 ConvertTo-Json 保证输出可解析。
    // 只取前 N 个，避免进程多时输出过大。
    let script = format!(
        "Get-Process | Sort-Object {} -Descending | Select-Object -First {} \
         Name, Id, WorkingSet64, CPU | ConvertTo-Json -Compress",
        sort, n
    );

    let text = run_ps(&script)?;
    let parsed: Value =
        serde_json::from_str(&text).map_err(|e| format!("进程列表解析失败：{e}"))?;

    // ConvertTo-Json 单条时返回对象而不是数组，统一成数组
    let list = match parsed {
        Value::Array(a) => a,
        v @ Value::Object(_) => vec![v],
        _ => vec![],
    };

    let out: Vec<Value> = list
        .into_iter()
        .map(|p| {
            let bytes = p.get("WorkingSet64").and_then(|v| v.as_i64()).unwrap_or(0);
            json!({
                "name": p.get("Name"),
                "pid": p.get("Id"),
                "memMB": (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0,
            })
        })
        .collect();

    Ok(json!(out))
}

pub fn list(_: &Args) -> Result<Value, String> {
    let script =
        "Get-Process | Select-Object -ExpandProperty Name -Unique | Sort-Object | ConvertTo-Json -Compress";
    let text = run_ps(script)?;
    let parsed: Value =
        serde_json::from_str(&text).map_err(|e| format!("程序列表解析失败：{e}"))?;
    let list = match parsed {
        Value::Array(a) => a,
        v @ Value::String(_) => vec![v],
        _ => vec![],
    };
    Ok(json!(list))
}
