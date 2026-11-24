// ============================================================
// social.js —— 社交与常识层
//
// 为什么需要这一层：
//   用户说「你好」时回一句「这个我判断不了，不想瞎猜」，
//   会让人觉得这软件坏了。寒暄不需要智能，需要的是别装死。
//
// 设计纪律：
//   1. 全部确定性，不调模型，零成本。
//   2. 同类问题多套说法轮换，不每次都一模一样。
//   3. 真答不了的（天气、新闻），说清为什么 + 给出去处，
//      不用「我无法」这种推卸话。
// ============================================================

// ─────────────────────────────────────────── 寒暄
// 每组多个变体，按调用次数轮换。重复同一句话会显得很假。

const GREETINGS = [
  '在。有什么要弄的？',
  '在的，说吧。',
  '嗯，我在。',
];

const HOW_ARE_YOU = [
  '挺好的，随时待命。\n你那边有什么不顺手的？',
  '还行。你这台机器我看着呢，有事直说。',
];

const THANKS = [
  '不客气。',
  '嗯，能用就行。',
  '小事。还有别的要弄吗？',
];

const BYE = [
  '行，有事再喊我。',
  '好，先这样。',
];

const WHO_ARE_YOU = [
  '我是装在你电脑上的助手。\n\n能帮的事：装软件、找官网、看配置、查是什么在吃内存。\n\n所有判断都在本地做，不上传你的数据。',
];

const PRAISE = [
  '能帮上就行。',
  '嗯，解决了就好。',
];

const COMPLAINT = [
  '哪儿不对？说一下具体情况，我看看。',
  '行，你说，我听着。',
  '是没弄好。哪一步跟你想的不一样？',
];

// ─────────────────────────────────────────── 骂人
//
// 用户骂人的时候，他真正想说的是「你没解决我的问题」，
// 而不是在评价这个软件的人格。
// 所以回应的重点永远是「回到问题上」，不是「回应攻击」。
//
// 词表要宽：同义词、方言、网络流行语、人身攻击都算。
// 但必须排除「这个东西是垃圾文件」这类正常用法。
const INSULT_WORDS = [
  '废物', '垃圾', '傻逼', '傻b', 'sb', '煞笔', '沙雕', '傻子',
  '唐氏', '唐', '智障', '弱智', '蠢', '笨', '脑残', '残废',
  '没用', '没卵用', '有屁用', '啥也不是', '一无是处',
  '什么破', '破软件', '烂', '差劲', '拉胯', '稀烂', '狗屎',
  '垃圾东西', '什么玩意', '坑爹', '坑人', '骗人', '骗子',
  '人工智障', '智熄', '完蛋玩意', '废柴', 'five',
];

// 这些词出现在句子里时，说明用户说的是文件/软件，不是骂人
const NOT_INSULT_CONTEXT = [
  '垃圾文件', '垃圾清理', '清理垃圾', '垃圾邮件', '垃圾箱', '回收站',
  '垃圾回收', '这个文件', '那个文件', '文件夹',
];

/**
 * 判断是不是在骂人。
 *
 * 两种都算：
 *   ① 整句就是骂（"垃圾" "废物" "傻逼"）
 *   ② 骂 + 指向软件/你（"这软件真垃圾" "你个废物"）
 *
 * 不算的：说某个东西是垃圾文件、问怎么清垃圾。
 */
export function isInsult(q) {
  const t = String(q || '').toLowerCase().trim();
  if (!t) return false;

  // 排除正常语境
  if (NOT_INSULT_CONTEXT.some((c) => t.includes(c))) return false;

  // 短句里出现骂人词，基本就是在骂
  if (t.length <= 12) {
    return INSULT_WORDS.some((w) => t.includes(w));
  }

  // 长句里要有明确的指向对象才算
  const hasInsult = INSULT_WORDS.some((w) => t.includes(w));
  const hasTarget = /你|这|那|软件|助手|程序|东西|玩意/.test(t);
  return hasInsult && hasTarget;
}

