import { api } from './lib/api.js';
import { TIERS, TIER_ORDER, GB, effectiveVram, judge } from './lib/tiers.js';
import { handle } from './lib/pipeline.js';
import { scamGuard, guideAnswer, C_DRIVE_TIPS, NET_OFFLINE_TIPS } from './lib/guide.js';
import { socialAnswer } from './lib/social.js';
import { delayFor, sleep } from './lib/delay.js';

const $ = (id) => document.getElementById(id);
const app = $('app');

let PROFILE = null;
let VERDICT = null;
let LLM_CONFIG = {
  mode: 'off',
  localBaseUrl: 'http://127.0.0.1:18090/v1',
  localModel: 'local',
  customBaseUrl: 'https://api.openai.com/v1',
  customModel: '',
  hasApiKey: false,
  apiKeyMasked: '',
  timeoutSecs: 45,
  disableThinking: true,
};

let LLM_DRAFT_MODE = 'off';
let KEY_CLEARED = false;
let CHAT_HISTORY = [];

// 对话权限模式：默认 review（请求审查），可切 auto（完全权限）。
// SKIP_FULL_PERM_WARN 仅驻留本次进程内存，重启自动恢复提醒。
let PERMISSION_MODE = 'review';
let SKIP_FULL_PERM_WARN = false;
let ACTIVE_AGENT = null;

// 流式输出状态
let STREAM_SEQ = 1;
let ACTIVE_STREAM = null;

// 模型下载源与各档下载状态
let DL_DRAFT_SOURCE = 'modelscope';
const MODEL_DL = {};

// 分层联网检索配置与对话内一键开关
let SEARCH_CONFIG = {
  mode: 'builtin',
  hasBochaKey: false,
  bochaKeyMasked: '',
  hasTavilyKey: false,
  tavilyKeyMasked: '',
  timeoutSecs: 8,
  maxResults: 4,
};
let SEARCH_DRAFT_MODE = 'builtin';
let BOCHA_KEY_CLEARED = false;
let TAVILY_KEY_CLEARED = false;
let WEB_SEARCH_ON = true;

// 多会话对话历史持久化
let CHAT_THREADS = [];
let CURRENT_THREAD_ID = null;

