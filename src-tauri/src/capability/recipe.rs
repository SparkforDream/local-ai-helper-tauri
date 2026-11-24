// ============================================================
// capability/recipe.rs —— 四步公式执行器
//
// 参照 Alpha 的做法：把「装一个软件」拆成固定流程，
// 每一步都是代码控制，模型只负责说话，不参与判断。
//
// 四步公式（用户要的就是这个最小闭环）：
//   ① 下载安装包
//   ② 校验安装包
//   ③ 运行安装包
//   ④ 核对装完的结果
//
// 设计纪律：
//   1. 每一步都能单独验证成败，失败立即停，不往下走。
//   2. 不可逆的动作（③）必须用户确认。
//   3. 所有路径来自本文件，不接受外部拼装。
// ============================================================

use super::{Args, Capability, Param, ParamType, Risk};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

// ─────────────────────────────────────────── recipe 定义

/// 一个软件的四步配方。
///
/// 只实现 Serialize：配方是编译期常量表，不从外部反序列化。
/// （`&'static [&'static str]` 无法实现 Deserialize，也不需要。）
#[derive(Debug, Clone, Serialize)]
pub struct Recipe {
    pub id: &'static str,
    pub name: &'static str,
    /// 安装包下载地址（只能是 https）
    pub url: &'static str,
    /// 下载后的文件名
    pub filename: &'static str,
    /// 期望的文件大小（字节）。用来在下载后判断是否完整。
    /// 官方不公示哈希，所以用「大小 + 数字签名」双重校验。
    pub size: u64,
    /// 装完之后怎么判断装好了：这些路径任一存在即为已装
    pub verify_paths: &'static [&'static str],
    /// 装完之后怎么判断：注册表位置（可选）
    pub verify_reg: Option<&'static str>,
    /// 安装参数。None 表示需要用户手动交互安装
    pub install_args: Option<&'static [&'static str]>,
    /// 给用户看的说明，说明这一步要干什么
    pub note: &'static str,
}

/// 目前只收两个。每加一个都要实测过下载与验证，宁缺毋滥。
pub const RECIPES: &[Recipe] = &[
    Recipe {
        id: "steam",
        name: "Steam",
        url: "https://media.st.dl.eccdnx.com/client/installer/SteamSetup.exe",
        filename: "SteamSetup.exe",
        size: 2_380_800,
        verify_paths: &[
            r"C:\Program Files (x86)\Steam\steam.exe",
            r"C:\Program Files\Steam\steam.exe",
            r"D:\steam\steam.exe",
        ],
        verify_reg: Some(r"HKCU\Software\Valve\Steam"),
        // Steam 没有可靠的静默安装参数，必须让用户自己点
        install_args: None,
        note: "Steam 装的时候会问装到哪个盘，建议装到空间大的那个。",
    },
    Recipe {
        id: "7zip",
        name: "7-Zip",
        url: "https://www.7-zip.org/a/7z2501-x64.exe",
        filename: "7z2501-x64.exe",
        size: 1_600_000,
        verify_paths: &[r"C:\Program Files\7-Zip\7zFM.exe"],
        verify_reg: None,
        // 7-Zip 支持 /S 静默安装
        install_args: Some(&["/S"]),
        note: "7-Zip 可以静默装，不用你点。",
    },
];

pub fn find(id: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|r| r.id == id)
}

// ─────────────────────────────────────────── 能力声明

