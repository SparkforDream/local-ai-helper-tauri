// ============================================================
// runtime.rs —— 本地 llama-server 托管器（自动发现、一键启停、退出清理）
//
// 设计原则：
//   1. 优先使用随安装包内置的 runtime\llama-server.exe（免解压、免配置）；
//   2. 健康检查用纯 Rust std::net::TcpStream 探测 /health，零子进程开销；
//   3. 拉起子进程设置 CREATE_NO_WINDOW + current_dir(runtime 目录)，不弹黑框不丢 DLL；
//   4. 退出应用（RunEvent::Exit）时自动 kill 托管的子进程，绝不留僵尸进程占显存。
// ============================================================

use crate::download;
use crate::llm;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct ManagedServer {
    child: Child,
    model_name: String,
    model_path: String,
}

static SERVER: OnceLock<Mutex<Option<ManagedServer>>> = OnceLock::new();

fn server_slot() -> &'static Mutex<Option<ManagedServer>> {
    SERVER.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub running: bool,
    pub loading: bool,
    pub managed: bool,
    pub server_found: bool,
    pub model_found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_path: Option<String>,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    Ready,
    Loading,
    Down,
}

pub fn parse_local_host_port(base_url: &str) -> (String, u16) {
    let trimmed = base_url.trim();
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .unwrap_or(trimmed);
    let authority = without_scheme.split('/').next().unwrap_or("");
    if let Some((h, p)) = authority.rsplit_once(':') {
        if let Ok(port) = p.parse::<u16>() {
            let host = if h.is_empty() || h == "localhost" {
                "127.0.0.1".to_string()
            } else {
                h.to_string()
            };
            return (host, port);
        }
    }
    ("127.0.0.1".into(), 18090)
}

pub fn find_server_exe() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("runtime").join("llama-server.exe"));
            candidates.push(dir.join("llama-server.exe"));
        }
    }
    if let Some(dev_root) = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent() {
        candidates.push(
            dev_root
                .join("assets")
                .join("runtime")
                .join("llama-server.exe"),
        );
    }
    candidates.push(PathBuf::from(
        r"D:\workspace\local-ai-helper\.runtime\vulkan\llama-server.exe",
    ));

    candidates.into_iter().find(|p| p.is_file())
}

fn find_model_for_tier(tier_opt: Option<&str>) -> Option<(String, PathBuf)> {
    // 若显式要求用导入模型，或未指定档位且用户已导入本地 .gguf，优先使用导入模型
    if tier_opt.is_none() || tier_opt == Some("imported") {
        let dl_cfg = download::load_config();
        let imp = dl_cfg.imported_model_path.trim();
        if !imp.is_empty() {
            let p = PathBuf::from(imp);
            if p.is_file() && download::is_valid_gguf(&p) {
                let name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("导入的本地模型")
                    .to_string();
                return Some((name, p));
            }
        }
    }

    let list = download::list_status();
    if let Some(tier) = tier_opt {
        if let Some(found) = list
            .iter()
            .find(|m| m.tier == tier && m.downloaded && m.file_path.is_some())
        {
            return Some((
                found.name.clone(),
                PathBuf::from(found.file_path.as_ref().unwrap()),
            ));
        }
    }
    // 没指定档位或指定档未下时，优先选 mid，其次任意已下载模型
    if let Some(mid) = list
        .iter()
        .find(|m| m.tier == "mid" && m.downloaded && m.file_path.is_some())
    {
        return Some((
            mid.name.clone(),
            PathBuf::from(mid.file_path.as_ref().unwrap()),
        ));
    }
    list.into_iter()
        .find(|m| m.downloaded && m.file_path.is_some())
        .map(|m| (m.name, PathBuf::from(m.file_path.unwrap())))
}

fn check_health(host: &str, port: u16) -> Health {
    let addr_str = format!("{host}:{port}");
    let Ok(mut addrs) = addr_str.to_socket_addrs() else {
        return Health::Down;
    };
    let Some(addr): Option<SocketAddr> = addrs.next() else {
        return Health::Down;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(600)) else {
        return Health::Down;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(600)));

    let req = format!("GET /health HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return Health::Down;
    }

    let mut buf = [0u8; 512];
    let n = stream.read(&mut buf).unwrap_or(0);
    if n == 0 {
        return Health::Down;
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    let first_line = head.lines().next().unwrap_or("");
    if first_line.contains(" 200") {
        Health::Ready
    } else if first_line.contains(" 503") {
        Health::Loading
    } else {
        Health::Down
    }
}

fn clean_dead_managed() {
    let mut guard = server_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(srv) = guard.as_mut() {
        if let Ok(Some(_)) = srv.child.try_wait() {
            *guard = None;
        }
    }
}

