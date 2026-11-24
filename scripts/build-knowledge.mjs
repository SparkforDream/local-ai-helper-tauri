#!/usr/bin/env node
/**
 * build-knowledge.mjs —— 把 official.yaml 编译成前端可直接 import 的模块
 *
 * 为什么不用 js-yaml：
 *   这个项目要控制依赖数量。official.yaml 的格式由我们自己定义，
 *   结构固定（顶层键 + 条目数组 + 行内/块状列表），一个小解析器就够，
 *   省掉一个 npm 依赖和它的供应链风险。
 *
 * 用法：node scripts/build-knowledge.mjs
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = join(ROOT, 'assets', 'official.yaml');
const OUT = join(ROOT, 'src', 'lib', 'official.data.js');

// ─────────────────────────────────────────── 极简 YAML 解析
// 只支持本项目用到的子集：
//   顶层 key: value
//   顶层 key: 后接 "  - " 开头的块状对象数组
//   条目内 key: value / key: [a, b, c] / key: 后接 "      - " 的字符串列表

function stripComment(line) {
  // 只处理行尾注释，且注释前必须有空格，避免误伤 URL 里的 #
  const i = line.indexOf(' #');
  return i >= 0 ? line.slice(0, i) : line;
}

function parseInlineList(v) {
  const inner = v.slice(1, -1).trim();
  if (!inner) return [];
  // 逗号分隔，但忽略引号内的逗号（本项目暂不出现，留个保险）
  return inner.split(',').map((s) => s.trim().replace(/^["']|["']$/g, '')).filter(Boolean);
}

function parseYaml(text) {
  const lines = text.split(/\r?\n/);
  const root = {};
  let curBlock = null;      // 顶层块名，如 entries
  let curEntry = null;      // 当前条目对象
  let curListKey = null;    // 当前正在填充的列表字段名

  for (const rawLine of lines) {
    const line = stripComment(rawLine);
    if (!line.trim()) continue;

    const indent = line.match(/^\s*/)[0].length;
    const body = line.trim();

    // 顶层键
    if (indent === 0 && body.endsWith(':')) {
      curBlock = body.slice(0, -1).trim();
      root[curBlock] = [];
      curEntry = null;
      curListKey = null;
      continue;
    }

    // 顶层键: 值（非数组）
    if (indent === 0 && body.includes(':')) {
      const [k, ...rest] = body.split(':');
      root[k.trim()] = rest.join(':').trim();
      curBlock = null;
      continue;
    }

    // 数组项起点：  - id: xxx
    if (indent === 2 && body.startsWith('- ')) {
      curEntry = {};
      root[curBlock].push(curEntry);
      curListKey = null;
      const kv = body.slice(2);
      if (kv.includes(':')) {
        const [k, ...rest] = kv.split(':');
        const v = rest.join(':').trim();
        curEntry[k.trim()] = v === '' ? null : v;
        if (v === '') curListKey = k.trim();
      }
      continue;
    }

    // 条目内字段（4 空格）
    if (indent === 4 && curEntry) {
      const idx = body.indexOf(':');
      if (idx < 0) continue;
      const k = body.slice(0, idx).trim();
      const v = body.slice(idx + 1).trim();

      if (v === '') {
        // 后面接块状列表，或接嵌套（alt 的跨行列表）
        curEntry[k] = [];
        curListKey = k;
      } else if (v.startsWith('[') && v.endsWith(']')) {
        curEntry[k] = parseInlineList(v);
        curListKey = null;
      } else {
        curEntry[k] = v.replace(/^["']|["']$/g, '');
        curListKey = null;
      }
      continue;
    }

    // 列表项（6 空格，- 开头）
    if (indent >= 6 && body.startsWith('- ') && curEntry && curListKey) {
      const item = body.slice(2).trim().replace(/^["']|["']$/g, '');
      if (!Array.isArray(curEntry[curListKey])) curEntry[curListKey] = [];
      curEntry[curListKey].push(item);
      continue;
    }
  }

  return root;
}

// ─────────────────────────────────────────── 规范化
function normalize(raw) {
  const entries = (raw.entries || []).map((e) => ({
    id: e.id,
    name: e.name,
    aliases: e.aliases || [],
    primary: e.primary,
    download: e.download || null,
    domain: e.domain || hostOf(e.primary),
    alt: e.alt || [],
    note: e.note || '',
    trap: e.trap || [],
    // 这个标记决定拼错时要不要提醒仿冒风险。
    // 只有那些"错拼本身是仿冒重灾区"的条目才置 true。
    typoIsTrap: e.typoIsTrap === 'true' || e.typoIsTrap === true,
  }));

  const blacklist = (raw.blacklistPatterns || []).map((b) => ({
    pattern: b.pattern,
    reason: b.reason,
  }));

  return {
    version: Number(raw.version || 1),
    verifiedAt: raw.verifiedAt || '',
    entries,
    blacklist,
  };
}

function hostOf(url) {
  if (!url) return '';
  try { return new URL(url).host; } catch { return ''; }
}

// ─────────────────────────────────────────── 主流程
const yamlText = readFileSync(SRC, 'utf8');
const parsed = normalize(parseYaml(yamlText));

// 数据自检：任何一条缺关键字段都直接失败，不让坏数据进构建
const problems = [];
parsed.entries.forEach((e, i) => {
  if (!e.id) problems.push(`第 ${i + 1} 条缺 id`);
  if (!e.name) problems.push(`${e.id} 缺 name`);
  if (!e.primary) problems.push(`${e.id} 缺 primary`);
  if (!e.aliases.length) problems.push(`${e.id} 缺 aliases`);
  if (e.primary && !e.primary.startsWith('https://')) {
    problems.push(`${e.id} 的 primary 不是 https：${e.primary}`);
  }
});

if (problems.length) {
  console.error('知识库有结构问题，构建中止：');
  problems.forEach((p) => console.error('  - ' + p));
  process.exit(1);
}

const banner = `// 本文件由 scripts/build-knowledge.mjs 自动生成，不要手改。
// 源文件：assets/official.yaml（人工维护，改完重新构建）
// 生成时间：${new Date().toISOString()}
// 条目数：${parsed.entries.length}  验证日期：${parsed.verifiedAt}
`;

const out = `${banner}
export const OFFICIAL = ${JSON.stringify(parsed, null, 2)};
`;

writeFileSync(OUT, out, 'utf8');
console.log(`✓ 已生成 ${OUT}`);
console.log(`  条目 ${parsed.entries.length} 条，黑名单 ${parsed.blacklist.length} 条`);
console.log(`  别名总数 ${parsed.entries.reduce((n, e) => n + e.aliases.length, 0)} 个`);
