// ============================================================
// trust.js —— 域名可信度评分
//
// 用途：白名单里没有的软件，靠启发式判断"能不能给链接"。
//
// 设计纪律：
//   1. 输出三档，不是「是/否」。宁可说不知道，也不给不确定的链接。
//   2. 每条扣分/加分都记进 signals，答应用户"为什么"。
//   3. 只做保守判断：拿不准就降档，绝不升档。
//   4. 这是纯函数，不联网，可离线跑，可单测。
// ============================================================

export const TRUST = {
  CONFIDENT: 'confident',   // 白名单命中，确定是官方
  LIKELY: 'likely',         // 启发式高分，但没核实
  UNKNOWN: 'unknown',       // 判不了，不给链接
};

// ── 顶级域分级 ────────────────────────────────
// 机构专属：注册需实名审核，假冒成本极高
const TLD_INSTITUTIONAL = new Set(['gov', 'edu', 'mil', 'go.jp', 'ac.uk']);

// 主流：受监管、注册有门槛
const TLD_MAINSTREAM = new Set([
  'com', 'org', 'net', 'io', 'dev', 'app', 'cn', 'co',
  'jp', 'de', 'fr', 'uk', 'kr', 'me', 'tv', 'ai', 'cc',
]);

// 低成本滥用：注册几块钱，仿冒站扎堆
const TLD_ABUSED = new Set([
  'top', 'xyz', 'buzz', 'click', 'download', 'site', 'online',
  'store', 'shop', 'vip', 'work', 'live', 'fun', 'icu', 'cyou',
  'rest', 'biz', 'info', 'link', 'host', 'press', 'space',
  'website', 'tech', 'life', 'world', 'today', 'email',
  'company', 'group', 'network', 'center', 'sale', 'deal', 'bid',
]);

// ── 域名里出现这些词，多半是下载站/仿冒站 ──────
//
// 必须区分两种匹配方式，否则会误伤合法域名：
//   bandisoft.com  里 soft 属于公司名，不该扣分
//   pcsoft-download.com 里 soft 是独立片段，该扣分
//
// 所以：
//   SUSPECT_SEGMENT  —— 只按连字符/点分词后逐段比对
//   SUSPECT_SUBSTR   —— 拼音/中文拼写，正常英文域名不会出现，可直接子串匹配

const SUSPECT_SEGMENT = [
  'download', 'downloads', 'free', 'green', 'portable', 'setup',
  'install', 'installer', 'crack', 'keygen', 'pcsoft',
];

const SUSPECT_SUBSTR = [
  'xiazai', 'guanwang', 'zhengban', 'zhongwen', 'guochan',
  'mianfei', 'pojie',
];

// ── 结尾拼年份，几乎都是 SEO 站 ────────────────
const YEAR_SUFFIX = /\d{4}$/;

/**
 * 分析域名可信度。
 *
 * @param {string} domain  纯域名，如 "store.steampowered.com"
 * @param {string[]} brandTokens  品牌词候选，如 ["steam", "蒸汽平台"]
 * @returns {{level: string, score: number, signals: Array<[number,string]>}}
 */