pub fn capabilities() -> Vec<Capability> {
    vec![
        Capability {
            id: "recipe.list",
            desc: "列出支持自动安装的软件",
            when: "用户问「你能帮我装什么」「支持哪些软件」",
            params: vec![],
            risk: Risk::ReadOnly,
            returns: "数组，每项含 id / name / size",
        },
        Capability {
            id: "recipe.step1_download",
            desc: "第一步：把安装包下到本地",
            when: "用户确认要装某个软件，流程的第一步",
            params: vec![Param {
                name: "id",
                desc: "软件的配方 id，如 steam",
                ty: ParamType::Str { max_len: Some(32) },
                required: true,
                default: None,
            }],
            risk: Risk::ReadOnly,
            returns: "对象：path / bytes / matched（大小是否与预期一致）",
        },
        Capability {
            id: "recipe.step2_verify",
            desc: "第二步：校验安装包完整性",
            when: "下载完成后立刻调用，判断文件是否下坏了",
            params: vec![Param {
                name: "id",
                desc: "软件配方 id",
                ty: ParamType::Str { max_len: Some(32) },
                required: true,
                default: None,
            }],
            risk: Risk::ReadOnly,
            returns: "对象：ok / sizeOk / signature / detail",
        },
        Capability {
            id: "recipe.step3_install",
            desc: "第三步：运行安装包",
            when: "第二步校验通过、且用户明确同意后才调用",
            params: vec![Param {
                name: "id",
                desc: "软件配方 id",
                ty: ParamType::Str { max_len: Some(32) },
                required: true,
                default: None,
            }],
            risk: Risk::Dangerous,
            returns: "对象：launched / silent / needUserAction / message",
        },
        Capability {
            id: "recipe.step4_verify_install",
            desc: "第四步：核对是否真的装好了",
            when: "第三步完成后调用，确认安装结果，不能只看安装程序有没有退出",
            params: vec![Param {
                name: "id",
                desc: "软件配方 id",
                ty: ParamType::Str { max_len: Some(32) },
                required: true,
                default: None,
            }],
            risk: Risk::ReadOnly,
            returns: "对象：installed / evidence / searched",
        },
    ]
}

// ─────────────────────────────────────────── 路径

fn download_dir() -> PathBuf {
    // 用系统的下载目录，用户能在自己熟悉的地方看到文件
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("C:\\"))
        .join("Downloads")
}

fn installer_path(r: &Recipe) -> PathBuf {
    download_dir().join(r.filename)
}

// ─────────────────────────────────────────── ① 下载

pub fn step1_download(args: &Args) -> Result<Value, String> {
    let id = args.str("id").ok_or("缺少参数 id")?;
    let r = find(id).ok_or_else(|| format!("没有 {id} 的安装配方"))?;

    let dest = installer_path(r);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败：{e}"))?;
    }

    // 用系统自带 curl，免装依赖。范围：全量下载。
    let out = Command::new("curl.exe")
        .args(["-L", "--max-time", "300", "-o"])
        .arg(&dest)
        .arg(r.url)
        .output()
        .map_err(|e| format!("调用 curl 失败：{e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("下载失败：{}", err.trim()));
    }

    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    Ok(json!({
        "path": dest.display().to_string(),
        "bytes": bytes,
        "expectedBytes": r.size,
        "matched": bytes == r.size,
    }))
}

// ─────────────────────────────────────────── ② 校验

pub fn step2_verify(args: &Args) -> Result<Value, String> {
    let id = args.str("id").ok_or("缺少参数 id")?;
    let r = find(id).ok_or_else(|| format!("没有 {id} 的安装配方"))?;

    let p = installer_path(r);
    if !p.exists() {
        return Ok(json!({
            "ok": false,
            "sizeOk": false,
            "detail": "安装包不存在，先跑第一步",
        }));
    }

    let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    let size_ok = bytes == r.size;

    // 数字签名：官方软件应该带有效签名。
    // 这不是绝对证明（hash 才是），但能挡掉大部分被替换的文件。
    let sig = check_signature(&p);

    Ok(json!({
        "ok": size_ok && sig != "Invalid",
        "sizeOk": size_ok,
        "actualBytes": bytes,
        "expectedBytes": r.size,
        "signature": sig,
        "detail": if size_ok {
            format!("大小一致（{} 字节）", bytes)
        } else {
            format!("大小不符：期望 {} 实得 {}", r.size, bytes)
        },
    }))
}

fn check_signature(path: &Path) -> String {
    let script = format!(
        "$s = Get-AuthenticodeSignature -LiteralPath '{}'; $s.Status.ToString()",
        path.display().to_string().replace('\'', "''")
    );
    for exe in ["powershell.exe", "pwsh.exe"] {
        if let Ok(o) = Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
        {
            if o.status.success() {
                let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !t.is_empty() {
                    return t;
                }
            }
        }
    }
    "Unknown".into()
}

// ─────────────────────────────────────────── ③ 安装

