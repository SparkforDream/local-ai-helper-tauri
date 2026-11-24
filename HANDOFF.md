# 项目交接文档

> 写给下一个接手的对话：读完这份就能直接开工，不用翻历史。
> 生成时间：2026-11-24 · 生成时状态已实测核对

---

## 一句话定位

面向电脑小白的本地 AI 助手：**不代劳高危操作**，专注"听懂问题 → 看机器状态 → 给建议 → 讲人话"。

用「表达纪律 + 边界诚实」区别于 winget GUI、软件管家、远程协助行业最底层的"装软件 / 清 C 盘"档位。

---

## 项目路径

```
D:\projects\local-ai-helper-tauri\           ← 当前主项目（Rust + Tauri 2.x，2026-11-24 从 C:\workspace 迁出）
```

⚠️ **不要在 `D:\workspace\` 下建任何项目。** 该路径下有某个异常会让任何需要加载 DLL 的程序在启动时抛 `EXCEPTION_ACCESS_VIOLATION 0xC0000005`（连 `electron --version` 都崩）。迁移到独立目录 `D:\projects\` 后正常。

### 目录结构

```
local-ai-helper-tauri\
├─ assets\
│  ├─ hardware.ps1        11.7KB  只读硬件采集，输出单个 JSON，实测 585ms
│  └─ official.yaml       10.2KB  15 条官网白名单，全部 HTTP 实测验证过
├─ scripts\
│  └─ build-knowledge.mjs  6.3KB  自研极简 YAML 解析器 → src/lib/official.data.js
├─ src\                           前端（原生 JS，无框架、无构建步骤）
│  ├─ index.html           6.2KB  5 个页面
│  ├─ app.js              21.1KB  主逻辑，见下方函数表
│  ├─ lib\
│  │  ├─ api.js            2.1KB  invoke 封装，浏览器预览走 mock
│  │  ├─ delay.js          3.7KB  节奏控制（不秒回的实现）
│  │  ├─ mock.js           1.4KB  浏览器预览假数据
│  │  ├─ official.data.js  9.1KB  由 yaml 生成，勿手改
│  │  ├─ pipeline.js      16.3KB  官网查询主链路，不调模型
│  │  ├─ social.js        11.1KB  寒暄 / 骂人 / 天气 / 时间
│  │  ├─ tiers.js          3.8KB  五档判档纯函数
│  │  ├─ trust.js         11.5KB  域名可信度三档评分
│  │  └─ typo.js          11.7KB  拼写容错 + 语气 lint
│  └─ styles\
│     ├─ base.css          3.5KB  主题变量（浅色默认 / 深色）
│     ├─ layout.css        4.0KB  侧边栏 + 页面切换
│     └─ pages.css         8.2KB  各页细节
├─ src-tauri\
│  ├─ tauri.conf.json            窗口 1000x720，productName 本地AI助手
│  ├─ capabilities\default.json  ⚠️ 缺这个文件前端 invoke 会静默挂起
│  └─ src\
│     ├─ lib.rs            6.7KB  命令注册
│     ├─ main.rs           0.2KB
│     └─ capability\
│        ├─ mod.rs        13.5KB  能力注册与派发、参数校验、风险分级
│        ├─ hw.rs          6.6KB  hw.all/cpu/mem/gpu/disk
│        ├─ proc.rs        4.2KB  proc.top / proc.list
│        ├─ fs.rs          6.2KB  fs.dir_size / fs.biggest
│        ├─ recipe.rs     14.8KB  安装四步配方（steam / 7zip）
│        ├─ detect.rs      8.0KB  安装位置三级探测
│        └─ flow.rs       13.3KB  四步编排状态机
└─ shots\                        20 张开发过程截图
```

**无 git 仓库**，纯文件项目。

---

## 当前状态（实测核对）

| 项 | 状态 |
|---|---|
| Rust 单测 | `29 passed; 0 failed`（3.55s） |
| 前端自测 | trust 14/14 · typo 6/6 · pipeline 5/5 · social 19/19+4/4 · delay 8/8 |
| debug 构建 | 12.8MB，可运行 |
| release 构建 | **未构建** |
| 打包 NSIS | **未做** |
| 本地模型接入 | **未做**（`flow` 完全不调模型） |

### 构建与测试命令

```powershell
cd D:\projects\local-ai-helper-tauri
$env:PATH = "C:\Users\86481\.cargo\bin;$env:PATH"