// 常用 OpenAI 兼容服务商一键预设
const API_PRESETS = {
  minimax: { name: 'MiniMax', baseUrl: 'https://api.minimaxi.com/v1', model: 'MiniMax-M2.7' },
  deepseek: { name: 'DeepSeek', baseUrl: 'https://api.deepseek.com/v1', model: 'deepseek-chat' },
  zhipu: { name: '智谱 GLM', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', model: 'glm-4-flash' },
  siliconflow: { name: '硅基流动', baseUrl: 'https://api.siliconflow.cn/v1', model: 'Qwen/Qwen3-8B' },
  openai: { name: 'OpenAI', baseUrl: 'https://api.openai.com/v1', model: 'gpt-4o-mini' },
};

const INITIAL_GREETING = '有什么想问的直接说。卡顿体检、C 盘占满、找正版官网、蓝屏/没声音急救都能直接查；涉及新建或删除空文件夹的操作，默认会先弹窗问你确认。';

// ══════════════════════════════════════════════
// 导航
// ══════════════════════════════════════════════

// 进入这些页时侧边栏让位
const ZEN_PAGES = new Set(['tier', 'tutorial']);

function goto(page) {
  document.querySelectorAll('.nav-item').forEach((n) =>
    n.classList.toggle('active', n.dataset.page === page));
  $('btn-gear').classList.toggle('active', page === 'settings');
  document.querySelectorAll('.page').forEach((p) =>
    p.classList.toggle('active', p.id === 'page-' + page));
  app.classList.toggle('zen', ZEN_PAGES.has(page));
  $('main').scrollTop = 0;
  if (page === 'settings') {
    refreshAuditLog();
    refreshRuntimeStatus();
  }
}

document.querySelectorAll('.nav-item').forEach((el) => {
  el.addEventListener('click', () => goto(el.dataset.page));
});

$('btn-gear').onclick = () => goto('settings');
$('btn-settings-back').onclick = () => goto('home');
$('btn-tier-back').onclick = () => goto('home');
$('btn-tut-back').onclick = () => goto('tier');
$('btn-refresh').onclick = () => collect();
$('btn-chat-settings').onclick = () => goto('settings');
$('btn-chat-clear').onclick = () => startNewThread();
$('btn-chat-history').onclick = () => toggleHistoryDrawer();
$('btn-open-deepseek').onclick = () => api.openExternal('https://chat.deepseek.com');
$('btn-tut-to-settings').onclick = () => goto('settings');
$('btn-tier-source').onclick = () => {
  goto('settings');
  setTimeout(() => {
    const el = $('setting-download-source');
    if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
  }, 50);
};
$('btn-tier-import').onclick = async () => {
  try {
    const picked = await api.downloadPickGguf();
    if (!picked) return;
    const saved = await api.downloadImportGguf(picked);
    renderImportedModelUI(saved.importedModelPath || '');
    setImportStatus(`已导入：${saved.importedModelPath}`, 'ok');
    refreshRuntimeStatus();
    goto('settings');
    setTimeout(() => {
      const el = $('setting-download-source');
      if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }, 50);
  } catch (e) {
    alert(`导入失败：${e.message || e}`);
  }
};

// ══════════════════════════════════════════════
// 主题
// ══════════════════════════════════════════════

const THEME_KEY = 'theme';

function applyTheme(name) {
  const theme = name === 'dark' ? 'dark' : 'light';
  document.documentElement.setAttribute('data-theme', theme);
  document.querySelectorAll('#theme-switch button').forEach((b) =>
    b.classList.toggle('on', b.dataset.themeVal === theme));
  try { localStorage.setItem(THEME_KEY, theme); } catch {}
}

function initTheme() {
  let saved = null;
  try { saved = localStorage.getItem(THEME_KEY); } catch {}
  applyTheme(saved || 'light');       // 默认浅色

  document.querySelectorAll('#theme-switch button').forEach((b) => {
    b.onclick = () => applyTheme(b.dataset.themeVal);
  });
}

// ══════════════════════════════════════════════
// 模型与自定义 OpenAI API 设置
// ══════════════════════════════════════════════

function applyLlmModeUI(mode) {
  LLM_DRAFT_MODE = mode === 'local' || mode === 'custom' ? mode : 'off';
  document.querySelectorAll('#llm-mode-switch button').forEach((b) =>
    b.classList.toggle('on', b.dataset.llmMode === LLM_DRAFT_MODE));
  $('llm-panel-off').hidden = LLM_DRAFT_MODE !== 'off';
  $('llm-panel-local').hidden = LLM_DRAFT_MODE !== 'local';
  $('llm-panel-custom').hidden = LLM_DRAFT_MODE !== 'custom';
  $('btn-llm-test').disabled = LLM_DRAFT_MODE === 'off';
}

function renderLlmForm(cfg) {
  LLM_CONFIG = cfg;
  KEY_CLEARED = false;

  $('llm-local-url').value = cfg.localBaseUrl || 'http://127.0.0.1:18090/v1';
  $('llm-local-model').value = cfg.localModel || 'local';
  $('llm-disable-thinking').checked = cfg.disableThinking !== false;

  $('llm-custom-url').value = cfg.customBaseUrl || 'https://api.openai.com/v1';
  $('llm-custom-model').value = cfg.customModel || '';

  const keyInput = $('llm-custom-key');
  keyInput.value = '';
  keyInput.placeholder = cfg.hasApiKey
    ? `已保存：${cfg.apiKeyMasked}（留空保持不变）`
    : 'sk-...';
  $('btn-clear-key').hidden = !cfg.hasApiKey;

  applyLlmModeUI(cfg.mode);
  renderChatEngine();
}

function readLlmFormInput() {
  const typedKey = $('llm-custom-key').value.trim();
  let customApiKey = null;
  if (typedKey) {
    customApiKey = typedKey;
  } else if (KEY_CLEARED) {
    customApiKey = '';
  }

  return {
    mode: LLM_DRAFT_MODE,
    localBaseUrl: $('llm-local-url').value.trim() || 'http://127.0.0.1:18090/v1',
    localModel: $('llm-local-model').value.trim() || 'local',
    customBaseUrl: $('llm-custom-url').value.trim(),
    customApiKey,
    customModel: $('llm-custom-model').value.trim(),
    timeoutSecs: LLM_CONFIG.timeoutSecs || 45,
    disableThinking: $('llm-disable-thinking').checked,
  };
}

function setLlmStatus(text, kind = '') {
  const el = $('llm-status');
  if (!text) {
    el.hidden = true;
    el.textContent = '';
    el.className = 'status-line';
    return;
  }
  el.hidden = false;
  el.textContent = text;
  el.className = kind ? `status-line ${kind}` : 'status-line';
}

function renderChatEngine() {
  const dot = $('chat-dot');
  const label = $('chat-engine-label');
  if (LLM_CONFIG.mode === 'local') {
    dot.classList.add('on');
    label.textContent = `本地模型 · ${LLM_CONFIG.localModel || 'local'}`;
  } else if (LLM_CONFIG.mode === 'custom') {
    dot.classList.add('on');
    label.textContent = `自定义 API · ${LLM_CONFIG.customModel || '未填模型名'}`;
  } else {
    dot.classList.remove('on');
    label.textContent = '仅内置规则（未接模型）';
  }
}

async function initLlmSettings() {
  document.querySelectorAll('#llm-mode-switch button').forEach((b) => {
    b.onclick = () => {
      applyLlmModeUI(b.dataset.llmMode);
      setLlmStatus('');
    };
  });

  $('btn-clear-key').onclick = () => {
    KEY_CLEARED = true;
    $('llm-custom-key').value = '';
    $('llm-custom-key').placeholder = '已标记清空，点「保存设置」生效';
    $('btn-clear-key').hidden = true;
    setLlmStatus('已标记清空 API Key，点击「保存设置」后生效。');
  };

  $('llm-custom-key').addEventListener('input', () => {
    if ($('llm-custom-key').value.trim()) KEY_CLEARED = false;
  });

  document.querySelectorAll('#api-preset-row [data-preset]').forEach((btn) => {
    btn.onclick = () => {
      const p = API_PRESETS[btn.dataset.preset];
      if (!p) return;
      $('llm-custom-url').value = p.baseUrl;
      $('llm-custom-model').value = p.model;
      document.querySelectorAll('#api-preset-row [data-preset]').forEach((x) =>
        x.classList.toggle('on', x === btn));
      setLlmStatus(`已填入 ${p.name} 默认地址与模型名，补上 API Key 后点「测试连接」或「保存设置」。`);
    };
  });

  $('btn-llm-save').onclick = async () => {
    const input = readLlmFormInput();
    if (input.mode === 'custom') {
      if (!input.customBaseUrl) {
        setLlmStatus('请先填写 Base URL。', 'err');
        return;
      }
      if (!input.customModel) {
        setLlmStatus('请先填写模型名。', 'err');
        return;
      }
    }

    $('btn-llm-save').disabled = true;
    try {
      const saved = await api.llmConfigSave(input);
      renderLlmForm(saved);
      setLlmStatus('设置已保存。', 'ok');
    } catch (e) {
      setLlmStatus(`保存失败：${e.message || e}`, 'err');
    } finally {
      $('btn-llm-save').disabled = false;
    }
  };

  $('btn-llm-test').onclick = async () => {
    const input = readLlmFormInput();
    $('btn-llm-test').disabled = true;
    setLlmStatus('正在测试连接…');
    try {
      const res = await api.llmTest(input);
      if (res && res.ok) {
        setLlmStatus(res.message || '连接正常。', 'ok');
      } else {
        setLlmStatus((res && res.error) || '连接失败。', 'err');
      }
    } catch (e) {
      setLlmStatus(`测试失败：${e.message || e}`, 'err');
    } finally {
      $('btn-llm-test').disabled = LLM_DRAFT_MODE === 'off';
    }
  };

  try {
    const cfg = await api.llmConfigGet();
    if (cfg) renderLlmForm(cfg);
  } catch {
    renderChatEngine();
  }
}

// ══════════════════════════════════════════════
// 采集
// ══════════════════════════════════════════════

async function collect() {
  $('home-loading').hidden = false;
  $('home-body').hidden = true;

  let res;
  try {
    res = await api.collect();
  } catch (e) {
    res = { ok: false, error: String(e) };
  }

  if (!res || !res.ok) {
    $('home-loading').innerHTML =
      `<p style="color:var(--dim);max-width:320px;text-align:center;line-height:1.8">
         读不到配置。先按最低档试，能用再往上换。
       </p>
       <button class="btn ghost sm" onclick="location.reload()">重试</button>`;
    return;
  }

  PROFILE = res.profile;
  VERDICT = judge(PROFILE);
  renderHome();
  renderTiers();
  renderSpecs();
  renderInfo();       // 采集完补上系统信息与检测时间
}

// ══════════════════════════════════════════════
// 首页
// ══════════════════════════════════════════════

function renderHome() {
  $('home-loading').hidden = true;
  $('home-body').hidden = false;

  $('hero-title').innerHTML = VERDICT.title;
  $('hero-line').textContent = VERDICT.line;

  const isMinimal = VERDICT.tier === 'minimal';
  $('btn-start').textContent = isMinimal ? '看网页版怎么开' : '选模型';
  $('btn-start').onclick = () => goto(isMinimal ? 'tutorial' : 'tier');
}

// ══════════════════════════════════════════════
// 模型下载源设置与进度追踪
// ══════════════════════════════════════════════

function applyDownloadSourceUI(source) {
  DL_DRAFT_SOURCE = source === 'hf_mirror' || source === 'custom' ? source : 'modelscope';
  document.querySelectorAll('#dl-source-switch button').forEach((b) =>
    b.classList.toggle('on', b.dataset.dlSource === DL_DRAFT_SOURCE));
  $('dl-panel-custom').hidden = DL_DRAFT_SOURCE !== 'custom';
}

function setDlStatus(text, kind = '') {
  const el = $('dl-status');
  if (!text) {
    el.hidden = true;
    el.textContent = '';
    el.className = 'status-line';
    return;
  }
  el.hidden = false;
  el.textContent = text;
  el.className = kind ? `status-line ${kind}` : 'status-line';
}

async function initDownloadSettings() {
  document.querySelectorAll('#dl-source-switch button').forEach((b) => {
    b.onclick = () => {
      applyDownloadSourceUI(b.dataset.dlSource);
      setDlStatus('');
    };
  });

  $('btn-dl-save').onclick = async () => {
    const cfg = {
      source: DL_DRAFT_SOURCE,
      customBaseUrl: $('dl-custom-url').value.trim(),
    };
    $('btn-dl-save').disabled = true;
    try {
      const saved = await api.downloadConfigSave(cfg);
      applyDownloadSourceUI(saved.source);
      $('dl-custom-url').value = saved.customBaseUrl || '';
      setDlStatus('下载源设置已保存。', 'ok');
    } catch (e) {
      setDlStatus(`保存失败：${e.message || e}`, 'err');
    } finally {
      $('btn-dl-save').disabled = false;
    }
  };

  try {
    const cfg = await api.downloadConfigGet();
    if (cfg) {
      applyDownloadSourceUI(cfg.source);
      $('dl-custom-url').value = cfg.customBaseUrl || '';
      renderImportedModelUI(cfg.importedModelPath || '');
    }
  } catch {
    applyDownloadSourceUI('modelscope');
  }

  $('btn-import-pick').onclick = async () => {
    try {
      const picked = await api.downloadPickGguf();
      if (picked) $('dl-import-path').value = picked;
    } catch (e) {
      setImportStatus(`打开选择框失败：${e.message || e}`, 'err');
    }
  };

  $('btn-import-apply').onclick = async () => {
    const p = $('dl-import-path').value.trim();
    if (!p) {
      setImportStatus('请先选择或输入 .gguf 文件路径。', 'err');
      return;
    }
    try {
      const saved = await api.downloadImportGguf(p);
      renderImportedModelUI(saved.importedModelPath || '');
      setImportStatus('已导入本地模型，可直接点上方「一键启动」使用。', 'ok');
      refreshRuntimeStatus();
    } catch (e) {
      setImportStatus(`导入失败：${e.message || e}`, 'err');
    }
  };

  $('btn-import-clear').onclick = async () => {
    try {
      await api.downloadClearImported();
      renderImportedModelUI('');
      setImportStatus('已清除导入记录（不会删除原文件）。');
      refreshRuntimeStatus();
    } catch (e) {
      setImportStatus(`清除失败：${e.message || e}`, 'err');
    }
  };
}

function renderImportedModelUI(path) {
  const input = $('dl-import-path');
  const clearBtn = $('btn-import-clear');
  if (input) input.value = path || '';
  if (clearBtn) clearBtn.hidden = !path;
}

function setImportStatus(text, kind = '') {
  const el = $('import-status');
  if (!el) return;
  if (!text) {
    el.hidden = true;
    el.textContent = '';
    el.className = 'status-line';
    return;
  }
  el.hidden = false;
  el.textContent = text;
  el.className = kind ? `status-line ${kind}` : 'status-line';
}

// ── 目录授权白名单设置（P2-a）────────────────
let FS_POLICY_DIRS = [];

function renderFsPolicyTags() {
  const box = $('fs-allow-tags');
  if (!box) return;
  if (!FS_POLICY_DIRS.length) {
    box.innerHTML = '<span class="llm-note">暂未添加授权目录</span>';
    return;
  }
  box.innerHTML = FS_POLICY_DIRS.map((d, idx) =>
    `<span class="dir-tag"><span>${escapeHtml(d)}</span><button type="button" data-rm-dir="${idx}" title="移除">×</button></span>`
  ).join('');
  box.querySelectorAll('[data-rm-dir]').forEach((b) => {
    b.onclick = () => {
      FS_POLICY_DIRS.splice(Number(b.dataset.rmDir), 1);
      renderFsPolicyTags();
    };
  });
}

function setFsPolicyStatus(text, kind = '') {
  const el = $('fs-policy-status');
  if (!el) return;
  if (!text) {
    el.hidden = true;
    el.textContent = '';
    el.className = 'status-line';
    return;
  }
  el.hidden = false;
  el.textContent = text;
  el.className = kind ? `status-line ${kind}` : 'status-line';
}

async function initFsPolicySettings() {
  try {
    const p = await api.fsPolicyGet();
    if (p) {
      $('chk-fs-restrict').checked = !!p.restrictToAllowed;
      FS_POLICY_DIRS = Array.isArray(p.allowedDirs) ? [...p.allowedDirs] : [];
    }
  } catch {}
  renderFsPolicyTags();

  const addDir = (raw) => {
    const s = String(raw || '').trim().replace(/^"|"$/g, '').replace(/[\\/]+$/, '');
    if (!s) return;
    if (!FS_POLICY_DIRS.some((x) => x.toLowerCase() === s.toLowerCase())) {
      FS_POLICY_DIRS.push(s);
      renderFsPolicyTags();
    }
    $('fs-allow-input').value = '';
    setFsPolicyStatus('');
  };

  $('btn-fs-pick').onclick = async () => {
    try {
      const picked = await api.fsPickFolder();
      if (picked) addDir(picked);
    } catch (e) {
      setFsPolicyStatus(`浏览失败：${e.message || e}`, 'err');
    }
  };

  $('btn-fs-add').onclick = () => addDir($('fs-allow-input').value);

  $('btn-fs-save').onclick = async () => {
    const btn = $('btn-fs-save');
    btn.disabled = true;
    try {
      const saved = await api.fsPolicySave({
        restrictToAllowed: $('chk-fs-restrict').checked,
        allowedDirs: FS_POLICY_DIRS,
      });
      $('chk-fs-restrict').checked = !!saved.restrictToAllowed;
      FS_POLICY_DIRS = Array.isArray(saved.allowedDirs) ? [...saved.allowedDirs] : [];
      renderFsPolicyTags();
      setFsPolicyStatus('目录授权白名单已保存。', 'ok');
    } catch (e) {
      setFsPolicyStatus(`保存失败：${e.message || e}`, 'err');
    } finally {
      btn.disabled = false;
    }
  };
}

function formatBytes(bytes) {
  const b = Number(bytes || 0);
  if (b >= GB) return `${(b / GB).toFixed(2)} GB`;
  if (b >= 1024 * 1024) return `${(b / (1024 * 1024)).toFixed(1)} MB`;
  if (b >= 1024) return `${Math.round(b / 1024)} KB`;
  return `${b} B`;
}

function formatSpeed(bps) {
  const v = Number(bps || 0);
  if (v <= 0) return '连接中…';
  if (v >= 1024 * 1024) return `${(v / (1024 * 1024)).toFixed(1)} MB/s`;
  if (v >= 1024) return `${Math.round(v / 1024)} KB/s`;
  return `${Math.round(v)} B/s`;
}

async function refreshModelStatuses() {
  try {
    const list = await api.downloadListStatus();
    for (const item of list || []) {
      const prev = MODEL_DL[item.tier] || {};
      MODEL_DL[item.tier] = {
        ...prev,
        status: item.downloaded ? 'done' : item.active ? 'downloading' : (item.partBytes > 0 ? 'paused' : 'idle'),
        downloadedBytes: item.downloaded ? item.totalBytes : item.partBytes,
        totalBytes: item.totalBytes,
        speedBps: prev.speedBps || 0,
        percent: item.downloaded ? 100 : (item.totalBytes ? (item.partBytes / item.totalBytes) * 100 : 0),
        filePath: item.filePath || prev.filePath || '',
      };
      updateTierCardDownloadUI(item.tier);
    }
  } catch {}
}

async function initDownloadEvents() {
  await api.onModelDownload((ev) => {
    if (!ev || !ev.tier) return;
    const prev = MODEL_DL[ev.tier] || {};
    MODEL_DL[ev.tier] = {
      ...prev,
      status: ev.status || prev.status || 'idle',
      downloadedBytes: ev.downloadedBytes ?? prev.downloadedBytes ?? 0,
      totalBytes: ev.totalBytes ?? prev.totalBytes ?? 0,
      speedBps: ev.speedBps ?? 0,
      percent: ev.percent ?? prev.percent ?? 0,
      error: ev.error || '',
      filePath: ev.filePath || prev.filePath || '',
    };
    updateTierCardDownloadUI(ev.tier);
  });
}

function updateTierCardDownloadUI(tier) {
  const card = document.querySelector(`.tier-card[data-tier-card="${tier}"]`);
  if (!card) return;
  const st = MODEL_DL[tier];
  const btn = card.querySelector('[data-act="download"]');
  const box = card.querySelector('.tc-dl');
  const fill = card.querySelector('.dl-fill');
  const meta = card.querySelector('.dl-meta');
  if (!st || !btn || !box || !fill || !meta) return;

  const isOver = card.classList.contains('over');
  const isCur = card.classList.contains('current');

  if (st.status === 'done') {
    btn.textContent = '去对话';
    btn.disabled = false;
    btn.classList.remove('ghost');
    btn.classList.add('primary');
    box.hidden = false;
    fill.style.width = '100%';
    fill.classList.add('done');
    meta.className = 'dl-meta ok';
    meta.textContent = '模型文件已就绪';
  } else {
    btn.classList.toggle('primary', isCur);
    btn.classList.toggle('ghost', !isCur);
    if (st.status === 'downloading') {
      btn.textContent = '暂停下载';
      btn.disabled = false;
      box.hidden = false;
      const pct = Math.max(0, Math.min(99.9, Number(st.percent || 0)));
      fill.style.width = `${pct.toFixed(1)}%`;
      fill.classList.remove('done');
      meta.className = 'dl-meta';
      meta.textContent = `${formatSpeed(st.speedBps)} · ${formatBytes(st.downloadedBytes)} / ${formatBytes(st.totalBytes)} (${pct.toFixed(0)}%)`;
    } else if (st.status === 'paused') {
      btn.textContent = '继续下载';
      btn.disabled = false;
      box.hidden = false;
      const pct = Math.max(0, Math.min(99.9, Number(st.percent || 0)));
      fill.style.width = `${pct.toFixed(1)}%`;
      fill.classList.remove('done');
      meta.className = 'dl-meta';
      meta.textContent = `已暂停 · ${formatBytes(st.downloadedBytes)} / ${formatBytes(st.totalBytes)} (${pct.toFixed(0)}%)`;
    } else if (st.status === 'error') {
      btn.textContent = '重试下载';
      btn.disabled = false;
      box.hidden = false;
      fill.classList.remove('done');
      meta.className = 'dl-meta err';
      meta.textContent = st.error || '下载失败，可点重试或在设置里切换下载源';
    } else {
      btn.textContent = isOver ? '仍要下载' : '下载';
      btn.disabled = false;
      box.hidden = true;
    }
  }
}

// ══════════════════════════════════════════════
// 档位：五列横排
// ══════════════════════════════════════════════

function renderTiers() {
  const vram = effectiveVram(PROFILE.gpus);

  $('tier-cards').innerHTML = TIER_ORDER.map((key) => {
    const t = TIERS[key];
    const isCur = key === VERDICT.tier;
    const over = vram !== null && t.vramNeed > 0 && vram < t.vramNeed * GB;

    const label = t.action === 'tutorial' ? '看怎么开'
                : over ? '仍要下载' : '下载';

    return `<article class="tier-card ${isCur ? 'current' : ''} ${over ? 'over' : ''}" data-tier-card="${key}">
      <div class="tc-level">${t.level}</div>
      <div class="tc-name">${t.name}</div>
      <div class="tc-size">${t.size}</div>
      <p class="tc-desc">${t.desc}</p>
      <div class="tc-fits">
        <div class="yes">${t.fits.join('、')}</div>
        ${t.notFit[0] !== '—' ? `<div class="no">${t.notFit.join('、')}</div>` : ''}
      </div>
      <div class="tc-action">
        <button class="btn ${isCur ? 'primary' : 'ghost'} sm block"
                data-act="${t.action}" data-tier="${key}">${label}</button>
      </div>
      <div class="tc-dl" hidden>
        <div class="dl-bar"><div class="dl-fill"></div></div>
        <div class="dl-meta"></div>
      </div>
    </article>`;
  }).join('');

  $('tier-cards').querySelectorAll('[data-act]').forEach((b) => {
    b.onclick = () => {
      if (b.dataset.act === 'tutorial') goto('tutorial');
      else onDownload(b.dataset.tier);
    };
  });

  for (const key of TIER_ORDER) {
    if (MODEL_DL[key]) updateTierCardDownloadUI(key);
  }
}

async function onDownload(key) {
  const t = TIERS[key];
  const st = MODEL_DL[key];

  if (st && st.status === 'downloading') {
    await api.downloadModelCancel(key);
    return;
  }
  if (st && st.status === 'done') {
    const card = document.querySelector(`.tier-card[data-tier-card="${key}"]`);
    const btn = card?.querySelector('[data-act="download"]');
    const meta = card?.querySelector('.dl-meta');
    if (btn) {
      btn.disabled = true;
      btn.textContent = '正在启动模型…';
    }
    if (meta) {
      meta.className = 'dl-meta';
      meta.textContent = '正在拉起内置推理引擎，首次加载需几秒…';
    }
    try {
      // 自动切到本地模型模式，免去手动去设置页改
      if (LLM_CONFIG.mode === 'off') {
        const saved = await api.llmConfigSave({
          mode: 'local',
          localBaseUrl: LLM_CONFIG.localBaseUrl || 'http://127.0.0.1:18090/v1',
          localModel: t.name || 'local',
          customBaseUrl: LLM_CONFIG.customBaseUrl || 'https://api.openai.com/v1',
          customApiKey: null,
          customModel: LLM_CONFIG.customModel || '',
          timeoutSecs: LLM_CONFIG.timeoutSecs || 45,
          disableThinking: LLM_CONFIG.disableThinking !== false,
        });
        renderLlmForm(saved);
      }
      await api.runtimeStart(key);
      await refreshRuntimeStatus();
      updateTierCardDownloadUI(key);
      goto('chat');
    } catch (e) {
      updateTierCardDownloadUI(key);
      if (meta) {
        meta.className = 'dl-meta err';
        meta.textContent = `启动失败：${e.message || e}`;
      }
    }
    return;
  }

  const vram = effectiveVram(PROFILE.gpus);
  const over = t.vramNeed > 0 && vram !== null && vram < t.vramNeed * GB;
  if (over && (!st || st.status === 'idle')) {
    const ok = confirm(`${t.name}（约 ${t.size}）超出当前可用显存，跑起来会偏慢。\n\n仍然要下载吗？`);
    if (!ok) return;
  }

  MODEL_DL[key] = {
    ...(st || {}),
    status: 'downloading',
    speedBps: 0,
    error: '',
  };
  updateTierCardDownloadUI(key);

  try {
    await api.downloadModelStart(key);
  } catch (e) {
    MODEL_DL[key] = {
      ...(MODEL_DL[key] || {}),
      status: 'error',
      error: e.message || String(e),
    };
    updateTierCardDownloadUI(key);
  }
}

// ══════════════════════════════════════════════
// 配置详情
// ══════════════════════════════════════════════

function renderSpecs() {
  const p = PROFILE;
  const cpu = p.cpu;
  const mem = p.memory;
  const gpu = (p.gpus || []).find((g) => g.vendor === 'nvidia' || g.vendor === 'amd');
  const disk = (p.disks || []).slice().sort((a, b) => (b.freeBytes || 0) - (a.freeBytes || 0))[0];
  const vram = effectiveVram(p.gpus);

  const rows = [
    {
      k: '处理器',
      v: cpu ? cpu.name.replace(/\(R\)|\(TM\)|\s*CPU\s*|@\s*[\d.]+GHz/gi, '').trim() : '未知',
      n: cpu ? `${cpu.physicalCores} 核 · ${cpu.baseMhz}MHz` : '',
    },
    {
      k: '内存',
      v: mem ? `${(mem.totalBytes / GB).toFixed(1)} GB` : '未知',
      n: mem ? `${mem.moduleCount || '?'} 条 · ${mem.moduleSpeedMhz || '?'}MHz` : '',
    },
    {
      k: '显卡',
      v: gpu ? gpu.name.replace(/NVIDIA |AMD |Radeon /g, '').trim() : '集成显卡',
      n: vram ? `可用显存 ${(vram / GB).toFixed(1)} GB` : (gpu ? '显存未知' : '无独立显卡'),
    },
    {
      k: '硬盘',
      v: disk ? `${disk.letter} 剩 ${(disk.freeBytes / GB).toFixed(1)} GB` : '未知',
      n: disk ? `共 ${(disk.totalBytes / GB).toFixed(1)} GB` : '',
    },
    {
      k: '系统',
      v: p.environment ? (p.environment.isWin11 ? 'Windows 11' : 'Windows 10') : '未知',
      n: p.environment ? `内部版本 ${p.environment.build}` : '',
    },
    { k: '采集耗时', v: `${p.elapsedMs} ms`, n: '' },
  ];

  $('spec-grid').innerHTML = rows.map((r) => `
    <div class="spec">
      <div class="k">${r.k}</div>
      <div class="v">${escapeHtml(r.v)}</div>
      ${r.n ? `<div class="n">${escapeHtml(r.n)}</div>` : ''}
    </div>`).join('');
}

function buildHardwareSummary() {
  if (!PROFILE) return null;
  const gb = (n) => (n / GB).toFixed(1);
  const lines = [];
  if (PROFILE.cpu) {
    lines.push(`CPU：${PROFILE.cpu.name.trim()}（${PROFILE.cpu.physicalCores} 核）`);
  }
  if (PROFILE.memory) {
    lines.push(`内存：共 ${gb(PROFILE.memory.totalBytes)}GB，空闲 ${gb(PROFILE.memory.freeBytes || 0)}GB`);
  }
  const gpu = (PROFILE.gpus || []).find((g) => g.vendor === 'nvidia' || g.vendor === 'amd');
  const vram = effectiveVram(PROFILE.gpus);
  if (gpu) {
    lines.push(`显卡：${gpu.name}${vram ? `（可用显存 ${gb(vram)}GB）` : ''}`);
  } else {
    lines.push('显卡：集成显卡');
  }
  for (const d of PROFILE.disks || []) {
    lines.push(`磁盘 ${d.letter}：剩余 ${gb(d.freeBytes || 0)}GB / 共 ${gb(d.totalBytes || 0)}GB`);
  }
  return lines.join('\n');
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// ══════════════════════════════════════════════
// 对话
// ══════════════════════════════════════════════

$('btn-send').onclick = send;
$('chat-text').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') send();
});

