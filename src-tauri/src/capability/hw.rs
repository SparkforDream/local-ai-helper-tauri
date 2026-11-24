// ============================================================
// capability/hw.rs —— 硬件能力
//
// 复用 assets/hardware.ps1（已验证 585ms 出完整 JSON），
// 不重复造轮子，也保证和「配置详情」页看到的是同一份数据。
// ============================================================

use super::{Args, Capability, Risk};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn capabilities() -> Vec<Capability> {
    vec![
        Capability {
            id: "hw.all",
            desc: "读这台机器的完整配置：CPU、内存、显卡、硬盘、系统版本",
            when: "用户问「我这台什么配置」「电脑怎么样」「我的机器行不行」时用这个",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "对象，含 cpu/memory/gpus/disks/environment 五个字段",
        },
        Capability {
            id: "hw.diagnose",
            desc: "按五项硬阈值全面体检卡顿根因：内存空闲、C盘剩余、磁盘响应时间、磁盘活动率、CPU是否降频",
            when: "用户说电脑卡、变慢、卡顿、反应迟钝，需要查真正瓶颈时用这个",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "对象：metrics（各项实测值）与 findings（命中的卡顿原因列表）",
        },
        Capability {
            id: "hw.cpu",
            desc: "读处理器型号、核心数、主频",
            when: "用户单独问 CPU，或需要判断 CPU 强弱",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "对象：name / physicalCores / logicalCores / baseMhz / avx2",
        },
        Capability {
            id: "hw.mem",
            desc: "读内存总量、空闲、条数、频率",
            when: "用户问「内存够不够」「为什么卡」，或需要判断能不能跑某软件",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "对象：totalBytes / freeBytes / moduleCount / moduleSpeedMhz",
        },
        Capability {
            id: "hw.gpu",
            desc: "读显卡型号与可用显存",
            when: "用户问能不能玩游戏、能不能跑 AI、问显卡型号",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "数组，每项含 name / vendor / vramBytes / vramSource",
        },
        Capability {
            id: "hw.disk",
            desc: "读所有固定硬盘的容量与剩余空间",
            when: "用户问「C盘还剩多少」「空间够不够装某个东西」",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "数组，每项含 letter / totalBytes / freeBytes",
        },
    ]
}

// ─────────────────────────────────────────── 采集

fn script_path() -> Option<PathBuf> {
    // 开发时：src-tauri/../assets/hardware.ps1
    // 打包后：exe 同级
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("assets").join("hardware.ps1"));
    if let Some(p) = dev {
        if p.exists() {
            return Some(p);
        }
    }
    let packed = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .map(|d| d.join("hardware.ps1"));
    packed.filter(|p| p.exists())
}

fn run_ps(script: &PathBuf) -> Result<Value, String> {
    // pwsh 优先，回落 powershell。小白机器上 PS7 常常没装。
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
        cmd.creation_flags(CREATE_NO_WINDOW);

        if let Ok(o) = cmd.output() {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if text.starts_with('{') {
                    return serde_json::from_str(&text)
                        .map_err(|e| format!("采集结果解析失败：{e}"));
                }
            }
        }
    }
    Err("采集脚本执行失败（PowerShell 可能被策略拦截）".into())
}

fn run_ps_inline(code: &str) -> Result<Value, String> {
    for exe in ["pwsh.exe", "powershell.exe"] {
        let mut cmd = Command::new(exe);
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            code,
        ]);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        if let Ok(o) = cmd.output() {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if text.starts_with('{') {
                    return serde_json::from_str(&text)
                        .map_err(|e| format!("体检结果解析失败：{e}"));
                }
            }
        }
    }
    Err("无法读取系统性能指标".into())
}

fn profile() -> Result<Value, String> {
    let p = script_path().ok_or_else(|| "找不到 hardware.ps1".to_string())?;
    run_ps(&p)
}

// ─────────────────────────────────────────── 实现

pub fn all(_: &Args) -> Result<Value, String> {
    profile()
}

