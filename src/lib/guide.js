// ============================================================
// guide.js —— 确定性反诈红线与电脑扫盲急救库
//
// 设计原则：
//   1. 反诈红线（scamGuard）优先级高于一切问答与官网查询，
//      避免在对方正被诱导装远程软件时还把下载地址递过去。
//   2. 话术克制、点到为止：不写「拔网线、长按电源强行关机」等
//      易引发数据损坏或硬件纠纷的激烈物理动作。
//   3. 常见新手误区与 5 大急救剧本全部确定性离线可用，
//      不开模型也能毫秒级给出可执行步骤。
// ============================================================

import { lintTone } from './typo.js';

// ── 供 app.js 实测诊断结果末尾附带的简洁操作提示 ──
export const C_DRIVE_TIPS = [
  '常见清理办法（别下第三方「一键清 C 盘」）：',
  '· 先查微信 / QQ / 钉钉：在软件「设置 → 文件管理」里把聊天文件改存到 D 盘',
  '· 打开系统「设置 → 系统 → 存储 → 存储感知」，安全清临时文件和回收站',
].join('\n');

export const NET_OFFLINE_TIPS = [
  '断网先排查这三步（不用急着重装系统）：',
  '· 路由器断电等 10 秒再插回，同时看笔记本有没有误按 Fn 快捷键关掉 Wi-Fi',
  '· 管理员运行命令行输入 netsh winsock reset 后重启电脑',
  '· 如果是刚装系统缺网卡驱动，可用手机插数据线开「USB 网络共享」临时联网下载官方驱动',
].join('\n');

// ============================================================
// 一、反诈红线（最高优先级）
// ============================================================

const SCAM_ACTOR_RE =
  /(客服|百万保障|百万医疗|直播扣费|扣费|取消会员|会员到期|关闭免密|免密支付|银联|征信|公检法|民警|退款客服|理赔|陌生人让我|对方让我)/;

const REMOTE_SIGNAL_RE =
  /(向日葵|todesk|to\s*desk|腾讯会议|屏幕共享|共享屏幕|远程控制|远程协助|识别码|连接码|临时密码|验证码|下载.*软件|装个.*软件)/;

const MILLION_GUARD_RE =
  /(百万保障|百万医疗|直播会员|免密扣费).{0,16}(取消|关闭|扣费|到期|收费|退订|怎么关)/;

const CHEAP_REPAIR_RE =
  /(淘宝|闲鱼|拼多多|网上找人|低价|几块钱|9\.9|一元|1元).{0,14}(远程修|远程装|修电脑|装系统|重装系统|远程清理)/;

const ACTIVE_HIJACK_RE =
  /(鼠标自己动|光标自己动|屏幕自己动|别人控制.*电脑|电脑被.*控制|远程.*黑屏|黑屏.*关不掉|退不出.*远程)/;

/**
 * 反诈与远控风险前置拦截。
 * 命中即返回确定性红牌提醒；普通查官网（如「向日葵官网在哪」）不触发，放行给官网白名单。
 *
 * @param {string} q
 * @returns {{text: string, tag: string, kind: string}|null}
 */