// 当前进行中的安装流程。同一时间只允许一个。
let ACTIVE_FLOW = null;

// ══════════════════════════════════════════════
// 权限档位与审查弹窗
// ══════════════════════════════════════════════

function applyPermissionUI() {
  document.querySelectorAll('#perm-switch button').forEach((b) =>
    b.classList.toggle('on', b.dataset.perm === PERMISSION_MODE));
  const hint = $('perm-hint');
  if (PERMISSION_MODE === 'auto') {
    hint.textContent = '完全权限：建/删空目录将自动执行（系统目录仍锁定，重启恢复审查）';
    hint.classList.add('warn');
  } else {
    hint.textContent = '默认只看不改，写或删空目录前会先弹一步确认';
    hint.classList.remove('warn');
  }
}

function initPermissionControls() {
  applyPermissionUI();

  document.querySelectorAll('#perm-switch button').forEach((b) => {
    b.onclick = () => {
      const target = b.dataset.perm;
      if (target === PERMISSION_MODE) return;
      if (target === 'review') {
        PERMISSION_MODE = 'review';
        applyPermissionUI();
        return;
      }
      if (SKIP_FULL_PERM_WARN) {
        PERMISSION_MODE = 'auto';
        applyPermissionUI();
        return;
      }
      $('chk-skip-fp-warn').checked = false;
      $('modal-full-perm').hidden = false;
    };
  });

  $('btn-fp-cancel').onclick = () => {
    $('modal-full-perm').hidden = true;
  };

  $('btn-fp-confirm').onclick = () => {
    if ($('chk-skip-fp-warn').checked) {
      SKIP_FULL_PERM_WARN = true;
    }
    PERMISSION_MODE = 'auto';
    $('modal-full-perm').hidden = true;
    applyPermissionUI();
  };

  $('btn-rv-approve').onclick = () => handleAgentDecision('approve');
  $('btn-rv-skip').onclick = () => handleAgentDecision('skip');
  $('btn-rv-cancel').onclick = () => handleAgentDecision('cancel');
}

