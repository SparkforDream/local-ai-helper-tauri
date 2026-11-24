// ============================================================
// capability/mod.rs —— 能力清单与执行器
//
// 设计参照 Alpha Enhanced 的做法：给模型一份「完整的能力清单」，
// 它只负责从清单里挑一个、填参数，不自由生成。
//
// 三条纪律：
//   1. 每个能力都是确定性的，同样的输入给同样的输出。
//   2. 每个能力都标风险等级，高风险必须用户确认。
//   3. 能力清单本身可以被序列化出去（模型/前端要能读到它才能选）。
// ============================================================

pub mod detect;
pub mod flow;
pub mod fs;
pub mod hw;
pub mod net;
pub mod proc;
pub mod recipe;
pub mod sys;

use serde::{Deserialize, Serialize};

/// 能力风险等级。决定执行前要不要弹确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// 只读，不改任何东西。直接执行。
    ReadOnly,
    /// 会产生可逆改动，或短暂占用资源。需要用户确认。
    Reversible,
    /// 不可逆或影响系统。必须用户逐项确认，且要写明后果。
    Dangerous,
}

impl Risk {
    pub fn needs_confirm(self) -> bool {
        !matches!(self, Risk::ReadOnly)
    }

    pub fn label(self) -> &'static str {
        match self {
            Risk::ReadOnly => "只读",
            Risk::Reversible => "可撤销",
            Risk::Dangerous => "不可逆",
        }
    }
}

/// 参数类型。用来生成 JSON Schema，也用来做运行时校验。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ParamType {
    Str {
        #[serde(default)]
        max_len: Option<usize>,
    },
    Int {
        #[serde(default)]
        min: Option<i64>,
        #[serde(default)]
        max: Option<i64>,
    },
    Bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: &'static str,
    pub desc: &'static str,
    #[serde(flatten)]
    pub ty: ParamType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// 一个能力（对应 Alpha 里的一个 Lua API 函数）
#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    /// 唯一标识，如 "hw.cpu"。模型就是靠这个字符串选。
    pub id: &'static str,
    /// 一句话说明，给模型和用户看。
    pub desc: &'static str,
    /// 什么时候该用这个能力。模型靠这句判断。
    pub when: &'static str,
    /// 参数定义
    pub params: Vec<Param>,
    /// 风险等级
    pub risk: Risk,
    /// 返回的是什么，给模型理解输出用
    pub returns: &'static str,
}

/// 执行结果
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 执行耗时，毫秒
    pub ms: u64,
}

impl Outcome {
    pub fn ok(data: serde_json::Value, ms: u64) -> Self {
        Self { ok: true, data: Some(data), error: None, ms }
    }
    pub fn err(msg: impl Into<String>, ms: u64) -> Self {
        Self { ok: false, data: None, error: Some(msg.into()), ms }
    }
}

/// 运行时参数包，做了类型收紧，避免能力实现里到处 get as_str。
#[derive(Debug, Clone)]
pub struct Args {
    inner: serde_json::Map<String, serde_json::Value>,
}

