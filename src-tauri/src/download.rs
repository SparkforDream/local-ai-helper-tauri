// ============================================================
// download.rs —— 本地模型断点续传下载器 + 下载源管理
//
// 设计原则：
//   1. 默认走 ModelScope（魔搭官方阿里云 CDN），免登录、支持 HTTP 206 断点续传；
//   2. 预留备用镜像与自定义源根地址，供专业人士或子女排查下载问题时切换；
//   3. 复用系统 curl.exe (-L -C -)，后台每 350ms 采样 .part 文件增量计算真实速度，
//      通过 Tauri 事件 "model-download" 实时推送进度与速度；
//   4. 下载完成后核验前 4 字节为 GGUF 魔数（防镜像返回 HTML 错误页假成功），再原子改名。
// ============================================================

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadSource {
    Modelscope,
    HfMirror,
    Custom,
}

impl Default for DownloadSource {
    fn default() -> Self {
        Self::Modelscope
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadConfig {
    #[serde(default)]
    pub source: DownloadSource,
    #[serde(default)]
    pub custom_base_url: String,
    #[serde(default)]
    pub imported_model_path: String,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            source: DownloadSource::Modelscope,
            custom_base_url: String::new(),
            imported_model_path: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub tier: &'static str,
    pub name: &'static str,
    pub ms_repo: &'static str,
    pub hf_repo: &'static str,
    pub filename: &'static str,
    pub approx_bytes: u64,
}

pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        tier: "low",
        name: "Qwen2.5-3B",
        ms_repo: "Qwen/Qwen2.5-3B-Instruct-GGUF",
        hf_repo: "Qwen/Qwen2.5-3B-Instruct-GGUF",
        filename: "qwen2.5-3b-instruct-q4_k_m.gguf",
        approx_bytes: 2_104_932_736,
    },
    ModelSpec {
        tier: "mid",
        name: "Qwen3-8B",
        ms_repo: "Qwen/Qwen3-8B-GGUF",
        hf_repo: "Qwen/Qwen3-8B-GGUF",
        filename: "Qwen3-8B-Q4_K_M.gguf",
        approx_bytes: 5_027_783_488,
    },
    ModelSpec {
        tier: "high",
        name: "Qwen3-14B",
        ms_repo: "Qwen/Qwen3-14B-GGUF",
        hf_repo: "Qwen/Qwen3-14B-GGUF",
        filename: "Qwen3-14B-Q4_K_M.gguf",
        approx_bytes: 9_000_000_000,
    },
    ModelSpec {
        tier: "ultra",
        name: "Qwen3-32B",
        ms_repo: "Qwen/Qwen3-32B-GGUF",
        hf_repo: "Qwen/Qwen3-32B-GGUF",
        filename: "Qwen3-32B-Q4_K_M.gguf",
        approx_bytes: 19_800_000_000,
    },
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub tier: String,
    pub name: String,
    pub filename: String,
    pub downloaded: bool,
    pub active: bool,
    pub part_bytes: u64,
    pub total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

static ACTIVE_DOWNLOADS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();

fn active_map() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    ACTIVE_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn config_path() -> PathBuf {
    base_data_dir().join("download.json")
}

pub fn models_dir() -> PathBuf {
    base_data_dir().join("models")
}

fn base_data_dir() -> PathBuf {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if !base.trim().is_empty() {
            return PathBuf::from(base).join("local-ai-helper");
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn load_config() -> DownloadConfig {
    let Ok(raw) = fs::read_to_string(config_path()) else {
        return DownloadConfig::default();
    };
    serde_json::from_str::<DownloadConfig>(&raw).unwrap_or_default()
}

pub fn save_config(cfg: DownloadConfig) -> Result<DownloadConfig, String> {
    let existing = load_config();
    let imported = if cfg.imported_model_path.trim().is_empty() && !existing.imported_model_path.is_empty() {
        existing.imported_model_path
    } else {
        cfg.imported_model_path.trim().trim_matches('"').to_string()
    };
    let cleaned = DownloadConfig {
        source: cfg.source,
        custom_base_url: cfg.custom_base_url.trim().trim_end_matches('/').to_string(),
        imported_model_path: imported,
    };
    if cleaned.source == DownloadSource::Custom {
        if cleaned.custom_base_url.is_empty() {
            return Err("选择自定义下载源时请填写地址".into());
        }
        if !(cleaned.custom_base_url.starts_with("http://")
            || cleaned.custom_base_url.starts_with("https://"))
        {
            return Err("自定义下载源地址须以 http:// 或 https:// 开头".into());
        }
    }
    write_config_file(&cleaned)?;
    Ok(cleaned)
}

fn write_config_file(cfg: &DownloadConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let body =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化下载配置失败：{e}"))?;
    fs::write(&path, body).map_err(|e| format!("写入下载配置失败：{e}"))
}

pub fn import_local_gguf(raw_path: &str) -> Result<DownloadConfig, String> {
    let cleaned = raw_path.trim().trim_matches('"');
    if cleaned.is_empty() {
        return Err("请选择或输入本地 .gguf 模型文件路径".into());
    }
    let p = PathBuf::from(cleaned);
    if !p.is_file() {
        return Err(format!("文件不存在：{cleaned}"));
    }
    if !is_valid_gguf(&p) {
        return Err("所选文件不是有效的 GGUF 模型文件（文件头校验未通过）".into());
    }
    let mut cfg = load_config();
    cfg.imported_model_path = p.display().to_string();
    write_config_file(&cfg)?;
    Ok(cfg)
}

pub fn clear_imported_gguf() -> Result<DownloadConfig, String> {
    let mut cfg = load_config();
    cfg.imported_model_path.clear();
    write_config_file(&cfg)?;
    Ok(cfg)
}

pub fn pick_gguf_dialog() -> Result<Option<String>, String> {
    let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.OpenFileDialog
$dlg.Title = '选择本地 GGUF 模型文件'
$dlg.Filter = 'GGUF 模型 (*.gguf)|*.gguf|所有文件 (*.*)|*.*'
$dlg.CheckFileExists = $true
if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    Write-Output $dlg.FileName
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
        .map_err(|e| format!("无法打开文件选择框：{e}"))?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        Ok(None)
    } else {
        Ok(Some(path))
    }
}

pub fn build_download_url(spec: &ModelSpec, cfg: &DownloadConfig) -> Result<String, String> {
    match cfg.source {
        DownloadSource::Modelscope => Ok(format!(
            "https://modelscope.cn/models/{}/resolve/master/{}",
            spec.ms_repo, spec.filename
        )),
        DownloadSource::HfMirror => Ok(format!(
            "https://hf-mirror.com/{}/resolve/main/{}",
            spec.hf_repo, spec.filename
        )),
        DownloadSource::Custom => {
            let base = cfg.custom_base_url.trim().trim_end_matches('/');
            if base.is_empty() {
                return Err("尚未填写自定义下载源地址".into());
            }
            if !(base.starts_with("http://") || base.starts_with("https://")) {
                return Err("自定义下载源地址必须以 http:// 或 https:// 开头".into());
            }
            if base.contains("{file}") {
                Ok(base.replace("{file}", spec.filename))
            } else {
                Ok(format!("{base}/{}/resolve/master/{}", spec.ms_repo, spec.filename))
            }
        }
    }
}

fn find_existing_model(filename: &str) -> Option<PathBuf> {
    let primary = models_dir().join(filename);
    if primary.is_file() {
        return Some(primary);
    }
    // 兼容已手动放到本机 .runtime/models 里的模型文件
    let legacy = PathBuf::from(r"D:\workspace\local-ai-helper\.runtime\models").join(filename);
    if legacy.is_file() {
        return Some(legacy);
    }
    None
}

pub fn is_valid_gguf(path: &Path) -> bool {
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).is_ok() && &magic == b"GGUF"
}

pub fn list_status() -> Vec<ModelStatus> {
    let map = active_map().lock().unwrap_or_else(|e| e.into_inner());
    let dir = models_dir();

    MODELS
        .iter()
        .map(|m| {
            let existing = find_existing_model(m.filename);
            let part_path = dir.join(format!("{}.part", m.filename));
            let part_bytes = fs::metadata(&part_path).map(|md| md.len()).unwrap_or(0);
            let total_bytes = existing
                .as_ref()
                .and_then(|p| fs::metadata(p).ok().map(|md| md.len()))
                .unwrap_or(m.approx_bytes);

            ModelStatus {
                tier: m.tier.to_string(),
                name: m.name.to_string(),
                filename: m.filename.to_string(),
                downloaded: existing.is_some(),
                active: map.contains_key(m.tier),
                part_bytes,
                total_bytes,
                file_path: existing.map(|p| p.display().to_string()),
            }
        })
        .collect()
}

pub fn start_download(app: AppHandle, tier: String) -> Result<String, String> {
    let spec = MODELS
        .iter()
        .find(|m| m.tier == tier)
        .ok_or_else(|| format!("未知档位：{tier}"))?
        .clone();

    if let Some(existing) = find_existing_model(spec.filename) {
        let _ = app.emit(
            "model-download",
            json!({
                "tier": spec.tier,
                "status": "done",
                "downloadedBytes": spec.approx_bytes,
                "totalBytes": spec.approx_bytes,
                "speedBps": 0,
                "percent": 100.0,
                "filePath": existing.display().to_string(),
            }),
        );
        return Ok(format!("{} 已存在于本机", spec.name));
    }

    let cfg = load_config();
    let url = build_download_url(&spec, &cfg)?;

    let cancel_flag = {
        let mut map = active_map().lock().unwrap_or_else(|e| e.into_inner());
        if map.contains_key(spec.tier) {
            return Err(format!("{} 正在下载中", spec.name));
        }
        let flag = Arc::new(AtomicBool::new(false));
        map.insert(spec.tier.to_string(), flag.clone());
        flag
    };

    let dir = models_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        let mut map = active_map().lock().unwrap_or_else(|e| e.into_inner());
        map.remove(spec.tier);
        return Err(format!("创建模型目录失败：{e}"));
    }