function openReviewModal(session) {
  const step = (session.steps || []).find((s) => s.status === 'waiting_confirm');
  if (!step) {
    $('modal-review').hidden = true;
    return;
  }

  const badge = $('modal-rv-badge');
  badge.textContent = step.riskLabel || (step.risk === 'dangerous' ? '不可逆' : '可撤销');
  badge.className = `risk-badge ${step.risk === 'dangerous' ? 'dangerous' : ''}`;

  $('modal-rv-meta').textContent = `待办第 ${step.id} / ${session.steps.length} 步 · 能力 ${step.capabilityId}`;
  $('modal-rv-action').textContent = step.title;

  const targetEl = $('modal-rv-target');
  const path = step.args && step.args.path;
  if (path) {
    targetEl.textContent = `目标路径：${path}`;
    targetEl.hidden = false;
  } else if (step.args && Object.keys(step.args).length) {
    targetEl.textContent = `参数：${JSON.stringify(step.args)}`;
    targetEl.hidden = false;
  } else {
    targetEl.hidden = true;
  }

  const consEl = $('modal-rv-consequence');
  if (consEl) {
    if (step.consequence) {
      consEl.textContent = step.consequence;
      consEl.className = `modal-consequence${step.risk === 'dangerous' ? ' dangerous' : ''}`;
      consEl.hidden = false;
    } else {
      consEl.hidden = true;
    }
  }

  $('btn-rv-approve').disabled = false;
  $('btn-rv-skip').disabled = false;
  $('btn-rv-cancel').disabled = false;
  $('modal-review').hidden = false;
}

function stepStatusIcon(status) {
  const map = {
    done: '✓',
    waiting_confirm: '◉',
    failed: '✕',
    skipped: '—',
    pending: '○',
  };
  return map[status] || '○';
}

function buildTodoCardHtml(session) {
  const items = (session.steps || []).map((s) => {
    const icon = stepStatusIcon(s.status);
    const tagCls = s.risk === 'dangerous' ? 'dangerous' : s.risk === 'reversible' ? 'reversible' : '';
    const tagHtml = s.risk !== 'read_only'
      ? `<span class="todo-tag ${tagCls}">${escapeHtml(s.riskLabel || '')}</span>`
      : '';
    let subHtml = '';
    if (s.error) {
      subHtml = `<div class="todo-sub err">${escapeHtml(s.error)}</div>`;
    } else if (s.output) {
      subHtml = `<div class="todo-sub">${escapeHtml(s.output)}</div>`;
    } else if (s.status === 'waiting_confirm') {
      const target = s.args?.path ? ` · ${s.args.path}` : '';
      subHtml = `<div class="todo-sub">等待审查确认${escapeHtml(target)}</div>`;
    }
    return `<div class="todo-item ${escapeHtml(s.status)}">
      <span class="todo-icon ${escapeHtml(s.status)}">${icon}</span>
      <div class="todo-body">
        <div class="todo-title">${escapeHtml(s.title)}</div>
        ${subHtml}
      </div>
      ${tagHtml}
    </div>`;
  }).join('');

  return `<div class="todo-card">${items}</div>`;
}

function buildThinkBoxHtml(thinking, isStreaming = false, keepOpen = false) {
  const text = String(thinking || '').trim();
  if (!text) return '';
  const title = isStreaming ? '正在思考…' : '查看思考过程';
  return `<details class="think-box"${keepOpen ? ' open' : ''}>
    <summary>${title}</summary>
    <div class="think-body">${escapeHtml(text)}</div>
  </details>`;
}

async function initStreamEvents() {
  await api.onLlmStream((ev) => {
    if (!ACTIVE_STREAM || !ev || ev.streamId !== ACTIVE_STREAM.id) return;
    const st = ACTIVE_STREAM;
    if (ev.kind === 'thinking') {
      st.thinking += ev.delta || '';
    } else if (ev.kind === 'content') {
      st.content += ev.delta || '';
    } else {
      return;
    }

    if (st.typingEl) {
      st.typingEl.remove();
      st.typingEl = null;
    }

    const log = $('chat-log');
    if (!st.div) {
      st.div = document.createElement('div');
      st.div.className = 'msg bot';
      log.appendChild(st.div);
    }

    const wasOpen = !!st.div.querySelector('.think-box')?.open;
    const trimmed = st.content.trimStart();
    const looksLikePlan =
      trimmed.startsWith('{') ||
      trimmed.startsWith('```') ||
      trimmed.startsWith('<tool_call>');

    const thinkHtml = buildThinkBoxHtml(st.thinking, !st.content, wasOpen);
    let bodyHtml = '';
    if (looksLikePlan) {
      bodyHtml = `<div class="stream-hint">正在整理待办步骤…</div>`;
    } else if (st.content) {
      bodyHtml = `<div class="bubble-text">${escapeHtml(st.content)}</div>`;
    } else {
      bodyHtml = `<div class="stream-hint">思考中…</div>`;
    }

    st.div.innerHTML = `<div class="bubble">${thinkHtml}${bodyHtml}</div>`;
    log.scrollTop = log.scrollHeight;
  });
}

function renderAgentMsg(div, session, meta, thinking = '') {
  const wasOpen = !!div.querySelector('.think-box')?.open;
  const thinkHtml = buildThinkBoxHtml(thinking, false, wasOpen);
  const summary = session.summary || '正在处理待办任务…';
  const waiting = session.status === 'waiting_confirm';
  const reopenBtn = waiting
    ? `<div class="msg-actions"><button class="btn primary sm" data-agent-review="1">审查这一步</button></div>`
    : '';
  const tag = meta ? `<div class="meta">${escapeHtml(meta)}</div>` : '';

  div.innerHTML = `<div class="bubble">${thinkHtml}${escapeHtml(summary)}${buildTodoCardHtml(session)}</div>${reopenBtn}${tag}`;
  const btn = div.querySelector('[data-agent-review]');
  if (btn) {
    btn.onclick = () => {
      if (ACTIVE_AGENT && ACTIVE_AGENT.session) {
        openReviewModal(ACTIVE_AGENT.session);
      }
    };
  }
  const log = $('chat-log');
  log.scrollTop = log.scrollHeight;
}

async function handleAgentDecision(decision) {
  if (!ACTIVE_AGENT || !ACTIVE_AGENT.session) {
    $('modal-review').hidden = true;
    return;
  }

  $('btn-rv-approve').disabled = true;
  $('btn-rv-skip').disabled = true;
  $('btn-rv-cancel').disabled = true;

  let next;
  try {
    next = await api.agentDecide(ACTIVE_AGENT.session, decision, PERMISSION_MODE);
  } catch (e) {
    $('modal-review').hidden = true;
    addMsg('bot', `执行审查动作失败：${e.message || e}`, 'error');
    ACTIVE_AGENT = null;
    return;
  }

  ACTIVE_AGENT.session = next;
  renderAgentMsg(ACTIVE_AGENT.div, next, ACTIVE_AGENT.meta, ACTIVE_AGENT.thinking || '');
  updateLastThreadAgentSession(next);

  if (next.status === 'waiting_confirm') {
    openReviewModal(next);
  } else {
    $('modal-review').hidden = true;
    if (next.summary) {
      CHAT_HISTORY.push({ role: 'assistant', content: next.summary });
      if (CHAT_HISTORY.length > 12) CHAT_HISTORY = CHAT_HISTORY.slice(-12);
    }
    ACTIVE_AGENT = null;
  }
}

function createEmptyThread() {
  return {
    id: `th-${Date.now()}-${Math.floor(Math.random() * 1000)}`,
    title: '新对话',
    updatedAt: Date.now(),
    messages: [],
  };
}

function getCurrentThread() {
  let th = CHAT_THREADS.find((t) => t.id === CURRENT_THREAD_ID);
  if (!th) {
    th = createEmptyThread();
    CHAT_THREADS.unshift(th);
    CURRENT_THREAD_ID = th.id;
  }
  return th;
}