cargo build --manifest-path src-tauri\Cargo.toml          # 构建
cargo test  --manifest-path src-tauri\Cargo.toml          # 单测
node src\lib\delay.js                                     # 前端自测（Node 26 无需 flag）
```

工具链：Rust 1.100.0 / cargo 1.100.0 / Tauri CLI 2.12.6（`cargo-tauri.exe`）
镜像：`rsproxy.cn`（清华源 403，已弃用）
MSVC 链接器：`C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools`

---

## 做过什么（按层）

### 1. 硬件采集（只读）

`assets/hardware.ps1`，stdout 只输出一个 JSON，实测 585ms。

踩过的坑，别重踩：

- `Win32_VideoController.AdapterRAM` 是 **uint32**，显存 >4GB 一律返回 `4293918720` 假值
- 核显的 AdapterRAM 是共享内存切片，数值虚高且无意义 → **必须按 vendor 过滤掉**
- 显存要三级优先：`nvidia-smi` → 注册表 `HardwareInformation.qwMemorySize`(REG_QWORD) → WMI 兜底
- PowerShell 返回列表必须写 `,$list` 而不是 `@(...)`，否则展成嵌套数组导致 JSON 解析错位
- PS7 测 AVX2 用 `[System.Runtime.Intrinsics.X86.Avx2]::IsSupported`，5.1 要用反射

本机实测输出（ROG Strix G614JV）：i7-13650HX 14核20线程 / avx2=true / 内存 16780566528B / RTX 4060 Laptop `vramBytes=8585740288`（via nvidia-smi）

### 2. 硬件驱动预留常量

```js
const DRIVER_RESERVE = 0.98 * GB
```

理由：RTX 4060 标称 8.0GB，vulkan 实测只报 7188MiB = 7.02GB。不预留就会推荐跑不动的档。

### 3. 官网白名单（15 条，全部 HTTP 实测）

`assets/official.yaml` → `scripts/build-knowledge.mjs` → `src/lib/official.data.js`

收录：7zip / bandizip / steam / watt-toolkit / chrome / edge / wechat / qq / wps / vscode / everything / obs / driver-nvidia / driver-amd / vcredist

**关键发现**：

- `7-zip.com` 与 `7-zip.org` **内容逐字节相同**（7137/20162 字节），DNS 不同 IP，各自独立证书 → **不是仿冒，是官方备用域**
- `www.7-zip.cn` / `www.7zip.cn` / `7-zip.com.cn` / `7zip.org.cn` / `bandizip.cn` / `bandizip.com.cn` **全部是不可达的死站**，但搜索结果排名很高 → 正是要挡的目标
- `google.com` 国内 SSL 不通，Chrome 必须用 `google.cn`

### 4. 域名可信度评分（trust.js）

阈值 68 分才升 `likely`，三档 `confident / likely / unknown`。

**规则永远判不了的 4 例**（必须靠白名单，已硬编码记录）：

```
code.visualstudio.com   产品名 Visual Studio Code，域名却是缩写
www.google.cn           Chrome 在 google.cn 下，无 chrome 字样
learn.microsoft.com     运行库页面挂在 learn 子域
steampp.net             域名 steampp 与产品名 Watt Toolkit 无字面关系
```

可疑词匹配必须分两种：`SUSPECT_SEGMENT` 只按 `-` 分词比对（否则 `bandisoft` 的 `soft` 被误伤），`SUSPECT_SUBSTR` 才走子串（拼音类 xiazai/guanwang/zhengban/zhongwen/guochan/mianfei/pojie）。

### 5. 拼写容错（typo.js）

```js
const allowed = k.length <= 3 ? 1 : 2
```

4 字符也容 2 —— 因为相邻字母互换在编辑距离里算 2 次，`7zpi` / `chrmoe` / `vsocde` 全靠这条。

长度 <3 的别名不参与距离比较（`7z` 会抢掉 `7zip`）；同距离优先更长别名。

**三条文案纪律**（`composeAnswer`）：

1. 不复述用户的错误写法
2. 不评论拼写
3. 必须提醒时把原因归给搜索结果（"搜这个词出来的多半是仿的站"）

`lintTone` 有 6 类说教正则拦截：说教式开场 / 直接点破 / 「而不是」对比 / 居高临下提醒 / 复述错误写法 / 补充式说教。

### 6. 官网查询主链路（pipeline.js，不调模型）

`findSoftware` 三轮：① 别名精确扫描（最长优先）② 逐词编辑距离 ③ 中文变体表扫整句

`detectIntent` 顺序（踩坑改出来的）：**先认具体软件 → 再认类别词 → 最后看语气**。
反例：「谷哥浏览器」含类别词"浏览器"，但用户点名 Chrome，必须先认实体。

`isOfficialSpelling(hit)` 用 `matchedAs` 是否等于正式名判断，**不能靠 `exact`**（`staem` 在别名表里所以 `exact=true`）。

### 7. 能力清单（capability/mod.rs）

```rust
enum Risk { ReadOnly, Reversible, Dangerous }
enum ParamType { Str{max_len}, Int{min,max}, Bool }
struct Capability { id, desc, when, params, risk, returns }
struct Outcome { ok, data, error, ms }
```

- `when` 字段写口语化描述，是给模型判断相关性的关键
- `execute()` 顺序：**先查 id 是否存在**（报「未知能力：X」比「缺参数」有用）→ 再校验参数 → 派发
- 参数含 `..` 报「含非法路径片段」

实测耗时：`hw.cpu` 912ms / `hw.mem` 752ms / `hw.gpu` 728ms / `hw.disk` 732ms / `proc.top` 365ms / `fs.dir_size`(C:\workspace) 7ms

### 8. 安装四步配方（recipe.rs）

结构：`Recipe { id, name, url, filename, size, verify_paths, verify_reg, install_args, note }`
（只 derive `Serialize`，因为 `&'static [&'static str]` 无法 `Deserialize`）

| 配方 | 下载地址 | install_args |
|---|---|---|
| steam | `https://media.st.dl.eccdnx.com/client/installer/SteamSetup.exe` (2_380_800B) | `None`（无静默参数，让用户自己选盘） |
| 7zip | `https://www.7-zip.org/a/7z2501-x64.exe` (1_600_000B) | `Some(["/S"])` |

