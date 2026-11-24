// ============================================================
// typo.js —— 拼写容错与静默纠错
//
// 解决的问题：
//   用户把 bandizip 打成 bandzip、staem 打成 steam、
//   谷歌打成谷哥。软件要能认出来，还要纠正得不让用户难堪。
//
// 三条纪律（这是本文件存在的全部理由）：
//   1. 不复述错误写法。说了"你说的 bandzip"就等于又强调了错的。
//   2. 不评论拼写。没有"其实""应该说""严格来讲"。
//   3. 必须提醒时，把责任推给搜索结果，不推给用户。
// ============================================================

// ─────────────────────────────────────────── 编辑距离
// 只用于拉丁字母串。中文走别名表，不做距离计算。
function levenshtein(a, b) {
  if (a === b) return 0;
  const m = a.length, n = b.length;
  if (!m) return n;
  if (!n) return m;

  let prev = Array.from({ length: n + 1 }, (_, i) => i);
  const cur = new Array(n + 1);

  for (let i = 1; i <= m; i++) {
    cur[0] = i;
    for (let j = 1; j <= n; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      cur[j] = Math.min(cur[j - 1] + 1, prev[j] + 1, prev[j - 1] + cost);
    }
    prev = cur.slice();
  }
  return prev[n];
}

function normalize(s) {
  return String(s || '')
    .toLowerCase()
    .replace(/[\s_\-.]/g, '')     // 去掉空格与分隔符
    .replace(/[^\w\u4e00-\u9fff]/g, '');
}

// ─────────────────────────────────────────── 中文常见错写
// 拼音相同或形近字。距离算法对中文无效，只能用表。
const CN_VARIANTS = {
  '微信': ['威信', '徽信', '维信', '微心'],
  '谷歌': ['谷哥', '古歌', '骨歌'],
  '浏览器': ['流览器', '浏缆器'],
  '压缩': ['压塑', '押缩'],
  '显卡': ['显咔', '县卡'],
  '内存': ['内寸', '內存'],
  '驱动': ['区动', '趋动'],
  '固态': ['固太', '故态'],
};

// ─────────────────────────────────────────── 主匹配

/**
 * 在别名集合里找最接近的条目。
 *
 * @param {string} input       用户输入
 * @param {Array}  entries     白名单条目，每条需有 aliases 字段
 * @returns {{entry, matchedAs, distance, exact, atRisk}|null}
 */
export function resolveSoftware(input, entries) {
  const raw = normalize(input);
  if (!raw) return null;

  // 1. 精确命中：别名或名字完全一致
  for (const e of entries) {
    const keys = [e.id, e.name, ...(e.aliases || [])].map(normalize);
    if (keys.includes(raw)) {
      return { entry: e, matchedAs: raw, distance: 0, exact: true, atRisk: false };
    }
  }

  // 2. 中文错写：走变体表
  for (const [correct, variants] of Object.entries(CN_VARIANTS)) {
    if (variants.some((v) => raw.includes(normalize(v)))) {
      for (const e of entries) {
        const keys = [e.name, ...(e.aliases || [])].map(normalize);
        if (keys.some((k) => k.includes(normalize(correct)))) {
          return { entry: e, matchedAs: raw, distance: 1, exact: false, atRisk: false };
        }
      }
    }
  }

  // 3. 编辑距离：只在拉丁字母输入上用，阈值随长度放宽
  if (/^[a-z0-9]+$/.test(raw)) {
    let best = null;
    for (const e of entries) {
      const keys = [e.id, e.name, ...(e.aliases || [])]
        .map(normalize)
        .filter((k) => /^[a-z0-9]+$/.test(k));

      for (const k of keys) {
        // 太短的别名（7z / qq）不参与距离比较：
        // 它们会被任何短输入命中，反而抢掉更精确的长别名。
        if (k.length < 3) continue;
        // 长度差距过大直接跳过，避免 "qq" 匹配到 "everything"
        if (Math.abs(k.length - raw.length) > 3) continue;

        const d = levenshtein(raw, k);
        // 容错阈值：长度 3 只容 1 次，4 及以上容 2 次。
        // 4 字符也放宽到 2，是因为相邻字母互换（7zpi ↔ 7zip）
        // 在编辑距离里算 2 次，容 1 会漏掉最常见的输入错误。
        const allowed = k.length <= 3 ? 1 : 2;
        if (d > allowed) continue;

        // 同距离时优先更长的别名（7zip 胜过 7z）
        if (!best || d < best.distance ||
            (d === best.distance && k.length > best.matchedAs.length)) {
          best = { entry: e, matchedAs: k, distance: d, exact: false, atRisk: e.typoIsTrap === true };
        }
      }
    }
    if (best) return best;
  }

  return null;
}

