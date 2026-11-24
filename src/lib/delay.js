// ============================================================
// delay.js —— 节奏控制
//
// 进度条设计的公开秘密：
//   没人喜欢秒回。0.3 秒出结果会让人觉得「它根本没思考」，
//   或者更糟 ——「它在敷衍我」。
//
//   但也不能瞎等。正确做法是「补足」：
//   真实耗时 80ms 的问题，补到 600ms；
//   真实耗时 2.4s 的问题，一秒都不加。
//
//   这样总耗时永远落在「像人」的区间里，慢的操作也不会被拖得更慢。
// ============================================================

/**
 * 不同情况的最短响应时间（毫秒）。
 *
 * 依据：
 *   寒暄       —— 人在回「嗯」之前会停顿一下，0.5-0.8s 最自然
 *   接情绪     —— 挨骂之后立刻接话显得敷衍，要留出「被打了一下」的时间
 *   技术问答   —— 查资料要时间，0.5s 上下
 *   能力边界   —— 判断「我不会」也需要时间，不能秒推
 *   安装流程   —— 有真实操作，不强加延迟
 */
export const MIN_DELAY = {
  greet: 520,
  emotion: 900,      // 挨骂后停顿最长 —— 这是刻意的
  tech: 620,
  boundary: 700,
  install: 0,        // 真操作，不用装
  default: 560,
};

/**
 * 等到「看起来像在思考」为止。
 *
 * @param {number} elapsedMs 已经花掉的时间
 * @param {number} minMs     这个场景的最短响应时间
 */
export async function paceTo(elapsedMs, minMs) {
  const remain = minMs - elapsedMs;
  if (remain <= 0) return;

  // 分两段等，避免一次 setTimeout 太长导致 UI 卡顿感
  await sleep(remain);
}

export function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

/**
 * 根据命中的处理层挑选合适的最短延迟。
 * 这个映射关系本身就是「产品态度」的体现。
 */
export function delayFor(meta = '') {
  if (/寒暄|接情绪/.test(meta)) return MIN_DELAY.emotion;
  if (/身份|能力范围/.test(meta)) return MIN_DELAY.tech;
  if (/不联网|本地时间/.test(meta)) return MIN_DELAY.tech;
  if (/安装流程/.test(meta)) return MIN_DELAY.install;
  if (/官网库|硬件/.test(meta)) return MIN_DELAY.tech;
  if (/超出范围/.test(meta)) return MIN_DELAY.boundary;
  return MIN_DELAY.default;
}

// ============================================================
// 自测：node --experimental-strip-types src/lib/delay.js
// ============================================================

if (typeof process !== 'undefined' && process.argv?.[1]?.endsWith('delay.js')) {
  const cases = [
    ['寒暄', 900],
    ['接情绪', 900],
    ['身份', 620],
    ['官网库 · steam', 620],
    ['硬件 · 实测', 620],
    ['安装流程 · Steam · 待下载', 0],
    ['超出范围', 700],
    ['未知标注', 560],
  ];

  let pass = 0;
  console.log('══ 延迟映射 ══\n');
  for (const [meta, expect] of cases) {
    const got = delayFor(meta);
    const ok = got === expect;
    if (ok) pass++;
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${meta.padEnd(26)} → ${got}ms`);
  }

  // paceTo 实测：补足到目标时间
  (async () => {
    console.log('\n══ 补足耗时 ══\n');
    const t0 = Date.now();
    await paceTo(0, 300);
    const d1 = Date.now() - t0;
    console.log(`${Math.abs(d1 - 300) < 60 ? 'PASS' : 'FAIL'}  从 0ms 补到 300ms，实得 ${d1}ms`);

    const t1 = Date.now();
    await paceTo(500, 300);       // 已超时，不该再等
    const d2 = Date.now() - t1;
    console.log(`${d2 < 50 ? 'PASS' : 'FAIL'}  已超时不再等，实得 ${d2}ms`);

    const all = pass === cases.length && Math.abs(d1 - 300) < 60 && d2 < 50;
    console.log(`\n${pass}/${cases.length} 通过${all ? '' : '（有失败）'}`);
    if (!all) process.exitCode = 1;
  })();
}
