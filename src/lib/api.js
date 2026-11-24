// Tauri 通信层：把 invoke 包一层，浏览器里也能跑（预览用 mock）
const isTauri = typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__;

function invoke(cmd, args) {
  return window.__TAURI_INTERNALS__.invoke(cmd, args);
}

const PREVIEW_LISTENERS = new Map();

function emitPreview(event, payload) {
  const list = PREVIEW_LISTENERS.get(event) || [];
  for (const fn of list) fn(payload);
}

async function listenEvent(event, cb) {
  if (!isTauri) {
    if (!PREVIEW_LISTENERS.has(event)) PREVIEW_LISTENERS.set(event, []);
    PREVIEW_LISTENERS.get(event).push(cb);
    return () => {
      const arr = PREVIEW_LISTENERS.get(event) || [];
      const i = arr.indexOf(cb);
      if (i >= 0) arr.splice(i, 1);
    };
  }
  const handler = window.__TAURI_INTERNALS__.transformCallback((e) => cb(e.payload));
  const eventId = await invoke('plugin:event|listen', {
    event,
    target: { kind: 'Any' },
    handler,
  });
  return () => invoke('plugin:event|unlisten', { event, eventId }).catch(() => {});
}

const PREVIEW_LLM_KEY = 'preview_llm_cfg';
const PREVIEW_DL_KEY = 'preview_dl_cfg';
const PREVIEW_DL_TIMERS = new Map();

function loadPreviewDownloadCfg() {
  try {
    const raw = localStorage.getItem(PREVIEW_DL_KEY);
    if (raw) return JSON.parse(raw);
  } catch {}
  return { source: 'modelscope', customBaseUrl: '' };
}