// ============================================================
// 文案生成 —— 这一段的每一句都经过推敲
// ============================================================

/**
 * 生成「静默纠错」的答案。
 *
 * 要点：
 *   - 直接用正确名字，不复述用户写法
 *   - 不评论拼写
 *   - 有仿冒风险时，说"搜这个词出来的多半是仿的"，不说"你打错了"
 */
export function composeAnswer(hit) {
  if (!hit) return null;

  const { entry, exact, atRisk } = hit;
  const lines = [];

  // 第一行：正确名字 + 地址。没有前言。
  lines.push(`${entry.name}：${entry.primary.replace(/^https?:\/\//, '')}`);

  // 只有错误写法本身是仿冒重灾区时才提一句，且归因给搜索
  if (!exact && atRisk) {
    lines.push('');
    lines.push('搜这个词出来的多半是仿的站。');
  }

  // 动作
  if (entry.download && entry.download !== entry.primary) {
    lines.push('');
    lines.push(`下载页：${entry.download.replace(/^https?:\/\//, '')}`);
  }

  return lines.join('\n');
}

// ============================================================
// 语气检查 —— 用来防止自己写歪
//
// 这是自检工具，不是给用户看的。
// 任何生成的答案都要过一遍，命中即改写。
// ============================================================

const TONE_VIOLATIONS = [
  // 说教型开场
  { p: /首先要?(指正|纠正|说明|指出)/, why: '说教式开场' },
  { p: /需要(先)?(纠正|指正)/, why: '说教式纠正' },
  { p: /(您|你)(可能|应该)?(是)?拼错/, why: '直接点破拼写错误' },
  { p: /正确的?(写法|拼写|说法)是/, why: '强调用户写法错误' },
  // 「A，而不是 B」在纠正语境里带贬义，不限中间长度
  { p: /(应该|应当|应)是?[^。！？\n]{0,20}(而不是|而非)/, why: '「而不是」结构在纠正语境里带贬义' },
  { p: /(顺带|顺便|另外)(一提|说一句|提一下)/, why: '补充式说教' },
  { p: /(请注意|提醒您|提醒你|需要注意)/, why: '居高临下的提醒' },

  // 复述错误写法的典型句式
  { p: /(你|您)说的是?\s*[a-zA-Z]{3,}\s*[，,]?\s*(其实|实际上|正确)/, why: '复述了用户的错误写法' },

  // 自我抬高
  { p: /作为(一个)?(AI|助手|语言模型)/, why: '自我指称' },
  { p: /(我|本助手)(必须|需要)(提醒|指出)/, why: '居高临下' },
];

/**
 * 检查文案是否有说教味。
 * @returns {{ok: boolean, hits: Array<{pattern: string, why: string}>}}
 */
export function lintTone(text) {
  const hits = [];
  for (const v of TONE_VIOLATIONS) {
    const m = String(text || '').match(v.p);
    if (m) hits.push({ pattern: m[0], why: v.why });
  }
  return { ok: hits.length === 0, hits };
}

// ============================================================
// 自测：node --experimental-strip-types src/lib/typo.js
// ============================================================

