// ============================================================
// pipeline.js —— 完整链路：用户输入 → 最终答案
//
// 这一层把三个零件串起来，所有判断都在代码里：
//   official.data.js  白名单（人工维护，实测验证）
//   trust.js          域名可信度（启发式，三档）
//   typo.js           拼写容错（编辑距离 + 中文变体）
//
// 模型的角色：本链路完全不调用模型。
// 软件地址类问题有确定答案，模型参与只会引入幻觉。
// ============================================================

import { OFFICIAL } from './official.data.js';
import { resolveSoftware, composeAnswer as composeTypoAnswer, lintTone } from './typo.js';
import { judgeDomain, analyzeDomain, TRUST, FALLBACK } from './trust.js';

// ─────────────────────────────────────────── 意图
export const INTENT = {
  OFFICIAL_SITE: 'cant_find_site',    // 找官网
  NONE: 'none',
};

// ─────────────────────────────────────────── 软件类别词
//
// 「解压软件」「播放器」这类是**类别**，不是具体软件。
// 必须和别名分开处理，否则：
//   "解压软件是什么" 会被判成"找 7-Zip"
//   "解压软件去哪下" 应该给出该类别的推荐，而不是某一个
//
// 处理策略：
//   问地址 → 给该类目下的推荐条目
//   问含义 → 交给术语库（本链路不处理）
const SOFT_CATEGORY = [
  {
    // 「程序」「工具」「推荐」都是用户会说的词，不能只收「解压软件」。
    // 少一个变体就会掉进「超出范围」，而用户其实说得很清楚。
    keys: [
      '解压软件', '解压程序', '解压工具', '解压缩', '压缩软件',
      '压缩程序', '压缩工具', '推荐个解压', '推荐解压',
    ],
    picks: ['7zip', 'bandizip'],
  },
  {
    keys: ['播放器', '视频播放', '播放软件'],
    picks: [],
    customAnswer:
      'PotPlayer 和 VLC 官网在国内常直连超时，搜索前排的「中文网」几乎全是捆绑假站。\n\n' +
      '建议直接打开系统自带的 Microsoft Store（微软应用商店）搜索安装，或先用 Windows 自带媒体播放器。',
  },
  { keys: ['浏览器'], picks: ['chrome', 'edge'] },
  {
    keys: ['pdf阅读', '看pdf', 'pdf软件', 'pdf工具'],
    picks: ['foxit', 'adobe-reader'],
    lead: '只看 PDF 直接把文件拖进自带 Edge 浏览器就行；需要专门软件再选这两家：',
  },
  {
    keys: ['杀毒软件', '弹窗拦截', '防毒软件', '拦截弹窗'],
    picks: ['huorong'],
    lead: 'Win10/11 自带 Windows Defender 平时够用，需要拦弹窗可单配火绒（别和其他管家叠装）：',
  },
  {
    keys: ['强力卸载', '卸载工具', '卸载软件', '清残留'],
    picks: ['geek-uninstaller'],
    lead: '彻底卸载顽固软件并清残留，可以用这款单文件免安装工具：',
  },
  {
    keys: ['远程控制', '远程协助', '远控软件'],
    picks: ['sunlogin', 'todesk'],
    lead: '仅限家人或熟人协助时使用，绝不把设备识别码发给陌生人或所谓客服：',
  },
  {
    keys: ['硬件检测工具', '硬件检测软件', '测温度软件'],
    picks: ['tbtool'],
    lead: '查硬件、测温度常用这款工具合集：',
  },
  { keys: ['输入法'], picks: [] },
  { keys: ['截图工具', '截图软件', '截图程序'], picks: [] },
  { keys: ['驱动'], picks: ['driver-nvidia', 'driver-amd'] },
];