function loadPreviewLlm() {
  try {
    const raw = localStorage.getItem(PREVIEW_LLM_KEY);
    if (raw) return JSON.parse(raw);
  } catch {}
  return {
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
}

function maskPreviewKey(k) {
  const s = String(k || '').trim();
  if (!s) return '';
  if (s.length <= 8) return '*'.repeat(s.length);
  return `${s.slice(0, 4)}****${s.slice(-4)}`;
}

export const api = {
  async collect() {
    if (!isTauri) {
      await new Promise((r) => setTimeout(r, 700));
      return { ok: true, profile: window.__MOCK__ };
    }
    return await invoke('collect_hardware');
  },

  async openExternal(url) {
    if (!isTauri) { window.open(url, '_blank'); return; }
    return await invoke('open_external', { url });
  },

  // ── 能力清单 ─────────────────────────────
  async capabilityList() {
    if (!isTauri) return [];
    return await invoke('capability_list');
  },

  async capabilityCall(id, args = {}) {
    if (!isTauri) return { ok: false, error: '浏览器预览不支持执行能力' };
    return await invoke('capability_call', { id, args });
  },

  // ── 四步编排 ─────────────────────────────
  async flowRecipes() {
    if (!isTauri) return [];
    return await invoke('flow_recipes');
  },

  async flowStart(recipe) {
    if (!isTauri) return mockFlow(recipe, 'start');
    return await invoke('flow_start', { recipe });
  },

  async flowAdvance(state, action) {
    if (!isTauri) return mockFlow(state.recipe, action, state);
    return await invoke('flow_advance', { state, action });
  },

  // ── 统一模型接口（本地 llama-server / 自定义 OpenAI API）──
  async llmConfigGet() {
    if (!isTauri) return loadPreviewLlm();
    return await invoke('llm_config_get');
  },

  async llmConfigSave(input) {
    if (!isTauri) {
      const prev = loadPreviewLlm();
      const next = {
        ...prev,
        mode: input.mode,
        localBaseUrl: input.localBaseUrl || 'http://127.0.0.1:18090/v1',
        localModel: input.localModel || 'local',
        customBaseUrl: input.customBaseUrl || '',
        customModel: input.customModel || '',
        disableThinking: input.disableThinking ?? prev.disableThinking,
      };
      if (input.customApiKey !== undefined && input.customApiKey !== null) {
        next.hasApiKey = !!input.customApiKey.trim();
        next.apiKeyMasked = maskPreviewKey(input.customApiKey);
      }
      try { localStorage.setItem(PREVIEW_LLM_KEY, JSON.stringify(next)); } catch {}
      return next;
    }
    return await invoke('llm_config_save', { input });
  },

  async llmTest(input = null) {
    if (!isTauri) {
      return { ok: false, latencyMs: 0, error: '浏览器预览不支持真实联网测试，请在桌面端测试。' };
    }
    return await invoke('llm_test', { input });
  },

  async llmChat(messages, hardwareSummary = null) {
    if (!isTauri) {
      return { ok: false, latencyMs: 0, error: '浏览器预览不支持调用模型，请在桌面端使用。' };
    }
    return await invoke('llm_chat', { messages, hardwareSummary });
  },

  // ── 受控 Agent（流式 + 待办规划 + 权限审查）──────
  onLlmStream(cb) {
    return listenEvent('llm-stream', cb);
  },

  async agentTurn(streamId, messages, hardwareSummary = null, permission = 'review') {
    if (!isTauri) {
      return mockAgentTurn(streamId, messages, permission);
    }
    return await invoke('agent_turn', { streamId, messages, hardwareSummary, permission });
  },

  async agentDecide(session, decision, permission = 'review') {
    if (!isTauri) {
      return mockAgentDecide(session, decision, permission);
    }
    return await invoke('agent_decide', { session, decision, permission });
  },

  async agentAuditList() {
    if (!isTauri) return [];
    return await invoke('agent_audit_list');
  },

  async agentAuditClear() {
    if (!isTauri) return;
    return await invoke('agent_audit_clear');
  },

  // ── 模型下载与下载源切换 ────────────────────
  onModelDownload(cb) {
    return listenEvent('model-download', cb);
  },

  async downloadConfigGet() {
    if (!isTauri) return loadPreviewDownloadCfg();
    return await invoke('download_config_get');
  },

  async downloadConfigSave(config) {
    if (!isTauri) {
      const next = {
        source: config.source || 'modelscope',
        customBaseUrl: String(config.customBaseUrl || '').trim().replace(/\/+$/, ''),
      };
      if (next.source === 'custom' && !/^https?:\/\//.test(next.customBaseUrl)) {
        throw new Error('自定义下载源地址须以 http:// 或 https:// 开头');
      }
      try { localStorage.setItem(PREVIEW_DL_KEY, JSON.stringify(next)); } catch {}
      return next;
    }
    return await invoke('download_config_save', { config });
  },

  async downloadListStatus() {
    if (!isTauri) {
      return [
        { tier: 'low', name: 'Qwen2.5-3B', filename: 'qwen2.5-3b-instruct-q4_k_m.gguf', downloaded: false, active: false, partBytes: 0, totalBytes: 2104932736 },
        { tier: 'mid', name: 'Qwen3-8B', filename: 'Qwen3-8B-Q4_K_M.gguf', downloaded: false, active: false, partBytes: 0, totalBytes: 5027783488 },
        { tier: 'high', name: 'Qwen3-14B', filename: 'Qwen3-14B-Q4_K_M.gguf', downloaded: false, active: false, partBytes: 0, totalBytes: 9000000000 },
        { tier: 'ultra', name: 'Qwen3-32B', filename: 'Qwen3-32B-Q4_K_M.gguf', downloaded: false, active: false, partBytes: 0, totalBytes: 19800000000 },
      ];
    }
    return await invoke('download_list_status');
  },

  async downloadModelStart(tier) {
    if (!isTauri) {
      startPreviewDownload(tier);
      return `开始模拟下载 ${tier}`;
    }
    return await invoke('download_model_start', { tier });
  },

  async downloadModelCancel(tier) {
    if (!isTauri) {
      const timer = PREVIEW_DL_TIMERS.get(tier);
      if (timer) {
        clearInterval(timer);
        PREVIEW_DL_TIMERS.delete(tier);
        emitPreview('model-download', { tier, status: 'paused', downloadedBytes: 1024 * 1024 * 600, totalBytes: 5027783488, speedBps: 0, percent: 12.0 });
        return true;
      }
      return false;
    }
    return await invoke('download_model_cancel', { tier });
  },

  // ── 本地推理引擎托管（自动启停）────────────
  async runtimeStatus() {
    if (!isTauri) {
      return {
        running: false,
        loading: false,
        managed: false,
        serverFound: true,
        modelFound: true,
        modelName: 'Qwen3-8B',
        endpoint: 'http://127.0.0.1:18090/v1',
        message: '预览模式：桌面端支持一键启停内置引擎',
      };
    }
    return await invoke('runtime_status');
  },

  async runtimeStart(tier = null) {
    if (!isTauri) {
      await new Promise((r) => setTimeout(r, 250));
      return {
        running: true,
        loading: false,
        managed: true,
        serverFound: true,
        modelFound: true,
        modelName: tier || 'Qwen3-8B',
        endpoint: 'http://127.0.0.1:18090/v1',
        message: '本地引擎运行中（预览模拟）',
      };
    }
    return await invoke('runtime_start', { tier });
  },

  async runtimeStop() {
    if (!isTauri) return true;
    return await invoke('runtime_stop');
  },

  // ── 导入本地 .gguf 模型（P2-b）────────────
  async downloadImportGguf(path) {
    if (!isTauri) {
      const prev = loadPreviewDownloadCfg();
      const next = { ...prev, importedModelPath: String(path || '').trim() };
      try { localStorage.setItem(PREVIEW_DL_KEY, JSON.stringify(next)); } catch {}
      return next;
    }
    return await invoke('download_import_gguf', { path });
  },

  async downloadClearImported() {
    if (!isTauri) {
      const prev = loadPreviewDownloadCfg();
      const next = { ...prev, importedModelPath: '' };
      try { localStorage.setItem(PREVIEW_DL_KEY, JSON.stringify(next)); } catch {}
      return next;
    }
    return await invoke('download_clear_imported');
  },

  async downloadPickGguf() {
    if (!isTauri) return 'D:\\models\\Qwen3-8B-Q4_K_M.gguf';
    return await invoke('download_pick_gguf');
  },

  // ── 目录授权白名单（P2-a）─────────────────
  async fsPolicyGet() {
    if (!isTauri) {
      try {
        const raw = localStorage.getItem('preview_fs_policy');
        if (raw) return JSON.parse(raw);
      } catch {}
      return { restrictToAllowed: false, allowedDirs: [] };
    }
    return await invoke('fs_policy_get');
  },

  async fsPolicySave(policy) {
    if (!isTauri) {
      try { localStorage.setItem('preview_fs_policy', JSON.stringify(policy)); } catch {}
      return policy;
    }
    return await invoke('fs_policy_save', { policy });
  },

  async fsPickFolder() {
    if (!isTauri) return 'D:\\123';
    return await invoke('fs_pick_folder');
  },

  // ── 小模型 GBNF 约束分诊（P1-a）───────────
  async triageClassify(query) {
    if (!isTauri) {
      const q = String(query || '');
      if (/转半天|转圈|不流畅/.test(q)) return { ok: true, intent: 'lag_diagnose', latencyMs: 35 };
      if (/变红|塞满/.test(q)) return { ok: true, intent: 'disk_biggest', latencyMs: 35 };
      if (/刷不出来|网通不通/.test(q)) return { ok: true, intent: 'net_check', latencyMs: 35 };
      return { ok: true, intent: 'dont_know', latencyMs: 35 };
    }
    return await invoke('triage_classify', { query });
  },

  // ── 多会话对话历史持久化 ────────────────────
  async historyLoad() {
    if (!isTauri) {
      try {
        const raw = localStorage.getItem('preview_chat_threads');
        if (raw) return JSON.parse(raw);
      } catch {}
      return [];
    }
    return await invoke('history_load');
  },

  async historySave(threads) {
    if (!isTauri) {
      try { localStorage.setItem('preview_chat_threads', JSON.stringify(threads)); } catch {}
      return;
    }
    return await invoke('history_save', { threads });
  },
};

function startPreviewDownload(tier) {
  if (PREVIEW_DL_TIMERS.has(tier)) return;
  let loaded = 0;
  const total = 5027783488;
  const step = 350 * 1024 * 1024;
  emitPreview('model-download', { tier, status: 'downloading', downloadedBytes: 0, totalBytes: total, speedBps: 8_400_000, percent: 0 });
  const timer = setInterval(() => {
    loaded += step;
    if (loaded >= total) {
      clearInterval(timer);
      PREVIEW_DL_TIMERS.delete(tier);
      emitPreview('model-download', { tier, status: 'done', downloadedBytes: total, totalBytes: total, speedBps: 0, percent: 100 });
    } else {
      emitPreview('model-download', {
        tier,
        status: 'downloading',
        downloadedBytes: loaded,
        totalBytes: total,
        speedBps: 8_400_000,
        percent: (loaded / total) * 100,
      });
    }
  }, 220);
  PREVIEW_DL_TIMERS.set(tier, timer);
}

// ── 浏览器预览用 Agent 流式演示（用于界面自测）────────
async function mockAgentTurn(streamId, messages, permission) {
  const last = (messages[messages.length - 1]?.content || '').trim();
  const wants123 = /123/.test(last) && /(建|创|删)/.test(last);

  if (!wants123) {
    const thinkParts = ['先理解一下问题，', '给出简洁人话回答。'];
    const bodyParts = ['这是流式预览回复：', '思考过程默认折叠，', '点上方标题就能展开看。'];
    for (const t of thinkParts) {
      await new Promise((r) => setTimeout(r, 80));
      emitPreview('llm-stream', { streamId, kind: 'thinking', delta: t });
    }
    for (const c of bodyParts) {
      await new Promise((r) => setTimeout(r, 80));
      emitPreview('llm-stream', { streamId, kind: 'content', delta: c });
    }
    return {
      ok: true,
      kind: 'chat',
      reply: bodyParts.join(''),
      thinking: thinkParts.join(''),
      model: 'preview',
      latencyMs: 420,
    };
  }

  emitPreview('llm-stream', { streamId, kind: 'thinking', delta: '用户想操作 D:\\123，拆成受控待办步骤。' });
  await new Promise((r) => setTimeout(r, 80));

  const isDel = /删/.test(last) && !/(建|创)/.test(last);
  const steps = isDel
    ? [{ id: 1, title: '删除空文件夹 D:\\123', capabilityId: 'fs.rmdir_empty', args: { path: 'D:\\123' }, risk: 'dangerous', riskLabel: '不可逆', needsConfirm: true, status: 'pending' }]
    : [{ id: 1, title: '在 D 盘根目录创建文件夹 123', capabilityId: 'fs.mkdir', args: { path: 'D:\\123' }, risk: 'reversible', riskLabel: '可撤销', needsConfirm: true, status: 'pending' }];

  const session = { id: 'preview-1', goal: last, permission, steps, status: 'running', summary: '' };
  if (permission === 'review') {
    session.steps[0].status = 'waiting_confirm';
    session.status = 'waiting_confirm';
    session.summary = `待办已建好，第 1 步「${session.steps[0].title}」需要你审查确认。`;
  } else {
    session.steps[0].status = 'done';
    session.steps[0].output = isDel ? '已删除空文件夹：D:\\123（预览模拟）' : '已创建文件夹：D:\\123（预览模拟）';
    session.status = 'done';
    session.summary = `任务完成（1/1 步）：${session.steps[0].output}。`;
  }
  return {
    ok: true,
    kind: 'task',
    reply: session.summary,
    thinking: '用户想操作 D:\\123，拆成受控待办步骤。',
    model: 'preview',
    latencyMs: 180,
    session,
  };
}

function mockAgentDecide(session, decision, permission) {
  const next = JSON.parse(JSON.stringify(session));
  next.permission = permission;
  const idx = next.steps.findIndex((s) => s.status === 'waiting_confirm');
  if (idx >= 0) {
    if (decision === 'approve') {
      next.steps[idx].status = 'done';
      next.steps[idx].output = `已执行：${next.steps[idx].args?.path || ''}（预览模拟）`;
      next.status = 'done';
      next.summary = `任务完成（1/1 步）：${next.steps[idx].output}。`;
    } else if (decision === 'skip') {
      next.steps[idx].status = 'skipped';
      next.steps[idx].output = '已跳过';
      next.status = 'done';
      next.summary = '已跳过当前步骤。';
    } else {
      next.steps[idx].status = 'skipped';
      next.status = 'cancelled';
      next.summary = '已取消剩余操作。';
    }
  }
  return next;
}

// ── 浏览器预览用的假流程（只为了让 UI 能演示）────────
function mockFlow(recipe, action) {
  const name = recipe === 'steam' ? 'Steam' : '7-Zip';
  if (action === 'start') {
    return {
      recipe, stage: 'already_installed',
      say: `${name} 已经装好了，在 D:\\steam。不用再装。`,
      actions: [], facts: { foundBy: '预览模式' },
    };
  }
  return {
    recipe, stage: 'failed',
    say: '浏览器预览里不能真正执行。请在桌面应用里试。',
    actions: [], facts: {},
  };
}