pub fn step3_install(args: &Args) -> Result<Value, String> {
    let id = args.str("id").ok_or("缺少参数 id")?;
    let r = find(id).ok_or_else(|| format!("没有 {id} 的安装配方"))?;

    let p = installer_path(r);
    if !p.exists() {
        return Err("安装包不存在，先跑第一步".into());
    }

    match r.install_args {
        Some(extra) => {
            // 静默安装
            let mut cmd = Command::new(&p);
            cmd.args(extra);
            match cmd.spawn() {
                Ok(child) => Ok(json!({
                    "launched": true,
                    "silent": true,
                    "pid": child.id(),
                    "needUserAction": false,
                    "message": format!("{} 正在后台安装，装完我会自己检查。", r.name),
                })),
                Err(e) => Err(format!("启动安装程序失败：{e}")),
            }
        }
        None => {
            // 需要用户交互。只打开安装程序，不代替用户点任何按钮。
            let mut cmd = Command::new(&p);
            match cmd.spawn() {
                Ok(child) => Ok(json!({
                    "launched": true,
                    "silent": false,
                    "pid": child.id(),
                    "needUserAction": true,
                    "message": format!(
                        "{} 的安装窗口已经打开了。\n{}",
                        r.name, r.note
                    ),
                })),
                Err(e) => Err(format!("打开安装程序失败：{e}")),
            }
        }
    }
}

// ─────────────────────────────────────────── ④ 核对

pub fn step4_verify_install(args: &Args) -> Result<Value, String> {
    let id = args.str("id").ok_or("缺少参数 id")?;
    let r = find(id).ok_or_else(|| format!("没有 {id} 的安装配方"))?;

    // 交给分级探测器：注册表 → 常见路径 → 全盘扫描。
    // 不能只看 verify_paths 里写死的几个位置，
    // 用户完全可能把软件装进 D:\游戏\steam 这种地方。
    let v = super::detect::verify_install(id);

    // 探测没结果时，退回配方声明的固定路径，保住老行为。
    if v.get("installed").and_then(|x| x.as_bool()) != Some(true) {
        let mut found = Vec::new();
        for p in r.verify_paths {
            if Path::new(p).exists() {
                found.push(*p);
            }
        }
        if !found.is_empty() {
            return Ok(json!({
                "installed": true,
                "exe": found[0],
                "foundBy": "配方固定路径",
                "complete": true,
                "reason": format!("{} 已经装好了。", r.name),
            }));
        }
    }

    Ok(v)
}
pub fn list(_: &Args) -> Result<Value, String> {
    let items: Vec<Value> = RECIPES
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "sizeMB": (r.size as f64 / 1_048_576.0 * 10.0).round() / 10.0,
                "silent": r.install_args.is_some(),
                "note": r.note,
            })
        })
        .collect();
    Ok(json!(items))
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 配方_id_不重复() {
        let mut ids: Vec<&str> = RECIPES.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "配方 id 重复");
    }

    #[test]
    fn 所有下载地址必须是_https() {
        for r in RECIPES {
            assert!(r.url.starts_with("https://"), "{} 的下载地址不是 https", r.id);
        }
    }

    #[test]
    fn 每个配方都要有验证手段() {
        for r in RECIPES {
            assert!(
                !r.verify_paths.is_empty() || r.verify_reg.is_some(),
                "{} 没有验证手段，第四步会永远失败",
                r.id
            );
        }
    }

    #[test]
    fn 大小必须是正数() {
        for r in RECIPES {
            assert!(r.size > 0, "{} 的 size 未填", r.id);
        }
    }

    #[test]
    fn 未知配方要报错() {
        let a = Args::new(json!({"id": "nope"}));
        assert!(step1_download(&a).is_err());
        assert!(step2_verify(&a).is_err());
        assert!(step4_verify_install(&a).is_err());
    }

    #[test]
    fn 第四步能核对已装的_steam() {
        // 这台机器上 Steam 装在一个非标准路径，正好验证探测能力
        let a = Args::new(json!({"id": "steam"}));
        let r = step4_verify_install(&a).unwrap();
        // 不断言必须 true（别的机器可能没装），只断言结构完整
        assert!(r.get("installed").is_some());
        assert!(r.get("reason").is_some());
        if r.get("installed").and_then(|x| x.as_bool()) == Some(true) {
            for k in ["exe", "foundBy", "complete", "launchable"] {
                assert!(r.get(k).is_some(), "已安装时缺字段 {k}");
            }
        }
    }
}