// ── 国内实测直连超时的软件：不硬塞打不开的链接，直接给微软商店/系统自带替代 ──
const STORE_ALTERNATIVES = [
  {
    id: 'potplayer',
    name: 'PotPlayer',
    keys: ['potplayer', 'pot player', 'pot播放器'],
    answer:
      'PotPlayer 官网（daum.net / potplayer.tv）在国内常直连超时，搜索前排的「PotPlayer 中文网」几乎全是捆绑假站。\n\n' +
      '· 优先直接按 Win 键打开 Microsoft Store（微软应用商店）搜索安装\n' +
      '· 平时只看常见视频，Windows 自带的媒体播放器也够用',
  },
  {
    id: 'vlc',
    name: 'VLC 播放器',
    keys: ['vlc', 'vlc播放器', 'videolan'],
    answer:
      'VLC 官网（videolan.org）在国内直连容易超时，别去第三方下载站下打包版。\n\n' +
      '· 按 Win 键打开 Microsoft Store（微软应用商店），直接搜「VLC」安装官方正版',
  },
  {
    id: 'sumatrapdf',
    name: 'SumatraPDF',
    keys: ['sumatrapdf', 'sumatra pdf', 'sumatra'],
    answer:
      'SumatraPDF 官网（sumatrapdfreader.org）在国内常连不上，别下第三方搬运站版本。\n\n' +
      '· 仅阅读 PDF 不需要专门装软件，直接把 PDF 拖进系统自带的 Edge 浏览器就能看',
  },
];

function matchStoreAlternative(text) {
  const t = String(text || '').toLowerCase();
  for (const item of STORE_ALTERNATIVES) {
    if (item.keys.some((k) => t.includes(k))) return item;
  }
  return null;
}

function matchCategory(text) {
  const t = String(text || '').toLowerCase();
  for (const c of SOFT_CATEGORY) {
    if (c.keys.some((k) => t.includes(k))) return c;
  }
  return null;
}

// ─────────────────────────────────────────── 中文口语询问词
// 用户不会说"我要下载 Bandizip"，会说"解压软件去哪下"。
const ASK_ADDRESS_CUES = [
  '官网', '下载', '在哪下', '哪里下', '哪下', '去下', '下个', '求个',
  '地址', '正版', '官方', '安装包', '怎么装', '怎么下', '给个链接',
  '搜出来好多', '找不到',
];

const NOT_ADDRESS_CUES = [
  '是什么', '啥意思', '啥叫', '什么意思',
  // 「区别」类：中文口语里"有啥区别""有什么区别""啥区别"都常见，
  // 只收一个会漏。这里全部枚举，不做正则，便于逐条核对。
  '有什么区别', '有啥区别', '啥区别', '区别是什么', '差别',
  '干嘛的', '是啥', '用途', '干什么用',
];

function hasAny(text, cues) {
  return cues.some((c) => text.includes(c));
}

/**
 * 判断这次命中用的是不是"正式写法"。
 *
 * 别名表里故意收了常见错拼（staem / stema / bandzip / 7zpi 等），
 * 因为它们是最容易被搜到假站的写法，必须能认出来。
 * 但这些不是正式名，不能当作"用户在问这个词的含义"。
 */
function isOfficialSpelling(hit) {
  if (!hit || !hit.entry) return false;
  const matched = String(hit.matchedAs || '').toLowerCase();
  const official = String(hit.entry.name || '').toLowerCase();
  const officialFlat = official.replace(/[^a-z0-9]/g, '');
  const matchedFlat = matched.replace(/[^a-z0-9]/g, '');
  return matchedFlat === officialFlat;
}

/**
 * 判断这句话是不是在问软件地址。
 *
 * 顺序（这个顺序是踩过坑改出来的）：
 *   1. 先认实体。认出了具体软件，几乎一定是找它，不用再看语气词。
 *      "staem是什么" 里有个"是什么"，但用户其实是在找地址，
 *      因为 staem 本身就是仿冒重灾区，用户是搜出来一堆站才这么问的。
 *   2. 没认出实体，才看询问词区分「找地址」和「问术语」。
 *
 * 反例（改之前会判错）：
 *   "威信电脑版"       → 没有询问词，但明显在找下载
 *   "staem是什么"      → 有"是什么"，但不是在问词义
 */
