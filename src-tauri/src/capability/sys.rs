// ============================================================
// capability/sys.rs —— 受限系统常用面板快捷打开
//
// 只放行固定白名单的只读/查看类系统界面（任务管理器、存储设置、网络设置、系统更新），
// 绝不允许执行任意程序或命令行。
// ============================================================

use super::{Args, Capability, Param, ParamType, Risk};
use serde_json::{json, Value};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn capabilities() -> Vec<Capability> {
    vec![Capability {
        id: "sys.open",
        desc: "打开 Windows 自带的常用查看界面（仅限任务管理器、存储空间、网络设置、系统更新）",
        when: "用户希望直接打开任务管理器、磁盘清理/存储设置、网络设置或系统更新页时使用",
        params: vec![Param {
            name: "target",
            desc: "只能取四个白名单值之一：taskmgr（任务管理器）/ storage（存储空间）/ network（网络设置）/ update（系统更新）",
            ty: ParamType::Str { max_len: Some(24) },
            required: true,
            default: None,
        }],
        risk: Risk::Reversible,
        returns: "对象：target / label / opened",
    }]
}

pub fn resolve_target(target: &str) -> Option<(&'static str, &'static str, &'static [&'static str])> {
    match target.trim().to_ascii_lowercase().as_str() {
        "taskmgr" => Some(("任务管理器", "taskmgr.exe", &[])),
        "storage" => Some(("存储空间设置", "explorer.exe", &["ms-settings:storagesense"])),
        "network" => Some(("网络设置", "explorer.exe", &["ms-settings:network"])),
        "update" => Some(("Windows 更新", "explorer.exe", &["ms-settings:windowsupdate"])),
        _ => None,
    }
}

pub fn open(args: &Args) -> Result<Value, String> {
    let raw = args.str("target").ok_or("缺少 target")?;
    let (label, exe, extra) = resolve_target(raw)
        .ok_or_else(|| format!("不支持打开该项：{raw}（仅允许 taskmgr / storage / network / update）"))?;

    let mut cmd = Command::new(exe);
    if !extra.is_empty() {
        cmd.args(extra);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.spawn()
        .map_err(|e| format!("打开{label}失败：{e}"))?;

    Ok(json!({
        "target": raw,
        "label": label,
        "opened": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 白名单只放行四个安全入口() {
        assert!(resolve_target("taskmgr").is_some());
        assert!(resolve_target("storage").is_some());
        assert!(resolve_target("network").is_some());
        assert!(resolve_target("update").is_some());
        assert!(resolve_target("cmd").is_none());
        assert!(resolve_target("regedit").is_none());
    }
}
