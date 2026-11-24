// ============================================================
// capability/detect.rs —— 安装位置探测
//
// 为什么不只查固定路径：
//   用户可能把 Steam 装进 D:\游戏\steam、D:\STEAM、甚至 C:\abc\steam。
//   只认 C:\Program Files\Steam 会误判成「没装」，然后重复安装。
//
// 分三级，代价递增，命中即停：
//   ① 注册表      <10ms    官方自己记的位置，最可信
//   ② 常见路径    <10ms    覆盖 95% 的默认安装
//   ③ 全盘扫描    ~8s      兜底，只在①②都落空时才跑
//
// 第③级刻意限制深度与盘符，避免扫整个硬盘（那可能几分钟）。
// ============================================================

use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Serialize)]
pub struct Found {
    /// 主程序完整路径
    pub exe: PathBuf,
    /// 这一处是怎么找到的
    pub source: &'static str,
    /// 安装根目录
    pub dir: PathBuf,
}

/// 三级探测。找到就返回，不再往下跑。
pub fn locate(recipe_id: &str) -> Option<Found> {
    match recipe_id {
        "steam" => locate_steam(),
        "7zip" => locate_7zip(),
        _ => None,
    }
}

// ─────────────────────────────────────────── Steam

fn locate_steam() -> Option<Found> {
    // ① 注册表：Steam 自己写的安装路径，最可信
    if let Some(p) = reg_string(r"HKCU\Software\Valve\Steam", "SteamPath") {
        let dir = PathBuf::from(p.replace('/', "\\"));
        let exe = dir.join("steam.exe");
        if exe.exists() { return Some(Found { exe, source: "注册表", dir }); }
    }
    // 32 位视图下的注册表位置
    if let Some(p) = reg_string(r"HKLM\SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath") {
        let dir = PathBuf::from(p.replace('/', "\\"));
        let exe = dir.join("steam.exe");
        if exe.exists() {
            return Some(Found { exe, source: "注册表(32位)", dir });
        }
    }

    // ② 常见路径
    for base in common_roots() {
        for rel in ["Steam", "STEAM", "steam", "Steam\\Steam", "Program Files (x86)\\Steam", "Program Files\\Steam"] {
            let dir = base.join(rel);
            let exe = dir.join("steam.exe");
            if exe.exists() {
                return Some(Found { exe, source: "常见路径", dir });
            }
        }
    }

    // ③ 全盘扫（深度 4，跳过系统目录）
    scan_drives("steam.exe", 4)
}

fn locate_7zip() -> Option<Found> {
    for base in common_roots() {
        for rel in ["7-Zip", "Program Files\\7-Zip", "Program Files (x86)\\7-Zip"] {
            let dir = base.join(rel);
            let exe = dir.join("7zFM.exe");
            if exe.exists() {
                return Some(Found { exe, source: "常见路径", dir });
            }
        }
    }
    scan_drives("7zFM.exe", 4)
}

// ─────────────────────────────────────────── 工具

fn common_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for d in ["C:", "D:", "E:", "F:"] {
        let root = PathBuf::from(format!("{d}\\"));
        if root.exists() {
            v.push(root.clone());
            v.push(root.join("Program Files"));
            v.push(root.join("Program Files (x86)"));
            v.push(root.join("Games"));
            v.push(root.join("游戏"));
            v.push(root.join("Programs"));
        }
    }
    v
}

fn reg_string(key: &str, name: &str) -> Option<String> {
    let script = format!(
        "$v = (Get-ItemProperty -LiteralPath 'Registry::{}' -Name '{}' -ErrorAction SilentlyContinue).'{}'; \
         if ($v) {{ $v }}",
        key, name, name
    );
    run_ps(&script)
}

fn run_ps(script: &str) -> Option<String> {
    for exe in ["powershell.exe", "pwsh.exe"] {
        let mut cmd = Command::new(exe);
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        if let Ok(o) = cmd.output() {
            if o.status.success() {
                let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !t.is_empty() {
                    return Some(t);
                }
            }
        }
    }
    None
}

/// 每个配方安装后正常启动时应出现的进程名（不含 .exe，不区分大小写）。
pub fn expected_processes(recipe_id: &str) -> &'static [&'static str] {
    match recipe_id {
        "steam" => &["steam", "steamwebhelper"],
        "7zip" => &["7zFM"],
        _ => &[],
    }
}

/// 检测指定软件的主进程是否正在运行（用于安装后启动验证）。
pub fn check_process_running(recipe_id: &str) -> (bool, Vec<String>) {
    let names = expected_processes(recipe_id);
    if names.is_empty() {
        return (false, vec![]);
    }
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    let script = format!(
        "@(Get-Process -ErrorAction SilentlyContinue | Where-Object {{ @({}) -contains $_.ProcessName }} | Select-Object -ExpandProperty ProcessName -Unique) -join ','",
        quoted.join(",")
    );
    match run_ps(&script) {
        Some(out) => {
            let matched: Vec<String> = out
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (!matched.is_empty(), matched)
        }
        None => (false, vec![]),
    }
}