/// 五项硬编码卡顿阈值判定（纯函数，便于单测）：
///   1. 内存空闲 < 15%      → 内存吃紧会卡
///   2. C 盘剩余 < 10%      → 系统盘满会拖慢
///   3. 磁盘响应时间 > 50ms  → 硬盘响应慢/老化
///   4. 磁盘活动时间 >= 95%  → 硬盘满载，瓶颈在硬盘不在 CPU/内存
///   5. CPU 频率 < 基频 50%  → CPU 严重降频
pub fn evaluate_lag_findings(metrics: &Value) -> Vec<Value> {
    let mut findings = Vec::new();

    if let Some(pct) = metrics.get("memFreePct").and_then(|v| v.as_f64()) {
        if pct < 15.0 {
            let free_gb = metrics.get("memFreeGb").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let total_gb = metrics.get("memTotalGb").and_then(|v| v.as_f64()).unwrap_or(0.0);
            findings.push(json!({
                "key": "mem_low",
                "severity": "high",
                "title": "内存不够用",
                "detail": format!("空闲只剩 {:.1} GB / {:.1} GB（{:.0}%，低于 15% 就会明显发卡）", free_gb, total_gb, pct),
                "advice": "关掉不用的浏览器标签页或后台占内存程序。",
            }));
        }
    }

    if let Some(pct) = metrics.get("cFreePct").and_then(|v| v.as_f64()) {
        if pct < 10.0 {
            let free_gb = metrics.get("cFreeGb").and_then(|v| v.as_f64()).unwrap_or(0.0);
            findings.push(json!({
                "key": "c_disk_low",
                "severity": "high",
                "title": "C 盘快满了",
                "detail": format!("C 盘只剩 {:.1} GB（占比 {:.0}%，低于 10% 会拖慢系统）", free_gb, pct),
                "advice": "先清理下载目录和回收站，或把大文件移到别的盘。",
            }));
        }
    }

    if let Some(ms) = metrics.get("diskRespMs").and_then(|v| v.as_f64()) {
        if ms > 50.0 {
            findings.push(json!({
                "key": "disk_slow",
                "severity": "high",
                "title": "硬盘响应太慢",
                "detail": format!("当前磁盘平均响应时间 {:.0} ms（超过 50 ms 说明硬盘吃力或老化）", ms),
                "advice": "如果在同时下载/杀毒先等它跑完；长期高于 50 ms 通常是机械盘或老化固态导致。",
            }));
        }
    }

    if let Some(active) = metrics.get("diskActivePct").and_then(|v| v.as_f64()) {
        if active >= 95.0 {
            findings.push(json!({
                "key": "disk_busy",
                "severity": "high",
                "title": "硬盘满载（100% 忙）",
                "detail": format!("磁盘活动时间 {:.0}%，卡顿瓶颈在硬盘读写，不在 CPU 或内存", active),
                "advice": "打开任务管理器按「磁盘」排序，看看是哪个程序在狂读写硬盘。",
            }));
        }
    }

    if let (Some(cur), Some(base)) = (
        metrics.get("cpuCurMhz").and_then(|v| v.as_f64()),
        metrics.get("cpuBaseMhz").and_then(|v| v.as_f64()),
    ) {
        if base > 0.0 && cur > 0.0 && (cur / base) < 0.5 {
            findings.push(json!({
                "key": "cpu_throttle",
                "severity": "high",
                "title": "处理器正在降频",
                "detail": format!("当前频率 {:.0} MHz，不到基准 {:.0} MHz 的一半（{:.0}%）", cur, base, cur / base * 100.0),
                "advice": "检查笔记本是否插上电源、散热风口有没有被堵住，或电源模式是不是切到了省电。",
            }));
        }
    }

    findings
}

pub fn diagnose(_: &Args) -> Result<Value, String> {
    let ps = r#"
$ErrorActionPreference = 'SilentlyContinue'
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$os  = Get-CimInstance Win32_OperatingSystem | Select-Object -First 1
$c   = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'" | Select-Object -First 1
$d   = Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk | Where-Object { $_.Name -eq '_Total' } | Select-Object -First 1

$memTotalGb = if ($os.TotalVisibleMemorySize) { [math]::Round($os.TotalVisibleMemorySize / 1048576, 2) } else { $null }
$memFreeGb  = if ($os.FreePhysicalMemory) { [math]::Round($os.FreePhysicalMemory / 1048576, 2) } else { $null }
$memFreePct = if ($os.TotalVisibleMemorySize) { [math]::Round(($os.FreePhysicalMemory / $os.TotalVisibleMemorySize) * 100, 1) } else { $null }

$cTotalGb   = if ($c.Size) { [math]::Round($c.Size / 1GB, 2) } else { $null }
$cFreeGb    = if ($c.FreeSpace) { [math]::Round($c.FreeSpace / 1GB, 2) } else { $null }
$cFreePct   = if ($c.Size) { [math]::Round(($c.FreeSpace / $c.Size) * 100, 1) } else { $null }

$diskRespMs = if ($null -ne $d.AvgDiskSecPerTransfer) { [math]::Round([double]$d.AvgDiskSecPerTransfer * 1000, 1) } else { $null }
$diskActPct = if ($null -ne $d.PercentDiskTime) { [math]::Min(100, [math]::Round([double]$d.PercentDiskTime, 1)) } else { $null }

[pscustomobject]@{
  cpuCurMhz     = if ($cpu.CurrentClockSpeed) { [int]$cpu.CurrentClockSpeed } else { $null }
  cpuBaseMhz    = if ($cpu.MaxClockSpeed) { [int]$cpu.MaxClockSpeed } else { $null }
  memTotalGb    = $memTotalGb
  memFreeGb     = $memFreeGb
  memFreePct    = $memFreePct
  cTotalGb      = $cTotalGb
  cFreeGb       = $cFreeGb
  cFreePct      = $cFreePct
  diskRespMs    = $diskRespMs
  diskActivePct = $diskActPct
} | ConvertTo-Json -Compress
"#;

    let metrics = run_ps_inline(ps)?;
    let findings = evaluate_lag_findings(&metrics);
    Ok(json!({
        "metrics": metrics,
        "findings": findings,
        "healthy": findings.is_empty(),
    }))
}