export function analyzeDomain(domain, brandTokens = []) {
  const signals = [];
  let score = 50;                       // 基线：中性

  const host = String(domain || '').toLowerCase().trim();
  if (!host) {
    return { level: TRUST.UNKNOWN, score: 0, signals: [[-50, '域名为空']] };
  }

  const parts = host.split('.');
  const tld = parts[parts.length - 1];
  // 二级域：取最后两段的前一段，处理 co.uk 这类
  const sld = parts.length >= 2 ? parts[parts.length - 2] : host;

  // ── 1. 顶级域 ────────────────────────────────
  if (TLD_INSTITUTIONAL.has(tld)) {
    score += 22;
    signals.push([22, `.${tld} 只有机构能注册`]);
  } else if (TLD_ABUSED.has(tld)) {
    score -= 26;
    signals.push([-26, `.${tld} 注册成本低，仿冒站扎堆`]);
  } else if (TLD_MAINSTREAM.has(tld)) {
    score += 8;
    signals.push([8, `.${tld} 是主流顶级域`]);
  }
  // 其他 tld 不加不减

  // ── 2. 品牌词 ────────────────────────────────
  // 关键：在完整 host 里找，不限于二级域。
  //   weixin.qq.com   品牌词在三级域，但这是官方的典型结构
  //   qq.com          二级域只有 2 字符，也完全合法
  // 所以不能用「二级域必须含品牌词」这种硬规则。
  const tokens = (brandTokens || [])
    .map((t) => String(t).toLowerCase().replace(/[^a-z0-9]/g, '').trim())
    .filter((t) => t.length >= 2);

  const hostFlat = host.replace(/[^a-z0-9]/g, '');

  let brandHit = false;
  let hitToken = '';
  for (const t of tokens) {
    if (hostFlat.includes(t)) { brandHit = true; hitToken = t; break; }
    // 允许前缀近似：bandizip ↔ bandisoft 共享 bandi
    const n = Math.min(6, t.length, hostFlat.length);
    if (n >= 4 && hostFlat.slice(0, n) === t.slice(0, n)) {
      brandHit = true; hitToken = t; break;
    }
  }

  if (brandHit) {
    score += 12;
    signals.push([12, `域名含品牌词「${hitToken}」`]);
  } else if (tokens.length) {
    score -= 6;
    signals.push([-6, '域名里找不到品牌词']);
  }

  // ── 3. 可疑词 ────────────────────────────────
  // 按片段匹配，避免 bandisoft 里的 soft 被误伤；
  // 拼音类走子串匹配，正常英文域名不会出现。
  const segs = sld.split('-');
  const hitSeg = SUSPECT_SEGMENT.filter((s) => segs.includes(s));
  const hitSub = SUSPECT_SUBSTR.filter((s) => sld.includes(s));
  const hitSuspect = [...hitSeg, ...hitSub];

  if (hitSuspect.length) {
    score -= 24;
    signals.push([-24, `域名含可疑词：${hitSuspect.join('、')}`]);
  }
  if (YEAR_SUFFIX.test(sld)) {
    score -= 18;
    signals.push([-18, '域名以年份结尾，典型 SEO 站']);
  }

  // ── 4. 连字符 ────────────────────────────────
  const hyphens = (sld.match(/-/g) || []).length;
  if (hyphens >= 3) {
    score -= 16;
    signals.push([-16, `域名有 ${hyphens} 个连字符`]);
  } else if (hyphens === 2) {
    score -= 8;
    signals.push([-8, '域名有 2 个连字符']);
  }

  // ── 5. 纯数字/无意义二级域 ───────────────────
  if (/^\d+$/.test(sld)) {
    score -= 14;
    signals.push([-14, '二级域是纯数字']);
  }

  // ── 6. 二级域过短（1 字符），多为跳转域名 ──
  // 注意：2 字符也可能合法（qq.com、7-zip 的 7z）。
  // 只有 1 个字符才反常。
  if (sld.length <= 1) {
    score -= 10;
    signals.push([-10, '二级域只有 1 个字符']);
  }

  // 收敛到 0~100
  score = Math.max(0, Math.min(100, score));

  // ── 定档 ─────────────────────────────────────
  // 阈值偏保守：宁可降到 UNKNOWN 让用户自己判断
  let level;
  if (score >= 68) level = TRUST.LIKELY;
  else level = TRUST.UNKNOWN;

  return { level, score, signals };
}

/**
 * 结合白名单与启发式的完整判定。
 *
 * @param {string} domain
 * @param {string[]} brandTokens
 * @param {object|null} whitelistHit  白名单条目，命中则直接 CONFIDENT
 */
export function judgeDomain(domain, brandTokens = [], whitelistHit = null) {
  if (whitelistHit) {
    return {
      level: TRUST.CONFIDENT,
      score: 100,
      signals: [[100, '在官方白名单里，已实测验证']],
      entry: whitelistHit,
    };
  }
  return analyzeDomain(domain, brandTokens);
}

// ============================================================
// 兜底层文案 —— 判不了的时候，教用户自己认
//
// 这里不写"我无法确认"这种推卸话，而是给出可执行的动作。
// 用户拿到的是能力，不是道歉。
// ============================================================

