// ============================================================
// capability/fs.rs —— 文件系统能力（只读 + 受控空目录建删）
//
// 安全边界（这是本文件最重要的部分）：
//   1. 只读能力带深度与超时限制，跳过系统目录。
//   2. 写/删能力仅允许操作单个空目录：fs.mkdir 只建单层，
//      fs.rmdir_empty 只调 std::fs::remove_dir（非空必拒，绝不递归删）。
//   3. 写/删路径硬锁：必须绝对路径、禁 ..、禁盘符根、禁系统目录。
// ============================================================

use super::{Args, Capability, Param, ParamType, Risk};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 写/删目录授权策略：开启 restrict_to_allowed 后，仅允许在 allowed_dirs 白名单内建/删空目录。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FsPolicy {
    #[serde(default)]
    pub restrict_to_allowed: bool,
    #[serde(default)]
    pub allowed_dirs: Vec<String>,
}

fn policy_path() -> PathBuf {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if !base.trim().is_empty() {
            return PathBuf::from(base)
                .join("local-ai-helper")
                .join("fs_policy.json");
        }
    }
    PathBuf::from("fs_policy.json")
}

pub fn load_policy() -> FsPolicy {
    let Ok(raw) = fs::read_to_string(policy_path()) else {
        return FsPolicy::default();
    };
    serde_json::from_str::<FsPolicy>(&raw).unwrap_or_default()
}

pub fn save_policy(input: FsPolicy) -> Result<FsPolicy, String> {
    let mut cleaned_dirs = Vec::new();
    for d in input.allowed_dirs {
        let s = d.trim().trim_matches('"').trim_end_matches(['\\', '/']).to_string();
        if s.is_empty() {
            continue;
        }
        let p = PathBuf::from(&s);
        if !p.is_absolute() || s.contains("..") {
            return Err(format!("授权目录必须是合法绝对路径：{s}"));
        }
        if !cleaned_dirs
            .iter()
            .any(|x: &String| x.eq_ignore_ascii_case(&s))
        {
            cleaned_dirs.push(s);
        }
    }
    let cleaned = FsPolicy {
        restrict_to_allowed: input.restrict_to_allowed,
        allowed_dirs: cleaned_dirs,
    };
    let path = policy_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let body = serde_json::to_string_pretty(&cleaned).map_err(|e| format!("序列化失败：{e}"))?;
    fs::write(&path, body).map_err(|e| format!("写入授权策略失败：{e}"))?;
    Ok(cleaned)
}

pub fn pick_folder_dialog() -> Result<Option<String>, String> {
    let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.FolderBrowserDialog
$dlg.Description = '选择允许助手创建/删除空文件夹的目录'
if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    Write-Output $dlg.SelectedPath
}
"#;
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-STA",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let out = cmd
        .output()
        .map_err(|e| format!("无法打开文件夹选择框：{e}"))?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        Ok(None)
    } else {
        Ok(Some(path))
    }
}