pub fn cpu(_: &Args) -> Result<Value, String> {
    let v = profile()?;
    v.get("cpu")
        .cloned()
        .ok_or_else(|| "没读到 CPU 信息".to_string())
}

pub fn mem(_: &Args) -> Result<Value, String> {
    let v = profile()?;
    v.get("memory")
        .cloned()
        .ok_or_else(|| "没读到内存信息".to_string())
}

pub fn gpu(_: &Args) -> Result<Value, String> {
    let v = profile()?;
    let gpus = v.get("gpus").cloned().unwrap_or_else(|| json!([]));

    // 核显的 AdapterRAM 是共享内存切片，虚高且无意义。
    // 这里只保留独显，并把可用显存算出来。
    let list = gpus.as_array().cloned().unwrap_or_default();
    let discrete: Vec<Value> = list
        .into_iter()
        .filter(|g| {
            matches!(
                g.get("vendor").and_then(|v| v.as_str()),
                Some("nvidia") | Some("amd")
            )
        })
        .map(|g| {
            let vram = g.get("vramBytes").and_then(|v| v.as_i64()).unwrap_or(0);
            // 驱动预留约 0.98GB，实测 RTX 4060 标称 8.0GB 实际可用 7.02GB
            let usable = (vram - 1_052_000_000).max(0);
            json!({
                "name": g.get("name"),
                "vendor": g.get("vendor"),
                "vramBytes": vram,
                "usableVramBytes": usable,
                "vramSource": g.get("vramSource"),
            })
        })
        .collect();

    Ok(json!({
        "discrete": discrete,
        "hasDiscreteGpu": !discrete.is_empty(),
    }))
}

pub fn disk(_: &Args) -> Result<Value, String> {
    let v = profile()?;
    let disks = v.get("disks").cloned().unwrap_or_else(|| json!([]));

    // 附上剩余比例，判断「够不够」时能直接用
    let list: Vec<Value> = disks
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            let total = d.get("totalBytes").and_then(|v| v.as_i64()).unwrap_or(0);
            let free = d.get("freeBytes").and_then(|v| v.as_i64()).unwrap_or(0);
            let ratio = if total > 0 {
                (free as f64 / total as f64 * 100.0).round() / 100.0
            } else {
                0.0
            };
            json!({
                "letter": d.get("letter"),
                "label": d.get("label"),
                "totalBytes": total,
                "freeBytes": free,
                "freeRatio": ratio,
            })
        })
        .collect();

    Ok(json!(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 五项卡顿硬阈值判定准确() {
        let bad = json!({
            "memTotalGb": 16.0,
            "memFreeGb": 1.6,
            "memFreePct": 10.0,
            "cFreeGb": 8.0,
            "cFreePct": 7.5,
            "diskRespMs": 120.0,
            "diskActivePct": 100.0,
            "cpuCurMhz": 800,
            "cpuBaseMhz": 2600,
        });
        let f = evaluate_lag_findings(&bad);
        assert_eq!(f.len(), 5);

        let good = json!({
            "memTotalGb": 16.0,
            "memFreeGb": 6.4,
            "memFreePct": 40.0,
            "cFreeGb": 80.0,
            "cFreePct": 35.0,
            "diskRespMs": 3.2,
            "diskActivePct": 8.0,
            "cpuCurMhz": 2500,
            "cpuBaseMhz": 2600,
        });
        assert!(evaluate_lag_findings(&good).is_empty());
    }
}