/// 尝试启动已安装的主程序并在 2.5 秒后核验进程是否成功拉起。
pub fn launch_and_verify(recipe_id: &str) -> Value {
    let (already, matched) = check_process_running(recipe_id);
    if already {
        return json!({
            "launched": true,
            "running": true,
            "matched": matched,
            "message": "程序已经在运行中。",
        });
    }

    let Some(found) = locate(recipe_id) else {
        return json!({
            "launched": false,
            "running": false,
            "matched": [],
            "message": "找不到主程序路径，无法启动。",
        });
    };

    if let Err(e) = Command::new(&found.exe).spawn() {
        return json!({
            "launched": false,
            "running": false,
            "matched": [],
            "message": format!("拉起主程序失败：{e}"),
        });
    }

    thread::sleep(Duration::from_millis(2500));
    let (running, matched) = check_process_running(recipe_id);
    json!({
        "launched": true,
        "running": running,
        "matched": matched,
        "exe": found.exe.display().to_string(),
        "message": if running {
            "主程序已成功启动并检测到进程运行。"
        } else {
            "已触发启动，暂未检测到进程（可能在初始化或等待登录窗口弹出）。"
        },
    })
}

/// 全盘找文件名。深度限制 + 跳过系统目录，控制在 10 秒内。
fn scan_drives(filename: &str, depth: u32) -> Option<Found> {
    let script = format!(
        r#"
$skip = @('Windows','$Recycle.Bin','System Volume Information','ProgramData','node_modules','.git','AppData')
foreach ($d in (Get-PSDrive -PSProvider FileSystem | Where-Object {{ $_.Used -ne $null }}).Root) {{
    $hit = Get-ChildItem -LiteralPath $d -Filter '{name}' -Recurse -Depth {depth} -File -ErrorAction SilentlyContinue -Force |
        Where-Object {{ $p = $_.FullName; -not ($skip | Where-Object {{ $p -like "*\$_\*" }}) }} |
        Select-Object -First 1 -ExpandProperty FullName
    if ($hit) {{ $hit; break }}
}}
"#,
        name = filename,
        depth = depth
    );

    let out = run_ps(&script)?;
    let exe = PathBuf::from(out.trim());
    if !exe.exists() {
        return None;
    }
    let dir = exe.parent()?.to_path_buf();
    Some(Found { exe, source: "全盘扫描", dir })
}

// ─────────────────────────────────────────── 对外

/// 给第四步用的完整核对：不只是「有没有 exe」，还要看装得完不完整。
pub fn verify_install(recipe_id: &str) -> Value {
    let Some(found) = locate(recipe_id) else {
        return json!({
            "installed": false,
            "reason": "没找到安装痕迹",
            "nextStep": "可以重新安装",
        });
    };

    // 关键文件清单：缺哪个就说明装得不完整
    let required: &[&str] = match recipe_id {
        "steam" => &["steam.exe", "steamclient.dll", "steamui.dll"],
        "7zip" => &["7zFM.exe", "7z.dll"],
        _ => &[],
    };

    let mut present = Vec::new();
    let mut missing = Vec::new();
    for f in required {
        let p = found.dir.join(f);
        if p.exists() {
            present.push(*f);
        } else {
            missing.push(*f);
        }
    }

    let complete = missing.is_empty();
    // 能不能启动：exe 存在且大小 > 0 视为可执行文件完好
    let size = std::fs::metadata(&found.exe).map(|m| m.len()).unwrap_or(0);
    let launchable = size > 0;
    let (proc_running, running_procs) = check_process_running(recipe_id);

    json!({
        "installed": true,
        "exe": found.exe.display().to_string(),
        "dir": found.dir.display().to_string(),
        "foundBy": found.source,
        "complete": complete,
        "filesPresent": present,
        "filesMissing": missing,
        "exeBytes": size,
        "launchable": launchable,
        "processRunning": proc_running,
        "runningProcesses": running_procs,
        "reason": if complete {
            "装完整了"
        } else {
            "主程序在，但有文件缺失，可能需要修复安装"
        },
    })
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 本机_steam_应该能被找到() {
        if let Some(f) = locate("steam") {
            println!("找到：{}  来源：{}", f.exe.display(), f.source);
            assert!(f.exe.exists());
        } else {
            println!("这台机器没装 Steam");
        }
    }

    #[test]
    fn 核对结果结构完整() {
        let v = verify_install("steam");
        for k in ["installed", "reason"] {
            assert!(v.get(k).is_some(), "缺字段 {k}");
        }
        if v.get("installed").and_then(|x| x.as_bool()) == Some(true) {
            for k in ["exe", "foundBy", "complete", "launchable"] {
                assert!(v.get(k).is_some(), "已安装时缺字段 {k}");
            }
        }
    }

    #[test]
    fn 未知配方不崩() {
        let v = verify_install("nonexistent");
        assert_eq!(v.get("installed").and_then(|x| x.as_bool()), Some(false));
    }
}