export function detectIntent(text) {
  const t = String(text || '').toLowerCase();
  const wantsAddress = hasAny(t, ASK_ADDRESS_CUES);
  const askingMeaning = hasAny(t, NOT_ADDRESS_CUES);

  // ── 第零步：先认具体软件 ──────────────────
  // 顺序很关键。"谷哥浏览器" 里含"浏览器"这个类别词，
  // 但用户其实点名了 Chrome（谷哥=谷歌=Chrome）。
  // 所以必须先找具体实体，找到了就不看类别。
  const software = findSoftware(t);

  // ── 第一步：认出了具体软件 ────────────────
  if (software) {
    // 只有「问含义」且用的不是错拼，才退回术语
    const looksLikeTypo = !isOfficialSpelling(software);
    if (askingMeaning && !wantsAddress && !looksLikeTypo) {
      return { intent: 'what_is_term', software: null };
    }
    return { intent: INTENT.OFFICIAL_SITE, software, confident: true };
  }

  // ── 第一点五步：国内直连超时软件（微软商店/自带替代）──
  const storeAlt = matchStoreAlternative(t);
  if (storeAlt) {
    if (askingMeaning && !wantsAddress) {
      return { intent: 'what_is_term', software: null };
    }
    return { intent: INTENT.OFFICIAL_SITE, software: null, storeAlt, confident: true };
  }

  // ── 第二步：没认出具体软件，看是不是说了类别 ──
  // 到这一步说明用户只给了类别（"解压软件""播放器"），没点名产品。
  const category = matchCategory(t);
  if (category) {
    if (askingMeaning && !wantsAddress) {
      return { intent: 'what_is_term', software: null, category };
    }
    if (category.picks.length || category.customAnswer) {
      return { intent: INTENT.OFFICIAL_SITE, software: null, category, confident: true };
    }
    return { intent: INTENT.OFFICIAL_SITE, software: null, category, confident: false };
  }

  // ── 第三步：都没有，看语气 ────────────────
  // 只要在问含义，就是术语类，交给下游
  if (askingMeaning) {
    return { intent: 'what_is_term', software: null };
  }
  if (wantsAddress) {
    return { intent: INTENT.OFFICIAL_SITE, software: null, confident: false };
  }

  return { intent: INTENT.NONE, software: null };
}

/**
 * 从一段话里抽软件。
 *
 * 两轮：
 *   1. 别名直接扫（最长匹配优先），覆盖「我想下个bandizip」这类
 *   2. 逐词跑编辑距离，覆盖「bandzip」「staem」这类错拼
 */