    let final_path = dir.join(spec.filename);
    let part_path = dir.join(format!("{}.part", spec.filename));
    let tier_key = spec.tier.to_string();

    thread::spawn(move || {
        let mut cmd = Command::new("curl.exe");
        cmd.args([
            "-sS",
            "-L",
            "-C",
            "-",
            "--fail",
            "--connect-timeout",
            "15",
            "-o",
        ])
        .arg(&part_path)
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                cleanup_active(&tier_key);
                let _ = app.emit(
                    "model-download",
                    json!({
                        "tier": tier_key,
                        "status": "error",
                        "error": format!("启动下载失败：{e}"),
                    }),
                );
                return;
            }
        };

        let mut prev_bytes = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
        let mut prev_time = Instant::now();
        let mut smooth_bps: f64 = 0.0;

        let _ = app.emit(
            "model-download",
            json!({
                "tier": tier_key,
                "status": "downloading",
                "downloadedBytes": prev_bytes,
                "totalBytes": spec.approx_bytes,
                "speedBps": 0,
                "percent": (prev_bytes as f64 / spec.approx_bytes as f64 * 100.0).min(99.9),
            }),
        );

        loop {
            if cancel_flag.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                cleanup_active(&tier_key);
                let cur = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
                let _ = app.emit(
                    "model-download",
                    json!({
                        "tier": tier_key,
                        "status": "paused",
                        "downloadedBytes": cur,
                        "totalBytes": spec.approx_bytes,
                        "speedBps": 0,
                        "percent": (cur as f64 / spec.approx_bytes as f64 * 100.0).min(99.9),
                    }),
                );
                return;
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    cleanup_active(&tier_key);
                    if status.success() {
                        if !is_valid_gguf(&part_path) {
                            let _ = fs::remove_file(&part_path);
                            let _ = app.emit(
                                "model-download",
                                json!({
                                    "tier": tier_key,
                                    "status": "error",
                                    "error": "下载到的文件不是有效 GGUF 模型（可能下载源返回了错误页），请尝试在设置里切换下载源",
                                }),
                            );
                            return;
                        }
                        if let Err(e) = fs::rename(&part_path, &final_path) {
                            let _ = app.emit(
                                "model-download",
                                json!({
                                    "tier": tier_key,
                                    "status": "error",
                                    "error": format!("保存模型文件失败：{e}"),
                                }),
                            );
                            return;
                        }
                        let final_size = fs::metadata(&final_path)
                            .map(|m| m.len())
                            .unwrap_or(spec.approx_bytes);
                        let _ = app.emit(
                            "model-download",
                            json!({
                                "tier": tier_key,
                                "status": "done",
                                "downloadedBytes": final_size,
                                "totalBytes": final_size,
                                "speedBps": 0,
                                "percent": 100.0,
                                "filePath": final_path.display().to_string(),
                            }),
                        );
                    } else {
                        let mut err_text = String::new();
                        if let Some(mut se) = child.stderr.take() {
                            let _ = se.read_to_string(&mut err_text);
                        }
                        let msg = if err_text.trim().is_empty() {
                            format!("下载中断（退出码 {:?}），可点继续断点续传或在设置中切换下载源", status.code())
                        } else {
                            let short: String = err_text.trim().chars().take(140).collect();
                            format!("下载失败：{short}")
                        };
                        let cur = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
                        let _ = app.emit(
                            "model-download",
                            json!({
                                "tier": tier_key,
                                "status": "error",
                                "downloadedBytes": cur,
                                "totalBytes": spec.approx_bytes,
                                "speedBps": 0,
                                "error": msg,
                            }),
                        );
                    }
                    return;
                }
                Ok(None) => {
                    thread::sleep(Duration::from_millis(350));
                    let cur = fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0);
                    let dt = prev_time.elapsed().as_secs_f64().max(0.05);
                    let delta = cur.saturating_sub(prev_bytes) as f64;
                    let inst_bps = delta / dt;
                    smooth_bps = if smooth_bps <= 0.1 {
                        inst_bps
                    } else {
                        smooth_bps * 0.65 + inst_bps * 0.35
                    };
                    prev_bytes = cur;
                    prev_time = Instant::now();