pub fn status() -> RuntimeStatus {
    clean_dead_managed();
    let cfg = llm::load_config();
    let (host, port) = parse_local_host_port(&cfg.local_base_url);
    let endpoint = format!("http://{host}:{port}/v1");
    let health = check_health(&host, port);
    let server_found = find_server_exe().is_some();
    let default_model = find_model_for_tier(None);

    let guard = server_slot().lock().unwrap_or_else(|e| e.into_inner());
    let (managed, pid, m_name, m_path) = match guard.as_ref() {
        Some(srv) => (
            true,
            Some(srv.child.id()),
            Some(srv.model_name.clone()),
            Some(srv.model_path.clone()),
        ),
        None => (
            false,
            None,
            default_model.as_ref().map(|(n, _)| n.clone()),
            default_model
                .as_ref()
                .map(|(_, p)| p.display().to_string()),
        ),
    };

    let running = health == Health::Ready;
    let loading = health == Health::Loading;
    let message = if running {
        if managed {
            format!(
                "本地引擎运行中（{}）",
                m_name.as_deref().unwrap_or("已托管")
            )
        } else {
            "检测到外部本地服务正在运行".into()
        }
    } else if loading {
        "本地模型正在加载进显存…".into()
    } else if !server_found {
        "未找到内置 llama-server.exe".into()
    } else if m_path.is_none() {
        "尚未下载本地模型，请先在「档位」页下载一档".into()
    } else {
        format!(
            "本地引擎未启动（已就绪模型：{}）",
            m_name.as_deref().unwrap_or("本地 GGUF")
        )
    };

    RuntimeStatus {
        running,
        loading,
        managed,
        server_found,
        model_found: m_path.is_some(),
        model_name: m_name,
        model_path: m_path,
        endpoint,
        pid,
        message,
    }
}

pub fn start(tier_opt: Option<String>) -> Result<RuntimeStatus, String> {
    clean_dead_managed();
    let cfg = llm::load_config();
    let (host, port) = parse_local_host_port(&cfg.local_base_url);

    // 已经健康运行中：若是同一模型或外部已启动，直接复用；若要求切换托管模型则先停再启
    let desired = find_model_for_tier(tier_opt.as_deref());
    {
        let guard = server_slot().lock().unwrap_or_else(|e| e.into_inner());
        if check_health(&host, port) == Health::Ready {
            let need_switch = match (&*guard, &desired) {
                (Some(cur), Some((_, want_path))) => {
                    tier_opt.is_some() && cur.model_path != want_path.display().to_string()
                }
                _ => false,
            };
            if !need_switch {
                drop(guard);
                return Ok(status());
            }
        }
    }

    stop();

    let server_exe = find_server_exe()
        .ok_or_else(|| "找不到内置推理引擎 llama-server.exe，请检查安装目录是否完整".to_string())?;
    let (model_name, model_path) = desired
        .ok_or_else(|| "还没有下载本地模型，请先去「档位」页下载一档".to_string())?;

    let mut cmd = Command::new(&server_exe);
    cmd.args(["-m"])
        .arg(&model_path)
        .args([
            "--host",
            &host,
            "--port",
            &port.to_string(),
            "-ngl",
            "99",
            "-c",
            "4096",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Some(dir) = server_exe.parent() {
        cmd.current_dir(dir);
    }

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let child = cmd
        .spawn()
        .map_err(|e| format!("启动本地推理引擎失败：{e}"))?;

    {
        let mut guard = server_slot().lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(ManagedServer {
            child,
            model_name: model_name.clone(),
            model_path: model_path.display().to_string(),
        });
    }

    // 轮询 /health 等待模型加载完成（最多 40 秒）
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        match check_health(&host, port) {
            Health::Ready => return Ok(status()),
            Health::Loading | Health::Down => {
                // 检查子进程是不是提前退出了（例如显存不足崩溃）
                let exited = {
                    let mut guard = server_slot().lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(srv) = guard.as_mut() {
                        match srv.child.try_wait() {
                            Ok(Some(st)) => {
                                *guard = None;
                                Some(st.code())
                            }
                            _ => None,
                        }
                    } else {
                        Some(None)
                    }
                };
                if let Some(code) = exited {
                    return Err(format!(
                        "本地推理引擎启动后异常退出（退出码 {:?}），可能是显存或内存不足，请尝试更小一档或关闭占用程序",
                        code
                    ));
                }
                thread::sleep(Duration::from_millis(350));
            }
        }
    }

    Err("本地模型加载超时（超过 40 秒），可能正在后台读取大模型，稍后可点「测试连接」确认".into())
}

pub fn stop() -> bool {
    let mut guard = server_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut srv) = guard.take() {
        let _ = srv.child.kill();
        let _ = srv.child.wait();
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_port_handles_default_and_custom() {
        assert_eq!(
            parse_local_host_port("http://127.0.0.1:18090/v1"),
            ("127.0.0.1".into(), 18090)
        );
        assert_eq!(
            parse_local_host_port("http://localhost:11434/v1"),
            ("127.0.0.1".into(), 11434)
        );
        assert_eq!(parse_local_host_port("invalid"), ("127.0.0.1".into(), 18090));
    }

    #[test]
    fn bundled_llama_server_is_discoverable() {
        let found = find_server_exe();
        assert!(found.is_some(), "assets/runtime/llama-server.exe 应能被发现");
    }
}