impl Args {
    pub fn new(v: serde_json::Value) -> Self {
        let inner = match v {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        Self { inner }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.inner.get(key).and_then(|v| v.as_str())
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.inner.get(key).and_then(|v| v.as_i64())
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.inner.get(key).and_then(|v| v.as_bool())
    }

    /// 带默认值的整数读取，并做区间收敛。
    pub fn int_or(&self, key: &str, default: i64, min: i64, max: i64) -> i64 {
        self.int(key).unwrap_or(default).clamp(min, max)
    }
}

/// 校验参数是否符合声明。返回第一条错误。
pub fn validate(cap: &Capability, args: &Args) -> Result<(), String> {
    for p in &cap.params {
        let present = match &p.ty {
            ParamType::Str { .. } => args.str(p.name).is_some(),
            ParamType::Int { .. } => args.int(p.name).is_some(),
            ParamType::Bool => args.bool(p.name).is_some(),
        };

        if p.required && !present {
            return Err(format!("缺少参数 {}", p.name));
        }
        if !present {
            continue;
        }

        if let ParamType::Str { max_len } = &p.ty {
            if let Some(s) = args.str(p.name) {
                if let Some(ml) = max_len {
                    if s.chars().count() > *ml {
                        return Err(format!("参数 {} 超长（上限 {}）", p.name, ml));
                    }
                }
                // 路径类参数不许出现 .. 逃逸
                if s.contains("..") {
                    return Err(format!("参数 {} 含非法路径片段", p.name));
                }
            }
        }
        if let ParamType::Int { min, max } = &p.ty {
            if let Some(v) = args.int(p.name) {
                if let Some(lo) = min {
                    if v < *lo {
                        return Err(format!("参数 {} 小于下限 {}", p.name, lo));
                    }
                }
                if let Some(hi) = max {
                    if v > *hi {
                        return Err(format!("参数 {} 大于上限 {}", p.name, hi));
                    }
                }
            }
        }
    }
    Ok(())
}

/// 全部能力的清单。模型和前端都从这里读。
pub fn all() -> Vec<Capability> {
    let mut v = Vec::new();
    v.extend(hw::capabilities());
    v.extend(proc::capabilities());
    v.extend(fs::capabilities());
    v.extend(net::capabilities());
    v.extend(sys::capabilities());
    v.extend(recipe::capabilities());
    v
}

/// 按 id 派发执行。
pub fn execute(id: &str, raw: serde_json::Value) -> Outcome {
    let t0 = std::time::Instant::now();
    let args = Args::new(raw);

    // 先确认这个 id 存在，再校验参数。顺序不能反：
    // 不存在的 id 报「未知能力」比报「缺参数」更有用。
    let cap = match all().into_iter().find(|c| c.id == id) {
        Some(c) => c,
        None => {
            return Outcome::err(format!("未知能力：{}", id), t0.elapsed().as_millis() as u64)
        }
    };

    if let Err(e) = validate(&cap, &args) {
        return Outcome::err(e, t0.elapsed().as_millis() as u64);
    }

    let result = match id {
        // 硬件
        "hw.all" => hw::all(&args),
        "hw.diagnose" => hw::diagnose(&args),
        "hw.cpu" => hw::cpu(&args),
        "hw.mem" => hw::mem(&args),
        "hw.gpu" => hw::gpu(&args),
        "hw.disk" => hw::disk(&args),
        // 进程
        "proc.top" => proc::top(&args),
        "proc.list" => proc::list(&args),
        // 文件系统
        "fs.dir_size" => fs::dir_size(&args),
        "fs.biggest" => fs::biggest(&args),
        "fs.exists" => fs::exists(&args),
        "fs.mkdir" => fs::mkdir(&args),
        "fs.rmdir_empty" => fs::rmdir_empty(&args),
        // 网络与系统面板
        "net.check" => net::check(&args),
        "sys.open" => sys::open(&args),
        // 四步配方
        "recipe.list" => recipe::list(&args),
        "recipe.step1_download" => recipe::step1_download(&args),
        "recipe.step2_verify" => recipe::step2_verify(&args),
        "recipe.step3_install" => recipe::step3_install(&args),
        "recipe.step4_verify_install" => recipe::step4_verify_install(&args),
        other => Err(format!("能力 {} 已注册但没有实现", other)),
    };

    let ms = t0.elapsed().as_millis() as u64;
    match result {
        Ok(data) => Outcome::ok(data, ms),
        Err(e) => Outcome::err(e, ms),
    }
}

// ============================================================
// 自测：cargo test capability
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 能力清单不为空且_id_不重复() {
        let caps = all();
        assert!(!caps.is_empty(), "能力清单为空");

        let mut ids: Vec<&str> = caps.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "存在重复的能力 id");
    }

    #[test]
    fn 每个能力都有说明和适用场景() {
        for c in all() {
            assert!(!c.desc.is_empty(), "{} 缺 desc", c.id);
            assert!(!c.when.is_empty(), "{} 缺 when", c.id);
            assert!(!c.returns.is_empty(), "{} 缺 returns", c.id);
        }
    }

    #[test]
    fn 未知能力要报未知而不是缺参数() {
        let r = execute("no.such", serde_json::json!({}));
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("未知能力"));
    }

    #[test]
    fn 必填参数缺失要报缺参数() {
        let r = execute("fs.dir_size", serde_json::json!({}));
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("缺少参数"));
    }

    #[test]
    fn 路径逃逸要被拦住() {
        let r = execute("fs.dir_size", serde_json::json!({"path": "C:\\..\\Windows"}));
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("非法路径"));
    }

    #[test]
    fn 只读能力不需要确认() {
        for c in all().iter().filter(|c| c.risk == Risk::ReadOnly) {
            assert!(!c.risk.needs_confirm(), "{} 是只读却要求确认", c.id);
        }
    }

    /// 打印完整清单，人工核对「模型能不能看懂这份清单」。
    /// cargo test dump_capability_list -- --nocapture
    #[test]
    fn dump_capability_list() {
        for c in all() {
            println!("{:<14} [{}]  {}", c.id, c.risk.label(), c.desc);
            println!("               何时用：{}", c.when);
            for p in &c.params {
                println!(
                    "               参数 {} {} — {}",
                    p.name,
                    if p.required { "必填" } else { "可选" },
                    p.desc
                );
            }
            println!("               返回：{}", c.returns);
            println!();
        }
    }
}

    #[test]
    fn live_smoke() {
        use serde_json::json;
        for (id, args) in [
            ("hw.cpu", json!({})),
            ("hw.mem", json!({})),
            ("hw.gpu", json!({})),
            ("hw.disk", json!({})),
            ("proc.top", json!({"n": 5})),
            ("fs.dir_size", json!({"path": "C:\\workspace"})),
        ] {
            let t0 = std::time::Instant::now();
            let r = execute(id, args);
            println!("── {}  ({}ms)", id, t0.elapsed().as_millis());
            match (r.ok, r.data, r.error) {
                (true, Some(d), _) => {
                    let s = serde_json::to_string(&d).unwrap_or_default();
                    println!("   {}", if s.len() > 260 { &s[..260] } else { &s });
                }
                (_, _, Some(e)) => println!("   ERR {}", e),
                _ => println!("   空"),
            }
        }
    }

    #[test]
    fn live_four_steps() {
        use serde_json::json;
        // 先看清单
        println!("── recipe.list");
        println!("   {}", serde_json::to_string(&recipe::list(&Args::new(json!({}))).unwrap()).unwrap());

        // ④ 先核对当前状态（已装的话就不用装了）
        println!("\n── step4 核对当前 Steam 状态");
        let r4 = recipe::step4_verify_install(&Args::new(json!({"id":"steam"}))).unwrap();
        println!("   {}", serde_json::to_string_pretty(&r4).unwrap());

        // ② 校验不存在的安装包 → 应明确说要先跑第一步
        println!("\n── step2 校验（安装包还不存在）");
        let r2 = recipe::step2_verify(&Args::new(json!({"id":"steam"}))).unwrap();
        println!("   {}", serde_json::to_string(&r2).unwrap());
    }

    #[test]
    fn live_detect_demo() {
        use serde_json::json;
        println!("── step4 分级探测（Steam）");
        let r = recipe::step4_verify_install(&Args::new(json!({"id":"steam"}))).unwrap();
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
    }


    #[test]
    fn live_flow_walkthrough() {
        use serde_json::json;
        // 场景一：Steam 已经装了 —— 应该直接告知，不给下载按钮
        println!("══ 场景一：用户说「帮我装 Steam」（实际已装）══");
        let s = flow::start("steam");
        println!("阶段：{:?}", s.stage);
        println!("{}", s.say);
        println!("按钮：{:?}", s.actions.iter().map(|a| &a.label).collect::<Vec<_>>());
        println!("证据：{:?}", s.facts.get("foundBy"));

        // 场景二：7-Zip —— 假设没装，走完整流程（只走到下载前，不真下）
        println!("\n══ 场景二：用户说「帮我装 7-Zip」══");
        let s2 = flow::start("7zip");
        println!("阶段：{:?}", s2.stage);
        println!("{}", s2.say);
        println!("按钮：{:?}", s2.actions.iter().map(|a| &a.label).collect::<Vec<_>>());

        // 场景三：用户中途反悔
        println!("\n══ 场景三：用户点「先不装」══");
        let s3 = flow::advance(&s2, "cancel");
        println!("阶段：{:?}", s3.stage);
        println!("{}", s3.say);

        // 场景四：未知软件
        println!("\n══ 场景四：用户说「帮我装 Photoshop」══");
        let s4 = flow::start("photoshop");
        println!("阶段：{:?}", s4.stage);
        println!("{}", s4.say);
    }