function persistThreads() {
  // 只保存真正发过消息的会话，避免一堆空「新对话」污染历史列表
  const toSave = CHAT_THREADS.filter((t) => t.messages && t.messages.length > 0);
  api.historySave(toSave).catch(() => {});
  renderHistoryDrawer();
}

function recordThreadMsg(entry) {
  const th = getCurrentThread();
  th.messages.push(entry);
  if (entry.who === 'user' && (th.title === '新对话' || !th.title)) {
    const clean = entry.text.trim().replace(/\s+/g, ' ');
    th.title = clean.length > 18 ? `${clean.slice(0, 18)}…` : clean;
  }
  th.updatedAt = Date.now();
  // 把活跃会话置顶
  CHAT_THREADS = [th, ...CHAT_THREADS.filter((t) => t.id !== th.id)];
  persistThreads();
}

function updateLastThreadAgentSession(session) {
  const th = getCurrentThread();
  for (let i = th.messages.length - 1; i >= 0; i--) {
    if (th.messages[i].who === 'bot' && th.messages[i].agentSession) {
      th.messages[i].agentSession = session;
      th.messages[i].text = session.summary || th.messages[i].text;
      th.updatedAt = Date.now();
      persistThreads();
      return;
    }
  }
}

function renderThreadMessages(th) {
  const log = $('chat-log');
  log.innerHTML = `<div class="msg bot"><div class="bubble">${escapeHtml(INITIAL_GREETING)}</div></div>`;
  CHAT_HISTORY = [];

  for (const m of th.messages || []) {
    if (m.who === 'user') {
      addMsg('user', m.text);
      CHAT_HISTORY.push({ role: 'user', content: m.text });
    } else if (m.agentSession) {
      const div = document.createElement('div');
      div.className = 'msg bot';
      log.appendChild(div);
      renderAgentMsg(div, m.agentSession, m.meta || '', m.thinking || '');
      if (m.text) CHAT_HISTORY.push({ role: 'assistant', content: m.text });
    } else {
      addMsg('bot', m.text, m.meta || '', 0, null, m.thinking || '');
      if (m.text) CHAT_HISTORY.push({ role: 'assistant', content: m.text });
    }
  }
  if (CHAT_HISTORY.length > 12) CHAT_HISTORY = CHAT_HISTORY.slice(-12);
}

function renderHistoryDrawer() {
  const drawer = $('history-drawer');
  if (!drawer) return;
  const saved = CHAT_THREADS.filter((t) => t.messages && t.messages.length > 0);
  if (!saved.length) {
    drawer.innerHTML = '<div class="history-empty">还没有历史对话记录。</div>';
    return;
  }
  drawer.innerHTML = saved.map((t) => {
    const active = t.id === CURRENT_THREAD_ID ? ' active' : '';
    const when = t.updatedAt ? new Date(t.updatedAt).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '';
    const count = t.messages.length;
    return `<div class="history-item${active}" data-open-thread="${escapeHtml(t.id)}">
      <div class="history-main">
        <div class="history-title">${escapeHtml(t.title || '未命名对话')}</div>
        <div class="history-meta">${escapeHtml(when)} · ${count} 条消息</div>
      </div>
      <button class="history-del" type="button" data-del-thread="${escapeHtml(t.id)}" title="删除该会话">×</button>
    </div>`;
  }).join('');

  drawer.querySelectorAll('[data-open-thread]').forEach((row) => {
    row.onclick = (e) => {
      if (e.target.closest('[data-del-thread]')) return;
      const id = row.dataset.openThread;
      const target = CHAT_THREADS.find((t) => t.id === id);
      if (!target) return;
      ACTIVE_FLOW = null;
      ACTIVE_AGENT = null;
      $('modal-review').hidden = true;
      CURRENT_THREAD_ID = target.id;
      renderThreadMessages(target);
      renderHistoryDrawer();
      drawer.hidden = true;
    };
  });

  drawer.querySelectorAll('[data-del-thread]').forEach((btn) => {
    btn.onclick = () => {
      const id = btn.dataset.delThread;
      CHAT_THREADS = CHAT_THREADS.filter((t) => t.id !== id);
      if (CURRENT_THREAD_ID === id) {
        if (CHAT_THREADS.length) {
          CURRENT_THREAD_ID = CHAT_THREADS[0].id;
          renderThreadMessages(CHAT_THREADS[0]);
        } else {
          startNewThread();
        }
      }
      persistThreads();
    };
  });
}

function toggleHistoryDrawer() {
  const drawer = $('history-drawer');
  if (!drawer) return;
  drawer.hidden = !drawer.hidden;
  if (!drawer.hidden) renderHistoryDrawer();
}

function startNewThread() {
  ACTIVE_FLOW = null;
  ACTIVE_AGENT = null;
  CHAT_HISTORY = [];
  $('modal-review').hidden = true;
  const fresh = createEmptyThread();
  CHAT_THREADS = [fresh, ...CHAT_THREADS.filter((t) => t.messages && t.messages.length > 0)];
  CURRENT_THREAD_ID = fresh.id;
  renderThreadMessages(fresh);
  renderHistoryDrawer();
  $('history-drawer').hidden = true;
  $('chat-text').focus();
}

async function initChatHistory() {
  try {
    const loaded = await api.historyLoad();
    if (Array.isArray(loaded) && loaded.length) {
      CHAT_THREADS = loaded;
      CURRENT_THREAD_ID = loaded[0].id;
      renderThreadMessages(loaded[0]);
      renderHistoryDrawer();
      return;
    }
  } catch {}
  const fresh = createEmptyThread();
  CHAT_THREADS = [fresh];
  CURRENT_THREAD_ID = fresh.id;
  renderHistoryDrawer();
}

function resetChat() {
  startNewThread();
}

function appendTypingBubble() {
  const log = $('chat-log');
  const div = document.createElement('div');
  div.className = 'msg bot typing-msg';
  div.innerHTML = `<div class="bubble typing"><span></span><span></span><span></span></div>`;
  log.appendChild(div);
  log.scrollTop = log.scrollHeight;
  return div;
}

async function send() {
  const input = $('chat-text');
  const q = input.value.trim();
  if (!q) return;
  input.value = '';
  addMsg('user', q);
  recordThreadMsg({ who: 'user', text: q });

  const btn = $('btn-send');
  btn.disabled = true;
  const t0 = performance.now();
  const typingEl = appendTypingBubble();

  const streamId = `s-${STREAM_SEQ++}`;
  ACTIVE_STREAM = { id: streamId, typingEl, div: null, thinking: '', content: '' };

  let answer;
  try {
    answer = await answerQuestion(q, streamId);
  } catch (e) {
    answer = { text: `出错了：${e.message || e}`, meta: 'error' };
  }

  const streamState = ACTIVE_STREAM;
  ACTIVE_STREAM = null;

  // 若已经有真实流式文字输出，无需人为补延迟；纯规则快答仍保持自然节奏
  if (!streamState.div) {
    const elapsed = performance.now() - t0;
    const minMs = delayFor(answer.meta || '');
    if (minMs > 0) {
      const remain = minMs - elapsed;
      if (remain > 0) await sleep(remain);
    }
  }

  if (streamState.typingEl) streamState.typingEl.remove();
  btn.disabled = false;

  const finalThinking = answer.thinking || streamState.thinking || '';

  if (answer.agentSession) {
    const log = $('chat-log');
    const div = streamState.div || document.createElement('div');
    if (!streamState.div) {
      div.className = 'msg bot';
      log.appendChild(div);
    }
    renderAgentMsg(div, answer.agentSession, answer.meta, finalThinking);
    recordThreadMsg({
      who: 'bot',
      text: answer.agentSession.summary || answer.text || '',
      thinking: finalThinking || null,
      meta: answer.meta || null,
      agentSession: answer.agentSession,
    });

    CHAT_HISTORY.push({ role: 'user', content: q });
    if (answer.agentSession.status === 'waiting_confirm') {
      ACTIVE_AGENT = {
        session: answer.agentSession,
        div,
        meta: answer.meta,
        thinking: finalThinking,
      };
      openReviewModal(answer.agentSession);
    } else {
      ACTIVE_AGENT = null;
      if (answer.agentSession.summary) {
        CHAT_HISTORY.push({ role: 'assistant', content: answer.agentSession.summary });
      }
    }
    if (CHAT_HISTORY.length > 12) CHAT_HISTORY = CHAT_HISTORY.slice(-12);
    return;
  }

  if (streamState.div) {
    const wasOpen = !!streamState.div.querySelector('.think-box')?.open;
    const thinkHtml = buildThinkBoxHtml(finalThinking, false, wasOpen);
    const srcHtml = buildSourcesHtml(answer.sources);
    const tag = answer.meta ? `<div class="meta">${escapeHtml(answer.meta)}</div>` : '';
    streamState.div.innerHTML = `<div class="bubble">${thinkHtml}${escapeHtml(answer.text)}${srcHtml}</div>${tag}`;
    bindSourceLinks(streamState.div);
    const log = $('chat-log');
    log.scrollTop = log.scrollHeight;
  } else {
    addMsg('bot', answer.text, answer.meta, performance.now() - t0, answer.actions, finalThinking, answer.sources);
  }

  recordThreadMsg({
    who: 'bot',
    text: answer.text || '',
    thinking: finalThinking || null,
    meta: answer.meta || null,
  });

  if (answer.flow) ACTIVE_FLOW = answer.flow;
  if (answer.recordHistory) {
    CHAT_HISTORY.push({ role: 'user', content: q });
    CHAT_HISTORY.push({ role: 'assistant', content: answer.text });
    if (CHAT_HISTORY.length > 12) CHAT_HISTORY = CHAT_HISTORY.slice(-12);
  }
}

function buildSourcesHtml(sources) {
  if (!Array.isArray(sources) || !sources.length) return '';
  const chips = sources.map((s) => {
    const auth = s.trust === '官方/权威';
    const badge = auth ? ' · 官方' : '';
    const title = s.title || s.domain || '';
    const url = s.url && /^https:\/\//.test(s.url) ? s.url : '';
    return `<button class="source-chip${auth ? ' auth' : ''}" type="button" data-open-url="${escapeHtml(url)}" title="${escapeHtml(title)}">${escapeHtml(s.domain || title)}${badge}</button>`;
  }).join('');
  return `<div class="sources-bar"><span>参考来源：</span>${chips}</div>`;
}

function bindSourceLinks(container) {
  container.querySelectorAll('[data-open-url]').forEach((btn) => {
    btn.onclick = () => {
      const url = btn.dataset.openUrl;
      if (url && /^https:\/\//.test(url)) {
        api.openExternal(url).catch(() => {});
      }
    };
  });
}

