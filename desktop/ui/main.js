const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open, message, confirm, save } = window.__TAURI__.dialog;
const t = (key) => window.i18n.t(key);

const els = {
  envBadge: document.getElementById('env-badge'),
  btnSetup: document.getElementById('btn-setup'),
  btnPick: document.getElementById('btn-pick'),
  btnPickImages: document.getElementById('btn-pick-images'),
  btnRefresh: document.getElementById('btn-refresh'),
  fileSearch: document.getElementById('file-search'),
  btnSelectAll: document.getElementById('btn-select-all'),
  btnSelectNone: document.getElementById('btn-select-none'),
  btnRun: document.getElementById('btn-run'),
  btnClearLog: document.getElementById('btn-clear-log'),
  btnExportLog: document.getElementById('btn-export-log'),
  folderPath: document.getElementById('folder-path'),
  fileList: document.getElementById('file-list'),
  fileCount: document.getElementById('file-count'),
  keepWork: document.getElementById('keep-work'),
  anyPosition: document.getElementById('any-position'),
  refine: document.getElementById('refine'),
  inverse: document.getElementById('inverse'),
  profileSelect: document.getElementById('profile-select'),
  profileLabel: document.getElementById('profile-label'),
  btnLearn: document.getElementById('btn-learn'),
  btnProfileDelete: document.getElementById('btn-profile-delete'),
  learnStatus: document.getElementById('learn-status'),
  log: document.getElementById('log'),
  reviewCandidate: document.getElementById('review-candidate'),
  reviewFinal: document.getElementById('review-final'),
  modelProgress: document.getElementById('model-progress'),
  modelProgressBar: document.getElementById('model-progress-bar'),
  modelProgressText: document.getElementById('model-progress-text'),
  logBox: document.getElementById('log-box'),
  resultBanner: document.getElementById('result-banner'),
  backupBanner: document.getElementById('backup-banner'),
  outputDirRow: document.getElementById('output-dir-row'),
  btnOutputDir: document.getElementById('btn-output-dir'),
  outputDirPath: document.getElementById('output-dir-path'),
  btnOutputDirReset: document.getElementById('btn-output-dir-reset'),
  resultList: document.getElementById('result-list'),
  resultListCount: document.getElementById('result-list-count'),
  resultListGrid: document.getElementById('result-list-grid'),
  dropOverlay: document.getElementById('drop-overlay'),
  btnMask: document.getElementById('btn-mask'),
  maskStatus: document.getElementById('mask-status'),
  btnMaskClear: document.getElementById('btn-mask-clear'),
  maskOverlay: document.getElementById('mask-overlay'),
  maskImg: document.getElementById('mask-img'),
  maskCanvasWrap: document.getElementById('mask-canvas-wrap'),
  maskRect: document.getElementById('mask-rect'),
  maskHint: document.getElementById('mask-hint'),
  btnMaskCancel: document.getElementById('btn-mask-cancel'),
  btnMaskReset: document.getElementById('btn-mask-reset'),
  btnMaskOk: document.getElementById('btn-mask-ok'),
  btnCancel: document.getElementById('btn-cancel'),
  runProgress: document.getElementById('run-progress'),
  runProgressBar: document.getElementById('run-progress-bar'),
  runProgressText: document.getElementById('run-progress-text'),
  outputRow: document.getElementById('output-row'),
  outputHint: document.getElementById('output-hint'),
  disclaimerOverlay: document.getElementById('disclaimer-overlay'),
  btnDisclaimerOk: document.getElementById('btn-disclaimer-ok'),
  setupHint: document.getElementById('setup-hint'),
  setupHintPath: document.getElementById('setup-hint-path'),
  setupHintWifi: document.getElementById('setup-hint-wifi'),
  btnSetupInline: document.getElementById('btn-setup-inline'),
  compareBox: document.getElementById('compare-box'),
  compareSource: document.getElementById('compare-source'),
  compareFinal: document.getElementById('compare-final'),
  compareHandle: document.getElementById('compare-handle'),
  lightbox: document.getElementById('lightbox'),
  lightboxStage: document.getElementById('lightbox-stage'),
  lightboxImg: document.getElementById('lightbox-img'),
  lightboxLoading: document.getElementById('lightbox-loading'),
  lightboxZoomLabel: document.getElementById('lightbox-zoom-label'),
  lightboxZoomIn: document.getElementById('lightbox-zoom-in'),
  lightboxZoomOut: document.getElementById('lightbox-zoom-out'),
  lightboxReset: document.getElementById('lightbox-reset'),
  lightboxClose: document.getElementById('lightbox-close'),
  lightboxPrev: document.getElementById('lightbox-prev'),
  lightboxNext: document.getElementById('lightbox-next'),
  lightboxCounter: document.getElementById('lightbox-counter'),
  lightboxCopy: document.getElementById('lightbox-copy'),
  lightboxReveal: document.getElementById('lightbox-reveal'),
  resultCompare: document.getElementById('result-compare'),
  rcStage: document.getElementById('rc-stage'),
  rcFinal: document.getElementById('rc-final'),
  rcSource: document.getElementById('rc-source'),
  rcHandle: document.getElementById('rc-handle'),
  rcLoading: document.getElementById('rc-loading'),
  rcFrame: document.getElementById('rc-frame'),
  rcPrev: document.getElementById('rc-prev'),
  rcNext: document.getElementById('rc-next'),
  rcCounter: document.getElementById('rc-counter'),
  rcReveal: document.getElementById('rc-reveal'),
  rcClose: document.getElementById('rc-close'),
  dSettings: document.getElementById('d-settings'),
  dSettingsClose: document.getElementById('d-settings-close'),
  dThemeSeg: document.getElementById('d-theme-seg'),
  dLangSeg: document.getElementById('d-lang-seg'),
  dModelStatus: document.getElementById('d-model-status'),
  dModelDownload: document.getElementById('d-model-download'),
  dBackupStatus: document.getElementById('d-backup-status'),
  dBackupRestore: document.getElementById('d-backup-restore'),
  dVersion: document.getElementById('d-version'),
  toastRoot: document.getElementById('toast-root'),
  // 移动端 App 化界面（v0.6）
  btnSettings: document.getElementById('btn-settings'),
  btnBackList: document.getElementById('btn-back-list'),
  btnTheme: document.getElementById('btn-theme'),
  btnLang: document.getElementById('btn-lang'),
  mRun: document.getElementById('m-run'),
  mRunRingFg: document.getElementById('m-run-ring-fg'),
  mRunCount: document.getElementById('m-run-count'),
  mRunStage: document.getElementById('m-run-stage'),
  mRunName: document.getElementById('m-run-name'),
  mRunEta: document.getElementById('m-run-eta'),
  mRunHint: document.getElementById('m-run-hint'),
  mRunCancel: document.getElementById('m-run-cancel'),
  mSettings: document.getElementById('m-settings'),
  mSettingsBack: document.getElementById('m-settings-back'),
  mSettingsOptions: document.getElementById('m-settings-options'),
  mThemeSeg: document.getElementById('m-theme-seg'),
  mLangSeg: document.getElementById('m-lang-seg'),
  mClearBatch: document.getElementById('m-clear-batch'),
  mAboutVersion: document.getElementById('m-about-version'),
  mViewIntro: document.getElementById('m-view-intro'),
  introOverlay: document.getElementById('intro-overlay'),
  btnIntroStart: document.getElementById('btn-intro-start'),
  lightboxSave: document.getElementById('lightbox-save'),
};

let targetRoot = null;
let running = false;
let importing = false;
let learning = false; // 学习水印档案（重 CPU 任务）：期间禁掉开始处理/导入/框选
let taskKind = null;
// 移动端模型下载：下载前所处的视图（empty/picked），下载结束/取消后恢复
let preSetupState = null;
// 环境检查（env_status）是否已完成：没完成前不自动下载模型（避免与 init 的视图切换打架）
let envChecked = false;
let lastRunCount = 0;
let lastModelPath = null;
let manualMask = null; // 相对右下角偏移 { dx1, dy1, dx2, dy2 }
let profiles = []; // 精确模式：可用水印档案
let selectedProfileId = null; // null = 自动识别
let runStartedAt = 0; // 本次处理开始时间（剩余时间估算）
let lastProgressName = ''; // 进度事件里正在处理的文件（用于列表状态）
// 桌面文件列表：文件名 → 卡片 DOM，处理进度/结果要逐张回写状态
let fileCards = new Map();
// 结果区：当前结果的逐张状态（done/skipped/nowm）与处理前源图目录
let resultStatus = new Map();
let resultSourceDir = null;
let resultMeta = new Map(); // basename → { state, reason }
// 覆盖模式备份（当前文件夹）信息，设置页与右侧提示共用
let backupState = { count: 0, dir: '' };
let appVersion = '';
const mobileParam = new URLSearchParams(location.search).get('mobile');
const isMobile =
  mobileParam === '1' ? true :
  mobileParam === '0' ? false :
  /android|iphone|ipad|ipod/i.test(navigator.userAgent);
const isMac = /mac/i.test(navigator.platform || navigator.userAgent);

function setState(state) {
  document.body.classList.remove('state-empty', 'state-picked', 'state-running', 'state-done');
  document.body.classList.add('state-' + state);
  // 全页滚动的视图切换：回到顶部，否则从列表中部切到结果页会看不到顶部横幅
  if (isMobile) window.scrollTo(0, 0);
  // 移动端结果页顶栏的「返回列表」只在结果页出现（桌面端始终隐藏）
  els.btnBackList.hidden = !(isMobile && state === 'done');
}

// 进度页副文案随任务类型切换（模型下载 / 去水印处理）；语言切换时由 applyDynamicLabels 重调
function setRunHint() {
  if (!els.mRunHint) return;
  const key = taskKind === 'model' ? 'downloadStayHint' : 'runStayHint';
  els.mRunHint.dataset.i18n = key;
  els.mRunHint.textContent = t(key);
}

function setRunning(value) {
  running = value;
  if (value) {
    runStartedAt = Date.now();
    lastProgressName = '';
  }
  document.body.setAttribute('aria-busy', value ? 'true' : 'false');
  els.btnRun.disabled = value || !targetRoot;
  els.btnRun.hidden = value;
  els.btnCancel.hidden = !value;
  els.btnPick.disabled = value;
  if (els.btnPickImages) els.btnPickImages.disabled = value || importing;
  if (els.fileSearch) els.fileSearch.disabled = value;
  els.btnSetup.disabled = value;
  if (els.dModelDownload) els.dModelDownload.disabled = value;
  els.btnRefresh.disabled = value || !targetRoot;
  els.btnMask.disabled = value || !targetRoot;
  els.btnLearn.disabled = value;
  els.btnProfileDelete.disabled = value;
  els.btnSettings.disabled = value;
  els.runProgress.hidden = !value;
  // 处理中锁定文件勾选：改选择只会让"开始处理（N）"与本次任务对不上，徒增困惑
  const checkboxes = els.fileList.querySelectorAll('input[type=checkbox]');
  checkboxes.forEach((el) => { el.disabled = value; });
  els.btnSelectAll.disabled = value || checkboxes.length === 0;
  els.btnSelectNone.disabled = value || checkboxes.length === 0;
  renderBackupUi();
  setRunHint();
  if (!value) {
    els.runProgressBar.style.width = '0%';
    els.runProgressText.textContent = '';
  } else if (isMobile) {
    // 移动端全屏进度页复位（等第一个 pipeline-progress 事件填充）
    els.mRunRingFg.style.strokeDashoffset = String(RUN_RING_LEN);
    els.mRunCount.textContent = '0/0';
    els.mRunStage.textContent = t('runPrep');
    els.mRunName.textContent = '';
    els.mRunEta.textContent = '';
  }
}

function overwriteMode() {
  // 移动端处理的始终是导入到应用私有目录的副本，"替换原图"只会误导用户
  // （相册里的原图根本不会被碰），固定走副本模式 + 保存到相册/分享导出。
  if (isMobile) return false;
  const checked = els.outputRow.querySelector('input[name=output-mode]:checked');
  return checked && checked.value === 'overwrite';
}

// 切换处理目录时回到最安全的「保存副本」模式（覆盖原图只应是一次显式选择）
function resetOutputModeForNewRoot() {
  const radio = els.outputRow.querySelector('input[name=output-mode][value=save]');
  if (radio) radio.checked = true;
  renderOutputHint();
}

function renderOutputHint() {
  els.outputHint.innerHTML = overwriteMode() ? t('outputHintOverwrite') : t('outputHintSave');
  els.outputHint.classList.toggle('warn', overwriteMode());
  els.outputDirRow.hidden = overwriteMode();
}

let outputDirOverride = null;

function renderOutputDir() {
  const custom = !!outputDirOverride;
  els.outputDirPath.textContent = custom ? outputDirOverride : t('outputDirDefault');
  els.outputDirPath.title = custom ? outputDirOverride : '';
  els.btnOutputDirReset.hidden = !custom;
}

// 用户设置持久化（纯 localStorage，零后端）：语言/主题各自另存，其余集中一个 JSON。
// 只存"跨会话仍成立"的偏好，不存与单张图绑定的状态（如手动框选）。
const SETTINGS_KEY = 'wm-settings';
const LAST_ROOT_KEY = 'wm-last-root';
// 移动端当前导入批次目录：pick 导入成功后写入；页面重建（rebuild_webview）或进程
// 重启后由 restoreLastFolder 用它把已导入列表恢复出来
const MOBILE_BATCH_KEY = 'wm-mobile-batch';

function readSettings() {
  try {
    return JSON.parse(localStorage.getItem(SETTINGS_KEY)) || {};
  } catch (_) {
    return {};
  }
}

function applySettings() {
  const s = readSettings();
  // 安全默认：每次启动都回到「保存副本」，不记忆「替换原图」——避免用户隔天打开
  // App、点一下开始就把原图覆盖掉。输出目录也只在本次会话有效（见 renderOutputDir）。
  if (typeof s.anyPosition === 'boolean') els.anyPosition.checked = s.anyPosition;
  if (typeof s.refine === 'boolean') els.refine.checked = s.refine;
  if (typeof s.inverse === 'boolean') els.inverse.checked = s.inverse;
  if (typeof s.keepWork === 'boolean') els.keepWork.checked = s.keepWork;
  // 档案 id 待 refreshProfiles() 后校验；已被删除的会在 renderProfiles 里回落成自动
  if (s.profileId) selectedProfileId = s.profileId;
  renderOutputHint();
  renderOutputDir();
}

function persistSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify({
      anyPosition: els.anyPosition.checked,
      refine: els.refine.checked,
      inverse: els.inverse.checked,
      keepWork: els.keepWork.checked,
      profileId: selectedProfileId || '',
    }));
  } catch (_) { /* 隐私模式等禁用存储时忽略 */ }
}

function rememberRoot(root) {
  if (isMobile || !root) return;
  try { localStorage.setItem(LAST_ROOT_KEY, root); } catch (_) {}
}

// 启动时恢复上次目录；目录已被删/移动就清掉记录，不打扰用户。
// 移动端恢复的是"上次导入批次"（pick 导入成功时写入 MOBILE_BATCH_KEY）——
// rebuild_webview 重建页面与进程重启后靠它把列表拉回来。
async function restoreLastFolder() {
  if (isMobile) {
    const batchDir = localStorage.getItem(MOBILE_BATCH_KEY);
    if (!batchDir) return;
    try {
      const names = await invoke('list_pngs', { root: batchDir, lang: window.i18n.lang });
      if (!names.length) {
        try { localStorage.removeItem(MOBILE_BATCH_KEY); } catch (_) {}
        return;
      }
      targetRoot = batchDir;
      clearMaskSelection();
      renderFolderPath();
      renderFiles(names);
      // 上次处理的结果还在（watermark-cleaned/）：直接进结果视图，用户可继续保存/分享；
      // 没有结果则停在列表页。
      await maybeRestoreMobileResults(batchDir);
    } catch (_) {
      try { localStorage.removeItem(MOBILE_BATCH_KEY); } catch (_) {}
    }
    return;
  }
  const lastRoot = localStorage.getItem(LAST_ROOT_KEY);
  if (!lastRoot) return;
  try {
    const names = await invoke('list_pngs', { root: lastRoot, lang: window.i18n.lang });
    targetRoot = lastRoot;
    els.btnRefresh.disabled = false;
    clearMaskSelection();
    renderFolderPath();
    renderFiles(names);
    await refreshBackupInfo();
  } catch (_) {
    try { localStorage.removeItem(LAST_ROOT_KEY); } catch (_) {}
  }
}