export function scamGuard(q) {
  const t = String(q || '').toLowerCase().trim();
  if (!t) return null;

  if (ACTIVE_HIJACK_RE.test(t)) {
    return {
      kind: 'active_remote_control',
      tag: '安全提醒 · 疑似远控',
      text: [
        '先停下来，不要输入任何银行卡密码、支付密码或短信验证码。',
        '',
        '建议立刻按这三步处理：',
        '1. 直接关闭正在运行的远程软件或会议窗口（如向日葵、ToDesk、腾讯会议）；如果窗口关不掉，按 Ctrl + Shift + Esc 打开任务管理器结束对应程序。',
        '2. 已经和陌生人沟通过资金、验证码或人脸识别的，立刻挂断电话，用手机联系家人或拨打 96110 核实。',
        '3. 确认安全前不要再打开对方发来的链接或安装包。',
      ].join('\n'),
    };
  }

  if ((SCAM_ACTOR_RE.test(t) && REMOTE_SIGNAL_RE.test(t)) || MILLION_GUARD_RE.test(t)) {
    return {
      kind: 'fake_support_remote',
      tag: '安全提醒 · 假客服套路',
      text: [
        '这是高发诈骗套路，先别装软件，也别继续操作。',
        '',
        '1. 微信、抖音、支付宝、银行等正规平台客服，都不会私下要求安装向日葵、ToDesk 或腾讯会议，也不会索要设备识别码、验证码或要求共享屏幕。',
        '2. 所谓「百万保障到期扣费」「误开直播会员」都是常见话术；还没装的直接停手，已经打开远程或会议窗口的直接关闭软件。',
        '3. 拿不准就直接挂断电话问家人，或拨打反诈专线 96110 核实。',
      ].join('\n'),
    };
  }

  if (CHEAP_REPAIR_RE.test(t)) {
    return {
      kind: 'cheap_remote_repair',
      tag: '安全提醒 · 低价远修风险',
      text: [
        '不建议在电商平台找几块钱的陌生人远程修电脑或装系统。',
        '',
        '· 常见套路是低价引流，连上后编造硬盘坏道或严重病毒不断加价',
        '· 远程装机常被塞入盗版激活工具或推广全家桶，甚至存在隐私与账号泄露风险',
        '· 日常卡顿、清空间、重装系统都可以先自己用系统自带功能排查；真需要维修优先找品牌官方售后或熟悉的人',
      ].join('\n'),
    };
  }

  return null;
}

// ============================================================
// 二、高频痛点误区与电脑扫盲急救剧本
// ============================================================