function addMsg(who, text, meta, ms, actions, thinking = '', sources = null) {
  const log = $('chat-log');
  const div = document.createElement('div');
  div.className = 'msg ' + who;

  const tag = meta ? `<div class="meta">${escapeHtml(meta)}</div>` : '';
  const thinkHtml = who === 'bot' ? buildThinkBoxHtml(thinking, false, false) : '';
  const srcHtml = who === 'bot' ? buildSourcesHtml(sources) : '';

  // 动作按钮：这是 agent 和聊天机器人的分界。
  // 有按钮，用户点一下就推进；没按钮，只是说说话。
  let btnHtml = '';
  if (actions && actions.length) {
    btnHtml = `<div class="msg-actions">${actions
      .map((a) => `<button class="btn sm" data-act="${escapeHtml(a.id)}">${escapeHtml(a.label)}</button>`)
      .join('')}</div>`;
  }

  div.innerHTML = `<div class="bubble">${thinkHtml}${escapeHtml(text)}${srcHtml}</div>${btnHtml}${tag}`;
  log.appendChild(div);
  log.scrollTop = log.scrollHeight;

  div.querySelectorAll('[data-act]').forEach((b) => {
    b.onclick = () => onFlowAction(b.dataset.act, b);
  });
  bindSourceLinks(div);
}

// ══════════════════════════════════════════════
// 流程动作
// ══════════════════════════════════════════════

async function onFlowAction(actionId, btnEl) {
  if (!ACTIVE_FLOW) {
    addMsg('bot', '流程已经结束了。要重新开始的话，再说一遍「帮我装 XXX」。', '流程');
    return;
  }

  // 防连点
  const wrap = btnEl.parentElement;
  wrap.querySelectorAll('button').forEach((b) => (b.disabled = true));
  btnEl.textContent = '处理中…';

  const t0 = performance.now();
  let next;
  try {
    next = await api.flowAdvance(ACTIVE_FLOW, actionId);
  } catch (e) {
    addMsg('bot', `出错了：${e.message || e}`, 'error');
    return;
  }

  ACTIVE_FLOW = next.stage === 'done' || next.stage === 'failed' || next.stage === 'already_installed'
    ? null
    : next;

  const meta = stageLabel(next.stage);
  addMsg('bot', next.say, meta, performance.now() - t0, next.actions);
}

function stageLabel(stage) {
  const map = {
    checking: '检查中',
    already_installed: '已安装',
    need_download: '待下载',
    downloaded: '已下载',
    need_consent: '待确认',
    awaiting_user: '等你操作',
    done: '完成',
    failed: '已停止',
  };
  return map[stage] || stage;
}

// ══════════════════════════════════════════════
// 真实回答：确定性优先，超出范围再走模型
// ══════════════════════════════════════════════

async function answerQuestion(q, streamId) {
  // ── -1. 反诈与远控红线：最高优先级，先于安装与官网查询 ──
  const scam = scamGuard(q);
  if (scam) return { text: scam.text, meta: scam.tag };

  // ── 0. 安装意图：优先于其它判断 ──────────
  const install = detectInstallIntent(q);
  if (install) {
    try {
      const flow = await api.flowStart(install.id);
      return {
        text: flow.say,
        meta: `安装流程 · ${install.name} · ${stageLabel(flow.stage)}`,
        actions: flow.actions,
        flow: flow.stage === 'failed' || flow.stage === 'already_installed' ? null : flow,
      };
    } catch (e) {
      return { text: `启动安装流程失败：${e.message || e}`, meta: 'error' };
    }
  }

  // ── 0.5 说要装，但没配方：给地址 + 说清能力边界 ──
  const miss = detectInstallMiss(q);
  if (miss) return miss;

  // ── 1. 官网 / 下载地址 ──────────────────
  const r = handle(q);

  if (r.source === 'whitelist' && r.answer) {
    return { text: r.answer, meta: `官网库 · ${r.meta.id || r.meta.category || ''}` };
  }
  if (r.source === 'fallback' && r.answer) {
    return { text: r.answer, meta: '未收录 · 给方法' };
  }

  // ── 2. 网络连通性自检（net.check）─────────
  const netRes = await answerNetCheck(q);
  if (netRes) return netRes;

  // ── 2.1 目录大项排查（fs.biggest）────────
  const bigDir = await answerBiggestDir(q);
  if (bigDir) return bigDir;

  // ── 2.2 卡顿五项硬阈值体检 + 进程排查 ────
  const proc = await answerProcess(q);
  if (proc) return proc;

  // ── 2.5 硬件相关：用真实采集值 ──────────
  const hw = answerHardware(q);
  if (hw) return hw;

  // ── 2.8 高频痛点扫盲与急救剧本（确定性离线可用）──
  const guide = guideAnswer(q);
  if (guide) return { text: guide.text, meta: guide.tag };

  // ── 3. 社交与常识层 ────────────────────
  // 放在技术判断之后、模型/兜底之前：
  // 技术问题不该被「你好」的规则抢走，但寒暄要有回应。
  const soc = socialAnswer(q);
  if (soc) return { text: soc.text, meta: soc.tag };

  // ── 3.5 小模型 GBNF 约束分诊（P1-a）：把漏过正则的口语直接路由回确定性 Rust 能力 ──
  if (LLM_CONFIG && LLM_CONFIG.mode !== 'off') {
    try {
      const tr = await api.triageClassify(q);
      if (tr && tr.ok && tr.intent && tr.intent !== 'dont_know') {
        if (tr.intent === 'lag_diagnose') {
          const routed = await answerProcess(q, { forceLag: true });
          if (routed) return { ...routed, meta: `${routed.meta} · 分诊命中` };
        }
        if (tr.intent === 'mem_top') {
          const routed = await answerProcess(q, { forceMem: true });
          if (routed) return { ...routed, meta: `${routed.meta} · 分诊命中` };
        }
        if (tr.intent === 'disk_biggest') {
          const routed = await answerBiggestDir(q, true);
          if (routed) return { ...routed, meta: `${routed.meta} · 分诊命中` };
        }
        if (tr.intent === 'net_check') {
          const routed = await answerNetCheck(q, true);
          if (routed) return { ...routed, meta: `${routed.meta} · 分诊命中` };
        }
        if (tr.intent === 'hw_overview') {
          const routed = answerHardware('我这台什么配置');
          if (routed) return { ...routed, meta: `${routed.meta} · 分诊命中` };
        }
      }
    } catch {}
  }

  // ── 4. 受控 Agent / 统一模型接口（本地 llama-server / 自定义 OpenAI API，流式）──
  if (LLM_CONFIG && LLM_CONFIG.mode !== 'off') {
    const nextHistory = [...CHAT_HISTORY, { role: 'user', content: q }];
    const useWebSearch = WEB_SEARCH_ON && SEARCH_CONFIG.mode !== 'off';
    const res = await api.agentTurn(streamId, nextHistory, buildHardwareSummary(), PERMISSION_MODE, useWebSearch);
    const modeTag = LLM_CONFIG.mode === 'local' ? '本地模型' : '自定义 API';
    const modelTag = res && res.model ? ` · ${res.model}` : '';

    if (res && res.ok && res.kind === 'task' && res.session) {
      const permTag = PERMISSION_MODE === 'auto' ? '完全权限' : '请求审查';
      return {
        text: res.reply || res.session.summary || '',
        thinking: res.thinking || '',
        meta: `Agent · ${permTag}${modelTag}`,
        agentSession: res.session,
      };
    }
    if (res && res.ok && res.reply) {
      const webTag = res.sources && res.sources.length
        ? ` · 联网(${res.searchEngine || '已过筛'})`
        : '';
      return {
        text: res.reply,
        thinking: res.thinking || '',
        sources: res.sources || null,
        meta: `${modeTag}${modelTag}${webTag}`,
        recordHistory: true,
      };
    }
    return {
      text: `模型暂时连不上：${(res && res.error) || '未知错误'}\n\n可以点右上角「模型设置」检查地址或先切回纯规则模式。`,
      meta: '模型连接失败',
    };
  }

  // ── 5. 能力边界（未开模型时）────────────
  return {
    text:
      '这个我判断不了，不想瞎猜。\n\n' +
      '我现在不用模型也能直接帮的是：\n' +
      '· 帮你装软件（Steam、7-Zip 现在支持）\n' +
      '· 找软件官网和下载地址（含常见假站提醒）\n' +
      '· 看你这台机器的配置、卡顿体检、C 盘大项与测网\n' +
      '· 常见急救与避坑（蓝屏、没声音、弹窗清理、反诈提醒）\n\n' +
      '如果要聊别的，可以点右上角「模型设置」接本地模型或自定义 OpenAI API。',
    meta: '超出范围',
  };
}

// ── 网络连通性自检（调用 Rust net.check）──────
async function answerNetCheck(q, forced = false) {
  const t = q.toLowerCase();
  const asksNet = forced || /(断网|没网|连不上网|网络通不通|测一下网|测网|网络正常|网好不好|上不了网|无法联网)/.test(t);
  if (!asksNet) return null;

  const res = await api.capabilityCall('net.check', {});
  if (!res || !res.ok || !res.data) {
    return {
      text: `浏览器预览里没法测真实网络，请在桌面应用里试。\n\n${NET_OFFLINE_TIPS}`,
      meta: '网络 · 预览',
    };
  }
  const d = res.data;
  const lines = [d.summary || '网络检测完成。'];
  let hasFail = false;
  if (Array.isArray(d.targets) && d.targets.length) {
    lines.push('');
    for (const item of d.targets) {
      if (!item.ok) hasFail = true;
      const flag = item.ok ? `正常（${item.ms} ms）` : `失败（${item.reason}）`;
      lines.push(`· ${item.name}：${flag}`);
    }
  }
  if (hasFail || /(断网|没网|连不上|上不了|无法联网)/.test(t)) {
    lines.push('', NET_OFFLINE_TIPS);
  }
  return { text: lines.join('\n'), meta: '网络 · 实测' };
}