const STAGE_LABEL = () => ({ prepare: t('stagePrepare'), inpaint: t('stageInpaint'), save: t('stageSave') });

// 重启/页面重建（rebuild_webview）后恢复"上次处理结果"：输出目录里还有图就进结果
// 视图，用户能继续保存到相册/分享，不必重跑管线。banner 用「上次处理结果」文案，
// 不冒充"刚刚处理完成"；没有结果则停留在列表页。
async function maybeRestoreMobileResults(batchDir) {
  const base = batchDir.replace(/\/+$/, '');
  let outNames = [];
  try {
    outNames = await invoke('list_pngs', { root: base + '/watermark-cleaned', lang: window.i18n.lang });
  } catch (_) {
    return;
  }
  if (!outNames.length) return;
  const outputs = outNames.map((n) => `${base}/watermark-cleaned/${n}`);
  const statusMap = new Map();
  outNames.forEach((n) => statusMap.set(n, { state: 'done' }));
  lastExitPayload = {
    success: true,
    restored: true,
    outputs,
    processed: outputs.length,
    outputDir: `${base}/watermark-cleaned`,
    noWatermark: [],
    skipped: [],
  };
  resultSourceDir = base; // 移动端恒副本模式：对比源图在批次目录、与输出同名
  renderExitBanner(lastExitPayload);
  renderResultList(outputs, statusMap);
  setState('done');
  const pairs = outputs.map((p) => ({ final: p, source: `${base}/${basename(p)}` }));
  if (pairs.length) setupMobileCompare(pairs);
}

// 移动端进度环：周长 2πr（r=52）
const RUN_RING_LEN = 2 * Math.PI * 52;

// 阶段权重：进度条单调前进（旧实现每个阶段各自 0→100%，保存阶段会让进度条"倒退"）
const STAGE_WEIGHT = { prepare: [0.02, 0.15], inpaint: [0.15, 0.9], save: [0.9, 1] };