fn norm_win_path(s: &str) -> String {
    s.trim()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// 目录边界精确匹配：D:\123 与 D:\123\sub 匹配 D:\123，但 D:\1234 不匹配。
pub fn is_path_within_allowed(target: &Path, allowed_dirs: &[String]) -> bool {
    let t = norm_win_path(&target.to_string_lossy());
    if t.is_empty() {
        return false;
    }
    for dir in allowed_dirs {
        let a = norm_win_path(dir);
        if a.is_empty() {
            continue;
        }
        if t == a || t.starts_with(&format!("{a}\\")) {
            return true;
        }
    }
    false
}

pub fn capabilities() -> Vec<Capability> {
    vec![
        Capability {
            id: "fs.dir_size",
            desc: "算某个目录占多少空间，只算一层子目录",
            when: "用户问「哪个文件夹占地方」「C盘被什么吃掉了」",
            params: vec![Param {
                name: "path",
                desc: "目录路径，如 C:\\\\Users\\\\name\\\\Downloads",
                ty: ParamType::Str { max_len: Some(260) },
                required: true,
                default: None,
            }],
            risk: Risk::ReadOnly,
            returns: "对象：path / totalBytes / entries（每个子项的大小，降序）",
        },
        Capability {
            id: "fs.biggest",
            desc: "在某个目录下找出最大的几个子项",
            when: "用户问「是什么占满了 C 盘」「哪个文件夹最大」",
            params: vec![
                Param {
                    name: "path",
                    desc: "要扫的目录",
                    ty: ParamType::Str { max_len: Some(260) },
                    required: true,
                    default: None,
                },
                Param {
                    name: "n",
                    desc: "返回几个，默认 10",
                    ty: ParamType::Int { min: Some(1), max: Some(50) },
                    required: false,
                    default: Some(json!(10)),
                },
            ],
            risk: Risk::ReadOnly,
            returns: "数组，每项含 name / bytes / isDir，降序",
        },
        Capability {
            id: "fs.exists",
            desc: "检查某个路径是否存在，以及是否是空文件夹",
            when: "需要确认某个文件夹或文件是否存在、是否为空时使用",
            params: vec![Param {
                name: "path",
                desc: "绝对路径，如 D:\\\\123",
                ty: ParamType::Str { max_len: Some(260) },
                required: true,
                default: None,
            }],
            risk: Risk::ReadOnly,
            returns: "对象：path / exists / isDir / isEmpty",
        },
        Capability {
            id: "fs.mkdir",
            desc: "创建一个单层空文件夹（父目录须已存在）",
            when: "用户要求在某位置新建一个文件夹，如在 D 盘根目录建 123",
            params: vec![Param {
                name: "path",
                desc: "要创建的文件夹绝对路径，如 D:\\\\123",
                ty: ParamType::Str { max_len: Some(260) },
                required: true,
                default: None,
            }],
            risk: Risk::Reversible,
            returns: "对象：path / created / alreadyExists",
        },
        Capability {
            id: "fs.rmdir_empty",
            desc: "删除一个空文件夹（如果里面有文件或子文件夹会拒绝删除）",
            when: "用户要求删除一个空文件夹，如删除 D:\\\\123",
            params: vec![Param {
                name: "path",
                desc: "要删除的空文件夹绝对路径，如 D:\\\\123",
                ty: ParamType::Str { max_len: Some(260) },
                required: true,
                default: None,
            }],
            risk: Risk::Dangerous,
            returns: "对象：path / removed",
        },
    ]
}

/// 这些目录跳过，理由分别是：
///   Windows / Program Files —— 系统文件，扫了也没用，还会误报
///   $Recycle.Bin —— 回收站，大小不等于实际可释放
///   System Volume Information —— 系统还原点，无权限
///   node_modules —— 数量巨大，扫起来极慢
const SKIP: &[&str] = &[
    "Windows",
    "Program Files",
    "Program Files (x86)",
    "ProgramData",
    "$Recycle.Bin",
    "System Volume Information",
    "node_modules",
    ".git",
];

fn should_skip(name: &str) -> bool {
    SKIP.iter().any(|s| s.eq_ignore_ascii_case(name))
}

/// 递归算目录大小，带深度和超时限制。
///
/// 深度上限 3：再深就对用户没意义了，而且会慢到不可接受。
fn dir_bytes(path: &Path, depth: usize, deadline: Instant) -> u64 {
    if depth > 3 || Instant::now() > deadline {
        return 0;
    }
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return 0,
    };

    let mut total = 0u64;
    for e in rd.flatten() {
        if Instant::now() > deadline {
            break;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if should_skip(&name) {
            continue;
        }
        let p = e.path();
        match e.metadata() {
            Ok(m) if m.is_dir() => total += dir_bytes(&p, depth + 1, deadline),
            Ok(m) => total += m.len(),
            Err(_) => {}
        }
    }
    total
}

fn validate_path(raw: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(raw);
    if !p.is_absolute() {
        return Err("请给绝对路径".into());
    }
    if !p.exists() {
        return Err(format!("路径不存在：{raw}"));
    }
    if !p.is_dir() {
        return Err("这个路径不是目录".into());
    }
    Ok(p)
}

pub fn dir_size(args: &Args) -> Result<Value, String> {
    let raw = args.str("path").ok_or("缺少 path")?;
    let root = validate_path(raw)?;

    // 单次扫描最多 8 秒，慢盘上也不至于卡死
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut entries = Vec::new();
    let mut total = 0u64;

    if let Ok(rd) = fs::read_dir(&root) {
        for e in rd.flatten() {
            if Instant::now() > deadline {
                break;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if should_skip(&name) {
                continue;
            }
            let p = e.path();
            let (bytes, is_dir) = match e.metadata() {
                Ok(m) if m.is_dir() => (dir_bytes(&p, 0, deadline), true),
                Ok(m) => (m.len(), false),
                Err(_) => continue,
            };
            total += bytes;
            entries.push(json!({
                "name": name,
                "bytes": bytes,
                "isDir": is_dir,
            }));
        }
    }

    entries.sort_by_key(|e| {
        std::cmp::Reverse(e.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0))
    });
    // 只回前 30 个，足够定位问题，又不会让模型输出过长
    entries.truncate(30);

    Ok(json!({
        "path": root.display().to_string(),
        "totalBytes": total,
        "entries": entries,
        "truncatedByTimeout": Instant::now() > deadline,
    }))
}

pub fn biggest(args: &Args) -> Result<Value, String> {
    let r = dir_size(args)?;
    let n = args.int_or("n", 10, 1, 50) as usize;

    let mut entries = r.get("entries").cloned().unwrap_or_else(|| json!([]));
    if let Some(arr) = entries.as_array_mut() {
        arr.truncate(n);
    }
    Ok(entries)
}

/// 禁止写/删的受保护系统顶层目录名（不区分大小写）。
const PROTECTED_TOP: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "$recycle.bin",
    "system volume information",
    "perflogs",
    "recovery",
];