if (typeof process !== 'undefined' && process.argv?.[1]?.endsWith('typo.js')) {
  const ENTRIES = [
    { id: 'bandizip', name: 'Bandizip', aliases: ['bandizip', '班迪'],
      primary: 'https://cn.bandisoft.com/bandizip/',
      download: 'https://cn.bandisoft.com/bandizip/download/',
      trap: ['安装最后一步取消勾选附带软件'] },

    { id: 'steam', name: 'Steam', aliases: ['steam', '蒸汽平台'],
      primary: 'https://store.steampowered.com/',
      download: 'https://store.steampowered.com/about/',
      typoIsTrap: true,       // staem 等错拼是仿冒站重灾区
      trap: ['带「游戏管家」「加速器」字样的都不是官方'] },

    { id: '7zip', name: '7-Zip', aliases: ['7zip', '7z', '七压'],
      primary: 'https://www.7-zip.org/',
      download: 'https://www.7-zip.org/download.html',
      trap: ['「7-Zip 中文版」都不是官方，官方只有 7-zip.org'] },

    { id: 'chrome', name: 'Chrome', aliases: ['chrome', '谷歌浏览器'],
      primary: 'https://www.google.cn/chrome/', download: null },

    { id: 'wechat', name: '微信', aliases: ['wechat', 'weixin'],
      primary: 'https://weixin.qq.com/', download: null },

    { id: 'vscode', name: 'Visual Studio Code', aliases: ['vscode'],
      primary: 'https://code.visualstudio.com/', download: null },

    { id: 'nvidia', name: 'NVIDIA 驱动', aliases: ['nvidia'],
      primary: 'https://www.nvidia.cn/geforce/drivers/', download: null },
  ];

  // ── 测试 1：拼写容错 ──────────────────────
  const typoCases = [
    ['bandzip', 'bandizip'],   // 少个 i
    ['bandi zip', 'bandizip'],
    ['bndizip', 'bandizip'],
    ['staem', 'steam'],        // 字母顺序
    ['stema', 'steam'],
    ['stean', 'steam'],
    ['7zpi', '7zip'],
    ['chrmoe', 'chrome'],
    ['chrom', 'chrome'],
    ['vsocde', 'vscode'],
    ['vscod', 'vscode'],
    ['nvdia', 'nvidia'],
    ['威信', 'wechat'],        // 中文变体
    ['谷哥浏览器', 'chrome'],
  ];

  console.log('══ 拼写容错 ══\n');
  let pass = 0;
  for (const [input, expectId] of typoCases) {
    const r = resolveSoftware(input, ENTRIES);
    const ok = r && r.entry.id === expectId;
    if (ok) pass++;
    const d = r ? `d=${r.distance}` : '未命中';
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${input.padEnd(14)} → ${(r?.entry.name || '—').padEnd(20)} ${d}`);
  }
  console.log(`\n${pass}/${typoCases.length} 命中\n`);

  // ── 测试 2：生成文案没有说教味 ────────────
  console.log('══ 文案语气 ══\n');
  const copyCases = ['bandzip', 'staem', '7zpi', 'chrome'];
  let tonePass = 0;

  for (const input of copyCases) {
    const hit = resolveSoftware(input, ENTRIES);
    if (!hit) { console.log(`SKIP ${input} 未命中`); continue; }
    const answer = composeAnswer(hit);
    const lint = lintTone(answer);

    if (lint.ok) tonePass++;
    console.log(`[${input}] → ${hit.entry.name}  ${hit.exact ? '精确' : '纠错'}`);
    console.log(answer.split('\n').map((l) => '    ' + l).join('\n'));
    if (!lint.ok) {
      console.log(`    ⚠ 语气问题：${lint.hits.map((h) => h.why).join('、')}`);
    }
    console.log();
  }
  console.log(`${tonePass}/${copyCases.length} 无说教味\n`);

  // ── 测试 3：反例必须被抓出来 ──────────────
  console.log('══ 反例检测 ══\n');
  const badSamples = [
    '首先要指正一点，Bandizip 的正确拼写是 bandizip。',
    '您可能拼错了，正确的写法是 bandizip。',
    '应该是 Bandizip，而不是 bandzip。',
    '请注意，官网只有一个是正确的。',
    '你说的是 bandzip，其实正确的是 bandizip。',
    '顺带一提，建议您从官网下载。',
  ];
  let catchCount = 0;
  for (const s of badSamples) {
    const r = lintTone(s);
    const caught = !r.ok;
    if (caught) catchCount++;
    console.log(`${caught ? 'CAUGHT' : 'MISSED'}  ${s}`);
    if (caught) console.log(`          ← ${r.hits.map((h) => h.why).join('、')}`);
  }
  console.log(`\n${catchCount}/${badSamples.length} 被抓出`);

  const allOk = pass === typoCases.length
    && tonePass === copyCases.length
    && catchCount === badSamples.length;
  if (!allOk) process.exitCode = 1;
}