四步：`step1_download`（系统 curl.exe，非 https 拒绝）→ `step2_verify`（大小比对 + `Get-AuthenticodeSignature` 数字签名）→ `step3_install`（`Risk::Dangerous`）→ `step4_verify_install`

### 9. 安装位置三级探测（detect.rs）

用户明确要求："不排除有人把 steam 放进 abc 文件夹"

1. **注册表** <10ms：`HKCU\Software\Valve\Steam` 的 `SteamPath`、`HKLM\SOFTWARE\WOW6432Node\Valve\Steam` 的 `InstallPath`
2. **常见路径** <10ms：C/D/E/F × Steam/STEAM/steam/Program Files(x86)\Steam/Program Files/Steam/Games/游戏/Programs
3. **全盘扫描** ~8s：`Get-ChildItem -Recurse -Depth 4`，跳 Windows/$Recycle.Bin/System Volume Information/ProgramData/node_modules/.git/AppData

本机实测：`{installed:true, exe:"d:\\steam\\steam.exe", exeBytes:5830808, foundBy:"注册表", complete:true, launchable:true}`

本机 7-Zip 实际装在 `C:\Program Files\7-Zip-Zstandard`（非标准路径）—— 验证了分级探测的价值。

### 10. 四步编排状态机（flow.rs）

```rust
enum Stage {
    Checking, AlreadyInstalled, NeedDownload, Downloaded,
    NeedConsent, AwaitingUser, Done, Failed
}
```

- `start()` **永远先查已装**，装了直接告知位置、不给下载按钮
- `NeedDownload` 必须让用户点头（下载是耗时动作）
- 每句 `say` 都是可直接显示的人话，无技术黑话

### 11. 社交与常识层（social.js）