function formatRemain(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return '';
  const sec = Math.round(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.round(sec / 60);
  if (min < 60) return `${min}m`;
  const h = Math.floor(min / 60);
  return `${h}h ${min % 60}m`;
}

function setRunProgress(stage, done, total, name) {
  const labels = STAGE_LABEL();
  const stageText = labels[stage] || stage;
  const frac = total > 0 ? Math.min(1, done / total) : 0;
  const [lo, hi] = STAGE_WEIGHT[stage] || [0, 1];
  const overall = Math.min(1, lo + (hi - lo) * frac);
  els.runProgressBar.style.width = Math.round(overall * 100) + '%';
  // 剩余时间：至少跑过 3 秒、进度 > 2% 才有参考价值
  let eta = '';
  const elapsed = Date.now() - runStartedAt;
  if (runStartedAt && elapsed > 3000 && overall > 0.02 && overall < 0.995) {
    eta = formatRemain((elapsed / overall) * (1 - overall));
  }
  els.runProgressText.textContent =
    t('progressLine')(stageText, done, total, name) + (eta ? t('progressEta')(eta) : '');
  if (isMobile) {
    // 全屏进度页：环 + 张数 + 当前文件 + 剩余时间（长任务让用户有预期）
    els.mRunRingFg.style.strokeDashoffset = String(RUN_RING_LEN * (1 - overall));
    els.mRunCount.textContent = `${done}/${total}`;
    els.mRunStage.textContent = stageText;
    els.mRunName.textContent = name || '';
    els.mRunEta.textContent = eta ? t('runEta')(eta) : '';
  } else if (name) {
    markRunningFile(name);
  }
}

function logLine(text) {
  els.log.textContent += text + '\n';
  els.log.scrollTop = els.log.scrollHeight;
}

// —— 轻提示（Toast）——
// 移动端日志区默认折叠，导出/分享/导入这类操作的成败只写日志等于没有反馈；
// 统一用浮层提示。key 相同的提示原地复用（导入进度每秒刷新不闪烁），
// sticky 提示不自动消失（进行中状态），由调用方替换或关闭。
const TOAST_LIMIT = 3;

function showToast(message, opts) {
  if (!els.toastRoot || !message) return null;
  const { type = 'info', key = '', sticky = false, timeout = type === 'error' ? 5000 : 3200 } = opts || {};
  let el = null;
  if (key) {
    el = els.toastRoot.querySelector(`.toast[data-key="${CSS.escape(key)}"]`);
  }
  if (el) {
    el.className = 'toast toast-' + type;
    el.textContent = message;
  } else {
    el = document.createElement('div');
    el.className = 'toast toast-' + type + ' toast-enter';
    el.textContent = message;
    if (key) el.dataset.key = key;
    el.addEventListener('click', () => el.remove());
    els.toastRoot.appendChild(el);
    while (els.toastRoot.children.length > TOAST_LIMIT) els.toastRoot.firstChild.remove();
  }
  if (el._toastTimer) clearTimeout(el._toastTimer);
  if (!sticky) {
    el._toastTimer = setTimeout(() => el.remove(), timeout);
  } else {
    el._toastTimer = null;
  }
  return el;
}

function dismissToast(key) {
  if (!els.toastRoot || !key) return;
  const el = els.toastRoot.querySelector(`.toast[data-key="${CSS.escape(key)}"]`);
  if (el) el.remove();
}

function showReview(container, path) {
  invoke('read_image_base64', { path, lang: window.i18n.lang })
    .then((preview) => {
      const dataUrl = preview.data_url;
      container.innerHTML = '';
      const img = document.createElement('img');
      img.src = dataUrl;
      img.alt = t('viewLarge');
      img.title = t('viewLarge');
      img.addEventListener('click', () => {
        lbIndex = -1; // 复查预览图不在结果列表里，不启用左右切换
        updateLightboxNav();
        openLightbox(dataUrl);
      });
      container.appendChild(img);
      container.classList.add('has-image');
    })
    .catch((err) => {
      container.innerHTML = `<div class="placeholder">${escapeHtml(t('previewFailed')(String(err)))}</div>`;
    });
}

// —— 大图预览灯箱：高清加载 + 滚轮/按钮缩放 + 拖拽平移 ——
// 关键：结果缩略图只有 200px，点开必须按**原图路径**重新读高清图，否则放大后是糊的。
const LB_MAX_SCALE = 8;
const LB_MIN_FACTOR = 0.2; // 相对"适应窗口"的最小缩放
let lbScale = 1, lbTx = 0, lbTy = 0, lbFit = 1;
let lbNatural = { w: 0, h: 0 };
// 结果列表里当前预览的是第几张（-1 = 非列表来源，如复查预览图）→ 决定是否显示左右切换
let lbIndex = -1;

function updateLightboxNav() {
  const has = resultPaths.length > 1 && lbIndex >= 0;
  const onResult = lbIndex >= 0;
  els.lightboxPrev.hidden = !has;
  els.lightboxNext.hidden = !has;
  els.lightboxCounter.hidden = !has;
  if (els.lightboxCopy) els.lightboxCopy.hidden = !(onResult && isMac);
  if (els.lightboxReveal) els.lightboxReveal.hidden = !(onResult && !isMobile);
  // 移动端灯箱单张「保存到相册」（结果是应用内副本，随手存某一张是高频需求）
  if (els.lightboxSave) els.lightboxSave.hidden = !(onResult && isMobile);
  if (has) els.lightboxCounter.textContent = t('imageCounter')(lbIndex + 1, resultPaths.length);
}

// 灯箱内切换上一张/下一张（首尾循环），复用列表顺序
function lightboxStep(delta) {
  const n = resultPaths.length;
  if (n < 2 || lbIndex < 0) return;
  lbIndex = ((lbIndex + delta) % n + n) % n;
  openLightboxPath(resultPaths[lbIndex]);
}

function setLightboxTransform() {
  els.lightboxImg.style.transform =
    `translate(-50%, -50%) translate(${lbTx}px, ${lbTy}px) scale(${lbScale})`;
  els.lightboxZoomLabel.textContent = Math.round(lbScale * 100) + '%';
  els.lightboxZoomIn.disabled = lbScale >= LB_MAX_SCALE - 0.001;
  els.lightboxZoomOut.disabled = lbScale <= Math.max(lbFit * LB_MIN_FACTOR, 0.05) + 0.001;
}

function lightboxFitScale() {
  const stage = els.lightboxStage.getBoundingClientRect();
  if (!lbNatural.w || !lbNatural.h || !stage.width || !stage.height) return 1;
  return Math.min(stage.width / lbNatural.w, stage.height / lbNatural.h);
}

function resetLightboxView() {
  lbFit = lightboxFitScale();
  lbScale = lbFit;
  lbTx = 0;
  lbTy = 0;
  setLightboxTransform();
}

function openLightbox(dataUrl) {
  els.lightboxLoading.hidden = true;
  els.lightboxImg.hidden = false;
  els.lightboxImg.src = dataUrl;
  els.lightbox.classList.add('open');
  const onLoad = () => {
    lbNatural = { w: els.lightboxImg.naturalWidth, h: els.lightboxImg.naturalHeight };
    resetLightboxView();
  };
  if (els.lightboxImg.complete && els.lightboxImg.naturalWidth) onLoad();
  else els.lightboxImg.addEventListener('load', onLoad, { once: true });
}

async function openLightboxPath(path) {
  lbIndex = resultPaths.indexOf(path);
  updateLightboxNav();
  els.lightboxImg.hidden = true;
  els.lightboxImg.src = '';
  els.lightboxLoading.hidden = false;
  els.lightbox.classList.add('open');
  try {
    const preview = await invoke('read_image_base64', { path, max: 4096, lang: window.i18n.lang });
    if (!els.lightbox.classList.contains('open')) return; // 加载期间用户已关闭
    openLightbox(preview.data_url);
  } catch (err) {
    closeLightbox();
    logLine(t('previewFailed')(String(err)));
  }
}

function closeLightbox() {
  els.lightbox.classList.remove('open');
  els.lightboxImg.hidden = true;
  els.lightboxImg.src = '';
  els.lightboxLoading.hidden = true;
  els.lightboxStage.classList.remove('grabbing');
  lbNatural = { w: 0, h: 0 };
  lbIndex = -1;
  updateLightboxNav();
}

// 以屏幕坐标点为锚缩放：该点下的图像像素保持不动（缩放体验自然）
function zoomLightboxAt(clientX, clientY, factor) {
  const stage = els.lightboxStage.getBoundingClientRect();
  const cx = clientX - stage.left - stage.width / 2;
  const cy = clientY - stage.top - stage.height / 2;
  const minScale = Math.max(lbFit * LB_MIN_FACTOR, 0.05);
  const next = Math.max(minScale, Math.min(LB_MAX_SCALE, lbScale * factor));
  if (Math.abs(next - lbScale) < 1e-4) return;
  const px = (cx - lbTx) / lbScale;
  const py = (cy - lbTy) / lbScale;
  lbScale = next;
  lbTx = cx - px * next;
  lbTy = cy - py * next;
  setLightboxTransform();
}

function bindLightbox() {
  const stage = els.lightboxStage;
  let dragging = false;
  let downOnImg = false;
  let moved = false;
  let startX = 0, startY = 0, startTx = 0, startTy = 0;
  // 双指缩放（触屏）：pointer 事件在多指下语义复杂，单独用 touch 事件实现
  let pinching = false;
  let pinchStartDist = 0;
  let pinchStartScale = 1;

  const touchDist = (touches) =>
    Math.hypot(touches[0].clientX - touches[1].clientX, touches[0].clientY - touches[1].clientY);

  stage.addEventListener('pointerdown', (ev) => {
    if (els.lightboxImg.hidden) return;
    if (!ev.isPrimary) {
      // 第二根手指落下 → 进入双指缩放，终止单指拖动
      pinching = true;
      dragging = false;
      stage.classList.remove('grabbing');
      return;
    }
    if (pinching) return;
    dragging = true;
    moved = false;
    downOnImg = ev.target === els.lightboxImg;
    startX = ev.clientX;
    startY = ev.clientY;
    startTx = lbTx;
    startTy = lbTy;
    stage.classList.add('grabbing');
    try { stage.setPointerCapture(ev.pointerId); } catch (_) {}
  });
  stage.addEventListener('pointermove', (ev) => {
    if (pinching || !dragging || !ev.isPrimary) return;
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
    if (Math.abs(dx) > 4 || Math.abs(dy) > 4) moved = true;
    lbTx = startTx + dx;
    lbTy = startTy + dy;
    setLightboxTransform();
  });
  const endDrag = (ev) => {
    if (!ev.isPrimary || !dragging) return;
    dragging = false;
    stage.classList.remove('grabbing');
    try {
      if (stage.hasPointerCapture && stage.hasPointerCapture(ev.pointerId)) {
        stage.releasePointerCapture(ev.pointerId);
      }
    } catch (_) {}
    // 单击图片外的空白 → 关闭；拖动或点图片本身不关闭
    if (!moved && !downOnImg) closeLightbox();
  };
  stage.addEventListener('pointerup', endDrag);
  stage.addEventListener('pointercancel', endDrag);
  stage.addEventListener('touchstart', (ev) => {
    if (els.lightboxImg.hidden || ev.touches.length !== 2) return;
    pinching = true;
    dragging = false;
    stage.classList.remove('grabbing');
    pinchStartDist = touchDist(ev.touches);
    pinchStartScale = lbScale;
    ev.preventDefault();
  }, { passive: false });
  stage.addEventListener('touchmove', (ev) => {
    if (!pinching || ev.touches.length !== 2) return;
    ev.preventDefault();
    const dist = touchDist(ev.touches);
    if (pinchStartDist <= 0 || dist <= 0) return;
    const minScale = Math.max(lbFit * LB_MIN_FACTOR, 0.05);
    const target = Math.max(minScale, Math.min(LB_MAX_SCALE, pinchStartScale * (dist / pinchStartDist)));
    const cx = (ev.touches[0].clientX + ev.touches[1].clientX) / 2;
    const cy = (ev.touches[0].clientY + ev.touches[1].clientY) / 2;
    zoomLightboxAt(cx, cy, target / lbScale);
  }, { passive: false });
  const endPinch = (ev) => {
    if (!pinching) return;
    if (ev.touches.length >= 2) return;
    pinching = false;
    if (ev.touches.length === 1) {
      // 松开一指后无缝继续单指拖动：以剩余手指为新起点
      dragging = true;
      downOnImg = false;
      moved = true;
      startX = ev.touches[0].clientX;
      startY = ev.touches[0].clientY;
      startTx = lbTx;
      startTy = lbTy;
      stage.classList.add('grabbing');
    }
  };
  stage.addEventListener('touchend', endPinch);
  stage.addEventListener('touchcancel', endPinch);
  stage.addEventListener('dblclick', (ev) => {
    if (els.lightboxImg.hidden) return;
    if (Math.abs(lbScale - lbFit) < 0.01) zoomLightboxAt(ev.clientX, ev.clientY, (lbFit * 2) / lbScale);
    else resetLightboxView();
  });
  stage.addEventListener('wheel', (ev) => {
    if (els.lightboxImg.hidden) return;
    ev.preventDefault();
    zoomLightboxAt(ev.clientX, ev.clientY, ev.deltaY < 0 ? 1.15 : 1 / 1.15);
  }, { passive: false });

  const centerZoom = (factor) => {
    const r = stage.getBoundingClientRect();
    zoomLightboxAt(r.left + r.width / 2, r.top + r.height / 2, factor);
  };
  els.lightboxZoomIn.addEventListener('click', () => centerZoom(1.25));
  els.lightboxZoomOut.addEventListener('click', () => centerZoom(1 / 1.25));
  els.lightboxReset.addEventListener('click', resetLightboxView);
  els.lightboxClose.addEventListener('click', closeLightbox);
  els.lightboxPrev.addEventListener('click', () => lightboxStep(-1));
  els.lightboxNext.addEventListener('click', () => lightboxStep(1));
  els.lightboxCopy.addEventListener('click', async () => {
    const path = resultPaths[lbIndex];
    if (!path) return;
    try {
      await invoke('copy_image_to_clipboard', { path, lang: window.i18n.lang });
      showToast(t('copyDone'), { type: 'success' });
    } catch (err) {
      logLine(t('logError')(String(err)));
      showToast(String(err), { type: 'error' });
    }
  });
  els.lightboxReveal.addEventListener('click', () => {
    const path = resultPaths[lbIndex];
    if (!path) return;
    invoke('reveal_path', { path, lang: window.i18n.lang })
      .catch((err) => logLine(t('logError')(String(err))));
  });
  els.lightboxSave.addEventListener('click', async () => {
    const path = resultPaths[lbIndex];
    if (!path) return;
    els.lightboxSave.disabled = true;
    try {
      const n = await invoke('export_results', { paths: [path], lang: window.i18n.lang });
      if (n > 0) showToast(t('exportDone')(n), { type: 'success' });
      else showToast(t('exportNone'), { type: 'error' });
    } catch (err) {
      logLine(t('logError')(String(err)));
      showToast(t('exportFailed')(String(err)), { type: 'error' });
    } finally {
      els.lightboxSave.disabled = false;
    }
  });
  window.addEventListener('resize', () => {
    if (els.lightbox.classList.contains('open') && !els.lightboxImg.hidden) resetLightboxView();
  });
}

// Android 返回键（MainActivity 经 evaluateJavascript 调用）：
// 返回 true = 已消费（关闭了弹层或正在处理），返回 false = 交由原生退出应用。
let backExitAt = 0;
window.__onAndroidBack = () => {
  const lightbox = document.getElementById('lightbox');
  if (lightbox && lightbox.classList.contains('open')) {
    closeLightbox();
    return true;
  }
  if (!els.maskOverlay.hidden) {
    closeMaskEditor();
    return true;
  }
  if (!els.disclaimerOverlay.hidden) {
    els.disclaimerOverlay.hidden = true;
    return true;
  }
  // 设置页打开：返回键先关设置页
  if (document.body.classList.contains('m-settings-open')) {
    closeSettings();
    return true;
  }
  // 处理中拦下返回，避免进程被杀导致任务中断、输出半成品；按返回没反应会让用户
  // 以为应用卡死，故明确提示一次（同 key 复用，连按不刷屏）
  if (running) {
    showToast(t('backBlockedToast'), { type: 'error', key: 'back-block' });
    return true;
  }
  // 结果页：返回列表页（结果保留、可再次导出），而不是直接退出
  if (document.body.classList.contains('state-done')) {
    setState('picked');
    return true;
  }
  // 已有导入批次时退出会丢掉整个会话（框选/结果都没有持久化）：2.5 秒内连按两次才退出
  if (targetRoot && Date.now() - backExitAt > 2500) {
    backExitAt = Date.now();
    showToast(t('backExitHint'), { key: 'back-exit' });
    return true;
  }
  return false;
};

// —— 移动端：App 式布局与设置页 ——

// Android 相册选择器（Photo Picker）：结果由原生侧经此全局回调异步回传
// （与 __onAndroidBack 同一模式；空数组 = 用户取消）。
let pendingPickResolve = null;
window.__onAndroidPickResult = (uris) => {
  const resolve = pendingPickResolve;
  pendingPickResolve = null;
  if (resolve) resolve(Array.isArray(uris) ? uris.filter(Boolean) : []);
};

// Kotlin 侧开始拷贝相册文件时调用：拷贝可能持续数秒~十几秒且发生在 Rust 导入之前，
// 不给提示用户会以为"选了图没反应"（尤其是慢机/大图批次）。
window.__onAndroidPickCopyStart = (count) => {
  const n = Number(count) || 0;
  if (n <= 0) return;
  showToast(t('importPreparing')(n), { key: 'import', sticky: true });
};

function pickImagesAndroid() {
  return new Promise((resolve, reject) => {
    pendingPickResolve = resolve;
    invoke('pick_images', { max: 0 }).catch((err) => {
      pendingPickResolve = null;
      reject(err);
    });
  });
}

function openSettings() {
  if (!isMobile) return;
  syncSettingsUi();
  // 「关于」区版本号（init 已缓存，失败为空字符串）
  if (els.mAboutVersion) els.mAboutVersion.textContent = appVersion ? t('dVersion')(appVersion) : '';
  document.body.classList.add('m-settings-open');
}

function closeSettings() {
  document.body.classList.remove('m-settings-open');
}

// 移动端 DOM 重排（仅 isMobile；桌面端结构保持不动）：
// 1.「添加图片」按钮从面板头搬进底部操作栏（与「开始处理」并排）
// 2. 高级选项/精确模式收进设置页（普通用户主流程零配置）
// 3. 前后对比 + 复查图包进折叠面板（结果页以"汇总 + 结果网格"为主）
function setupMobileLayout() {
  const runButtons = document.querySelector('.run-buttons');
  if (runButtons && els.btnPick) {
    runButtons.insertBefore(els.btnPick, runButtons.firstChild);
  }
  // 移动端"导出日志"改为"分享日志"：save 保存对话框是桌面语义，Android 上没有
  // （点击后的行为由 btn-export-log 的 click handler 按平台分流）
  els.btnExportLog.dataset.i18n = 'shareLog';
  if (els.mSettingsOptions) {
    document.querySelectorAll('.run-area > .advanced-options').forEach((d) => {
      els.mSettingsOptions.appendChild(d);
    });
    // 副本模式下没有备份概念，keep-work 对移动端无意义
    if (els.keepWork && els.keepWork.parentElement) {
      els.keepWork.parentElement.style.display = 'none';
    }
  }
  const rightPanel = document.querySelector('.right-panel');
  if (rightPanel && els.logBox) {
    const box = document.createElement('details');
    box.className = 'collapse';
    const sum = document.createElement('summary');
    sum.dataset.i18n = 'viewCompare';
    sum.textContent = t('viewCompare');
    box.appendChild(sum);
    if (els.compareBox) box.appendChild(els.compareBox);
    const reviewGrid = rightPanel.querySelector('.review-grid');
    if (reviewGrid) box.appendChild(reviewGrid);
    rightPanel.insertBefore(box, els.logBox);
  }
}

// 设置页交互绑定（仅移动端调用）
function bindMobileSettings() {
  els.btnSettings.addEventListener('click', openSettings);
  els.mSettingsBack.addEventListener('click', closeSettings);
  els.mThemeSeg.querySelectorAll('button').forEach((b) => {
    b.addEventListener('click', () => setThemeMode(b.dataset.mode));
  });
  els.mLangSeg.querySelectorAll('button').forEach((b) => {
    b.addEventListener('click', () => {
      const lang = b.dataset.lang;
      if (lang !== window.i18n.lang) {
        window.i18n.setLang(lang);
        applyLang();
      }
    });
  });
  els.mClearBatch.addEventListener('click', async () => {
    const ok = await confirm(t('clearBatchConfirm'), {
      title: t('clearBatchConfirmTitle'),
      kind: 'warning',
      okLabel: t('ok'),
      cancelLabel: t('cancel'),
    }).catch(() => false);
    if (!ok) return;
    try {
      const n = await invoke('clear_import_batch');
      targetRoot = null;
      try { localStorage.removeItem(MOBILE_BATCH_KEY); } catch (_) {}
      clearReviews();
      clearMaskSelection();
      renderFiles([]);
      closeSettings();
      logLine(t('batchClearedLog'));
      if (n > 0) showToast(t('clearBatchDone')(n), { type: 'success' });
    } catch (err) {
      logLine(t('logError')(String(err)));
      showToast(t('logError')(String(err)), { type: 'error' });
    }
  });
  els.mRunCancel.addEventListener('click', () => els.btnCancel.click());
  els.btnBackList.addEventListener('click', () => setState('picked'));
  // 「关于」区：回看首次启动引导（不重置 wm-intro-ok，仅再展示一次）
  els.mViewIntro.addEventListener('click', () => {
    closeSettings();
    els.introOverlay.hidden = false;
  });
}

function escapeHtml(text) {
  return text.replace(/[&<>"']/g, (ch) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[ch]);
}

// 前后对比 slider：左拖右滑看处理前/后差异（两张复查拼图尺寸一致）
async function setupCompare(sourcePath, finalPath) {
  try {
    const [sourcePreview, finalPreview] = await Promise.all([
      invoke('read_image_base64', { path: sourcePath, lang: window.i18n.lang }),
      invoke('read_image_base64', { path: finalPath, lang: window.i18n.lang }),
    ]);
    els.compareSource.src = sourcePreview.data_url;
    els.compareFinal.src = finalPreview.data_url;
    els.compareBox.hidden = false;
    setComparePos(50);
  } catch (_) {
    els.compareBox.hidden = true;
  }
}

function setComparePos(pct) {
  const pos = Math.max(0, Math.min(100, pct));
  els.compareSource.style.clipPath = `inset(0 ${100 - pos}% 0 0)`;
  els.compareHandle.style.left = pos + '%';
}

// —— 移动端逐张前后对比（整图）——
// 桌面端沿用「角落复查拼图」滑块（setupCompare）；移动端改为逐张结果图整图对比：
// 处理前=批次内原图、处理后=输出图，多张时可左右切换、单张时隐藏导航。
// 拼图对比在手机上是一张随张数变长的长条，无法放大细节、也不能只看某一张。
const MCOMPARE_CACHE_MAX = 3;
let mCompare = { pairs: [], index: 0, cache: new Map(), nav: null, loading: 0 };

function ensureCompareNav() {
  if (mCompare.nav) return mCompare.nav;
  const nav = document.createElement('div');
  nav.className = 'compare-nav';
  nav.hidden = true;
  const mkBtn = (cls, labelKey, glyph) => {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = cls;
    btn.dataset.i18nAria = labelKey;
    btn.setAttribute('aria-label', t(labelKey));
    btn.textContent = glyph;
    // 按钮上的指针事件不进入滑块拖动/点击跳转
    btn.addEventListener('pointerdown', (ev) => ev.stopPropagation());
    btn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      stepCompare(cls === 'compare-prev' ? -1 : 1);
    });
    return btn;
  };
  const prev = mkBtn('compare-prev', 'prevImage', '‹');
  const next = mkBtn('compare-next', 'nextImage', '›');
  const counter = document.createElement('span');
  counter.className = 'compare-counter';
  nav.append(prev, counter, next);
  els.compareBox.appendChild(nav);
  mCompare.nav = { el: nav, prev, next, counter };
  return mCompare.nav;
}

function updateCompareNav() {
  const nav = mCompare.nav;
  if (!nav) return;
  const n = mCompare.pairs.length;
  nav.el.hidden = n <= 1;
  nav.counter.textContent = n > 0 ? t('imageCounter')(mCompare.index + 1, n) : '';
  nav.prev.disabled = n <= 1;
  nav.next.disabled = n <= 1;
}

function stepCompare(delta) {
  const n = mCompare.pairs.length;
  if (n <= 1) return;
  showMobileCompareAt(((mCompare.index + delta) % n + n) % n);
}

async function showMobileCompareAt(i) {
  const pair = mCompare.pairs[i];
  if (!pair) return;
  mCompare.index = i;
  updateCompareNav();
  const cached = mCompare.cache.get(pair.source);
  if (cached) {
    applyCompareImages(cached[0], cached[1]);
    return;
  }
  mCompare.loading += 1;
  els.compareBox.classList.add('loading');
  let fallbackIdx = -1;
  try {
    const [sourcePreview, finalPreview] = await Promise.all([
      invoke('read_image_base64', { path: pair.source, lang: window.i18n.lang }),
      invoke('read_image_base64', { path: pair.final, lang: window.i18n.lang }),
    ]);
    // LRU：只留最近 3 张，大图 base64 常驻多了内存吃紧
    mCompare.cache.set(pair.source, [sourcePreview.data_url, finalPreview.data_url]);
    while (mCompare.cache.size > MCOMPARE_CACHE_MAX) {
      mCompare.cache.delete(mCompare.cache.keys().next().value);
    }
    if (mCompare.index === i) applyCompareImages(sourcePreview.data_url, finalPreview.data_url);
  } catch (_) {
    // 个别原图读不出（理论上副本模式必然存在）：剔除该张，退到就近一张
    fallbackIdx = mCompare.pairs.indexOf(pair);
    if (fallbackIdx >= 0) mCompare.pairs.splice(fallbackIdx, 1);
  } finally {
    mCompare.loading = Math.max(0, mCompare.loading - 1);
    if (mCompare.loading === 0) els.compareBox.classList.remove('loading');
  }
  if (fallbackIdx >= 0) {
    if (mCompare.pairs.length === 0) {
      els.compareBox.hidden = true;
      updateCompareNav();
    } else {
      showMobileCompareAt(Math.max(0, Math.min(fallbackIdx, mCompare.pairs.length - 1)));
    }
  }
}

function applyCompareImages(sourceUrl, finalUrl) {
  els.compareSource.src = sourceUrl;
  els.compareFinal.src = finalUrl;
  els.compareBox.hidden = false;
  setComparePos(50);
}

async function setupMobileCompare(pairs) {
  mCompare.pairs = pairs.slice();
  mCompare.index = 0;
  mCompare.cache.clear();
  mCompare.loading = 0;
  ensureCompareNav();
  updateCompareNav();
  await showMobileCompareAt(0);
}

let envReady = false;

async function refreshEnv() {
  try {
    const status = await invoke('env_status');
    envChecked = true;
    envReady = status.ready;
    // Keep data-i18n in sync so a later applyI18n() (e.g. language toggle)
    // preserves the real state instead of resetting to "检查环境中…".
    els.envBadge.dataset.i18n = status.ready ? 'envReady' : 'envNotReady';
    els.envBadge.textContent = t(status.ready ? 'envReady' : 'envNotReady');
    els.envBadge.className = 'badge ' + (status.ready ? 'ok' : 'bad');
    // 就绪徽章只作为"缺模型"的告警，不要长期占着顶栏
    els.envBadge.hidden = status.ready;
    els.setupHint.hidden = status.ready;
    els.btnSetup.hidden = status.ready;
    els.btnSetupInline.hidden = status.ready;
    if (!status.ready) {
      lastModelPath = status.model_path;
      els.setupHintPath.textContent = t('modelSavePath')(status.model_path);
      els.setupHintWifi.hidden = !isMobile;
      logLine(t('logEnv')(status.hint));
    }
    return status.ready;
  } catch (err) {
    envChecked = true;
    envReady = false;
    els.envBadge.dataset.i18n = 'envFailed';
    els.envBadge.textContent = t('envFailed');
    els.envBadge.className = 'badge bad';
    els.envBadge.hidden = false;
    logLine(t('logEnv')(String(err)));
    return false;
  }
}

// 桌面端：把"选中的图片文件"变成"其所在文件夹 + 只勾选这些文件"。
// 关键：绝不再走移动端的 import_files（复制到应用私有目录）——桌面用户要的是原地处理，
// 覆盖模式也必须作用于真实原图，而不是用户看不见的副本。
function parentDir(path) {
  const s = String(path || '').replace(/[\\/]+$/, '');
  const i = Math.max(s.lastIndexOf('/'), s.lastIndexOf('\\'));
  return i > 0 ? s.slice(0, i) : '';
}

async function openImagesFromPaths(paths) {
  if (running || importing) return;
  const all = Array.isArray(paths) ? paths.filter(Boolean) : [];
  const images = all.filter((p) => /\.(png|jpe?g|webp)$/i.test(p));
  const heic = all.some((p) => /\.(heic|heif)$/i.test(p));
  if (images.length === 0) {
    showToast(heic ? t('dragHeic') : t('dropNoImage'), { type: 'error', timeout: 5000 });
    return;
  }
  const dirs = [...new Set(images.map(parentDir))];
  if (dirs.length > 1 || !dirs[0]) {
    showToast(t('dragSameFolder'), { type: 'error', timeout: 5000 });
    return;
  }
  targetRoot = dirs[0];
  rememberRoot(targetRoot);
  els.btnRefresh.disabled = false;
  clearReviews();
  clearMaskSelection();
  outputDirOverride = null; // 输出目录不跨文件夹沿用，避免结果写到上次的目录
  renderOutputDir();
  resetOutputModeForNewRoot();
  if (els.fileSearch) els.fileSearch.value = '';
  await refreshFiles(images.map((p) => basename(p)));
  if (heic) showToast(t('dragHeic'), { type: 'info', timeout: 6000 });
}

async function refreshFiles(selectedNames) {
  if (!targetRoot) return;
  renderFolderPath();
  try {
    const names = await invoke('list_pngs', { root: targetRoot, lang: window.i18n.lang });
    renderFiles(names, selectedNames);
  } catch (err) {
    els.fileList.innerHTML = `<div class="placeholder">${escapeHtml(t('readFailed')(String(err)))}</div>`;
  }
  await refreshBackupInfo();
}

// —— 原图备份（覆盖模式可撤销）——
async function refreshBackupInfo() {
  if (!targetRoot) {
    backupState = { count: 0, dir: '' };
    renderBackupUi();
    return;
  }
  try {
    backupState = await invoke('backup_info', { root: targetRoot });
  } catch (_) {
    backupState = { count: 0, dir: '' };
  }
  renderBackupUi();
}

function renderBackupUi() {
  const count = backupState.count || 0;
  if (els.dBackupStatus) {
    els.dBackupStatus.textContent = count > 0 && targetRoot
      ? `${t('dBackupFound')(count)} ${t('dBackupHint')}`
      : t('dBackupNone');
    els.dBackupRestore.disabled = !(count > 0) || running;
  }
  if (!els.backupBanner) return;
  const show = !isMobile && count > 0 && els.resultBanner.hidden && !running;
  if (!show) {
    els.backupBanner.hidden = true;
    return;
  }
  els.backupBanner.hidden = false;
  els.backupBanner.className = 'result-banner warn';
  els.backupBanner.innerHTML = '';
  const text = document.createElement('div');
  text.className = 'banner-main';
  text.textContent = t('backupFoundBanner')(count);
  els.backupBanner.appendChild(text);
  const btn = document.createElement('button');
  btn.className = 'btn small';
  btn.type = 'button';
  btn.textContent = t('restoreNow');
  btn.addEventListener('click', restoreOriginals);
  els.backupBanner.appendChild(btn);
}

// 「恢复原图」统一入口：本次结果横幅 / 备份提示 / 设置页共用
async function restoreOriginals() {
  if (!targetRoot || running) return;
  const ok = await confirm(t('restoreConfirm'), {
    title: t('restoreOriginal'),
    kind: 'warning',
    okLabel: t('ok'),
    cancelLabel: t('cancel'),
  }).catch(() => false);
  if (!ok) return;
  try {
    const n = await invoke('restore_backup', { root: targetRoot, lang: window.i18n.lang });
    logLine(t('restoreDone')(n));
    showToast(t('restoreDone')(n), { type: 'success' });
    clearReviews();
    els.resultBanner.hidden = false;
    els.resultBanner.className = 'result-banner ok';
    els.resultBanner.innerHTML = '';
    const box = document.createElement('div');
    box.className = 'banner-main';
    box.textContent = `✓ ${t('restoreDone')(n)}`;
    els.resultBanner.appendChild(box);
    await refreshFiles();
  } catch (err) {
    logLine(t('restoreFailed')(String(err)));
    showToast(t('restoreFailed')(String(err)), { type: 'error' });
  }
}

const PICK_ICON =
  '<svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">' +
  '<rect x="6" y="10" width="36" height="28" rx="4"/><circle cx="17" cy="20" r="3.5"/>' +
  '<path d="M6 33l10-9 7 6 8-8 11 11"/></svg>';

let lastEmptyKey = null;

function renderEmptyState(messageKey) {
  lastEmptyKey = messageKey || (isMobile ? 'emptyMobile' : 'emptyDesktop');
  const title = t(lastEmptyKey);
  if (isMobile) {
    // 小白引导：把"怎么用"直接写在空态里（选图 → 开始 → 存相册）
    const steps = [t('emptyStep1'), t('emptyStep2'), t('emptyStep3')]
      .map((text, i) => '<li><span class="empty-step-num">' + (i + 1) + '</span><span>' + text + '</span></li>')
      .join('');
    els.fileList.innerHTML =
      '<div class="empty-state">' +
      '<div class="empty-icon">' + PICK_ICON + '</div>' +
      '<p class="empty-title">' + title + '</p>' +
      '<ul class="empty-steps">' + steps + '</ul>' +
      '<button class="btn primary" data-action="pick" type="button">' + t('pickPhotos') + '</button>' +
      '</div>';
  } else {
    // 桌面左栏高度有限（下面还有框选/保存方式/高级选项）：操作按钮必须第一屏可见，
    // 三张竖向步骤卡在 860x600 下会被裁掉，改为紧凑提示 + 两个入口按钮。
    els.fileList.innerHTML =
      '<div class="empty-state">' +
      '<div class="empty-icon">' + PICK_ICON + '</div>' +
      '<p class="empty-title">' + title + '</p>' +
      '<p class="empty-hint">' + t('emptyStep1Desk') + ' → ' + t('emptyStep2Desk') + '</p>' +
      '<div class="empty-actions">' +
      '<button class="btn primary" data-action="pick" type="button">' + t('pickFolder') + '</button>' +
      '<button class="btn" data-action="pick-images" type="button">' + t('pickImages') + '</button>' +
      '</div>' +
      '<p class="empty-note">' + t('emptyDesktopNote') + ' ' + t('emptyUnsupportedNote') + '</p>' +
      '</div>';
  }
}

// —— 桌面文件列表：缩略图行 / 逐张状态 / 搜索 ——
function fileStateText(state) {
  return {
    running: t('fileStateRunning'),
    done: t('fileStateDone'),
    skipped: t('fileStateSkipped'),
    nowm: t('fileStateNoWatermark'),
  }[state] || '';
}

function setFileState(name, state, reason) {
  const entry = fileCards.get(name);
  if (!entry || !entry.stateEl) return;
  const el = entry.stateEl;
  if (!state) {
    el.textContent = '';
    delete el.dataset.state;
    el.removeAttribute('title');
    return;
  }
  el.textContent = fileStateText(state);
  el.dataset.state = state;
  if (reason) el.title = reason;
  else el.removeAttribute('title');
}

function clearFileStates() {
  for (const name of fileCards.keys()) setFileState(name, '');
  lastProgressName = '';
}

function markRunningFile(name) {
  if (lastProgressName && lastProgressName !== name) setFileState(lastProgressName, '');
  lastProgressName = name || '';
  if (name) setFileState(name, 'running');
}

function applyFileSearch() {
  if (isMobile || !els.fileSearch) return;
  const q = (els.fileSearch.value || '').trim().toLowerCase();
  let visible = 0;
  els.fileList.querySelectorAll('.file-item').forEach((item) => {
    const show = !q || (item.dataset.name || '').toLowerCase().includes(q);
    item.classList.toggle('hidden-by-search', !show);
    if (show) visible += 1;
  });
  let empty = els.fileList.querySelector('.file-search-empty');
  if (!empty) {
    empty = document.createElement('div');
    empty.className = 'placeholder file-search-empty';
    els.fileList.appendChild(empty);
  }
  empty.textContent = t('searchEmpty');
  empty.hidden = !(q && visible === 0);
}

function renderFiles(names, selectedNames) {
  els.fileList.innerHTML = '';
  fileCards.clear();
  thumbQueue.length = 0; // 列表重渲染：取消遗留的缩略图任务（img 已脱离 DOM）
  setState(names.length > 0 ? 'picked' : 'empty');
  els.fileCount.textContent = t('fileCount')(names.length);
  els.btnSelectAll.disabled = importing || names.length === 0;
  els.btnSelectNone.disabled = importing || names.length === 0;
  els.btnRun.disabled = running || importing || names.length === 0;
  if (names.length === 0) {
    renderEmptyState(isMobile ? 'emptyNoPngMobile' : 'emptyNoPngDesktop');
    updateRunButton();
    return;
  }
  if (isMobile) {
    renderFileGrid(names);
    updateRunButton();
    return;
  }
  // 拖拽/选择图片时只勾选用户点名的那些；打开文件夹/恢复上次目录时默认全选
  const selectedSet = selectedNames ? new Set(selectedNames) : null;
  for (const name of names) {
    const item = document.createElement('label');
    item.className = 'file-item';
    item.dataset.name = name;

    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.value = name;
    checkbox.checked = selectedSet ? selectedSet.has(name) : true;
    checkbox.disabled = running || importing;
    checkbox.addEventListener('change', updateRunButton);
    item.appendChild(checkbox);

    const thumb = document.createElement('img');
    thumb.className = 'file-thumb';
    thumb.alt = '';
    thumb.loading = 'lazy';
    thumbQueue.push({
      path: targetRoot.replace(/\/+$/, '') + '/' + name,
      img: thumb,
    });
    pumpThumbQueue();
    item.appendChild(thumb);

    const label = document.createElement('span');
    label.className = 'file-name';
    label.textContent = name;
    label.title = name;
    item.appendChild(label);

    const stateEl = document.createElement('span');
    stateEl.className = 'file-state';
    item.appendChild(stateEl);

    els.fileList.appendChild(item);
    fileCards.set(name, { card: item, thumb, stateEl });
  }
  applyFileSearch();
  updateRunButton();
}

// 移动端文件网格：缩略图卡片（整卡点选/取消，左上角 × 单张移除）
function renderFileGrid(names) {
  for (const name of names) {
    const card = document.createElement('label');
    card.className = 'm-file-card';

    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.value = name;
    checkbox.checked = true;
    checkbox.disabled = running || importing;
    checkbox.addEventListener('change', updateRunButton);
    card.appendChild(checkbox);

    const thumb = document.createElement('div');
    thumb.className = 'm-file-thumb';
    const img = document.createElement('img');
    img.alt = name;
    thumb.appendChild(img);
    card.appendChild(thumb);
    // 缩略图懒加载：复用结果列表的 3 并发队列（读 200px JPEG/PNG 缩略图）
    thumbQueue.push({ path: targetRoot.replace(/\/+$/, '') + '/' + name, img });
    pumpThumbQueue();

    const check = document.createElement('span');
    check.className = 'm-file-check';
    check.textContent = '✓';
    card.appendChild(check);

    const del = document.createElement('button');
    del.className = 'm-file-del';
    del.type = 'button';
    del.textContent = '×';
    del.title = t('removeImageHint');
    del.addEventListener('click', async (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (running || importing) return;
      // 局部移除：只摘掉这张卡片（不再 refreshFiles 整表重建——那会把
      // 所有缩略图重新读取并重放渐入动画，删除一张的代价变成整页闪烁）
      card.classList.add('removing');
      try {
        await Promise.all([
          invoke('remove_import_file', { dir: targetRoot, name, lang: window.i18n.lang }),
          new Promise((r) => setTimeout(r, 180)), // 与 .removing 动画时长一致
        ]);
        // 缩略图队列里可能还有这张未发起的任务，摘掉省一次无效读取
        for (let i = thumbQueue.length - 1; i >= 0; i--) {
          if (thumbQueue[i].img === img) thumbQueue.splice(i, 1);
        }
        card.remove();
        const remain = els.fileList.querySelectorAll('.m-file-card').length;
        if (remain === 0) {
          renderFiles([]); // 删空：走空态（同步 selectAll/btnRun 等全部状态）
        } else {
          els.fileCount.textContent = t('fileCount')(remain);
          updateRunButton();
        }
      } catch (err) {
        card.classList.remove('removing');
        logLine(t('logError')(String(err)));
        showToast(t('removeImageFail')(String(err)), { type: 'error' });
      }
    });
    card.appendChild(del);

    const label = document.createElement('span');
    label.className = 'm-file-name';
    label.textContent = name;
    card.appendChild(label);

    els.fileList.appendChild(card);
  }
}

function selectedFiles() {
  return [...els.fileList.querySelectorAll('input[type=checkbox]:checked')].map((el) => el.value);
}

function updateRunButton() {
  const boxes = [...els.fileList.querySelectorAll('input[type=checkbox]')];
  const count = boxes.filter((el) => el.checked).length;
  // 老 WebView（Chromium <105，如 ColorOS 自带 97）不支持 :has()：勾选态落到卡片
  // class 上（勾选圈/边框高亮），任何勾选变化路径都会经过这里，无需逐处同步
  boxes.forEach((el) => {
    const card = el.closest('.m-file-card');
    if (card) card.classList.toggle('checked', el.checked);
  });
  els.btnRun.textContent = count > 0 ? `${t('run')}（${count}）` : t('run');
  els.btnRun.disabled = running || importing || learning || !targetRoot || count === 0;
  if (els.btnPickImages) els.btnPickImages.disabled = running || importing || learning;
  els.btnMask.disabled = running || importing || learning || !targetRoot || count === 0;
  updateMaskUi();
}

function clearMaskSelection() {
  manualMask = null;
  updateMaskUi();
}

function setPickLabel() {
  // 移动端始终是"追加"语义（可多次添加）；桌面端仍为选择文件夹
  els.btnPick.textContent = isMobile ? t('addImages') : t('pickFolder');
}

function renderFolderPath() {
  els.folderPath.textContent = targetRoot || t('noFolder');
}

// Re-apply text owned by JS (elements without static data-i18n), so a language
// switch never clobbers dynamic state back to a default label.
function applyDynamicLabels() {
  setPickLabel();
  renderFolderPath();
  setMaskButtons();
  renderOutputHint();
  renderOutputDir();
  renderProfileOptions();
  setRunHint();
  if (els.lightboxSave) els.lightboxSave.textContent = t('lightboxSave');
  els.profileLabel.placeholder = t('profileLabelPlaceholder');
  els.fileCount.textContent = t('fileCount')(els.fileList.querySelectorAll('input[type=checkbox]').length);
  if (lastModelPath) els.setupHintPath.textContent = t('modelSavePath')(lastModelPath);
  updateRunButton();
  if (document.body.classList.contains('state-empty')) renderEmptyState(lastEmptyKey);
  if (lastExitPayload) renderExitBanner(lastExitPayload);
}

// —— 主题（默认跟随系统，手动切换后记忆；设置页与顶栏按钮共用） ——
function themeMode() {
  return localStorage.getItem('wm-theme') || 'auto';
}

function applyTheme(mode) {
  const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  const dark = mode === 'dark' || (mode !== 'light' && prefersDark);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  const icon = document.getElementById('btn-theme-icon');
  if (icon) icon.textContent = dark ? t('themeLight') : t('themeDark');
  // Android edge-to-edge：状态栏/导航栏透明，图标深浅须与 App 主题同步（否则
  // 深色主题 + 系统浅色时深色图标压在深色背景上看不清）；失败静默不影响主题
  if (isMobile) invoke('set_system_bars', { dark }).catch(() => {});
  syncSettingsUi();
}

function setThemeMode(mode) {
  localStorage.setItem('wm-theme', mode);
  applyTheme(mode);
}

// —— 语言（顶栏按钮 + 手机设置页共用） ——
function syncMenuLang() {
  if (window.__WM_HAS_MENU) {
    invoke('set_menu_lang', { lang: window.i18n.lang }).catch((err) => {
      // 菜单重建失败（加速键/平台）只影响菜单文案，不影响处理；记一行便于排查
      logLine(t('logMenuLangFailed')(String(err)));
    });
  }
}

function applyLang() {
  els.btnLang.textContent = window.i18n.lang === 'zh' ? 'EN' : '中';
  window.i18n.applyI18n();
  applyDynamicLabels();
  syncMenuLang();
  syncSettingsUi();
}

// 设置页分段按钮的选中态（主题/语言）：在主题或语言变化后同步
function syncSettingsUi() {
  const mode = themeMode();
  const lang = window.i18n.lang;
  for (const seg of [els.mThemeSeg, els.dThemeSeg]) {
    if (!seg) continue;
    seg.querySelectorAll('button[data-mode]').forEach((b) => {
      b.classList.toggle('active', b.dataset.mode === mode);
    });
  }
  for (const seg of [els.mLangSeg, els.dLangSeg]) {
    if (!seg) continue;
    seg.querySelectorAll('button[data-lang]').forEach((b) => {
      b.classList.toggle('active', b.dataset.lang === lang);
    });
  }
}

// —— 桌面设置面板（Cmd+, / 菜单「设置…」）——
function openDesktopSettings() {
  if (isMobile) return;
  syncSettingsUi();
  refreshSettingsInfo();
  els.dSettings.hidden = false;
}

function closeDesktopSettings() {
  els.dSettings.hidden = true;
}

async function refreshSettingsInfo() {
  try {
    if (!appVersion) appVersion = await invoke('app_version');
    els.dVersion.textContent = appVersion ? t('dVersion')(appVersion) : '';
  } catch (_) {
    els.dVersion.textContent = '';
  }
  try {
    const status = await invoke('env_status');
    els.dModelStatus.textContent = status.ready
      ? `${t('modelReadyText')} · ${t('modelPathLabel')(status.model_path)}`
      : t('modelMissingText');
    els.dModelDownload.hidden = !!status.ready;
    els.dModelDownload.textContent = t('setupModel');
  } catch (err) {
    els.dModelStatus.textContent = String(err);
    els.dModelDownload.hidden = false;
  }
  renderBackupUi();
}

function bindDesktopSettings() {
  if (!els.dSettings) return;
  els.dSettingsClose.addEventListener('click', closeDesktopSettings);
  els.dSettings.addEventListener('click', (ev) => {
    if (ev.target === els.dSettings) closeDesktopSettings();
  });
  els.dThemeSeg.querySelectorAll('button').forEach((b) => {
    b.addEventListener('click', () => setThemeMode(b.dataset.mode));
  });
  els.dLangSeg.querySelectorAll('button').forEach((b) => {
    b.addEventListener('click', () => {
      if (b.dataset.lang !== window.i18n.lang) {
        window.i18n.setLang(b.dataset.lang);
        applyLang();
      }
    });
  });
  els.dModelDownload.addEventListener('click', () => {
    if (!running) startModelDownload(false);
  });
  els.dBackupRestore.addEventListener('click', restoreOriginals);
}

// —— 精确模式：水印档案（学习/选择/删除）——
function renderProfileOptions() {
  const sel = els.profileSelect;
  sel.innerHTML = '';
  const auto = document.createElement('option');
  auto.value = '';
  auto.textContent = t('profileAuto');
  sel.appendChild(auto);
  for (const p of profiles) {
    const opt = document.createElement('option');
    opt.value = p.id;
    opt.textContent = `${p.label || p.id}${p.builtin ? ` · ${t('profileBuiltin')}` : ''} (${p.width}×${p.height})`;
    sel.appendChild(opt);
  }
  sel.value = profiles.some((p) => p.id === selectedProfileId) ? selectedProfileId : '';
  selectedProfileId = sel.value || null;
  renderProfileDelete();
}

function renderProfileDelete() {
  const current = profiles.find((p) => p.id === selectedProfileId);
  els.btnProfileDelete.hidden = !current || current.builtin;
}

async function refreshProfiles() {
  try {
    profiles = (await invoke('list_watermarks')) || [];
  } catch (_) {
    profiles = [];
  }
  renderProfileOptions();
}

// 学习水印档案期间锁住会串状态的操作：学习是重 CPU 同步命令（数秒~数十秒），
// 与处理/导入并发会互相拖慢，也让"开始处理（N）"与任务对不上。
function setLearning(value) {
  learning = value;
  const busy = value || running || importing;
  els.btnLearn.disabled = busy;
  els.btnProfileDelete.disabled = busy;
  els.btnPick.disabled = busy;
  els.btnRefresh.disabled = busy || !targetRoot;
  if (els.fileSearch) els.fileSearch.disabled = busy;
  els.fileList.querySelectorAll('input[type=checkbox]').forEach((el) => { el.disabled = busy; });
  const count = els.fileList.querySelectorAll('input[type=checkbox]').length;
  els.btnSelectAll.disabled = busy || count === 0;
  els.btnSelectNone.disabled = busy || count === 0;
  updateRunButton();
}

async function learnProfile() {
  const files = selectedFiles();
  if (!targetRoot) {
    els.learnStatus.textContent = t('learnNeedRoot');
    return;
  }
  if (files.length < 3) {
    els.learnStatus.textContent = t('learnNeedFiles');
    return;
  }
  const paths = files.map((name) => `${targetRoot}/${name}`);
  setLearning(true);
  els.learnStatus.textContent = t('learnRunning');
  try {
    const res = await invoke('learn_watermark', {
      root: targetRoot,
      files: paths,
      label: els.profileLabel.value.trim(),
      lang: window.i18n.lang,
    });
    await refreshProfiles();
    selectedProfileId = res.id;
    renderProfileOptions();
    els.learnStatus.textContent = t('learnDone')(res.label || res.id, Number(res.mean_score || 0).toFixed(2));
  } catch (err) {
    els.learnStatus.textContent = String(err);
  } finally {
    setLearning(false);
  }
}

function updateMaskUi() {
  const has = !!manualMask;
  els.btnMaskClear.hidden = !has;
  // 显示可读的框选尺寸（约 W×H 像素），原始右下角偏移量对普通用户没有意义
  const w = has ? Math.abs(manualMask.dx2 - manualMask.dx1) : 0;
  const h = has ? Math.abs(manualMask.dy2 - manualMask.dy1) : 0;
  els.maskStatus.textContent = has
    ? t('maskSelectedLabel')(`${w}×${h}`)
    : t('maskAuto');
  els.maskStatus.classList.toggle('manual', has);
}

// —— 手动框选水印区域 ——
let maskSel = null;
// 预览可能被缩小（大图），框选坐标要按它映射回原图尺寸
let maskOrigin = null; // 原图像素坐标 { x1, y1, x2, y2 }
// 移动端框选双指缩放：img 与 maskRect 共用同一变换（transform-origin: 0 0）。
// maskImgPoint 用 getBoundingClientRect（含变换）映射到原图坐标；renderMaskRect 用
// offsetWidth（布局尺寸、不含变换）换算内容坐标，缩放后框与图仍对齐。
let maskZoom = { s: 1, tx: 0, ty: 0 };

function applyMaskZoom() {
  const z = maskZoom;
  const tr = (z.s === 1 && z.tx === 0 && z.ty === 0)
    ? ''
    : `translate(${z.tx}px, ${z.ty}px) scale(${z.s})`;
  els.maskImg.style.transform = tr;
  els.maskRect.style.transform = tr;
}

function resetMaskZoom() {
  maskZoom = { s: 1, tx: 0, ty: 0 };
  applyMaskZoom();
}

function maskImgPoint(ev) {
  const img = els.maskImg;
  const rect = img.getBoundingClientRect();
  const scale = img.naturalWidth / rect.width;
  const x = (ev.clientX - rect.left) * scale;
  const y = (ev.clientY - rect.top) * scale;
  return {
    x: Math.max(0, Math.min(img.naturalWidth, x)),
    y: Math.max(0, Math.min(img.naturalHeight, y)),
  };
}

function renderMaskRect() {
  if (!maskSel) {
    els.maskRect.hidden = true;
    return;
  }
  const img = els.maskImg;
  // 用布局宽度（offsetWidth，不含 transform）：框选指示层与 img 同受 maskZoom 变换，
  // 指示层坐标必须写在"内容坐标"里，否则缩放后框会二次缩放、与图片错位
  const scale = img.offsetWidth / img.naturalWidth;
  els.maskRect.hidden = false;
  els.maskRect.style.left = `${Math.min(maskSel.x1, maskSel.x2) * scale}px`;
  els.maskRect.style.top = `${Math.min(maskSel.y1, maskSel.y2) * scale}px`;
  els.maskRect.style.width = `${Math.abs(maskSel.x2 - maskSel.x1) * scale}px`;
  els.maskRect.style.height = `${Math.abs(maskSel.y2 - maskSel.y1) * scale}px`;
}

function setMaskButtons() {
  els.btnMaskOk.disabled = !maskSel;
  els.btnMaskReset.disabled = !maskSel;
  if (maskSel) {
    const k = maskOrigin && els.maskImg.naturalWidth ? maskOrigin.w / els.maskImg.naturalWidth : 1;
    const w = Math.round(Math.abs(maskSel.x2 - maskSel.x1) * k);
    const h = Math.round(Math.abs(maskSel.y2 - maskSel.y1) * k);
    els.maskHint.textContent = t('maskHintSelected')(w, h);
  } else {
    els.maskHint.textContent = t('maskHint');
  }
}

async function openMaskEditor() {
  const files = selectedFiles();
  if (files.length === 0 || !targetRoot || running) return;
  const path = targetRoot.replace(/\/+$/, '') + '/' + files[0];
  els.maskRect.hidden = true;
  maskSel = null;
  maskOrigin = null;
  resetMaskZoom();
  setMaskButtons();
  els.maskImg.src = '';
  els.maskOverlay.hidden = false;
  try {
    // 大图只取缩略图（原 10MB 上限会让大图直接无法框选）；原图尺寸用于坐标映射
    const preview = await invoke('read_image_base64', { path, max: 2048, lang: window.i18n.lang });
    maskOrigin = { w: preview.width, h: preview.height };
    els.maskImg.src = preview.data_url;
  } catch (err) {
    els.maskOverlay.hidden = true;
    logLine(t('logMaskFailed')(String(err)));
  }
}

function closeMaskEditor() {
  els.maskOverlay.hidden = true;
  els.maskImg.src = '';
  resetMaskZoom();
}

function confirmMaskSelection() {
  if (!maskSel || !maskOrigin) return;
  const img = els.maskImg;
  // 预览若是缩小过的，先把框选坐标换算回原图像素
  const k = img.naturalWidth ? maskOrigin.w / img.naturalWidth : 1;
  manualMask = {
    dx1: Math.round(Math.min(maskSel.x1, maskSel.x2) * k - maskOrigin.w),
    dy1: Math.round(Math.min(maskSel.y1, maskSel.y2) * k - maskOrigin.h),
    dx2: Math.round(Math.max(maskSel.x1, maskSel.x2) * k - maskOrigin.w),
    dy2: Math.round(Math.max(maskSel.y1, maskSel.y2) * k - maskOrigin.h),
  };
  updateMaskUi();
  logLine(`[${t('maskSetLog')}] ${manualMask.dx1}, ${manualMask.dy1}, ${manualMask.dx2}, ${manualMask.dy2}`);
  closeMaskEditor();
}

function resetReviews() {
  els.resultBanner.hidden = true;
  els.reviewCandidate.innerHTML = `<div class="placeholder"><span class="spinner"></span>${t('processingHint')}</div>`;
  els.reviewFinal.innerHTML = `<div class="placeholder"><span class="spinner"></span>${t('processingHint')}</div>`;
}

// 选新文件夹/导入后清空旧预览，不显示"正在处理"
function clearReviews() {
  els.resultBanner.hidden = true;
  els.reviewCandidate.innerHTML = `<div class="placeholder">${t('placeholderBefore')}</div>`;
  els.reviewFinal.innerHTML = `<div class="placeholder">${t('placeholderAfter')}</div>`;
  els.reviewCandidate.classList.remove('has-image');
  els.reviewFinal.classList.remove('has-image');
  clearResultList();
  clearCompare();
}

// 清空前后对比 slider：避免"恢复原图"后仍显示已失效的处理结果对比
function clearCompare() {
  els.compareBox.hidden = true;
  els.compareBox.classList.remove('loading');
  els.compareSource.removeAttribute('src');
  els.compareFinal.removeAttribute('src');
  mCompare.pairs = [];
  mCompare.cache.clear();
  mCompare.loading = 0;
  if (mCompare.nav) mCompare.nav.el.hidden = true;
}

function basename(p) {
  const parts = String(p).split(/[\\/]/);
  return parts[parts.length - 1] || String(p);
}

let resultPaths = [];
const thumbQueue = [];
let thumbActive = 0;
const THUMB_CONCURRENCY = 3;

// 桌面完成通知：批量任务要几分钟，用户多半已经切走去看别的
const notif = window.__TAURI__.notification ? window.__TAURI__.notification : null;
let notifGranted = false;

async function initNotifications() {
  if (!notif) return;
  try {
    notifGranted = await notif.isPermissionGranted();
    if (!notifGranted) notifGranted = (await notif.requestPermission()) === 'granted';
  } catch (_) {
    notifGranted = false;
  }
}

async function notifyDone(title, body) {
  if (!notif || !notifGranted || !title) return;
  // "用户正看着"就不打扰：桌面看窗口聚焦（hasFocus=false 表示切走了）；
  // Android WebView 切后台后 hasFocus() 依然恒为 true（实测 focusLog 全 1），
  // 必须改用 visibilityState——否则通知永远被这个判断挡掉
  const watching = isMobile ? document.visibilityState === 'visible' : document.hasFocus();
  if (watching) return;
  try {
    if (isMobile) {
      // 移动端经 IPC 直传 autoCancel（插件 JS 封装不透传该字段）：点击通知即消失，
      // 否则通知会一直挂在通知栏直到用户手动划掉
      await window.__TAURI_INTERNALS__.invoke('plugin:notification|notify', {
        options: { title, body: body || '', autoCancel: true },
      });
    } else {
      await notif.sendNotification({ title, body: body || '' });
    }
  } catch (_) { /* 通知失败不影响主流程 */ }
}

function clearResultList() {
  resultPaths = [];
  thumbQueue.length = 0;
  els.resultList.hidden = true;
  els.resultListGrid.innerHTML = '';
  els.resultListCount.textContent = '';
}

function pumpThumbQueue() {
  while (thumbActive < THUMB_CONCURRENCY && thumbQueue.length > 0) {
    const job = thumbQueue.shift();
    thumbActive += 1;
    invoke('read_thumbnail_base64', { path: job.path, max: 200, lang: window.i18n.lang })
      .then((dataUrl) => {
        job.img.src = dataUrl;
        job.img.dataset.ready = '1';
        job.img.classList.remove('pending');
      })
      .catch(() => { job.img.classList.remove('pending'); })
      .finally(() => {
        thumbActive -= 1;
        pumpThumbQueue();
      });
  }
}

/// 结果列表：逐张缩略图（点击大图预览；桌面端可逐张对比/定位文件）。
function renderResultList(paths, statusMap) {
  clearResultList();
  const list = (paths || []).filter(Boolean);
  if (list.length === 0) return;
  resultPaths = list;
  resultStatus = statusMap instanceof Map ? statusMap : new Map();
  resultMeta = new Map();
  els.resultList.hidden = false;
  els.resultListCount.textContent = t('resultListCount')(list.length);
  list.forEach((path) => {
    const name = basename(path);
    const meta = resultStatus.get(name) || { state: 'done' };
    resultMeta.set(name, meta);
    const item = document.createElement('div');
    item.className = 'result-item';
    item.dataset.name = name;
    item.tabIndex = 0;
    item.setAttribute('role', 'button');
    item.setAttribute('aria-label', `${name} · ${t('viewLarge')}`);

    const open = () => openLightboxPath(path);
    item.addEventListener('click', open);
    item.addEventListener('keydown', (ev) => {
      if (ev.target !== item) return;
      if (ev.key === 'Enter' || ev.key === ' ') {
        ev.preventDefault();
        open();
      }
    });

    const img = document.createElement('img');
    img.className = 'result-thumb pending';
    img.alt = name;
    img.title = t('viewLarge');
    item.appendChild(img);
    thumbQueue.push({ path, img });
    pumpThumbQueue();

    const nameEl = document.createElement('div');
    nameEl.className = 'result-item-name';
    nameEl.textContent = name;
    nameEl.title = path;
    item.appendChild(nameEl);

    const stateEl = document.createElement('div');
    stateEl.className = 'result-item-state';
    stateEl.dataset.state = meta.state;
    stateEl.textContent = fileStateText(meta.state);
    if (meta.reason) stateEl.title = meta.reason;
    item.appendChild(stateEl);

    if (!isMobile) {
      const actions = document.createElement('div');
      actions.className = 'result-item-actions';
      if (resultSourceDir) {
        const compare = document.createElement('button');
        compare.className = 'result-item-btn compare';
        compare.type = 'button';
        compare.textContent = t('compareOpen');
        compare.disabled = meta.state !== 'done';
        if (compare.disabled) compare.title = t('compareUnavailable');
        compare.addEventListener('click', (ev) => {
          ev.stopPropagation();
          openResultCompare(resultPaths.indexOf(path));
        });
        actions.appendChild(compare);
      }
      const reveal = document.createElement('button');
      reveal.className = 'result-item-btn';
      reveal.type = 'button';
      reveal.textContent = t('viewInFolder');
      reveal.addEventListener('click', (ev) => {
        ev.stopPropagation();
        invoke('reveal_path', { path, lang: window.i18n.lang })
          .catch((err) => logLine(t('logError')(String(err))));
      });
      actions.appendChild(reveal);
      item.appendChild(actions);
    }
    els.resultListGrid.appendChild(item);
  });
}

// 语言切换后更新结果卡片状态文案与按钮文案（不重载缩略图）
function refreshResultLabels() {
  if (!els.resultListGrid) return;
  els.resultListGrid.querySelectorAll('.result-item').forEach((item) => {
    const name = item.dataset.name || '';
    const state = item.querySelector('.result-item-state');
    const meta = resultMeta.get(name);
    if (state && meta) state.textContent = fileStateText(meta.state);
    const compare = item.querySelector('.result-item-btn.compare');
    if (compare) compare.textContent = t('compareOpen');
    const reveal = item.querySelector('.result-item-btn:not(.compare)');
    if (reveal) reveal.textContent = t('viewInFolder');
  });
  updateRcNav();
}

// —— 桌面逐张前后对比（结果图 vs 源图，滑块）——
let rcIndex = -1;

function setRcPos(pct) {
  const pos = Math.max(0, Math.min(100, pct));
  els.rcSource.style.clipPath = `inset(0 ${100 - pos}% 0 0)`;
  els.rcHandle.style.left = pos + '%';
}

function updateRcNav() {
  const n = resultPaths.length;
  const show = rcIndex >= 0 && n > 1;
  els.rcPrev.hidden = !show;
  els.rcNext.hidden = !show;
  els.rcCounter.hidden = !show;
  if (show) els.rcCounter.textContent = t('imageCounter')(rcIndex + 1, n);
}

async function openResultCompare(index) {
  if (index < 0 || index >= resultPaths.length || !resultSourceDir) return;
  const final = resultPaths[index];
  const source = resultSourceDir.replace(/\/+$/, '') + '/' + basename(final);
  rcIndex = index;
  els.resultCompare.hidden = false;
  els.rcFrame.hidden = true;
  els.rcFinal.removeAttribute('src');
  els.rcSource.removeAttribute('src');
  els.rcLoading.hidden = false;
  updateRcNav();
  try {
    const [after, before] = await Promise.all([
      invoke('read_image_base64', { path: final, lang: window.i18n.lang }),
      invoke('read_image_base64', { path: source, lang: window.i18n.lang }),
    ]);
    if (els.resultCompare.hidden) return; // 加载期间已关闭
    els.rcFinal.src = after.data_url;
    els.rcSource.src = before.data_url;
    els.rcFrame.hidden = false;
    els.rcLoading.hidden = true;
    setRcPos(50);
  } catch (err) {
    els.resultCompare.hidden = true;
    els.rcLoading.hidden = true;
    showToast(t('compareSourceMissing'), { type: 'error' });
    logLine(t('logError')(String(err)));
  }
}

function closeResultCompare() {
  els.resultCompare.hidden = true;
  els.rcFrame.hidden = true;
  els.rcFinal.removeAttribute('src');
  els.rcSource.removeAttribute('src');
  rcIndex = -1;
}

function stepResultCompare(delta) {
  const n = resultPaths.length;
  if (n <= 1 || rcIndex < 0) return;
  openResultCompare(((rcIndex + delta) % n + n) % n);
}

function bindResultCompare() {
  if (!els.resultCompare) return;
  let drag = null;
  const pct = (ev) => {
    const rect = els.rcStage.getBoundingClientRect();
    return ((ev.clientX - rect.left) / rect.width) * 100;
  };
  els.rcStage.addEventListener('pointerdown', (ev) => {
    drag = { id: ev.pointerId, x: ev.clientX, moved: false };
    try { els.rcStage.setPointerCapture(ev.pointerId); } catch (_) {}
  });
  els.rcStage.addEventListener('pointermove', (ev) => {
    if (!drag || ev.pointerId !== drag.id) return;
    if (Math.abs(ev.clientX - drag.x) > 4) drag.moved = true;
    if (drag.moved) setRcPos(pct(ev));
  });
  const end = (ev) => {
    if (!drag || ev.pointerId !== drag.id) return;
    const moved = drag.moved || Math.abs(ev.clientX - drag.x) > 4;
    drag = null;
    if (moved) return;
    // 图片本身 pointer-events:none，事件目标恒为 stage；用图片矩形区分"点图跳位"和"点空白关闭"
    const rect = els.rcFinal.getBoundingClientRect();
    const inImage = ev.clientX >= rect.left && ev.clientX <= rect.right
      && ev.clientY >= rect.top && ev.clientY <= rect.bottom;
    if (inImage) setRcPos(pct(ev));
    else closeResultCompare();
  };
  els.rcStage.addEventListener('pointerup', end);
  els.rcStage.addEventListener('pointercancel', () => { drag = null; });
  els.rcPrev.addEventListener('click', () => stepResultCompare(-1));
  els.rcNext.addEventListener('click', () => stepResultCompare(1));
  els.rcClose.addEventListener('click', closeResultCompare);
  els.rcReveal.addEventListener('click', () => {
    const path = resultPaths[rcIndex];
    if (!path) return;
    invoke('reveal_path', { path, lang: window.i18n.lang })
      .catch((err) => logLine(t('logError')(String(err))));
  });
}

function setModelProgress(done, total) {
  els.modelProgress.hidden = false;
  const mb = 1048576;
  const percent = total > 0 ? Math.floor((done * 100) / total) : 0;
  els.modelProgressBar.style.width = percent + '%';
  els.modelProgressText.textContent = total > 0
    ? `${(done / mb).toFixed(1)} / ${(total / mb).toFixed(1)} MB（${percent}%）`
    : `${(done / mb).toFixed(1)} MB`;
  // 移动端全屏下载视图：进度环 + 百分比 + MB 明细（环复用处理页的 326.7 周长）
  if (isMobile && taskKind === 'model') {
    els.mRunStage.textContent = t('downloadStage');
    els.mRunRingFg.style.strokeDashoffset = String(RUN_RING_LEN * (1 - (total > 0 ? Math.min(1, done / total) : 0)));
    els.mRunCount.textContent = total > 0 ? `${percent}%` : '';
    els.mRunName.textContent = total > 0
      ? `${(done / mb).toFixed(1)} / ${(total / mb).toFixed(1)} MB`
      : `${(done / mb).toFixed(1)} MB`;
  }
}

// 当前移动端视图（下载结束/取消后恢复回这里）
function currentMobileState() {
  for (const s of ['empty', 'picked', 'done']) {
    if (document.body.classList.contains('state-' + s)) return s;
  }
  return 'empty';
}

function startModelDownload(auto) {
  if (running) return;
  taskKind = 'model';
  if (isMobile) {
    // 全屏下载视图：空态下 run-area 被整体隐藏，取消按钮（在 m-run 里）必须可见。
    // 下载前的视图记下来，结束/取消后恢复（否则停在"处理中"全屏页）。
    preSetupState = currentMobileState();
    setState('running');
  }
  setRunning(true);
  if (isMobile) {
    els.mRunStage.textContent = t('downloadStage');
    els.mRunCount.textContent = '0%';
    els.btnSetup.textContent = t('setupDownloading');
  }
  logLine(auto ? t('logModelAuto') : t('logModelStart'));
  logLine(t('downloadCancelHint'));
  invoke('setup_model', { lang: window.i18n.lang }).catch((err) => {
    logLine(t('logError')(String(err)));
    taskKind = null;
    els.modelProgress.hidden = true;
    if (isMobile) {
      els.btnSetup.textContent = t('setupModel');
      if (preSetupState) {
        setState(preSetupState);
        preSetupState = null;
      }
    }
    setRunning(false);
  });
}

// 首次启动不强行下载 200MB：先说明并等用户同意「使用须知」；取消/失败后可从
// 顶部提示或设置页重新开始（已下载部分自动续传）。
function maybeAutoDownloadModel() {
  // env_status 未返回前不启动（用户可能先点掉了免责声明）：否则 init 后续的
  // setState/restoreLastFolder 会把"下载中"的全屏视图覆盖掉。init 末尾会兜底再调。
  if (!envChecked) return;
  if (envReady || running || taskKind) return;
  if (!localStorage.getItem('wm-disclaimer-ok')) return;
  startModelDownload(true);
}

let lastExitPayload = null;

function renderExitBanner(payload) {
  els.resultBanner.hidden = false;
  const skipped = payload.success ? (payload.skipped || []) : [];
  els.resultBanner.className =
    'result-banner ' + (!payload.success ? 'err' : skipped.length ? 'warn' : 'ok');
  els.resultBanner.innerHTML = '';
  if (payload.success) {
    const dir = payload.outputDir || 'watermark-cleaned/';
    const main = document.createElement('div');
    main.className = 'banner-main';
    const written = typeof payload.processed === 'number' ? payload.processed : lastRunCount;
    const noWm = Array.isArray(payload.noWatermark) ? payload.noWatermark : [];
    const title = document.createElement('div');
    title.className = 'banner-title';
    // ✓ 拆成独立徽章：移动端渲染成绿色圆徽章（桌面端 CSS 退化为原「✓ 」前缀）
    const check = document.createElement('span');
    check.className = 'banner-check';
    check.setAttribute('aria-hidden', 'true');
    title.appendChild(check);
    const titleText = document.createElement('span');
    // 恢复的旧结果不冒充"刚完成"；全部图都没有水印（passthrough）时"处理完成"也会
    // 让人误会真的做了什么：两种都直接说清楚
    titleText.textContent = payload.restored
      ? t('restoredTitle')(written)
      : noWm.length > 0 && noWm.length >= written
        ? t('noWatermarkBanner')(written)
        : `${t('doneBanner')} · ${t('fileCount')(written)}`;
    title.appendChild(titleText);
    const sub = document.createElement('div');
    sub.className = 'banner-sub';
    // 移动端处理的是应用内副本、输出走「保存到相册/分享」，显示 watermark-cleaned
    // 目录路径没有意义（用户在系统文件管理里也看不到应用私有目录）
    sub.textContent = payload.restored
      ? t('restoredSub')
      : payload.overwritten
        ? t('doneSubOverwrite')
        : (isMobile ? t('doneSubSaveMobile') : t('doneSubSave')(dir));
    main.appendChild(title);
    main.appendChild(sub);
    // 部分图未检测到水印：它们是原样输出，用户需要知道"这几张没改"
    if (noWm.length > 0 && noWm.length < written) {
      const note = document.createElement('div');
      note.className = 'banner-sub';
      // 数量少就直接点名，用户能对上具体文件；多了才退回只报数量（结果卡有角标）
      note.textContent = noWm.length <= 4
        ? t('noWatermarkNames')(noWm.join('、'))
        : t('noWatermarkNote')(noWm.length);
      main.appendChild(note);
    }
    // 未过自检的图不写坏结果：明确告诉用户"这几张还是带水印的原图"
    if (skipped.length) {
      const warn = document.createElement('div');
      warn.className = 'banner-sub warn';
      warn.textContent = t('skippedBanner')(skipped.length, skipped.map((s) => s.name).join('、'));
      main.appendChild(warn);
    }
    els.resultBanner.appendChild(main);
    if (!isMobile && payload.outputDir) {
      const btn = document.createElement('button');
      btn.className = 'btn small';
      btn.type = 'button';
      btn.textContent = t('openFolder');
      btn.addEventListener('click', () => {
        invoke('open_path', { path: payload.outputDir, lang: window.i18n.lang }).catch((err) => logLine(t('logError')(String(err))));
      });
      els.resultBanner.appendChild(btn);
    } else if (isMobile) {
      const paths = payload.outputs || [];
      const wrap = document.createElement('div');
      wrap.className = 'banner-actions';
      const mk = (label, cmd, doneText, failText, noneText, savedLabel, variant) => {
        const btn = document.createElement('button');
        btn.className = 'btn small ' + (variant || 'primary');
        btn.type = 'button';
        btn.textContent = label;
        btn.disabled = paths.length === 0;
        btn.addEventListener('click', async () => {
          btn.disabled = true;
          try {
            const n = await invoke(cmd, { paths, lang: window.i18n.lang });
            if (n > 0) {
              logLine(doneText(n));
              showToast(doneText(n), { type: 'success' });
              // 保存类按钮成功后短暂显示「已保存 ✓」——小白用户需要明确的完成反馈
              if (savedLabel) {
                btn.textContent = savedLabel;
                btn.classList.add('saved');
                setTimeout(() => {
                  btn.textContent = label;
                  btn.classList.remove('saved');
                }, 4000);
              }
            } else {
              logLine(noneText);
              showToast(noneText, { type: 'error' });
            }
          } catch (err) {
            logLine(failText(String(err)));
            showToast(failText(String(err)), { type: 'error' });
          } finally {
            btn.disabled = false;
          }
        });
        return btn;
      };
      wrap.appendChild(mk(t('exportGallery'), 'export_results', t('exportDone'), t('exportFailed'), t('exportNone'), t('exportSaved'), 'primary'));
      wrap.appendChild(mk(t('shareResults'), 'share_results', t('shareDone'), t('shareFailed'), t('shareNone'), null, 'secondary'));
      els.resultBanner.appendChild(wrap);
    }
    // 覆盖模式：备份保留在 original-watermark-backup/，给一个"后悔药"。
    // 移动端无覆盖模式（处理的是应用内副本），不显示。
    if (payload.overwritten && targetRoot && !isMobile) {
      const restore = document.createElement('button');
      restore.className = 'btn small danger ghost';
      restore.type = 'button';
      restore.textContent = t('restoreOriginal');
      restore.title = t('restoreHint');
      restore.addEventListener('click', restoreOriginals);
      els.resultBanner.appendChild(restore);
    }
  } else if (payload.cancelled) {
    els.resultBanner.textContent = t('cancelledBanner');
  } else {
    const box = document.createElement('div');
    box.className = 'banner-main';
    const title = document.createElement('div');
    title.className = 'banner-title';
    title.textContent = `${t('failedBanner')}：${payload.error || 'exit ' + payload.code}`;
    const help = document.createElement('div');
    help.className = 'banner-sub warn';
    help.textContent = t('failedHelp');
    box.append(title, help);
    els.resultBanner.appendChild(box);
  }
}

function handleExit(payload) {
  const kind = taskKind;
  taskKind = null;
  els.modelProgress.hidden = true;
  setRunning(false);
  if (kind === 'model') {
    // 移动端全屏下载视图收尾：恢复顶栏按钮文案与下载前的视图（成功/取消/失败都恢复）
    if (isMobile) {
      els.btnSetup.textContent = t('setupModel');
      if (preSetupState) {
        setState(preSetupState);
        preSetupState = null;
      }
    }
    if (payload.success) {
      logLine(t('logModelDone'));
      notifyDone(t('notifyModelDoneTitle'), t('notifyModelDoneBody'));
      if (isMobile) showToast(t('notifyModelDoneTitle'), { type: 'success', timeout: 3200 });
    } else if (payload.cancelled) {
      logLine(t('downloadCancelled'));
      showToast(t('downloadCancelled'), { type: 'info' });
    } else {
      logLine(t('logModelFailed')(payload.error || ''));
      notifyDone(t('notifyModelFailTitle'), payload.error || '');
      if (isMobile) {
        showToast(t('notifyModelFailTitle'), { type: 'error', timeout: 6000 });
      }
    }
    refreshEnv();
    if (!els.dSettings.hidden) refreshSettingsInfo();
    return;
  }
  logLine(payload.success ? t('logRunDone') : t('logRunFailed')(payload.code));
  if (!payload.success) els.logBox.open = true;
  markRunningFile('');
  lastExitPayload = payload;

  // 逐张状态：outputs 里是真正写出的图（含"未检测到水印"的透传图），
  // skipped 是自检未过被保留原图的。结果网格与左侧文件列表都标出来，
  // 用户不必从日志里找"哪张没处理"。
  const statusMap = new Map();
  const skipped = payload.success ? (payload.skipped || []) : [];
  const noWm = payload.success && Array.isArray(payload.noWatermark) ? payload.noWatermark : [];
  for (const entry of skipped) {
    const name = entry.name || entry;
    statusMap.set(name, { state: 'skipped', reason: t('skippedReasonHint') });
    setFileState(name, 'skipped', t('skippedReasonHint'));
  }
  for (const name of noWm) {
    statusMap.set(name, { state: 'nowm', reason: t('nowmReasonHint') });
    setFileState(name, 'nowm', t('nowmReasonHint'));
  }
  if (payload.success) {
    for (const path of payload.outputs || []) {
      const name = basename(path);
      if (!statusMap.has(name)) statusMap.set(name, { state: 'done' });
      setFileState(name, statusMap.get(name).state);
    }
  }
  resultSourceDir = payload.success && targetRoot
    ? (payload.overwritten
        ? targetRoot.replace(/\/+$/, '') + '/original-watermark-backup'
        : targetRoot.replace(/\/+$/, ''))
    : null;
  renderExitBanner(payload);
  renderResultList(payload.success ? payload.outputs : [], statusMap);
  setState('done');
  if (!payload.cancelled) {
    notifyDone(
      payload.success ? t('notifyDoneTitle') : t('notifyFailTitle'),
      payload.success
        ? t('notifyDoneBody')(typeof payload.processed === 'number' ? payload.processed : lastRunCount)
        : (payload.error || `exit ${payload.code}`)
    );
    // 移动端：用户正在前台时补浮层提示（结果区较长，banner 常在屏幕外）
    if (payload.success) {
      showToast(
        t('notifyDoneBody')(typeof payload.processed === 'number' ? payload.processed : lastRunCount),
        { type: 'success' }
      );
    }
  }
  const logText = els.log.textContent;
  const lastMatch = (re) => [...logText.matchAll(re)].pop();
  const sourceMatch = lastMatch(/source review: (.+)/g);
  const finalMatch = lastMatch(/final review: (.+)/g);
  if (sourceMatch) showReview(els.reviewCandidate, sourceMatch[1].trim());
  if (finalMatch) showReview(els.reviewFinal, finalMatch[1].trim());
  const mobilePairs = isMobile && payload.success && targetRoot
    ? (payload.outputs || []).filter(Boolean).map((p) => ({
        final: p,
        // 移动端恒为副本模式：原图就在批次目录、与输出同名（输出在 watermark-cleaned/）
        source: targetRoot.replace(/\/+$/, '') + '/' + basename(p),
      }))
    : [];
  if (mobilePairs.length > 0) {
    setupMobileCompare(mobilePairs);
  } else if (sourceMatch && finalMatch) {
    setupCompare(sourceMatch[1].trim(), finalMatch[1].trim());
  }
  refreshEnv();
  refreshBackupInfo();
}

// 首次启动引导层：仅在移动端、disclaimer 同意后展示一次（localStorage 记 wm-intro-ok）
function maybeShowIntro() {
  if (!isMobile) return;
  if (localStorage.getItem('wm-intro-ok')) return;
  els.introOverlay.hidden = false;
}

async function init() {
  applySettings();
  const ready = await refreshEnv();
  await refreshProfiles();
  // 版本号：桌面设置页与移动端「关于」区共用（失败不显示，不影响功能）
  if (!appVersion) appVersion = await invoke('app_version').catch(() => '');

  // 主题 / 语言（实现见模块级 applyTheme/applyLang，设置页与顶栏按钮共用）
  applyTheme(themeMode());
  els.btnTheme.addEventListener('click', () => {
    const dark = document.documentElement.dataset.theme === 'dark';
    setThemeMode(dark ? 'light' : 'dark');
  });
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => applyTheme(themeMode()));

  // 系统菜单文案在 Rust 侧：装了菜单就把当前语言同步过去（页面加载与 JS 初始化先后
  // 不确定，故既在这里主动调一次，也留给原生回调 __WM_MENU_READY 再调一次）。
  window.__WM_MENU_READY = syncMenuLang;
  applyLang();
  els.btnLang.addEventListener('click', () => {
    window.i18n.setLang(window.i18n.lang === 'zh' ? 'en' : 'zh');
    applyLang();
  });

  els.btnDisclaimerOk.addEventListener('click', () => {
    localStorage.setItem('wm-disclaimer-ok', '1');
    els.disclaimerOverlay.hidden = true;
    maybeShowIntro();
    maybeAutoDownloadModel();
  });
  if (!localStorage.getItem('wm-disclaimer-ok')) {
    els.disclaimerOverlay.hidden = false;
  } else {
    maybeShowIntro();
  }
  els.btnIntroStart.addEventListener('click', () => {
    try { localStorage.setItem('wm-intro-ok', '1'); } catch (_) {}
    els.introOverlay.hidden = true;
  });
  els.btnSetupInline.addEventListener('click', () => {
    if (!running) startModelDownload(false);
  });

  // 对比滑块手势：单击跳转、横向拖动调分割线、纵向滑动交给页面滚动（touch-action: pan-y）。
  // 旧实现整块 touch-action:none + pointerdown 立即跳位：手机上手指落在对比图上想继续
  // 滚页面会被"钉住"，且一碰就跳位置。鼠标（桌面）不受方向判定限制，体验与旧版一致。
  let compareDrag = null;
  const comparePct = (ev) => {
    const rect = els.compareBox.getBoundingClientRect();
    return ((ev.clientX - rect.left) / rect.width) * 100;
  };
  els.compareBox.addEventListener('pointerdown', (ev) => {
    if (els.compareBox.hidden) return;
    if (ev.target.closest && ev.target.closest('.compare-nav')) return; // 导航按钮单独处理
    compareDrag = { id: ev.pointerId, x0: ev.clientX, y0: ev.clientY, locked: false };
  });
  els.compareBox.addEventListener('pointermove', (ev) => {
    if (!compareDrag || ev.pointerId !== compareDrag.id) return;
    const dx = ev.clientX - compareDrag.x0;
    const dy = ev.clientY - compareDrag.y0;
    if (!compareDrag.locked) {
      const isMouse = ev.pointerType === 'mouse';
      if (!isMouse && Math.abs(dx) < 5 && Math.abs(dy) < 5) return;
      if (!isMouse && Math.abs(dx) <= Math.abs(dy)) { compareDrag = null; return; } // 纵向：让页面滚
      compareDrag.locked = true;
      try { els.compareBox.setPointerCapture(ev.pointerId); } catch (_) {}
    }
    setComparePos(comparePct(ev));
  });
  els.compareBox.addEventListener('pointerup', (ev) => {
    if (!compareDrag || ev.pointerId !== compareDrag.id) return;
    const moved = Math.abs(ev.clientX - compareDrag.x0) + Math.abs(ev.clientY - compareDrag.y0);
    if (!compareDrag.locked && moved < 6) setComparePos(comparePct(ev)); // 单击：跳到点击处
    compareDrag = null;
  });
  els.compareBox.addEventListener('pointercancel', () => { compareDrag = null; });

  listen('pipeline-log', (event) => logLine(event.payload));
  listen('pipeline-exit', (event) => handleExit(event.payload));
  // 导入进度（Rust 每复制完一张上报一次）：同一 key 原地刷新，结束由 importPaths 收尾
  listen('import-progress', (event) => {
    if (!importing) return;
    const p = event.payload || {};
    showToast(t('importingToast')(p.done || 0, p.total || 0), { key: 'import', sticky: true });
  });
  // 处理中尝试关窗被原生拦下：把窗口带回前台并说明原因
  listen('quit-blocked', () => {
    els.logBox.open = true;
    logLine(t('quitBlockedLog'));
    showToast(t('quitBlockedToast'), { type: 'error', timeout: 5000 });
  });
  initNotifications();
  listen('model-progress', (event) => setModelProgress(event.payload.done, event.payload.total));
  listen('pipeline-progress', (event) => {
    const p = event.payload;
    setRunProgress(p.stage, p.done, p.total, p.name);
  });
  // 系统菜单（桌面端）：菜单项只发动作 id，动作仍走下面已有的按钮点击——
  // 逻辑只实现一次，「处理中禁止开始/选图」等守卫都在按钮的 disabled 上。
  listen('menu-action', (event) => {
    const id = event.payload;
    const busyHint = () => showToast(running ? t('menuBusy') : t('menuNeedSource'), { type: 'info' });
    if (id === 'menu-pick') {
      if (!running && !els.btnPick.disabled) els.btnPick.click();
      else busyHint();
    } else if (id === 'menu-pick-images') {
      if (!running && !importing && els.btnPickImages) els.btnPickImages.click();
      else busyHint();
    } else if (id === 'menu-start') {
      if (!els.btnRun.hidden && !els.btnRun.disabled) els.btnRun.click();
      else busyHint();
    } else if (id === 'menu-export-log') {
      if (!els.btnExportLog.disabled) els.btnExportLog.click();
    } else if (id === 'menu-settings') {
      openDesktopSettings();
    }
  });

  if (isMobile) {
    setPickLabel();
  } else {
    // 桌面默认收起日志：主界面应该突出"图片 + 结果"，日志只在出错/排障时需要
    els.logBox.open = false;
  }

  els.fileList.addEventListener('click', (e) => {
    if (e.target.closest('[data-action="pick"]')) els.btnPick.click();
    else if (e.target.closest('[data-action="pick-images"]')) els.btnPickImages.click();
  });
  if (els.fileSearch) els.fileSearch.addEventListener('input', applyFileSearch);
  if (els.btnPickImages) {
    els.btnPickImages.addEventListener('click', async () => {
      if (running || importing) return;
      try {
        const picked = await open({
          multiple: true,
          filters: [{ name: t('imageFilter'), extensions: ['png', 'jpg', 'jpeg', 'webp'] }],
          title: t('selectImagesTitle'),
        });
        if (!picked) return;
        await openImagesFromPaths(Array.isArray(picked) ? picked : [picked]);
      } catch (err) {
        logLine(t('logError')(String(err)));
        showToast(t('logError')(String(err)), { type: 'error' });
      }
    });
  }

  bindLightbox();
  bindResultCompare();
  if (!isMobile) bindDesktopSettings();
  // 长按/右键图片不弹系统菜单：WebView 的"保存图片/复制图片"会把预览当原图导出，误导用户
  document.addEventListener('contextmenu', (ev) => {
    if (ev.target && ev.target.tagName === 'IMG') ev.preventDefault();
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      if (!els.resultCompare.hidden) { closeResultCompare(); return; }
      if (!els.dSettings.hidden) { closeDesktopSettings(); return; }
      closeLightbox();
      if (!els.maskOverlay.hidden) closeMaskEditor();
      return;
    }
    // 逐张前后对比打开时支持左右翻页
    if (!els.resultCompare.hidden) {
      if (e.key === 'ArrowLeft') { e.preventDefault(); stepResultCompare(-1); return; }
      if (e.key === 'ArrowRight') { e.preventDefault(); stepResultCompare(1); return; }
    }
    // 灯箱打开时用 ←/→ 在结果列表里翻页
    if (els.lightbox.classList.contains('open') && lbIndex >= 0) {
      if (e.key === 'ArrowLeft') { e.preventDefault(); lightboxStep(-1); return; }
      if (e.key === 'ArrowRight') { e.preventDefault(); lightboxStep(1); return; }
    }
    const mod = e.metaKey || e.ctrlKey;
    if (!mod) return;
    // 设置窗口是 macOS 惯例（Cmd+,），菜单项未设加速键，必须由前端接住
    if (e.key === ',') {
      e.preventDefault();
      openDesktopSettings();
      return;
    }
    // 桌面端装了系统菜单时，带快捷键的菜单项会先消费按键 → 这里跳过，避免弹两次对话框。
    // 浏览器预览（ui-preview.sh）没有菜单，仍由这里兜底。
    if (window.__WM_HAS_MENU) return;
    const el = document.activeElement;
    const typing = el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
    if (typing) return;
    if (e.key === 'o' || e.key === 'O') {
      e.preventDefault();
      if (!running && !els.btnPick.disabled) els.btnPick.click();
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (!els.btnRun.hidden && !els.btnRun.disabled) els.btnRun.click();
    }
  });

  els.btnMask.addEventListener('click', openMaskEditor);
  els.btnMaskClear.addEventListener('click', () => {
    clearMaskSelection();
    logLine('[' + t('maskClearedLog') + ']');
  });
  els.btnMaskCancel.addEventListener('click', closeMaskEditor);
  els.btnMaskReset.addEventListener('click', () => {
    maskSel = null;
    resetMaskZoom();
    renderMaskRect();
    setMaskButtons();
  });
  els.btnMaskOk.addEventListener('click', confirmMaskSelection);

  let maskDragging = false;
  const maskPointers = new Map(); // pointerId → {x, y}（client 坐标）
  let maskPinch = null; // 双指缩放会话 { s0, tx0, ty0, dist0, mid0x, mid0y }
  const pinchMetrics = () => {
    const pts = [...maskPointers.values()];
    if (pts.length < 2) return null;
    const [a, b] = pts;
    return {
      dist: Math.hypot(a.x - b.x, a.y - b.y),
      midX: (a.x + b.x) / 2,
      midY: (a.y + b.y) / 2,
    };
  };
  els.maskCanvasWrap.addEventListener('pointerdown', (ev) => {
    if (els.maskOverlay.hidden || !els.maskImg.naturalWidth) return;
    ev.preventDefault();
    maskPointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    try { els.maskCanvasWrap.setPointerCapture(ev.pointerId); } catch (_) {}
    // 移动端双指：进入缩放（终止画框，保留已有框选；三指及以上忽略）
    if (isMobile && maskPointers.size === 2) {
      maskDragging = false;
      const st = pinchMetrics();
      const wrap = els.maskCanvasWrap.getBoundingClientRect();
      if (st && st.dist > 0) {
        maskPinch = {
          s0: maskZoom.s, tx0: maskZoom.tx, ty0: maskZoom.ty,
          dist0: st.dist,
          mid0x: st.midX - wrap.left,
          mid0y: st.midY - wrap.top,
        };
      }
      return;
    }
    if (maskPointers.size > 1) return;
    maskDragging = true;
    const p = maskImgPoint(ev);
    maskSel = { x1: p.x, y1: p.y, x2: p.x, y2: p.y };
    renderMaskRect();
    setMaskButtons();
  });
  els.maskCanvasWrap.addEventListener('pointermove', (ev) => {
    if (!maskPointers.has(ev.pointerId)) return;
    maskPointers.set(ev.pointerId, { x: ev.clientX, y: ev.clientY });
    if (maskPinch) {
      const st = pinchMetrics();
      if (!st || st.dist <= 0) return;
      const wrap = els.maskCanvasWrap.getBoundingClientRect();
      const midX = st.midX - wrap.left;
      const midY = st.midY - wrap.top;
      const s = Math.max(1, Math.min(6, maskPinch.s0 * (st.dist / maskPinch.dist0)));
      // 以两指中点为锚：锚点下的内容（图片）坐标在缩放前后保持不动
      const k = s / maskPinch.s0;
      let tx = midX - k * (maskPinch.mid0x - maskPinch.tx0);
      let ty = midY - k * (maskPinch.mid0y - maskPinch.ty0);
      if (s <= 1.001) { tx = 0; ty = 0; } // 回到 1x：回原点，避免"虚位"
      maskZoom = { s, tx, ty };
      applyMaskZoom();
      return;
    }
    if (!maskDragging) return;
    const p = maskImgPoint(ev);
    maskSel.x2 = p.x;
    maskSel.y2 = p.y;
    renderMaskRect();
    setMaskButtons();
  });
  const endMaskPointer = (ev) => {
    maskPointers.delete(ev.pointerId);
    if (maskPointers.size < 2) maskPinch = null;
    if (maskPointers.size === 0) maskDragging = false;
  };
  els.maskCanvasWrap.addEventListener('pointerup', endMaskPointer);
  els.maskCanvasWrap.addEventListener('pointercancel', endMaskPointer);

  els.btnClearLog.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    els.log.textContent = '';
  });

  els.btnExportLog.addEventListener('click', async (e) => {
    e.preventDefault();
    e.stopPropagation();
    const text = els.log.textContent || '';
    if (!text.trim()) {
      // 冷启动日志区为空：原来静默 return，用户点了没反应像坏了
      if (isMobile) showToast(t('logEmpty'), { type: 'info' });
      return;
    }
    if (isMobile) {
      // Android 上没有"保存文件对话框"（save 是桌面语义）：导出日志走系统分享，
      // 用户可发给微信/邮件/笔记，同样能反馈问题。
      try {
        await invoke('share_log', { content: text, lang: window.i18n.lang });
        logLine(`[${t('shareLog')}]`);
        showToast(t('shareLogDone'), { type: 'success' });
      } catch (err) {
        logLine(t('logError')(String(err)));
        showToast(t('logError')(String(err)), { type: 'error' });
      }
      return;
    }
    try {
      const path = await save({
        title: t('exportLogTitle'),
        defaultPath: `watermark-cleaner-log-${Date.now()}.txt`,
        filters: [{ name: 'Text', extensions: ['txt'] }],
      });
      if (!path) return;
      await invoke('write_text_file', { path, content: text, lang: window.i18n.lang });
      logLine(`[${t('exportLog')}] ${path}`);
    } catch (err) {
      logLine(t('logError')(String(err)));
    }
  });

  async function importPaths(paths, opts = {}) {
    if (importing) return;
    const append = !!(opts.append && targetRoot);
    importing = true;
    // 导入大图/多图要数秒~十几秒（复制到应用私有目录），期间锁住会串状态的按钮
    els.btnPick.disabled = true;
    els.btnRefresh.disabled = true;
    els.btnRun.disabled = true;
    showToast(t('importingToast')(0, paths.length), { key: 'import', sticky: true });
    try {
      const imported = await invoke('import_files', {
        paths,
        dir: append ? targetRoot : null,
        lang: window.i18n.lang,
      });
      targetRoot = imported.dir;
      // 追加：不清结果与框选（同一批次的旧结果仍然有效，处理时会整体重跑）；
      // 换批：清掉旧预览，避免"结果对不上新图片"的假象
      if (!append) {
        clearReviews();
        clearMaskSelection();
      }
      const names = imported.names || [];
      const skipped = imported.skipped || [];
      const total = typeof imported.total === 'number' ? imported.total : names.length;
      if (append) {
        logLine(t('importAddedLog')(names.length, total));
      } else {
        logLine(`[${t('importedLog')}] ${names.length}`);
      }
      skipped.forEach((reason) => logLine(`[${t('importFailed')}] ${reason}`));
      if (skipped.length === 0) {
        showToast(
          append ? t('importAddedToast')(names.length, total) : t('importDoneToast')(names.length),
          { key: 'import', type: 'success' }
        );
      } else {
        showToast(t('importSkippedToast')(names.length, skipped.length), {
          key: 'import', type: 'error', timeout: 6000,
        });
      }
      await refreshFiles();
      if (isMobile && targetRoot) {
        try { localStorage.setItem(MOBILE_BATCH_KEY, targetRoot); } catch (_) {}
      }
    } catch (err) {
      dismissToast('import');
      logLine(`[${t('importFailed')}] ` + String(err));
      showToast(t('importFailToast'), { type: 'error' });
      await message(String(err), { title: t('importFailed'), okLabel: t('ok') }).catch(() => {});
    } finally {
      importing = false;
      els.btnPick.disabled = running;
      els.btnRefresh.disabled = !targetRoot;
      updateRunButton();
    }
  }

  els.btnPick.addEventListener('click', async () => {
    if (isMobile) {
      // 首选系统相册选择器（Android 13+ 系统 Photo Picker，旧版本系统自动回退）；
      // 启动失败（如极旧版本）回退系统文件选择器，保证总能选到图。
      let paths = null;
      try {
        paths = await pickImagesAndroid();
      } catch (err) {
        logLine(`[Picker] ${String(err)}`);
        const picked = await open({
          multiple: true,
          filters: [{ name: t('imageFilter'), extensions: ['png', 'jpg', 'jpeg', 'webp'] }],
          title: t('pickPhotosTitle'),
        });
        if (!picked) return;
        paths = Array.isArray(picked) ? picked : [picked];
      }
      if (!paths || paths.length === 0) return; // 用户取消
      await importPaths(paths, { append: !!targetRoot });
      // ColorOS 12 从系统 picker 返回后 WebView 输入分发会卡死（点击全部失效、轻量
      // 解冻救不回），导入完成后整体重建 WebView（等价冷启动、必然恢复）。取消返回
      // 无此问题，故只在有实际选择时触发；重建后主题/语言自 localStorage 恢复。
      invoke('rebuild_webview').catch(() => {});
      return;
    }
    const picked = await open({ directory: true, multiple: false, title: t('pickFolderTitle') });
    if (!picked) return;
    targetRoot = picked;
    rememberRoot(picked);
    els.btnRefresh.disabled = false;
    clearReviews();
    clearMaskSelection();
    outputDirOverride = null;
    renderOutputDir();
    resetOutputModeForNewRoot();
    if (els.fileSearch) els.fileSearch.value = '';
    await refreshFiles();
  });

  // 桌面拖拽导入：把图片文件拖进窗口即导入（含高亮反馈）
  if (!isMobile && window.__TAURI__.window) {
    try {
      const { getCurrentWindow } = window.__TAURI__.window;
      getCurrentWindow().onDragDropEvent(async (event) => {
        const payload = event.payload;
        if (running) {
          els.dropOverlay.hidden = true;
          return;
        }
        if (payload.type === 'enter' || payload.type === 'over') {
          els.dropOverlay.hidden = false;
          return;
        }
        if (payload.type === 'leave') {
          els.dropOverlay.hidden = true;
          return;
        }
        if (payload.type === 'drop') {
          els.dropOverlay.hidden = true;
          const all = Array.isArray(payload.paths) ? payload.paths : [];
          const images = all.filter((p) => /\.(png|jpe?g|webp)$/i.test(p));
          const others = all.filter((p) => !images.includes(p));
          // 拖进来的可能是**文件夹**：桌面端直接把它当作处理目录（原地处理、可覆盖原图），
          // 与「选择文件夹」按钮同一语义（复制副本只用于移动端的文件导入）。
          let dir = null;
          if (!isMobile) {
            for (const p of others) {
              if (await invoke('is_directory', { path: p })) { dir = p; break; }
            }
          }
          if (dir) {
            targetRoot = dir;
            rememberRoot(dir);
            els.btnRefresh.disabled = false;
            clearReviews();
            clearMaskSelection();
            outputDirOverride = null;
            renderOutputDir();
            resetOutputModeForNewRoot();
            if (els.fileSearch) els.fileSearch.value = '';
            await refreshFiles();
          } else if (images.length > 0) {
            // 桌面端拖入图片：以图片所在文件夹为处理目录、只勾选这些图（原地处理）。
            // 不能走 import_files——那会把图片复制进应用私有目录，导致"覆盖原图"假覆盖。
            await openImagesFromPaths(images);
          } else if (all.length > 0) {
            const heic = all.some((p) => /\.(heic|heif)$/i.test(p));
            showToast(heic ? t('dragHeic') : t('dropNoImage'), { type: 'error', timeout: 5000 });
          }
        }
      });
    } catch (_) { /* 旧版本 API 缺失时忽略 */ }
  }

  for (const el of [els.anyPosition, els.refine, els.inverse, els.keepWork]) {
    el.addEventListener('change', persistSettings);
  }
  els.btnRefresh.addEventListener('click', refreshFiles);
  els.btnSelectAll.addEventListener('click', () => {
    els.fileList.querySelectorAll('input[type=checkbox]').forEach((el) => { el.checked = true; });
    updateRunButton();
  });
  els.btnSelectNone.addEventListener('click', () => {
    els.fileList.querySelectorAll('input[type=checkbox]').forEach((el) => { el.checked = false; });
    updateRunButton();
  });

  els.btnSetup.addEventListener('click', () => {
    if (!running) startModelDownload(false);
  });

  els.outputRow.addEventListener('change', () => {
    renderOutputHint();
    persistSettings();
  });
  if (els.btnOutputDir) {
    els.btnOutputDir.addEventListener('click', async () => {
      const picked = await open({ directory: true, multiple: false, title: t('pickOutputDir') });
      if (!picked) return;
      outputDirOverride = Array.isArray(picked) ? picked[0] : picked;
      renderOutputDir();
      persistSettings();
    });
    els.btnOutputDirReset.addEventListener('click', () => {
      outputDirOverride = null;
      renderOutputDir();
      persistSettings();
    });
  }

  els.profileSelect.addEventListener('change', () => {
    selectedProfileId = els.profileSelect.value || null;
    renderProfileDelete();
    els.learnStatus.textContent = '';
    persistSettings();
  });

  els.btnLearn.addEventListener('click', () => {
    if (!running) learnProfile();
  });

  els.btnProfileDelete.addEventListener('click', async () => {
    const current = profiles.find((p) => p.id === selectedProfileId);
    if (!current) return;
    const ok = await confirm(t('profileDeleteConfirm')(current.label || current.id), {
      title: t('profileDelete'),
      kind: 'warning',
      okLabel: t('ok'),
      cancelLabel: t('cancel'),
    }).catch(() => false);
    if (!ok) return;
    try {
      await invoke('delete_watermark', { id: current.id, lang: window.i18n.lang });
      selectedProfileId = null;
      await refreshProfiles();
      persistSettings();
      els.learnStatus.textContent = '';
    } catch (err) {
      els.learnStatus.textContent = String(err);
    }
  });

  els.btnRun.addEventListener('click', async () => {
    const files = selectedFiles();
    if (files.length === 0 || running) return;
    const overwrite = overwriteMode();
    if (overwrite) {
      const ok = await confirm(t('confirmOverwrite')(files.length), {
        title: t('overwriteTitle'),
        kind: 'warning',
        okLabel: t('ok'),
        cancelLabel: t('cancel'),
      }).catch(() => false);
      if (!ok) return;
    }
    lastRunCount = files.length;
    persistSettings();
    closeDesktopSettings();
    lastExitPayload = null;
    els.resultBanner.hidden = true;
    els.backupBanner.hidden = true;
    clearResultList();
    clearFileStates();
    setRunning(true);
    setState('running');
    resetReviews();
    logLine(t('startLog')(files.length, overwrite ? t('modeOverwrite') : t('modeSave')));
    try {
      await invoke('run_pipeline', {
        root: targetRoot,
        files,
        keepWork: els.keepWork.checked,
        overwriteOriginal: overwrite,
        anyPosition: els.anyPosition.checked,
        refine: els.refine.checked,
        inverse: els.inverse.checked,
        profileId: selectedProfileId,
        outputDir: overwrite ? null : outputDirOverride,
        lang: window.i18n.lang,
        maskBox: manualMask
          ? [manualMask.dx1, manualMask.dy1, manualMask.dx2, manualMask.dy2]
          : null,
      });
    } catch (err) {
      logLine(t('logError')(String(err)));
      setRunning(false);
    }
  });

  els.btnCancel.addEventListener('click', async () => {
    // 模型下载可断点续传、取消无损失；处理任务取消会丢弃本批（未落盘）→ 二次确认防误触
    if (taskKind !== 'model') {
      const ok = await confirm(t('cancelConfirm'), {
        title: t('cancelConfirmTitle'),
        kind: 'warning',
        okLabel: t('ok'),
        cancelLabel: t('cancel'),
      }).catch(() => false);
      if (!ok) return;
    }
    // 确认框停留期间处理可能已自然完成：说清楚，避免"点了确定却没反应"的困惑
    if (taskKind !== 'model' && !running) {
      showToast(t('cancelAlreadyDone'), { type: 'info' });
      return;
    }
    els.btnCancel.disabled = true;
    logLine(taskKind === 'model' ? t('downloadCancelRequested') : t('cancelLog'));
    try {
      await invoke('cancel_pipeline');
    } catch (err) {
      logLine(t('logCancelFailed')(String(err)));
    } finally {
      els.btnCancel.disabled = false;
    }
  });

  // 只有已同意免责声明才自动下载模型；首次启动等用户点「同意」后再开始。
  // 放在 restoreLastFolder 之后：下载走全屏视图，先恢复批次再启动，避免视图被覆盖。
  setState('empty');
  renderEmptyState();
  await restoreLastFolder();
  maybeAutoDownloadModel();
  // 联调便利：?settings=1 直接打开设置；?autorun=1 自动点开始（仅桌面）
  if (!isMobile && new URLSearchParams(location.search).get('settings') === '1') {
    openDesktopSettings();
  }
  if (!isMobile && new URLSearchParams(location.search).get('autorun') === '1') {
    setTimeout(() => els.btnRun.click(), 400);
  }
}

// 移动端布局在 init 之前同步完成（DOM 已解析）：避免首帧闪现桌面双栏结构
if (isMobile) {
  document.body.classList.add('mobile');
  els.btnSettings.removeAttribute('hidden');
  setupMobileLayout();
  bindMobileSettings();
}

init();