const GUIDE_RULES = [
  {
    kind: 'antivirus_conflict',
    tag: '扫盲 · 杀毒与防打架',
    re: /(360|电脑管家|金山毒霸|毒霸|迈克菲|mcafee|windows\s*defender|杀毒|防毒|安不安全.*管家|装哪个杀毒|要装杀毒|杀毒软件.*打架)/i,
    // 纯问「火绒官网在哪」前面已由 pipeline 接走，这里只处理选型/打架/需不需要装杀毒
    exclude: /(官网|下载|在哪下|哪里下|地址)/,
    text: [
      'Windows 10 / 11 自带的 Windows Defender 平时防毒就够用了，不需要再叠装多个杀毒软件。',
      '',
      '· 同时装 360、腾讯电脑管家、金山毒霸会互相抢资源，反而把电脑拖得很卡',
      '· 如果只是想拦右下角弹窗广告，可以只留系统自带防毒，单配一个「火绒安全」（huorong.cn）',
      '· 已经装了多个管家的，在「设置 → 应用 → 已安装的应用」里只保留一个或全部卸载干净',
    ].join('\n'),
  },
  {
    kind: 'popup_adware',
    tag: '扫盲 · 弹窗与流氓软件',
    re: /(弹窗|弹广告|右下角.*广告|流氓软件|全家桶|卸载不掉|卸不掉|删不掉.*软件|高速下载器|内存占90|高危漏洞.*弹窗)/i,
    text: [
      '碰到「内存占 90%」「发现高危漏洞」这类弹窗别点「一键清理 / 立即加速」，它本身就是捆绑入口。',
      '',
      '清理顽固弹窗和流氓软件可以按这三步：',
      '1. 按 Ctrl + Shift + Esc 打开任务管理器 →「启动应用」，把不认识或不需要的第三方自启项设为禁用。',
      '2. 按 Win + R 输入 mrt 回车，运行微软自带的「恶意软件删除工具」快速扫描。',
      '3. 常规卸载不掉的软件，可用 Geek Uninstaller（geekuninstaller.com）彻底卸载并清残留；平时装软件避开几百 KB 的「高速下载器」，安装时一律点「自定义安装」取消附加勾选。',
    ].join('\n'),
  },
  {
    kind: 'elder_guard',
    tag: '扫盲 · 长辈防误装',
    re: /(长辈.*电脑|老人.*电脑|爸妈.*电脑|防误装|防止乱点|标准用户)/i,
    text: [
      '给长辈配电脑，最省心的防误装办法是从权限上隔离：',
      '',
      '1. 打开「设置 → 账户 → 其他用户」，新建一个日常登录账号，账户类型设为「标准用户」（非管理员），自己保留管理员密码。这样点到捆绑安装包会被系统拦住。',
      '2. 常用软件优先从系统自带的 Microsoft Store（微软应用商店）安装，避免搜索引擎点进假官网。',
      '3. 保留系统自带 Windows Defender，需要清静可单配火绒开弹窗拦截。',
    ].join('\n'),
  },
  {
    kind: 'blue_screen',
    tag: '急救 · 蓝屏排查',
    re: /(蓝屏|bsod|停机代码|蓝屏代码|死机蓝屏|重启.*蓝屏)/i,
    text: [
      '突然蓝屏先别慌，偶发一次大概率只是驱动或软件临时冲突，不代表硬件坏了。',
      '',
      '按顺序排查：',
      '1. 第一次蓝屏先正常重启观察；如果最近刚装了新软件、外设或更新了驱动，先卸载或回退。',
      '2. 如果反复蓝屏，用手机拍下屏幕底部的英文「终止代码」（例如 MEMORY_MANAGEMENT、IRQL_NOT_LESS_OR_EQUAL），再发给我一起看。',
      '3. 可以用管理员身份打开命令提示符（CMD），输入 sfc /scannow 回车，让系统自动检查并修复受损的系统文件。',
    ].join('\n'),
  },
  {
    kind: 'no_sound',
    tag: '急救 · 没声音排查',
    re: /(没声音|没有声音|听不见声音|静音.*没声|声卡驱动|耳机没声|外放没声|放不出声音|扬声器没声)/i,
    text: [
      '电脑没声音先别急着搜「万能声卡驱动」，多数是输出口或服务设置问题。',
      '',
      '按顺序查这三项：',
      '1. 点任务栏右下角小喇叭，确认没静音，再点音量条右侧箭头，核对输出设备是不是选错了（例如切到了显示器 HDMI）。台式机有线耳机优先插机箱背后的绿色圆孔。',
      '2. 右键小喇叭 →「声音问题疑难解答」，按系统提示跑一遍自动修复。',
      '3. 按 Win + R 输入 services.msc 回车，找到「Windows Audio」，确认状态是「正在运行」，不是就右键点「启动」。',
    ].join('\n'),
  },
  {
    kind: 'driver_and_reinstall',
    tag: '扫盲 · 驱动与重装系统',
    re: /(驱动精灵|驱动人生|万能驱动|重装系统|重置此电脑|重置电脑|怎么装系统|做系统盘|u盘装系统)/i,
    text: [
      '装驱动和重装系统尽量走系统自带或硬件官方渠道，别用第三方驱动管家：',
      '',
      '· 驱动：显卡去 NVIDIA（nvidia.cn）或 AMD（amd.com）官网下；主板/声卡/网卡按电脑底壳或机身型号去品牌官网下，不用「驱动精灵 / 驱动人生」。',
      '· 重装系统：系统还能进桌面时，优先用自带「设置 → 系统 → 恢复 → 重置此电脑」（提前备份桌面和 C 盘重要资料）。',
      '· 做纯净 U 盘系统盘：只用微软官方的 MediaCreationTool，不要用带捆绑的一键装机工具或来路不明的 GHO 镜像。',
    ].join('\n'),
  },
  {
    kind: 'c_drive_guide',
    tag: '急救 · C 盘清理指引',
    re: /(微信.*占.*c\s*盘|聊天记录.*迁|c\s*盘.*怎么清|怎么清理\s*c\s*盘|清理\s*c\s*盘|一键清\s*c\s*盘)/i,
    text: [
      'C 盘爆满别下第三方「一键清 C 盘」，容易把系统或软件配置清坏。',
      '',
      '最安全有效的是这三步：',
      '1. 搬走聊天记录（头号元凶）：打开微信「设置 → 文件管理 → 更改」，把文件默认保存位置迁到 D 盘；QQ 和钉钉同理。',
      '2. 开系统自带清理：打开「设置 → 系统 → 存储 → 存储感知」，清理临时文件、旧 Windows 更新缓存和回收站。',
      '3. 也可以直接对我说「C 盘什么最占空间」，我帮你扫一遍非系统大文件夹。',
    ].join('\n'),
  },
];