- 寒暄 / 感谢 / 道别 / 身份 / 夸奖 / 抱怨：多套变体轮换（`pick()` + `_tick`）
- **骂人识别 40+ 词**，`isInsult()` 两条件：短句（≤12）含骂词即判定；长句需同时含指向词（你|这|那|软件|助手|程序|东西|玩意）
- **防误判**：`NOT_INSULT_CONTEXT` 排除「垃圾文件 / 清理垃圾 / 垃圾箱 / 回收站 / 垃圾邮件 / 这个文件」
- 天气：「我这儿不联网，看不到实时天气」+ 引导到 Win 键搜"天气" / `weather.com.cn`
- 时间：用本地 `new Date()`

实测：骂人 14/14 识别，误判防护 4/4，技术问题 4/4 正确放行。

### 12. 节奏控制（delay.js）

用户原话（m01355）："不要秒回复，即使你能做到，也不要秒回，要假装卡个几秒"

```js
const MIN_DELAY = { greet:520, emotion:900, tech:620, boundary:700, install:0, default:560 }
```

**只补差，不叠加**：真实耗时 80ms 补到 620ms；真实耗时 2.4s 一秒都不加。

接情绪给 900ms 是刻意的 —— 挨骂后立刻接话显得敷衍。

等待时显示三点跳动的「正在输入」占位（`.typing` + `typingBounce 1.1s`）。

### 13. UI

共 6 个页面：`home` / `tier` / `tutorial` / `chat` / `spec` / `settings`（左下角⚙️进入）

- 主题：`[data-theme]` 双套 CSS 变量，**默认浅色**，浅色底是纯白 `#ffffff`（用户明确要求"不是灰白"）
- 档位页五列横排，进入时侧边栏淡出（`ZEN_PAGES`）
- 设置页：版本号 / 运行环境 / 上次检测时间

**已修 bug，别再引入**：

- `[hidden]` 被 `display:flex` 覆盖 → `base.css` 加 `[hidden] { display: none !important; }`
- `.page.active` 用 block 导致首页内容重叠 → 统一 `flex-direction: column`
- `.hero` 用 `margin: 11vh auto 0` 在 flex 里会跑出屏幕 → 改 `margin: auto`
- `capabilities/default.json` 缺失会导致 invoke 静默挂起（转圈不报错，最难查）

---

## 核心技术判断（已验证，别推翻）

### 判断一：小模型的幻觉不是"不听话"

幻觉来自解码时**必须在不知道的地方分配概率**。所以有效手段是**限制输出空间**，不是换更大的模型。

四个降幻觉手段，按效果排序：

1. **生成换分类** —— 模型从"答题者"变"分诊台"
2. **GBNF 语法约束解码** —— 物理上无法输出枚举外内容
3. **温度 0 + 固定种子**
4. **输出后置数字白名单校验**

意图枚举必须包含 `dont_know` 作为体面退路。

### 判断二：模型不能做判断，只能做翻译

实测对比（`bench_verdict.py`）：

```
naive（给数字让模型判断）：
  「内存有15.6GB，空闲1.7GB，足够日常使用」     ← 11% 空闲说成够用
  「能玩黑神话」→「全够用」还扯 C 盘

verdict（代码算结论 + 模型只措辞）：
  「内存不够用了，空闲只有1.7GB，总共15.6GB，占比11%。低于15%就会卡。」
```

**数字全对、判断全错** → 必须走 `数字 → 代码算结论 → 模型 → 人话`。

### 判断三：意图路由用 richer prompt 有效

47 条真实口吻用例：

| 方式 | 准确率 | 弃答 |
|---|---|---|
| bare（只列意图名） | 80.9% (38/47) | 10/10 |
| **rich（定义+6条判断规则+18条few-shot）** | **95.7% (45/47)** | 10/10 |

rich 有效的原因：模型**只做关键词模式匹配，不需理解语义**。

剩下 2 个错例：
- 「这个文件夹挺大的能不能删」→ `dont_know`（弃答规则执行过严）
- 「staem是什么 我搜出来好多」→ `what_is_term`（不认识错字）

### 判断四：采样参数不是瓶颈

实测五组参数（temp 0.8 / 0.2 / 0.0 / 重复惩罚 1.08 / min_p 0.05），输出**几乎逐字相同**。

最优组合：`temperature 0.2 / top_k 20 / top_p 0.9 / seed 42`

---

## 本地模型环境（已备好，未接入）