fn validate_write_dir_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空".into());
    }
    if trimmed.contains("..") {
        return Err("路径不能包含 ..".into());
    }
    let p = PathBuf::from(trimmed);
    if !p.is_absolute() {
        return Err("请提供绝对路径（例如 D:\\123）".into());
    }

    let mut normal_parts: Vec<String> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => {}
            Component::Normal(os) => normal_parts.push(os.to_string_lossy().to_string()),
            Component::CurDir | Component::ParentDir => {
                return Err("路径包含非法片段".into());
            }
        }
    }

    if normal_parts.is_empty() {
        return Err("禁止直接操作磁盘根目录".into());
    }

    let first_lower = normal_parts[0].to_ascii_lowercase();
    if PROTECTED_TOP.iter().any(|&s| s == first_lower) {
        return Err(format!("禁止在受保护的系统目录内操作：{trimmed}"));
    }
    // 禁止直接动 C:\Users 或 C:\Users\<用户名> 根，但允许其下的桌面/下载等子目录
    if first_lower == "users" && normal_parts.len() <= 2 {
        return Err(format!("禁止直接操作用户主目录本身：{trimmed}"));
    }

    // 禁止包含 Windows 非法文件名字符（盘符后的部分）
    for seg in &normal_parts {
        if seg.is_empty()
            || seg.ends_with('.')
            || seg.ends_with(' ')
            || seg.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        {
            return Err(format!("文件夹名不合法：{seg}"));
        }
    }

    // 若开启了自定义授权目录白名单，强制校验目标路径落在白名单目录内
    let policy = load_policy();
    if policy.restrict_to_allowed {
        if policy.allowed_dirs.is_empty() {
            return Err("已开启「仅限授权目录」，但白名单为空；请先在设置里添加允许操作的目录".into());
        }
        if !is_path_within_allowed(&p, &policy.allowed_dirs) {
            return Err(format!(
                "路径不在授权目录白名单内，已拦截：{trimmed}（可在设置 → 授权目录中调整）"
            ));
        }
    }

    Ok(p)
}

pub fn exists(args: &Args) -> Result<Value, String> {
    let raw = args.str("path").ok_or("缺少 path")?;
    let p = PathBuf::from(raw.trim());
    if !p.is_absolute() {
        return Err("请给绝对路径".into());
    }
    let ex = p.exists();
    let is_dir = ex && p.is_dir();
    let is_empty = if is_dir {
        fs::read_dir(&p)
            .map(|mut rd| rd.next().is_none())
            .unwrap_or(false)
    } else {
        false
    };
    Ok(json!({
        "path": p.display().to_string(),
        "exists": ex,
        "isDir": is_dir,
        "isEmpty": is_empty,
    }))
}

pub fn mkdir(args: &Args) -> Result<Value, String> {
    let raw = args.str("path").ok_or("缺少 path")?;
    let p = validate_write_dir_path(raw)?;

    if p.exists() {
        if p.is_dir() {
            return Ok(json!({
                "path": p.display().to_string(),
                "created": false,
                "alreadyExists": true,
            }));
        }
        return Err(format!("同名文件已存在，不能创建文件夹：{}", p.display()));
    }

    let parent = p
        .parent()
        .ok_or_else(|| "无法确定上级目录".to_string())?;
    if !parent.exists() || !parent.is_dir() {
        return Err(format!("上级目录不存在：{}", parent.display()));
    }

    fs::create_dir(&p).map_err(|e| format!("创建文件夹失败：{e}"))?;
    Ok(json!({
        "path": p.display().to_string(),
        "created": true,
        "alreadyExists": false,
    }))
}