/**
 * 确定性电脑扫盲与急救知识回答。
 *
 * @param {string} q
 * @returns {{text: string, tag: string, kind: string}|null}
 */
export function guideAnswer(q) {
  const t = String(q || '').trim();
  if (!t) return null;

  for (const rule of GUIDE_RULES) {
    if (!rule.re.test(t)) continue;
    if (rule.exclude && rule.exclude.test(t)) continue;
    return { kind: rule.kind, tag: rule.tag, text: rule.text };
  }
  return null;
}

// ============================================================
// 自测：node src/lib/guide.js
// ============================================================

if (typeof process !== 'undefined' && process.argv?.[1]?.endsWith('guide.js')) {
  const scamCases = [
    ['客服让我下载向日葵关闭百万保障', 'fake_support_remote'],
    ['抖音客服说直播扣费让我开腾讯会议共享屏幕', 'fake_support_remote'],
    ['微信百万保障到期扣费怎么关闭', 'fake_support_remote'],
    ['淘宝9.9远程修电脑靠谱吗', 'cheap_remote_repair'],
    ['闲鱼找人远程装系统安全吗', 'cheap_remote_repair'],
    ['我电脑鼠标自己动好像被别人控制了', 'active_remote_control'],
    // 正常查官网不应被反诈误拦截（应放行给官网白名单并附带反诈提示）
    ['向日葵官网在哪', null],
    ['腾讯会议怎么下载', null],
    ['todesk下载地址', null],
  ];

  const guideCases = [
    ['需要装360还是腾讯电脑管家', 'antivirus_conflict'],
    ['电脑右下角老弹广告怎么关', 'popup_adware'],
    ['有个流氓软件卸载不掉怎么办', 'popup_adware'],
    ['给老人配电脑怎么防误装软件', 'elder_guard'],
    ['电脑突然蓝屏了怎么办', 'blue_screen'],
    ['电脑突然没声音了怎么修', 'no_sound'],
    ['驱动精灵好用吗', 'driver_and_reinstall'],
    ['怎么重装系统最安全', 'driver_and_reinstall'],
    ['怎么清理C盘比较安全', 'c_drive_guide'],
    // 不该被误抢的普通问题
    ['C盘还剩多少', null],
    ['steam官网在哪', null],
    ['火绒官网在哪', null],
    ['你好', null],
  ];

  let pass = 0;
  let total = 0;
  let toneFail = 0;

  console.log('══ 反诈红线测试 ══\n');
  for (const [q, expectKind] of scamCases) {
    total++;
    const r = scamGuard(q);
    const got = r ? r.kind : null;
    const toneOk = !r || lintTone(r.text).ok;
    const noExtreme = !r || !/(拔网线|拔掉网线|长按电源|强行关机)/.test(r.text);
    const ok = got === expectKind && toneOk && noExtreme;
    if (ok) pass++;
    if (!toneOk) toneFail++;
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${q.padEnd(26)} → ${String(got || '放行').padEnd(22)}`);
    if (!noExtreme) console.log('        ⚠ 含禁用激烈物理动作');
    if (!toneOk) console.log('        ⚠ 语气检查未通过');
  }

  console.log('\n══ 扫盲与急救剧本测试 ══\n');
  for (const [q, expectKind] of guideCases) {
    total++;
    const r = guideAnswer(q);
    const got = r ? r.kind : null;
    const toneOk = !r || lintTone(r.text).ok;
    const ok = got === expectKind && toneOk;
    if (ok) pass++;
    if (!toneOk) toneFail++;
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${q.padEnd(26)} → ${String(got || '放行').padEnd(22)}`);
    if (!toneOk) console.log('        ⚠ 语气检查未通过');
  }

  console.log(`\n${pass}/${total} 通过，语气失败 ${toneFail} 条`);
  if (pass !== total) process.exitCode = 1;
}