                    let _ = app.emit(
                        "model-download",
                        json!({
                            "tier": tier_key,
                            "status": "downloading",
                            "downloadedBytes": cur,
                            "totalBytes": spec.approx_bytes,
                            "speedBps": smooth_bps as u64,
                            "percent": (cur as f64 / spec.approx_bytes as f64 * 100.0).min(99.9),
                        }),
                    );
                }
                Err(e) => {
                    cleanup_active(&tier_key);
                    let _ = app.emit(
                        "model-download",
                        json!({
                            "tier": tier_key,
                            "status": "error",
                            "error": format!("监控下载进程失败：{e}"),
                        }),
                    );
                    return;
                }
            }
        }
    });

    Ok(format!("开始下载 {}", spec.name))
}

pub fn cancel_download(tier: &str) -> bool {
    let map = active_map().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(flag) = map.get(tier) {
        flag.store(true, Ordering::Relaxed);
        return true;
    }
    false
}

fn cleanup_active(tier: &str) {
    let mut map = active_map().lock().unwrap_or_else(|e| e.into_inner());
    map.remove(tier);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_builder_handles_all_three_sources() {
        let mid = MODELS.iter().find(|m| m.tier == "mid").unwrap();

        let ms_url = build_download_url(mid, &DownloadConfig::default()).unwrap();
        assert_eq!(
            ms_url,
            "https://modelscope.cn/models/Qwen/Qwen3-8B-GGUF/resolve/master/Qwen3-8B-Q4_K_M.gguf"
        );

        let hf_url = build_download_url(
            mid,
            &DownloadConfig {
                source: DownloadSource::HfMirror,
                custom_base_url: String::new(),
                imported_model_path: String::new(),
            },
        )
        .unwrap();
        assert_eq!(
            hf_url,
            "https://hf-mirror.com/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf"
        );

        let custom_tpl = build_download_url(
            mid,
            &DownloadConfig {
                source: DownloadSource::Custom,
                custom_base_url: "http://192.168.1.5:8080/models/{file}".into(),
                imported_model_path: String::new(),
            },
        )
        .unwrap();
        assert_eq!(
            custom_tpl,
            "http://192.168.1.5:8080/models/Qwen3-8B-Q4_K_M.gguf"
        );
    }

    #[test]
    fn gguf_magic_validation_rejects_html() {
        let p_ok = std::env::temp_dir().join(format!("test_ok_{}.gguf", std::process::id()));
        let p_bad = std::env::temp_dir().join(format!("test_bad_{}.gguf", std::process::id()));
        fs::write(&p_ok, b"GGUF\x03\x00\x00\x00").unwrap();
        fs::write(&p_bad, b"<html>404</html>").unwrap();

        assert!(is_valid_gguf(&p_ok));
        assert!(!is_valid_gguf(&p_bad));

        let _ = fs::remove_file(&p_ok);
        let _ = fs::remove_file(&p_bad);
    }
}