pub fn rmdir_empty(args: &Args) -> Result<Value, String> {
    let raw = args.str("path").ok_or("缺少 path")?;
    let p = validate_write_dir_path(raw)?;

    if !p.exists() {
        return Err(format!("文件夹不存在：{}", p.display()));
    }
    if !p.is_dir() {
        return Err(format!("这不是文件夹，拒绝删除：{}", p.display()));
    }

    // 二次显式检查是否为空；即使并发变化，std::fs::remove_dir 也会拒绝非空目录
    let mut rd = fs::read_dir(&p).map_err(|e| format!("无法读取文件夹内容：{e}"))?;
    if rd.next().is_some() {
        return Err(format!("文件夹不是空的，为防误删已拒绝：{}", p.display()));
    }
    drop(rd);

    fs::remove_dir(&p).map_err(|e| format!("删除空文件夹失败：{e}"))?;
    Ok(json!({
        "path": p.display().to_string(),
        "removed": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 系统目录要被跳过() {
        assert!(should_skip("Windows"));
        assert!(should_skip("windows"));
        assert!(should_skip("$Recycle.Bin"));
        assert!(!should_skip("Downloads"));
    }

    #[test]
    fn 相对路径要被拒绝() {
        assert!(validate_path("relative/path").is_err());
    }

    #[test]
    fn 不存在的路径要被拒绝() {
        assert!(validate_path("C:\\this\\does\\not\\exist\\xyz").is_err());
    }

    #[test]
    fn 写删护栏拒绝盘符根和系统目录() {
        assert!(validate_write_dir_path("D:\\").is_err());
        assert!(validate_write_dir_path("C:\\Windows\\test123").is_err());
        assert!(validate_write_dir_path("C:\\Program Files\\abc").is_err());
        assert!(validate_write_dir_path("C:\\Users").is_err());
        assert!(validate_write_dir_path("C:\\Users\\demo").is_err());
        assert!(validate_write_dir_path("D:\\foo\\..\\bar").is_err());
        assert!(validate_write_dir_path("D:\\123").is_ok());
        assert!(validate_write_dir_path("C:\\Users\\demo\\Desktop\\123").is_ok());
    }

    #[test]
    fn 目录白名单边界精确匹配不误放行前缀() {
        let allowed = vec!["D:\\123".to_string(), "D:\\test_dir\\".to_string()];
        assert!(is_path_within_allowed(Path::new("D:\\123"), &allowed));
        assert!(is_path_within_allowed(Path::new("d:\\123\\sub"), &allowed));
        assert!(is_path_within_allowed(Path::new("D:\\test_dir\\a"), &allowed));
        assert!(!is_path_within_allowed(Path::new("D:\\1234"), &allowed));
        assert!(!is_path_within_allowed(Path::new("E:\\123"), &allowed));
    }

    #[test]
    fn 空目录创建检查删除闭环且非空拒绝() {
        let base = std::env::temp_dir().join(format!("local_ai_helper_fs_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        let target = base.join("123");
        let t_str = target.to_string_lossy().to_string();

        // 创建
        let mk = mkdir(&Args::new(json!({ "path": t_str }))).unwrap();
        assert_eq!(mk["created"], true);

        // 检查存在且为空
        let ex = exists(&Args::new(json!({ "path": t_str }))).unwrap();
        assert_eq!(ex["exists"], true);
        assert_eq!(ex["isDir"], true);
        assert_eq!(ex["isEmpty"], true);

        // 放入一个文件，验证非空必拒
        let inner = target.join("a.txt");
        fs::write(&inner, b"hi").unwrap();
        let deny = rmdir_empty(&Args::new(json!({ "path": t_str })));
        assert!(deny.is_err());
        assert!(deny.unwrap_err().contains("不是空的"));

        // 删掉内部文件后再删空目录 -> 成功
        fs::remove_file(&inner).unwrap();
        let rm = rmdir_empty(&Args::new(json!({ "path": t_str }))).unwrap();
        assert_eq!(rm["removed"], true);
        assert!(!target.exists());

        let _ = fs::remove_dir_all(&base);
    }
}