// ─────────────────────────────────────────── 能力清单
const CAPABILITIES = [
  '现在能做的：',
  '',
  '· 装软件 —— Steam、7-Zip 可以一句话装完',
  '· 找官网 —— 报软件名就给正版地址',
  '· 看配置 —— CPU、内存、显卡、硬盘剩余',
  '· 查占用 —— 哪个程序在吃内存',
  '· 解释术语 —— 什么是固态、显存、驱动',
  '',
  '不会的：聊天、写文章、查天气新闻、替你操作游戏。',
];

// ─────────────────────────────────────────── 匹配表
// 顺序很重要：越具体的越靠前，避免被泛化的规则抢走。

const RULES = [
  // 身份
  { re: /^(你是谁|你叫什么|你是什么|介绍一下你自己|你谁啊|啥东西)$/i, kind: 'who' },
  { re: /你能(做|干)(什么|啥)|有什么(功能|用)|你会什么|能帮我做什么/, kind: 'capability' },

  // 寒暄
  { re: /^(你好|您好|hi|hello|哈喽|嗨|在吗|在不在|喂)\s*[?？!！。]*$/i, kind: 'greet' },
  { re: /(你好|您好).{0,6}(吗|呀|啊)?$/, kind: 'greet' },
  { re: /最近(怎么样|好吗)|过得好吗|你还好吗/, kind: 'howareyou' },
  { re: /^(谢谢|多谢|感谢|谢了|3q|thanks|thx)/i, kind: 'thanks' },
  { re: /^(再见|拜拜|byebye|bye|走了|晚安)/i, kind: 'bye' },
  { re: /(厉害|牛逼|牛啊|可以啊|真棒|不错)/, kind: 'praise' },
  { re: /(没用|垃圾|不好用|什么破|难用|烂)/, kind: 'complaint' },
  // 真答不了的：说清楚 + 给出去处
  {
    re: /天气|气温|下雨|多少度|冷不冷|热不热/,
    kind: 'weather',
  },
  {
    re: /新闻|热搜|今天发生|最近发生|股市|股票|汇率|油价/,
    kind: 'news',
  },
  {
    re: /几点|现在时间|今天几号|今天星期|日期/,
    kind: 'time',
  },
  {
    re: /讲个笑话|说个笑话|逗我|唱歌|讲个故事/,
    kind: 'chat_skill',
  },
  {
    re: /^(嗯+|哦+|啊+|哈+|额+|呃+|\?+|？+|\.+|。+)$/,
    kind: 'filler',
  },
];

// ─────────────────────────────────────────── 轮换计数
let _tick = 0;
function pick(arr) {
  if (!arr.length) return '';
  const v = arr[_tick % arr.length];
  _tick++;
  return v;
}

// ─────────────────────────────────────────── 天气
// 没联网就别装懂。说清楚，给一个能自己看的地方。

function weatherAnswer() {
  return [
    '我这儿不联网，看不到实时天气。',
    '',
    '两个办法：',
    '· 按 Win 键，输「天气」，回车 —— Windows 自带天气应用',
    '· 浏览器打开 weather.com.cn',
    '',
    '要是你想问的是「电脑怕不怕热」，那倒是能说：',
    '笔记本进风口别堵住，夏天垫高一点会更稳。',
  ].join('\n');
}

function newsAnswer() {
  return [
    '我不联网，看不到新闻。',
    '',
    '能说的是：我这台只知道你机器的状态，不知道外面发生了什么。',
  ].join('\n');
}

function timeAnswer() {
  const d = new Date();
  const week = ['日', '一', '二', '三', '四', '五', '六'][d.getDay()];
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  return `现在 ${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日 星期${week} ${hh}:${mm}。`;
}

// ─────────────────────────────────────────── 对外

/**
 * 尝试用社交层回答。命中返回字符串，没命中返回 null。
 *
 * @param {string} q 用户原话
 * @returns {{text: string, tag: string}|null}
 */