export function findSoftware(text) {
  const t = String(text || '').toLowerCase();

  // ── 第一轮：别名精确扫描 ──────────────────
  let bestHit = null;
  let bestLen = 0;

  for (const entry of OFFICIAL.entries) {
    const keys = [entry.name, ...entry.aliases]
      .map((k) => String(k).toLowerCase())
      .filter((k) => k.length >= 2);

    for (const k of keys) {
      if (!t.includes(k)) continue;
      if (k.length > bestLen) {
        bestHit = { entry, matchedAs: k, exact: true, distance: 0, atRisk: false };
        bestLen = k.length;
      }
    }
  }
  if (bestHit) return bestHit;

  // ── 第二轮：逐词编辑距离 ──────────────────
  // 只在拉丁字母词上跑，中文靠第一轮的别名表
  const words = t
    .split(/[\s,，。！？、;；:：()（）"'“”]+/)
    .map((w) => w.replace(/[^a-z0-9\-]/g, ''))
    .filter((w) => w.length >= 3 && /^[a-z0-9\-]+$/.test(w));

  for (const w of words) {
    const hit = resolveSoftware(w, OFFICIAL.entries);
    if (hit) return hit;
  }

  // ── 第三步：中文错字 ──────────────────────
  // 中文的形近/音近错写靠变体表，不走编辑距离。
  // 表里的变体可能出现在任何位置，所以用 includes 扫整句。
  const hitCN = resolveSoftware(t, OFFICIAL.entries);
  if (hitCN) return hitCN;

  return null;
}

// ============================================================
// 主入口
// ============================================================

/**
 * 处理一句用户输入，返回结构化答案。
 *
 * @returns {{
 *   intent: string,
 *   answer: string,
 *   source: 'whitelist'|'fallback',
 *   toneOk: boolean,
 *   meta: object
 * }}
 */
export function handle(text) {
  const detected = detectIntent(text);

  // 不是找官网，本链路不处理。
  // 但要把意图原样往上报，交给下游（术语库 / 模型）接手。
  if (detected.intent !== INTENT.OFFICIAL_SITE) {
    return {
      intent: detected.intent,
      answer: null,
      source: 'none',
      toneOk: true,
      meta: {},
    };
  }

  const hit = detected.software;

  // ── 国内超时软件：直接给微软商店/系统自带方案 ──
  if (detected.storeAlt) {
    const answer = detected.storeAlt.answer;
    const lint = lintTone(answer);
    return {
      intent: INTENT.OFFICIAL_SITE,
      answer,
      source: 'whitelist',
      toneOk: lint.ok,
      meta: {
        id: detected.storeAlt.id,
        name: detected.storeAlt.name,
        trust: TRUST.CONFIDENT,
      },
    };
  }

  // ── 类别词：给该类目下的推荐 ──────────────
  if (detected.category && detected.category.customAnswer) {
    const answer = detected.category.customAnswer;
    const lint = lintTone(answer);
    return {
      intent: INTENT.OFFICIAL_SITE,
      answer,
      source: 'whitelist',
      toneOk: lint.ok,
      meta: {
        category: detected.category.keys[0],
        picks: [],
        trust: TRUST.CONFIDENT,
      },
    };
  }

  if (detected.category && detected.category.picks.length) {
    const picks = detected.category.picks
      .map((id) => OFFICIAL.entries.find((e) => e.id === id))
      .filter(Boolean);

    const lines = [];
    const kind = detected.category.keys[0];
    lines.push(detected.category.lead || `${kind}有两家够用，都免费：`);
    lines.push('');
    for (const e of picks) {
      lines.push(`${e.name}：${e.primary.replace(/^https?:\/\//, '')}`);
    }

    const answer = lines.join('\n');
    const lint = lintTone(answer);
    return {
      intent: INTENT.OFFICIAL_SITE,
      answer,
      source: 'whitelist',
      toneOk: lint.ok,
      meta: {
        category: detected.category.keys[0],
        picks: picks.map((e) => e.id),
        trust: TRUST.CONFIDENT,
      },
    };
  }

  // ── 白名单命中 ────────────────────────────
  if (hit) {
    const answer = composeTypoAnswer(hit);
    const lint = lintTone(answer);
    return {
      intent: INTENT.OFFICIAL_SITE,
      answer,
      source: 'whitelist',
      toneOk: lint.ok,
      meta: {
        id: hit.entry.id,
        name: hit.entry.name,
        matchedAs: hit.matchedAs,
        exact: hit.exact,
        distance: hit.distance,
        domain: hit.entry.domain,
        trust: TRUST.CONFIDENT,
      },
    };
  }

  // ── 没认出软件，走兜底 ────────────────────
  // 兜底不道歉、不推卸，直接给方法 + 邀请用户把地址发来
  const lines = [];
  lines.push('这个我这儿没有收录。');
  lines.push('');
  lines.push('搜的时候按下面几条看，能挡掉大部分假的：');
  for (const tip of FALLBACK.teachUser) lines.push('· ' + tip);
  lines.push('');
  lines.push(FALLBACK.selfCheck);

  const answer = lines.join('\n');
  const lint = lintTone(answer);

  return {
    intent: INTENT.OFFICIAL_SITE,
    answer,
    source: 'fallback',
    toneOk: lint.ok,
    meta: { trust: TRUST.UNKNOWN },
  };
}

/**
 * 校验一个用户给的域名，用于「把地址发来我帮你看看」之后。
 */
export function checkDomain(domain, softwareId = null) {
  const entry = softwareId
    ? OFFICIAL.entries.find((e) => e.id === softwareId) || null
    : null;

  const brandTokens = entry
    ? [entry.name, ...entry.aliases]
    : [];

  const r = judgeDomain(domain, brandTokens, null);

  // 如果给了 softwareId，额外做一次确定比对
  if (entry) {
    const host = String(domain || '').toLowerCase().replace(/^https?:\/\//, '').split('/')[0];
    const officialHosts = [entry.domain, ...entry.alt.map((u) => {
      try { return new URL(u).host; } catch { return ''; }
    })].filter(Boolean);

    if (officialHosts.includes(host)) {
      return { level: TRUST.CONFIDENT, score: 100, matches: true, entry,
               reason: `这就是 ${entry.name} 的官方域名` };
    }
    return {
      level: r.level, score: r.score, matches: false, entry,
      reason: `和你说的 ${entry.name} 对不上。官方域名是 ${entry.domain}`,
      signals: r.signals,
    };
  }

  return { ...r, matches: null };
}

// ============================================================
// 自测
// ============================================================

if (typeof process !== 'undefined' && process.argv?.[1]?.endsWith('pipeline.js')) {
  // 真实用户口吻，含错字、口语、不完整句子
  const CASTS = [
    // [输入, 期望 source, 期望 meta.id / null, 期望 intent]
    ['bandizip 在哪下', 'whitelist', 'bandizip', INTENT.OFFICIAL_SITE],
    ['bandzip官网', 'whitelist', 'bandizip', INTENT.OFFICIAL_SITE],
    ['我想下个7zip', 'whitelist', '7zip', INTENT.OFFICIAL_SITE],
    ['7zpi下载', 'whitelist', '7zip', INTENT.OFFICIAL_SITE],
    ['steam官网在哪', 'whitelist', 'steam', INTENT.OFFICIAL_SITE],
    ['staem是什么 我搜出来好多', 'whitelist', 'steam', INTENT.OFFICIAL_SITE],
    ['怎么下原神官服', 'fallback', null, INTENT.OFFICIAL_SITE],
    ['求个PS下载地址', 'fallback', null, INTENT.OFFICIAL_SITE],
    ['微信电脑版在哪下', 'whitelist', 'wechat', INTENT.OFFICIAL_SITE],
    ['chrome下载', 'whitelist', 'chrome', INTENT.OFFICIAL_SITE],
    ['chrmoe官网', 'whitelist', 'chrome', INTENT.OFFICIAL_SITE],
    ['谷哥浏览器在哪下', 'whitelist', 'chrome', INTENT.OFFICIAL_SITE],
    ['威信电脑版', 'whitelist', 'wechat', INTENT.OFFICIAL_SITE],
    ['n卡驱动怎么装', 'whitelist', 'driver-nvidia', INTENT.OFFICIAL_SITE],
    ['vscode下载地址', 'whitelist', 'vscode', INTENT.OFFICIAL_SITE],
    ['watt toolkit 官网', 'whitelist', 'watt-toolkit', INTENT.OFFICIAL_SITE],
    ['everything在哪下', 'whitelist', 'everything', INTENT.OFFICIAL_SITE],
    ['火绒官网在哪', 'whitelist', 'huorong', INTENT.OFFICIAL_SITE],
    ['火荣安全下载', 'whitelist', 'huorong', INTENT.OFFICIAL_SITE],
    ['winrar在哪下', 'whitelist', 'winrar', INTENT.OFFICIAL_SITE],
    ['geek卸载工具官网', 'whitelist', 'geek-uninstaller', INTENT.OFFICIAL_SITE],
    ['福昕pdf官方地址', 'whitelist', 'foxit', INTENT.OFFICIAL_SITE],
    ['向日葵官网', 'whitelist', 'sunlogin', INTENT.OFFICIAL_SITE],
    ['todesk下载', 'whitelist', 'todesk', INTENT.OFFICIAL_SITE],
    ['腾讯会议在哪下', 'whitelist', 'tencent-meeting', INTENT.OFFICIAL_SITE],
    ['potplayer官网在哪', 'whitelist', 'potplayer', INTENT.OFFICIAL_SITE],
    ['vlc播放器怎么下', 'whitelist', 'vlc', INTENT.OFFICIAL_SITE],
    ['sumatrapdf下载', 'whitelist', 'sumatrapdf', INTENT.OFFICIAL_SITE],
    // 类别词：用户只说了类别，没点名产品
    ['解压软件去哪下', 'whitelist', null, INTENT.OFFICIAL_SITE],   // meta.category
    ['播放器去哪下', 'whitelist', null, INTENT.OFFICIAL_SITE],
    ['解压软件是什么', 'none', null, 'what_is_term'],
    // 术语类，本链路不处理
    ['显存和内存有啥区别', 'none', null, 'what_is_term'],
    ['bandizip 是什么软件', 'none', null, 'what_is_term'],
  ];

  let pass = 0;
  let toneFail = 0;

  console.log('══ 链路测试 ══\n');

  for (const [input, expectSource, expectId, expectIntent] of CASTS) {
    const r = handle(input);
    const srcOk = r.source === expectSource;
    const idOk = expectId === null ? true : r.meta.id === expectId;
    const intentOk = expectIntent ? r.intent === expectIntent : true;
    const ok = srcOk && idOk && intentOk && r.toneOk;
    if (ok) pass++;
    if (!r.toneOk) toneFail++;

    const got = r.meta.id || r.meta.category || r.source;
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${input.padEnd(24)} → ${String(got).padEnd(16)} [${r.source}]`);
    if (!ok && !r.toneOk) console.log('        ⚠ 语气未通过');
  }

  console.log(`\n${pass}/${CASTS.length} 通过，语气失败 ${toneFail} 条\n`);

  // ── 展示几个完整答案 ──────────────────────
  console.log('══ 答案样例 ══\n');
  for (const q of ['bandzip官网', 'staem是什么 我搜出来好多', '怎么下原神官服']) {
    const r = handle(q);
    console.log(`问：${q}`);
    if (r.answer) {
      console.log(r.answer.split('\n').map((l) => '  ' + l).join('\n'));
    } else {
      console.log(`  （本链路不处理，intent=${r.intent}）`);
    }
    console.log(`  〔来源 ${r.source}〕\n`);
  }

  // ── 域名校验 ──────────────────────────────
  console.log('══ 域名校验 ══\n');
  const domainCases = [
    ['cn.bandisoft.com', 'bandizip', true],
    ['www.bandisoft.com', 'bandizip', true],      // 国际站在 alt 里，也算官方
    ['bandizip.cn', 'bandizip', false],           // 仿冒
    ['store.steampowered.com', 'steam', true],
    ['steam-download.com', 'steam', false],       // 仿冒
  ];
  let dPass = 0;
  for (const [domain, id, expectMatch] of domainCases) {
    const r = checkDomain(domain, id);
    const ok = r.matches === expectMatch;
    if (ok) dPass++;
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${domain.padEnd(24)} 匹配=${r.matches}  ${r.reason}`);
  }
  console.log(`\n${dPass}/${domainCases.length} 通过`);

  if (pass !== CASTS.length || dPass !== domainCases.length) process.exitCode = 1;
}