// ── 目录大项排查（调用 Rust fs.biggest）──────
async function answerBiggestDir(q, forced = false) {
  const t = q.toLowerCase();
  const asksBig = forced || /(哪个文件夹最大|什么占满|被什么吃掉|哪个占地方|占空间最大|大文件夹|什么最占空间|c\s*盘.*(爆满|飘红|满了|红了))/.test(t);
  if (!asksBig) return null;

  const driveMatch = t.match(/([c-z])\s*盘/);
  const drive = (driveMatch ? driveMatch[1] : 'c').toUpperCase();
  const targetPath = `${drive}:\\`;

  const res = await api.capabilityCall('fs.biggest', { path: targetPath, n: 8 });
  if (!res || !res.ok || !Array.isArray(res.data)) {
    const extra = drive === 'C' ? `\n\n${C_DRIVE_TIPS}` : '';
    return { text: `扫描 ${targetPath} 失败：${(res && res.error) || '请在桌面应用里试'}${extra}`, meta: '磁盘扫描' };
  }
  if (!res.data.length) {
    const extra = drive === 'C' ? `\n\n${C_DRIVE_TIPS}` : '';
    return { text: `${targetPath} 下没有扫到可统计的非系统大项。${extra}`, meta: '磁盘 · 实测' };
  }

  const lines = [`${targetPath} 里最占地方的前 ${res.data.length} 项（已自动跳过系统目录）：`];
  for (const item of res.data) {
    const kind = item.isDir ? '文件夹' : '文件';
    lines.push(`· ${item.name}（${kind}）—— ${formatBytes(item.bytes)}`);
  }
  lines.push('', '不认识的文件别随便删；可以优先看下载目录、视频或游戏文件夹。');
  if (drive === 'C') {
    lines.push('', C_DRIVE_TIPS);
  }
  return { text: lines.join('\n'), meta: '磁盘 · 实测' };
}

// ── 卡顿五项硬阈值体检 + 实时进程排查（hw.diagnose + proc.top）──
async function answerProcess(q, opts = {}) {
  const t = q.toLowerCase();
  const asksLag = !!opts.forceLag || /好卡|很卡|太卡|变卡|卡顿|卡死|风扇响|发烫|为什么卡|反应慢/.test(t);
  const asksMemHog = !!opts.forceMem || /(吃内存|占内存|占用内存|占资源|后台程序|什么程序在跑|哪个程序)/.test(t);
  if (!asksLag && !asksMemHog) return null;

  const lines = [];

  if (asksLag) {
    const diag = await api.capabilityCall('hw.diagnose', {});
    if (diag && diag.ok && diag.data) {
      const findings = Array.isArray(diag.data.findings) ? diag.data.findings : [];
      const m = diag.data.metrics || {};
      if (findings.length) {
        lines.push(`检出 ${findings.length} 个直接导致卡顿的原因：`);
        for (const f of findings) {
          lines.push(`· ${f.title}：${f.detail}`);
          if (f.advice) lines.push(`  建议：${f.advice}`);
        }
      } else {
        lines.push('五项常见卡顿指标目前都正常（内存、C 盘、磁盘响应、磁盘忙度、CPU 频率均未超线）。');
      }
      const statParts = [];
      if (m.memFreePct != null) statParts.push(`内存空闲 ${m.memFreePct}%`);
      if (m.cFreePct != null) statParts.push(`C盘剩 ${m.cFreePct}%`);
      if (m.diskRespMs != null) statParts.push(`磁盘响应 ${m.diskRespMs}ms`);
      if (m.diskActivePct != null) statParts.push(`磁盘忙度 ${m.diskActivePct}%`);
      if (m.cpuCurMhz != null && m.cpuBaseMhz != null) statParts.push(`CPU ${m.cpuCurMhz}/${m.cpuBaseMhz}MHz`);
      if (statParts.length) lines.push('', `实测摘要：${statParts.join(' · ')}`);
    }
  }

  const res = await api.capabilityCall('proc.top', { n: 5, by: 'mem' });
  if (res && res.ok && Array.isArray(res.data) && res.data.length) {
    if (lines.length) lines.push('');
    lines.push('现在最占内存的 5 个程序：');
    for (const p of res.data) {
      const mb = Number(p.memMB || 0);
      const size = mb >= 1024 ? `${(mb / 1024).toFixed(2)} GB` : `${Math.round(mb)} MB`;
      lines.push(`· ${p.name} —— ${size}`);
    }
  }

  if (lines.length) {
    return { text: lines.join('\n'), meta: asksLag ? '体检 · 五项硬阈值实测' : '进程 · 实测' };
  }

  // 浏览器预览兜底
  if (PROFILE && PROFILE.memory) {
    const free = PROFILE.memory.freeBytes || 0;
    const total = PROFILE.memory.totalBytes || 0;
    const ratio = total ? free / total : 1;
    return {
      text: ratio < 0.15
        ? `内存空闲只有 ${(free / GB).toFixed(1)}GB（共 ${(total / GB).toFixed(1)}GB），低于 15% 会卡。先关掉不用的浏览器标签页。`
        : `内存目前剩 ${(free / GB).toFixed(1)}GB / 共 ${(total / GB).toFixed(1)}GB，没满。完整五项卡顿体检和进程列表请在桌面应用里看。`,
      meta: '硬件 · 实测',
    };
  }
  return null;
}

// ── 设置页操作审计记录 ──────────────────────
async function refreshAuditLog() {
  const box = $('audit-list');
  if (!box) return;
  let items = [];
  try {
    items = (await api.agentAuditList()) || [];
  } catch {
    items = [];
  }
  if (!items.length) {
    box.innerHTML = '<div class="audit-empty">还没有执行过本机操作。</div>';
    return;
  }
  box.innerHTML = items.slice(0, 20).map((it) => {
    const ts = it.timestampMs ? new Date(it.timestampMs).toLocaleTimeString('zh-CN') : '';
    const status = it.ok ? '成功' : '失败';
    const target = it.target ? ` · ${it.target}` : '';
    return `<div class="audit-item">
      <div class="audit-row">
        <span class="audit-title">${escapeHtml(it.title)}（${status}）</span>
        <span class="audit-time">${escapeHtml(ts)} · ${escapeHtml(it.riskLabel || '')}</span>
      </div>
      <div class="audit-detail ${it.ok ? '' : 'err'}">${escapeHtml(it.detail || '')}${escapeHtml(target)}</div>
    </div>`;
  }).join('');
}

function initAuditControls() {
  $('btn-audit-refresh').onclick = () => refreshAuditLog();
  $('btn-audit-clear').onclick = async () => {
    await api.agentAuditClear();
    refreshAuditLog();
  };
}

// ── 内置本地推理引擎控制（设置页）──────────────
async function refreshRuntimeStatus() {
  const txt = $('rt-status-text');
  const startBtn = $('btn-rt-start');
  const stopBtn = $('btn-rt-stop');
  if (!txt || !startBtn || !stopBtn) return;
  try {
    const st = await api.runtimeStatus();
    txt.textContent = st.message || '未知状态';
    startBtn.hidden = !!st.running;
    stopBtn.hidden = !(st.running && st.managed);
  } catch {
    txt.textContent = '无法获取本地引擎状态';
  }
}

function initRuntimeControls() {
  $('btn-rt-start').onclick = async () => {
    const startBtn = $('btn-rt-start');
    startBtn.disabled = true;
    startBtn.textContent = '启动中…';
    setLlmStatus('正在启动内置 llama-server 并加载模型…');
    try {
      const st = await api.runtimeStart(null);
      setLlmStatus(st.message || '本地引擎已启动。', 'ok');
    } catch (e) {
      setLlmStatus(`启动失败：${e.message || e}`, 'err');
    } finally {
      startBtn.disabled = false;
      startBtn.textContent = '一键启动';
      refreshRuntimeStatus();
    }
  };

  $('btn-rt-stop').onclick = async () => {
    await api.runtimeStop();
    setLlmStatus('已停止托管的本地引擎，显存已释放。');
    refreshRuntimeStatus();
  };
}

// ── 安装意图识别 ────────────────────────────
// 先用规则，够用。等规则覆盖不到再上模型。
const INSTALL_CUES = ['装', '安装', '下个', '帮我弄', '帮我搞'];

// 有安装配方的软件。加新软件时这里和 Rust 侧 RECIPES 要同步。
const INSTALLABLE = [
  { id: 'steam', name: 'Steam', keys: ['steam', 'staem', '蒸汽'] },
  { id: '7zip', name: '7-Zip', keys: ['7zip', '7-zip', '7z', '七压'] },
];

function detectInstallIntent(q) {
  const t = q.toLowerCase();
  if (!INSTALL_CUES.some((c) => t.includes(c))) return null;

  const hit = INSTALLABLE.find((k) => k.keys.some((key) => t.includes(key)));
  return hit || null;
}

/**
 * 用户说了「安装 XXX」，但那个软件没配方。
 * 不能只说「我判断不了」——他明明说得很清楚，是我能力不够。
 * 要把「能做的」和「不能做的」分开说，并给出下一步。
 */
function detectInstallMiss(q) {
  const t = q.toLowerCase();
  if (!INSTALL_CUES.some((c) => t.includes(c))) return null;

  // 已经在 INSTALLABLE 里的不算 miss
  if (INSTALLABLE.some((k) => k.keys.some((key) => t.includes(key)))) return null;

  // 看看官网库里有没有它
  const r = handle(q);
  if (r.source === 'whitelist' && r.answer) {
    return {
      text:
        `${r.answer}\n\n` +
        '我没法替你自动装这个，只能给地址。\n' +
        '现在能自动装的只有 Steam 和 7-Zip。',
      meta: `官网库 · ${r.meta.id || '有地址无配方'}`,
    };
  }
  return null;
}

/**
 * 硬件类问题：所有数字都来自 PROFILE（真实采集），
 * 没有任何一个是写死的。
 */
