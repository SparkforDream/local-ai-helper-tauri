// ============================================================
// 五档判档：纯函数，不依赖任何外部状态
// 与 src/tier/tiers.ts 保持同一套门槛，改一处必须改两处。
// ============================================================

export const GB = 1024 ** 3;

// 驱动预留：实测 RTX 4060 Laptop 标称 8.0GB，vulkan 报可用 7.02GB
const DRIVER_RESERVE = 0.98 * GB;

export const TIERS = {
  minimal: {
    level: '极低',
    name: '网页版',
    size: '—',
    vramNeed: 0,
    desc: '这台跑不动本地模型。',
    fits: ['开浏览器就能问'],
    notFit: ['离线使用', '本地模型'],
    action: 'tutorial',
  },
  low: {
    level: '低',
    name: '轻量模型',
    size: '1.9 GB',
    vramNeed: 2.6,
    desc: '只能跑小模型，回答简短。',
    fits: ['解释术语', '常见故障'],
    notFit: ['长对话', '复杂推理'],
    action: 'download',
  },
  mid: {
    level: '中',
    name: '标准模型',
    size: '5.5 GB',
    vramNeed: 6.6,
    desc: '日常最稳的一档，速度也快。',
    fits: ['解释术语', '常见故障', '多轮追问'],
    notFit: ['超长文档'],
    action: 'download',
  },
  high: {
    level: '高',
    name: '增强模型',
    size: '9.0 GB',
    vramNeed: 11.2,
    desc: '长一点的问题能接住。',
    fits: ['长解释', '多轮追问', '复杂排查'],
    notFit: ['专业代码'],
    action: 'download',
  },
  ultra: {
    level: '极高',
    name: '旗舰模型',
    size: '19.8 GB',
    vramNeed: 23.0,
    desc: '除了慢一点，基本什么都能聊。',
    fits: ['几乎任何问题', '长文档', '复杂推理'],
    notFit: ['—'],
    action: 'download',
  },
};

export const TIER_ORDER = ['minimal', 'low', 'mid', 'high', 'ultra'];

// 独显可用显存。核显一律按 0 处理：
// AdapterRAM 给的是共享内存切片，数值虚高且无意义。
export function effectiveVram(gpus) {
  if (!gpus || !gpus.length) return null;
  let best = 0;
  let found = false;
  for (const g of gpus) {
    if (g.vendor !== 'nvidia' && g.vendor !== 'amd') continue;
    if (!g.vramBytes) continue;
    found = true;
    if (g.vramBytes > best) best = g.vramBytes;
  }
  if (!found) return 0;
  return Math.max(0, best - DRIVER_RESERVE);
}

export function judge(p) {
  const vram = effectiveVram(p.gpus);
  const ram = p.memory?.totalBytes ?? null;
  const ramGb = ram ? ram / GB : 0;

  if (ramGb > 0 && ramGb < 8) {
    return {
      tier: 'minimal',
      title: '这台跑不动本地模型。',
      line: '按 Windows 键，输 Edge，回车，地址栏打 chat.deepseek.com。',
    };
  }

  if (vram === null) {
    return {
      tier: 'minimal',
      title: '没能读到你的显卡。',
      line: '先按最低档试，跑得动再往上换。',
    };
  }

  if (vram >= TIERS.ultra.vramNeed * GB) {
    return { tier: 'ultra', title: '这台有<em>干货</em>。', line: '除了慢一点，基本什么都能聊。' };
  }
  if (vram >= TIERS.high.vramNeed * GB) {
    return { tier: 'high', title: '能跑<em>比较大</em>的模型。', line: '长一点的问题也接得住。' };
  }
  if (vram >= TIERS.mid.vramNeed * GB) {
    return { tier: 'mid', title: '刚好是我<em>最擅长</em>的那一档。', line: '日常问答、看硬件、解释术语都够用。' };
  }
  if (vram >= TIERS.low.vramNeed * GB) {
    return { tier: 'low', title: '能跑，但只能跑<em>小模型</em>。', line: '回答会简短，复杂问题容易绕。' };
  }

  if (ramGb >= 16) {
    return { tier: 'low', title: '没有独显，但内存够。', line: '纯内存跑会慢一些，每个字要等一两秒。' };
  }
  if (ramGb >= 8) {
    return { tier: 'low', title: '能跑最小的模型。', line: '只够开这一个程序，再开别的会开始卡。' };
  }

  return { tier: 'minimal', title: '这台跑不动本地模型。', line: '' };
}
