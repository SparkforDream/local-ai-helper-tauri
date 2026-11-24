// ============================================================
// format.js —— 零依赖、防 XSS 的轻量 Markdown 与安全链接渲染器
//
// 安全纪律：
//   1. 先整体 escapeHtml，任何原始 HTML 标签绝不透传；
//   2. 只把 https:// 或白名单/官方已知域名渲染成可点击按钮（经 openExternal 打开），
//      绝不放行 javascript: / file: / data: 协议，也不误伤 .dll / 版本号；
//   3. 流式输出时遇到未闭合的 ``` 代码块平稳降级，不闪屏不破坏布局。
// ============================================================

import { OFFICIAL } from './official.data.js';

export function escapeHtml(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// 收集官方白名单里的已知安全域名（含主域与常用自带页面）
const SAFE_BARE_DOMAINS = (() => {
  const set = new Set([
    'chat.deepseek.com',
    'weather.com.cn',
    'github.com',
    'learn.microsoft.com',
  ]);
  for (const e of OFFICIAL.entries || []) {
    if (e.domain) set.add(String(e.domain).toLowerCase());
    for (const u of [e.primary, e.download, ...(e.alt || [])]) {
      if (!u) continue;
      try {
        set.add(new URL(u).host.toLowerCase());
      } catch {}
    }
  }
  return Array.from(set).sort((a, b) => b.length - a.length);
})();

function isKnownSafeHost(host) {
  const h = String(host || '').toLowerCase();
  return SAFE_BARE_DOMAINS.some((d) => h === d || h.endsWith(`.${d}`));
}

function linkifyInline(escaped) {
  // 1. 显式 https:// 链接（排除末尾中文标点与括号）
  const withHttps = escaped.replace(
    /https:\/\/[a-zA-Z0-9._~:/?#[\]@!$&'()*+,;=%-]+/g,
    (raw) => {
      const cleaned = raw.replace(/[.,;:!?)'"]+$/, '');
      const trail = raw.slice(cleaned.length);
      return `<button class="md-link" type="button" data-open-url="${cleaned}" title="在浏览器中打开 ${cleaned}">${cleaned}</button>${trail}`;
    }
  );

  // 2. 白名单裸域名（如 www.huorong.cn/、store.steampowered.com/about/），跳过已在 HTML 标签属性里的文本
  return withHttps.replace(
    /(<[^>]+>)|((?:www\.)?[a-zA-Z0-9-]+(?:\.[a-zA-Z0-9-]+)+(?:\/[a-zA-Z0-9._~:/?#[\]@!$&'*+,;=%-]*)?)/g,
    (full, tag, candidate) => {
      if (tag) return tag;
      const cleaned = candidate.replace(/[.,;:!?)'"]+$/, '');
      const trail = candidate.slice(cleaned.length);
      const host = cleaned.split('/')[0].toLowerCase();
      if (!isKnownSafeHost(host)) return full;
      const url = `https://${cleaned}`;
      return `<button class="md-link" type="button" data-open-url="${url}" title="在浏览器中打开 ${url}">${cleaned}</button>${trail}`;
    }
  );
}

function formatInlineText(raw) {
  // 先抽出行内代码 `...`，防止内部符号被加粗或链接化破坏
  const codeTokens = [];
  const withPlaceholders = String(raw).replace(/`([^`\n]+)`/g, (_, code) => {
    const idx = codeTokens.length;
    const esc = escapeHtml(code.trim());
    codeTokens.push(
      `<button class="md-inline-code" type="button" data-copy="${esc}" title="点击复制命令">${esc}</button>`
    );
    return `\u0000CODE${idx}\u0000`;
  });

  let html = escapeHtml(withPlaceholders);

  // 加粗 **...**
  html = html.replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>');

  // 安全链接
  html = linkifyInline(html);

  // 还原行内代码
  html = html.replace(/\u0000CODE(\d+)\u0000/g, (_, i) => codeTokens[Number(i)] || '');
  return html;
}

/**
 * 把助手回答（纯文本或轻量 Markdown）安全渲染为 HTML。
 *
 * @param {string} text
 * @returns {string}
 */
export function renderRichText(text) {
  const src = String(text ?? '').replace(/\r\n/g, '\n');
  if (!src) return '';

  // 按 ``` 代码围栏切分，兼容流式输出时尚未闭合的尾部代码块
  const parts = src.split(/```/);
  const out = [];

  for (let i = 0; i < parts.length; i++) {
    const chunk = parts[i];
    if (i % 2 === 1) {
      // 代码块内部：去掉首行语言标识（如 powershell / cmd / bash）
      const lines = chunk.split('\n');
      let body = chunk;
      if (lines.length > 1 && /^[a-zA-Z0-9_+-]{1,18}$/.test(lines[0].trim())) {
        body = lines.slice(1).join('\n');
      }
      const trimmed = body.replace(/^\n+|\n+$/g, '');
      const esc = escapeHtml(trimmed);
      out.push(
        `<div class="md-code-wrap"><pre class="md-code"><code>${esc}</code></pre>` +
        `<button class="code-copy-btn" type="button" data-copy="${esc}">复制</button></div>`
      );
    } else if (chunk) {
      out.push(formatInlineText(chunk));
    }
  }

  return out.join('');
}

// ============================================================
// 自测：node src/lib/format.js
// ============================================================

if (typeof process !== 'undefined' && process.argv?.[1]?.endsWith('format.js')) {
  let pass = 0;
  let total = 0;
  function check(name, cond, detail = '') {
    total++;
    if (cond) pass++;
    console.log(`${cond ? 'PASS' : 'FAIL'}  ${name}${cond ? '' : '  ← ' + detail}`);
  }

  const xss = renderRichText('<script>alert(1)</script> [x](javascript:alert(1)) <img src=x onerror=alert(1)>');
  check('中和原始 HTML 与脚本标签', !xss.includes('<script>') && !xss.includes('<img') && xss.includes('&lt;script&gt;'), xss);

  const official = renderRichText('火绒安全：www.huorong.cn/\n\n下载页：store.steampowered.com/about/');
  check('白名单裸域名渲染为可点 https 按钮',
    official.includes('data-open-url="https://www.huorong.cn/"') &&
    official.includes('data-open-url="https://store.steampowered.com/about/"'),
    official);

  const nonLinks = renderRichText('报错 msvcp140.dll 或版本 v0.1.0 不该变链接');
  check('DLL 与版本号不被误判为链接', !nonLinks.includes('data-open-url'), nonLinks);

  const boldAndCode = renderRichText('先按 **Win + R** 输入 `sfc /scannow` 回车');
  check('加粗与行内命令按钮正常渲染',
    boldAndCode.includes('<strong>Win + R</strong>') &&
    boldAndCode.includes('data-copy="sfc /scannow"'),
    boldAndCode);

  const fence = renderRichText('运行命令：\n```cmd\nnetsh winsock reset\n```');
  check('围栏代码块与复制按钮正常渲染',
    fence.includes('<pre class="md-code">') && fence.includes('data-copy="netsh winsock reset"'),
    fence);

  const streamingFence = renderRichText('正在输出：\n```cmd\nsfc /scan');
  check('流式未闭合代码块不崩溃',
    streamingFence.includes('<pre class="md-code">') && streamingFence.includes('sfc /scan'),
    streamingFence);

  console.log(`\n${pass}/${total} 通过`);
  if (pass !== total) process.exitCode = 1;
}