export const FALLBACK = {
  // 不在白名单、启发式也判不了时，给这套方法来代替链接
  teachUser: [
    '看地址栏，不看页面长什么样。',
    '域名就是公司名或产品名的那一截。中间多出 download、zh、cn 这类词的，基本不是。',
    '搜索结果里带「广告」标记的那个不要点。',
    '下载页只有一个下载按钮。出现「高速下载」「普通下载」两个的，都带捆绑。',
  ],

  // 直接能用的自查动作
  selfCheck: '把地址栏那一段复制下来，发给我，我帮你看看。',

  // 不同类型软件的特殊提醒
  byCategory: {
    compressor: '解压软件只认 7-Zip 和 Bandizip 两家。别信搜索结果里的「中文版」。',
    driver: '驱动只从显卡厂商官网下。任何「驱动管家」「驱动精灵」都别用。',
    runtime: '运行库只从微软官方下。网上流传的「合集包」来源不明。',
    game: '游戏客户端只从平台官网下。带「管家」「加速器」字样的都不是。',
  },
};

// ============================================================
// 自测：node --experimental-strip-types src/lib/trust.js
// ============================================================

if (typeof process !== 'undefined' && process.argv?.[1]?.endsWith('trust.js')) {
  // 分两类断言：
  //   heuristic  —— 靠规则应该判对的
  //   whitelist  —— 规则必然判不了、只能靠白名单兜的
  // 第二类不是失败，是设计边界，必须显式记录。
  const heuristic = [
    ['store.steampowered.com', ['steam'], TRUST.LIKELY],
    ['www.7-zip.org', ['7zip', '7-zip'], TRUST.LIKELY],
    ['cn.bandisoft.com', ['bandizip', 'bandi'], TRUST.LIKELY],
    ['www.voidtools.com', ['voidtools'], TRUST.LIKELY],
    ['im.qq.com', ['qq'], TRUST.LIKELY],
    ['weixin.qq.com', ['weixin'], TRUST.LIKELY],
    ['www.microsoft.com', ['microsoft'], TRUST.LIKELY],

    // 仿冒或下载站，一律降档
    ['steam-download.com', ['steam'], TRUST.UNKNOWN],
    ['steamxiazai.top', ['steam'], TRUST.UNKNOWN],
    ['chrome-cn-2024.com', ['chrome'], TRUST.UNKNOWN],
    ['7zip.icu', ['7zip'], TRUST.UNKNOWN],
    ['bandizip-free.xyz', ['bandizip'], TRUST.UNKNOWN],
    ['ps-zhongwen-mianfei.com', ['photoshop'], TRUST.UNKNOWN],

    // 品牌词完全不匹配，规则判不了，应降档而非硬猜
    ['www.voidtools.com', ['everything'], TRUST.UNKNOWN],
  ];

  // 这些域名靠规则必然判不出来，必须走白名单。
  // 放在这里是为了留下证据：说明白名单不是可选项。
  const whitelistOnly = [
    ['code.visualstudio.com', ['vscode'], '产品名 Visual Studio Code，域名却是缩写'],
    ['www.google.cn', ['chrome'], 'Chrome 在 google.cn 下，无 chrome 字样'],
    ['learn.microsoft.com', ['vcredist'], '运行库页面挂在 learn 子域'],
    ['steampp.net', ['watt toolkit'], '域名 steampp 与产品名 Watt Toolkit 无字面关系'],
  ];

  let pass = 0;
  let total = 0;

  console.log('══ 启发式可判 ══\n');
  for (const [domain, brand, expect] of heuristic) {
    total++;
    const r = analyzeDomain(domain, brand);
    const ok = r.level === expect;
    if (ok) pass++;
    console.log(
      `${ok ? 'PASS' : 'FAIL'}  ${domain.padEnd(26)} → ${r.level.padEnd(9)} ${String(r.score).padStart(3)}分`
    );
    for (const [d, why] of r.signals) {
      console.log(`        ${d > 0 ? '+' : ''}${d}  ${why}`);
    }
  }

  console.log('\n══ 规则判不了，必须靠白名单 ══\n');
  for (const [domain, brand, why] of whitelistOnly) {
    const r = analyzeDomain(domain, brand);
    // 这里不断言"必须失败"，只记录现状，证明白名单的必要性
    console.log(
      `  ${domain.padEnd(26)} 规则判定 ${r.level.padEnd(9)} ${String(r.score).padStart(3)}分`
    );
    console.log(`        原因：${why}`);
  }

  console.log(`\n${pass}/${total} 通过`);
  if (pass !== total) process.exitCode = 1;
}