function answerHardware(q) {
  if (!PROFILE) return null;
  const t = q.toLowerCase();

  const gb = (n) => (n / GB).toFixed(1);

  // ── 磁盘 ─────────────────────────────────
  // 用户不会说「C盘」，他会说「硬盘」「存储」「磁盘」。
  // 反过来，只提「C盘」时也不能漏掉 —— 盘符本身就是问磁盘的信号。
  // 注意：中文里「内存」常被误用来指硬盘剩余空间，所以磁盘判断要先于内存。
  const asksCDrive = /c盘|c 盘|d盘|d 盘/.test(t);
  const asksDisk = /硬盘|磁盘|存储|空间/.test(t) || asksCDrive;
  const asksAmount = /剩|多少|够|够不够|还有/.test(t);

  if (asksDisk && asksAmount) {
    const disks = PROFILE.disks || [];

    if (asksCDrive) {
      const disk = disks.find((d) => d.letter === 'C:') || disks[0];
      if (!disk) return { text: '没读到 C 盘信息。', meta: '硬件' };

      const free = disk.freeBytes || 0;
      const total = disk.totalBytes || 0;
      const ratio = total ? free / total : 0;

      const lines = [`C 盘剩 ${gb(free)}GB，总共 ${gb(total)}GB。`];
      if (ratio < 0.1) {
        lines.push('');
        lines.push('低于 10% 会开始拖慢系统。建议先清一点。');
      } else if (ratio > 0.3) {
        lines.push('');
        lines.push('空间够用。');
      }
      return { text: lines.join('\n'), meta: '硬件 · 实测' };
    }

    // 没指定盘符 → 全部列出来，让他自己看哪个紧
    const lines = [`你这台有 ${disks.length} 块盘：`];
    for (const d of disks) {
      const free = d.freeBytes || 0;
      const total = d.totalBytes || 0;
      const ratio = total ? free / total : 0;
      const warn = ratio < 0.1 ? '　← 快满了' : '';
      lines.push(`${d.letter} 剩 ${gb(free)}GB / 共 ${gb(total)}GB${warn}`);
    }
    return { text: lines.join('\n'), meta: '硬件 · 实测' };
  }

  // 内存
  if (/内存/.test(t) && !/显存/.test(t)) {
    const mem = PROFILE.memory;
    if (!mem) return { text: '没读到内存信息。', meta: '硬件' };

    const free = mem.freeBytes || 0;
    const total = mem.totalBytes || 0;
    const ratio = total ? free / total : 0;

    const lines = [`内存共 ${gb(total)}GB，当前空闲 ${gb(free)}GB。`];
    if (ratio < 0.15) {
      lines.push('');
      lines.push('空闲低于 15%，这就会卡。关掉不用的浏览器标签和后台程序。');
    } else {
      lines.push('');
      lines.push('够用。');
    }
    return { text: lines.join('\n'), meta: '硬件 · 实测' };
  }

  // 显存 / 显卡
  if (/显存|显卡/.test(t)) {
    const gpu = (PROFILE.gpus || []).find(
      (g) => g.vendor === 'nvidia' || g.vendor === 'amd');
    if (!gpu) return {
      text: '这台没有独立显卡，用的是集成显卡。',
      meta: '硬件 · 实测',
    };

    const vram = effectiveVram(PROFILE.gpus);
    return {
      text: `${gpu.name}\n\n可用显存 ${gb(vram)}GB。`,
      meta: '硬件 · 实测',
    };
  }

  // 配置总览
  if (/配置|什么配置|我这台|机器怎么样|电脑怎么样/.test(t)) {
    const cpu = PROFILE.cpu;
    const gpu = (PROFILE.gpus || []).find(
      (g) => g.vendor === 'nvidia' || g.vendor === 'amd');
    const lines = [];
    if (cpu) lines.push(`处理器：${cpu.name.replace(/\s*CPU\s*|\(R\)|\(TM\)|@.*/gi, '').trim()}`);
    if (PROFILE.memory) lines.push(`内存：${gb(PROFILE.memory.totalBytes)}GB`);
    if (gpu) lines.push(`显卡：${gpu.name}`);
    if (VERDICT) lines.push('', `能跑 ${TIERS[VERDICT.tier].level} 档模型。`);
    return { text: lines.join('\n'), meta: '硬件 · 实测' };
  }

  return null;
}

// ══════════════════════════════════════════════
// 软件信息
// ══════════════════════════════════════════════

const APP_VERSION = '0.1.0';

function renderInfo() {
  $('info-version').textContent = `v${APP_VERSION}`;

  // 有真实采集结果就显示系统信息，否则只报外壳版本。
  // 环境名里的中文在部分 PowerShell 输出里会乱码，所以只用版本号+构建号，
  // 语言无关，任何时候都不会出现方块字。
  const env = PROFILE?.environment;
  const shell = isTauriEnv() ? 'Tauri / WebView2' : '浏览器预览';
  $('info-runtime').textContent = env
    ? `Windows ${env.isWin11 ? '11' : '10'} · build ${env.build} · ${shell}`
    : shell;

  $('info-collected').textContent = PROFILE?.collectedAt
    ? new Date(PROFILE.collectedAt).toLocaleString('zh-CN')
    : '还没检测过';
}

function isTauriEnv() {
  return typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__;
}

// ══════════════════════════════════════════════
// 分层联网检索设置与对话栏开关
// ══════════════════════════════════════════════

function applySearchModeUI(mode) {
  SEARCH_DRAFT_MODE = mode || 'builtin';
  document.querySelectorAll('#search-mode-switch button').forEach((b) =>
    b.classList.toggle('on', b.dataset.searchMode === SEARCH_DRAFT_MODE));
  for (const m of ['off', 'builtin', 'bocha', 'tavily']) {
    const panel = $(`search-panel-${m}`);
    if (panel) panel.hidden = m !== SEARCH_DRAFT_MODE;
  }
}

function updateChatSearchBtn() {
  const btn = $('btn-web-search');
  if (!btn) return;
  const modeNames = {
    off: '已关',
    builtin: '内置',
    bocha: '博查',
    tavily: 'Tavily',
  };
  if (SEARCH_CONFIG.mode === 'off') {
    btn.classList.remove('on');
    btn.textContent = '联网：已关';
    return;
  }
  btn.classList.toggle('on', WEB_SEARCH_ON);
  btn.textContent = WEB_SEARCH_ON
    ? `联网：${modeNames[SEARCH_CONFIG.mode] || '开'}`
    : '联网：暂停';
}

function renderSearchForm(cfg) {
  SEARCH_CONFIG = cfg || SEARCH_CONFIG;
  BOCHA_KEY_CLEARED = false;
  TAVILY_KEY_CLEARED = false;

  const bochaInput = $('search-bocha-key');
  if (bochaInput) {
    bochaInput.value = '';
    bochaInput.placeholder = SEARCH_CONFIG.hasBochaKey
      ? `已保存：${SEARCH_CONFIG.bochaKeyMasked}（留空保持不变）`
      : 'sk-...';
  }
  const bochaClear = $('btn-clear-bocha-key');
  if (bochaClear) bochaClear.hidden = !SEARCH_CONFIG.hasBochaKey;

  const tavilyInput = $('search-tavily-key');
  if (tavilyInput) {
    tavilyInput.value = '';
    tavilyInput.placeholder = SEARCH_CONFIG.hasTavilyKey
      ? `已保存：${SEARCH_CONFIG.tavilyKeyMasked}（留空保持不变）`
      : 'tvly-...';
  }
  const tavilyClear = $('btn-clear-tavily-key');
  if (tavilyClear) tavilyClear.hidden = !SEARCH_CONFIG.hasTavilyKey;

  applySearchModeUI(SEARCH_CONFIG.mode);
  updateChatSearchBtn();
}

function readSearchFormInput() {
  const bochaTyped = ($('search-bocha-key')?.value || '').trim();
  const tavilyTyped = ($('search-tavily-key')?.value || '').trim();
  return {
    mode: SEARCH_DRAFT_MODE,
    bochaApiKey: bochaTyped ? bochaTyped : (BOCHA_KEY_CLEARED ? '' : null),
    tavilyApiKey: tavilyTyped ? tavilyTyped : (TAVILY_KEY_CLEARED ? '' : null),
    timeoutSecs: SEARCH_CONFIG.timeoutSecs || 8,
    maxResults: SEARCH_CONFIG.maxResults || 4,
  };
}

function setSearchStatus(text, kind = '') {
  const el = $('search-status');
  if (!el) return;
  if (!text) {
    el.hidden = true;
    el.textContent = '';
    el.className = 'status-line';
    return;
  }
  el.hidden = false;
  el.textContent = text;
  el.className = kind ? `status-line ${kind}` : 'status-line';
}

async function initSearchSettings() {
  document.querySelectorAll('#search-mode-switch button').forEach((b) => {
    b.onclick = () => {
      applySearchModeUI(b.dataset.searchMode);
      setSearchStatus('');
    };
  });

  const chatToggle = $('btn-web-search');
  if (chatToggle) {
    chatToggle.onclick = () => {
      if (SEARCH_CONFIG.mode === 'off') {
        goto('settings');
        $('setting-web-search')?.scrollIntoView({ behavior: 'smooth', block: 'center' });
        return;
      }
      WEB_SEARCH_ON = !WEB_SEARCH_ON;
      updateChatSearchBtn();
    };
  }

  const clearBocha = $('btn-clear-bocha-key');
  if (clearBocha) {
    clearBocha.onclick = () => {
      BOCHA_KEY_CLEARED = true;
      $('search-bocha-key').value = '';
      $('search-bocha-key').placeholder = '已标记清空，点「保存检索设置」生效';
      clearBocha.hidden = true;
    };
  }
  $('search-bocha-key')?.addEventListener('input', () => {
    if ($('search-bocha-key').value.trim()) BOCHA_KEY_CLEARED = false;
  });

  const clearTavily = $('btn-clear-tavily-key');
  if (clearTavily) {
    clearTavily.onclick = () => {
      TAVILY_KEY_CLEARED = true;
      $('search-tavily-key').value = '';
      $('search-tavily-key').placeholder = '已标记清空，点「保存检索设置」生效';
      clearTavily.hidden = true;
    };
  }
  $('search-tavily-key')?.addEventListener('input', () => {
    if ($('search-tavily-key').value.trim()) TAVILY_KEY_CLEARED = false;
  });

  $('btn-search-save').onclick = async () => {
    const input = readSearchFormInput();
    $('btn-search-save').disabled = true;
    try {
      const saved = await api.searchConfigSave(input);
      if (saved.mode !== 'off') WEB_SEARCH_ON = true;
      renderSearchForm(saved);
      setSearchStatus('检索设置已保存。', 'ok');
    } catch (e) {
      setSearchStatus(`保存失败：${e.message || e}`, 'err');
    } finally {
      $('btn-search-save').disabled = false;
    }
  };

  $('btn-search-test').onclick = async () => {
    const input = readSearchFormInput();
    $('btn-search-test').disabled = true;
    setSearchStatus('正在实测检索与安全过滤…');
    try {
      const res = await api.searchTest(input);
      if (res && res.ok && res.items && res.items.length) {
        const domains = [...new Set(res.items.map((x) => x.domain))].slice(0, 3).join('、');
        const blk = res.blockedCount ? `，过滤高危 ${res.blockedCount} 条` : '';
        setSearchStatus(`检索正常（${res.engine} · ${res.latencyMs}ms）：拿到 ${res.items.length} 条可用结果${blk}（${domains}）。`, 'ok');
      } else {
        setSearchStatus(`未拿到可用结果：${(res && res.error) || '请检查网络或 Key'}`, 'err');
      }
    } catch (e) {
      setSearchStatus(`测试失败：${e.message || e}`, 'err');
    } finally {
      $('btn-search-test').disabled = false;
    }
  };

  try {
    const loaded = await api.searchConfigGet();
    renderSearchForm(loaded);
  } catch {
    renderSearchForm(SEARCH_CONFIG);
  }
}

// ══════════════════════════════════════════════
// 启动
// ══════════════════════════════════════════════

initTheme();
initPermissionControls();
initLlmSettings();
initSearchSettings();
initDownloadSettings();
initFsPolicySettings();
initAuditControls();
initRuntimeControls();
initStreamEvents();
initDownloadEvents();
initChatHistory();
renderInfo();
collect().then(() => {
  refreshModelStatuses();
  refreshRuntimeStatus();
});
