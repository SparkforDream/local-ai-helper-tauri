# 本地 AI 助手（local-ai-helper）

面向不太懂电脑的普通人（新手、长辈）的 Windows 桌面轻量级助手。

核心原则很明确：**不替人乱动高危系统项，专注「听懂问题 → 读真实机器状态 → 代码算结论 → 用人话讲清楚」。**

---

## 一、项目定位

市面上给小白用的工具大多集中在「一键装软件 / 一键清 C 盘」，或者直接扔一个通用大模型对话框让对方自己分辨真假。

这个项目做的是中间那层**有边界、讲人话的轻度 Agent**：能用确定性代码查清的事，直接给实测数字和结论，不靠大模型猜；涉及下载和安装，先查本机是不是已经装过，校验体积与数字签名，高危动作必须本人点头；遇到超出能力范围的问题，直接说判断不了，不编造网址、不编造参数、不说教。

---

## 二、核心重难点与对应解法

### 1. 搜索引擎假官网与捆绑下载陷阱

普通人搜「Steam」「7-Zip」「Bandizip」极易点进仿冒站或带高速下载器的捆绑包，拼错一个字母搜出来的几乎全是李鬼站。对应做法是内置经过 HTTP 实测核验的官网白名单（当前 15 条），配合编辑距离与中文变体的拼写容错，守住表达纪律——不复述错字、不点评拼写、不居高临下说教。未收录软件给出三档域名可信度评分（`confident` / `likely` / `unknown`）与辨别方法；常见软件支持四步安全安装编排（探测已装 → HTTPS 下载 → 体积与数字签名核验 → 确认后安装 → 装后进程启动验证）。

### 2. 「电脑很卡」但看不懂真正瓶颈

新手常把「CPU 占用不高」当成没问题，而真正拖慢机器的原因分散在各类系统指标里。对应做法是在约 585ms 内完成只读硬件采集，绕过 Windows WMI `AdapterRAM` 的 4GB 溢出假值与核显共享内存虚高问题（按 `nvidia-smi` → 注册表 `qwMemorySize` → WMI 三级优先取值）。卡顿体检按五项硬编码阈值直接判因（内存空闲 `< 15%`、C 盘剩 `< 10%`、磁盘响应 `> 50ms`、磁盘忙度 `>= 95%`、CPU 降频 `< 50%`），并联动 `proc.top`、`fs.biggest` 与 `net.check` 给出具体根因。

### 3. 小模型做 Agent 的幻觉与安全边界

3B~8B 本地小模型扛不住多轮自由工具调用，直接把硬件数字喂给模型常出现「数字全对、判断全错」。因此确定性能力永远优先；口语化提问经 GBNF 语法约束 + 温度 0 + 18 条示例分诊台（`triage.rs`）优先路由回 Rust 确定性能力。涉及文件操作时采用「模型拆待办 → Rust 权限制动 → 逐项执行」闭环，默认「请求审查」弹窗确认，可选「完全权限」（附二次风险提醒及进程内不再提醒勾选），并在设置页提供自定义授权目录白名单与操作审计记录。

### 4. 不同硬件档位的体面适配与零解压交付

按扣除驱动预留（`0.98 GB`）后的真实可用显存与内存自动划分五档（极低 / 低 / 中 / 高 / 极高），低配机直接给出网页版 AI 三步打开引导。安装包采用 NSIS 单文件中文 `.exe`（免 UAC 提权），内置精简版 Vulkan `llama-server` 推理引擎，退出软件自动清理托管进程；模型权重默认走魔搭官方阿里云 CDN 断点续传并实时显示 `MB/s` 下载速度，同时支持导入本地已有 `.gguf` 文件以及在设置页切换下载源。

---

## 三、模型与自定义 API 支持

在左下角 **设置 → 模型与接口** 中，可随时在三种模式间切换：

| 模式 | 说明 | 适用场景 |
|---|---|---|
| **关闭（纯规则）** | 不调用任何模型，仍可使用硬件检测、五项卡顿体检、大目录扫描、网络自检、官网白名单、软件安装编排 | 默认模式，零依赖、零配置 |
| **本地模型** | 自动托管内置 `llama-server`（默认 `http://127.0.0.1:18090/v1`），支持选模型页点「去对话」自动拉起、设置页一键启停、导入本地 `.gguf` | 已下载或导入本地模型的机器 |
| **自定义 API（自行导入 API Key）** | 支持任意 OpenAI 兼容接口，内置 MiniMax、DeepSeek、智谱 GLM、硅基流动、OpenAI 一键预设及「测试连接」 | 希望获得更强开放问答能力，或由子女帮长辈配好账号 |

**API Key 保管方式**：Key 保存在本机 `%LOCALAPPDATA%\local-ai-helper\llm.json`，设置页仅回显脱敏掩码，支持留空保持原 Key 或一键清空。网络请求仅在 Rust 后端发起，鉴权头通过 `stdin` 传给系统 `curl.exe`，不进入进程命令行参数（`argv`）。对话支持 SSE 流式输出、可折叠思考过程以及多会话本地持久化。

---

## 四、技术栈与目录结构

桌面外壳与后端使用 Rust + Tauri 2.x（零额外网络 crate，复用 Windows 10/11 原生 `curl.exe` 与 PowerShell）；前端使用原生 HTML / CSS / ES Module JS（无打包器、无前端框架，支持纯白浅色与深色双主题）。

```text
local-ai-helper-tauri/
├─ assets/
│  ├─ hardware.ps1          # 只读硬件采集脚本
│  ├─ official.yaml         # 官网白名单源数据
│  └─ runtime/              # 打包用内置 Vulkan llama-server（不入 Git）
├─ scripts/
│  └─ build-knowledge.mjs   # YAML → src/lib/official.data.js 生成脚本
├─ src/                     # 原生前端（首页 / 档位 / 网页引导 / 问问题 / 配置详情 / 设置）
│  ├─ index.html
│  ├─ app.js
│  ├─ lib/                  # api / pipeline / trust / typo / tiers / social / delay / mock
│  └─ styles/               # base / layout / pages
└─ src-tauri/
   ├─ tauri.conf.json
   ├─ capabilities/default.json
   └─ src/
      ├─ lib.rs             # Tauri 命令注册与退出清理
      ├─ llm.rs             # 统一 OpenAI 兼容层（含 SSE 流式与思考链拆分）
      ├─ triage.rs          # 小模型 GBNF 约束分诊台
      ├─ agent.rs           # 受控待办规划、权限闸门、观察回喂与操作审计
      ├─ download.rs        # 魔搭断点续传下载器、下载源切换与本地 .gguf 导入
      ├─ runtime.rs         # 内置 llama-server 托管启停
      ├─ history.rs         # 多会话对话历史持久化
      └─ capability/        # 确定性能力集（hw / proc / fs / net / sys / recipe / detect / flow）
```

---

## 五、本地构建与打包

环境要求：Windows 10/11、Rust 工具链、Tauri CLI 2.x、Node.js（用于前端自测）。打包内置引擎前需将 Vulkan 版 `llama-server.exe` 及依赖 DLL 放入 `assets/runtime/`。

```powershell
# 运行 Rust 单元测试（53 项）
cargo test --manifest-path src-tauri\Cargo.toml

# 构建 Debug 桌面程序
cargo build --manifest-path src-tauri\Cargo.toml

# 构建 Release 与 NSIS 中文一键安装程序
cargo tauri build --bundles nsis

# 运行前端模块自测
node src\lib\trust.js
node src\lib\typo.js
node src\lib\pipeline.js
node src\lib\social.js
node src\lib\delay.js
```

> 本说明随项目迭代持续维护。