export function socialAnswer(q) {
  const t = String(q || '').trim();
  if (!t) return null;

  // ── 骂人：优先于所有其它规则 ──────────────
  // 骂人的句子也可能含其它关键词（"这垃圾软件能不能退"），
  // 但那不是重点。先接住情绪，再谈事。
  if (isInsult(t)) {
    return { text: pick(COMPLAINT), tag: '接情绪' };
  }

  for (const r of RULES) {
    if (!r.re.test(t)) continue;

    switch (r.kind) {
      case 'who':
        return { text: pick(WHO_ARE_YOU), tag: '身份' };
      case 'capability':
        return { text: CAPABILITIES.join('\n'), tag: '能力范围' };
      case 'greet':
        return { text: pick(GREETINGS), tag: '寒暄' };
      case 'howareyou':
        return { text: pick(HOW_ARE_YOU), tag: '寒暄' };
      case 'thanks':
        return { text: pick(THANKS), tag: '寒暄' };
      case 'bye':
        return { text: pick(BYE), tag: '寒暄' };
      case 'praise':
        return { text: pick(PRAISE), tag: '寒暄' };
      case 'complaint':
        return { text: pick(COMPLAINT), tag: '接情绪' };
      case 'weather':
        return { text: weatherAnswer(), tag: '不联网' };
      case 'news':
        return { text: newsAnswer(), tag: '不联网' };
      case 'time':
        return { text: timeAnswer(), tag: '本地时间' };
      case 'chat_skill':
        return {
          text:
            '这个我不擅长。\n\n' +
            '我是干活的，不是聊天的。真能帮上的：装软件、找官网、看配置、查占用。',
          tag: '超出范围',
        };
      case 'filler':
        return {
          text: '嗯？有什么要弄的直接说。',
          tag: '寒暄',
        };
      default:
        return null;
    }
  }
  return null;
}

// ============================================================
// 自测
// ============================================================

if (typeof process !== 'undefined' && process.argv?.[1]?.endsWith('social.js')) {
  const cases = [
    ['你好', '寒暄'],
    ['您好', '寒暄'],
    ['你好啊', '寒暄'],
    ['hi', '寒暄'],
    ['在吗', '寒暄'],
    ['你是谁', '身份'],
    ['你能做什么', '能力范围'],
    ['最近怎么样', '寒暄'],
    ['谢谢', '寒暄'],
    ['再见', '寒暄'],
    ['厉害', '寒暄'],
    ['什么破软件', '接情绪'],
    ['今天天气怎么样', '不联网'],
    ['明天会下雨吗', '不联网'],
    ['现在几点了', '本地时间'],
    ['今天星期几', '本地时间'],
    ['讲个笑话', '超出范围'],
    ['嗯', '寒暄'],
    ['？？？', '寒暄'],
  ];

  let pass = 0;
  console.log('══ 社交层测试 ══\n');
  for (const [q, expect] of cases) {
    const r = socialAnswer(q);
    const ok = r && r.tag === expect;
    if (ok) pass++;
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${q.padEnd(16)} → ${r ? r.tag : '未命中'}`);
    if (ok) console.log(r.text.split('\n').map((l) => '        ' + l).join('\n'));
    console.log();
  }

  // 不该被社交层抢走的技术问题
  console.log('══ 不该被抢走的 ══\n');
  const tech = ['C盘还剩多少', '帮我装steam', 'steam官网在哪', '我电脑好卡'];
  let safe = 0;
  for (const q of tech) {
    const r = socialAnswer(q);
    const ok = r === null;
    if (ok) safe++;
    console.log(`${ok ? 'PASS' : 'FAIL'}  ${q.padEnd(16)} → ${r ? '被抢走：' + r.tag : '正确放行'}`);
  }

  const all = pass === cases.length && safe === tech.length;
  console.log(`\n社交层 ${pass}/${cases.length}，放行 ${safe}/${tech.length}`);
  if (!all) process.exitCode = 1;
}
