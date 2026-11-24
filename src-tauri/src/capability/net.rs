// ============================================================
// capability/net.rs —— 只读网络连通性自检
//
// 帮小白区分「完全断网 / DNS 解析异常 / 国内直连正常 / 魔搭下载源可达」，
// 复用系统 curl.exe，零额外 crate。
// ============================================================

use super::{Args, Capability, Risk};
use serde_json::{json, Value};
use std::process::Command;
use std::time::Instant;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn capabilities() -> Vec<Capability> {
    vec![Capability {
        id: "net.check",
        desc: "检测当前电脑是否联网、网页和模型下载源能否正常连上及延迟",
        when: "用户问「是不是断网了」「网页打不开」「下载不了模型是不是网络问题」时使用",
        params: vec![],
        risk: Risk::ReadOnly,
        returns: "对象：online / summary / targets（各站点可达性与毫秒延迟）",
    }]
}

fn probe_url(name: &str, url: &str) -> Value {
    let mut cmd = Command::new("curl.exe");
    cmd.args([
        "-sS",
        "-I",
        "--connect-timeout",
        "4",
        "--max-time",
        "6",
        "-o",
        "NUL",
        "-w",
        "%{http_code}",
        url,
    ]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let t0 = Instant::now();
    match cmd.output() {
        Ok(o) => {
            let ms = t0.elapsed().as_millis() as u64;
            let code_str = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let code = code_str.parse::<u16>().unwrap_or(0);
            let ok = o.status.success() && (200..400).contains(&code);
            let exit = o.status.code().unwrap_or(-1);
            let reason = if ok {
                "正常".to_string()
            } else if exit == 6 {
                "DNS 解析失败".to_string()
            } else if exit == 28 {
                "连接超时".to_string()
            } else if exit == 7 {
                "无法建立连接".to_string()
            } else {
                format!("异常（HTTP {code} / 退出码 {exit}）")
            };
            json!({
                "name": name,
                "url": url,
                "ok": ok,
                "ms": ms,
                "reason": reason,
            })
        }
        Err(e) => json!({
            "name": name,
            "url": url,
            "ok": false,
            "ms": 0,
            "reason": format!("检测命令启动失败：{e}"),
        }),
    }
}

pub fn check(_: &Args) -> Result<Value, String> {
    let targets = vec![
        probe_url("国内常规网页（百度）", "https://www.baidu.com"),
        probe_url("模型下载源（魔搭）", "https://modelscope.cn"),
    ];

    let any_ok = targets.iter().any(|t| t["ok"] == true);
    let all_ok = targets.iter().all(|t| t["ok"] == true);

    let summary = if all_ok {
        "网络连接正常，常规网页和魔搭下载源都能直接连上。"
    } else if any_ok {
        "基础网络已连上，但部分站点连不上（可能是代理或对应站点临时抽风）。"
    } else {
        "当前连不上外网，请检查 Wi-Fi、网线或路由器是否正常。"
    };

    Ok(json!({
        "online": any_ok,
        "allOk": all_ok,
        "summary": summary,
        "targets": targets,
    }))
}