```
D:\workspace\local-ai-helper\.runtime\
├─ vulkan\          llama.cpp b12606 vulkan 版（38MB，一套通吃 NVIDIA/AMD/Intel）
└─ Qwen3-8B-Q4_K_M.gguf   4.68GB
```

**关键参数**：

- 必须 `--chat-template-kwargs '{"enable_thinking":false}'`，否则 Qwen3 思考链吃光 token
- **必须用 llama-server 常驻**（`HTTP 127.0.0.1:18090`，`/v1/chat/completions`）。`llama-cli` 每问冷启动 5-9s
- 模型从 **ModelScope** 下载（7.28MB/s，659s 下完 4.68GB）。`hf-mirror` 实测仅 0.04MB/s，已彻底排除

---

## 五档规则

```js
DRIVER_RESERVE = 0.98 * GB
```

| 档 | vramNeed | 模型 | 大小 |
|---|---|---|---|
| minimal | 0 | 不跑模型，走浏览器引导 | — |
| low | 2.6GB | Qwen2.5-3B | 1.9GB |
| mid | 6.6GB | Qwen3-8B | 5.5GB |
| high | 11.2GB | Qwen3-14B | 9.0GB |
| ultra | 23.0GB | Qwen3-32B | 19.8GB |

RAM < 8GB → 直接 minimal。

自测 5/5：4GB内存老本→极低 / 核显+16GB→低 / 8GB显存+32GB→中 / 12GB显存→高 / 24GB显存→极高

`minimal` 档引导文案：Win 键 → 输 Edge 回车 → 地址栏输 `chat.deepseek.com`

---

## 硬件阈值（硬编码，模型不得改）

```
内存空闲 < 15%      → 会卡
C 盘剩余 < 10%      → 会拖慢
磁盘响应时间 > 50ms  → 硬盘老化
磁盘活动时间 100%    → 瓶颈在硬盘，不在 CPU/内存
CPU 频率 < 基频 50%  → 降频
```

这些阈值来自真实求助帖调研（V2EX、Reddit r/sysadmin），不是拍脑袋。

**调研发现的核心痛点**：用户会把"占用低"当成"没问题"。真正的答案（CPU 降频 / 磁盘响应 500ms）散落在几十条回复里，需要用户自己挖两天。产品的价值就是**在 585ms 内直接给出那个数字**。

---

## 待办（按建议优先级）

1. **本地模型接入** —— 当前 `flow` 完全不调模型，只有确定性代码。用户最关心这块
2. **release 构建 + NSIS 打包** —— `tauri.conf.json` 里 `targets: ["nsis"]` 已配好，没执行过
3. **往 RECIPES 加更多软件** —— 现在只有 steam / 7zip
4. **启动验证** —— 用户 m01212 提过："steam 刚安装完不得先进登录界面啊，只检测那一个进程不就得了"
5. **词表扩充** —— 官网白名单 15 条，可继续加

---

## 用户明确表达过的产品态度（别软化）

> "我要的是支持更多的用户，而不是真的伺候那些连极小模型都跑 5token/s 的渣机"（m00037）

> "用户因为你不配当场卸载，本来对我这种为爱发电，随缘赞助的一点关系都没有，爱滚就滚"（m00039）

> "哪怕只有五个用户用过，也会由衷的说这真解决我的卡顿问题了，解决我找不到 steam 官网的问题了，所以怎么让几 B 的小模型对于很多奇葩问题有更高的准确率，更低的幻觉率才是最重要的"（m00094）

> "低档可以让它仅负责问题怎么回答，但是什么都不实操，agent 来 cosLLM"（m00157）

**所以**：

- 不写防御性输出（"核心功能免费""模型下载不用翻墙""只读没上传"这类全去掉）
- 不做订阅、不做会员等级、不做自动续费
- 赞助只解锁外观 / 徽章 / 挂名
- 不承诺"能解决"，只承诺"我陪你试"

---

## 协作注意事项

- **截图前先告诉用户一声**（用户 m01272 明确要求："我得提前知道不耽误工作"）
- 用户偏好直接、不绕弯的表达；讨厌防御性文案和安全说教
- 技术讨论要有实测数据支撑，不要"我觉得"
- 前端改动后要真的跑起来看效果，不要只说"应该没问题"
