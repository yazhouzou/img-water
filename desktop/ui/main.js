const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open, message, confirm, save } = window.__TAURI__.dialog;
const t = (key) => window.i18n.t(key);

const els = {
  envBadge: document.getElementById('env-badge'),
  btnSetup: document.getElementById('btn-setup'),
  btnPick: document.getElementById('btn-pick'),
  btnRefresh: document.getElementById('btn-refresh'),
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
};

let targetRoot = null;
let running = false;
let taskKind = null;
let lastRunCount = 0;
let lastModelPath = null;
let manualMask = null; // 相对右下角偏移 { dx1, dy1, dx2, dy2 }
let profiles = []; // 精确模式：可用水印档案
let selectedProfileId = null; // null = 自动识别
const mobileParam = new URLSearchParams(location.search).get('mobile');
const isMobile =
  mobileParam === '1' ? true :
  mobileParam === '0' ? false :
  /android|iphone|ipad|ipod/i.test(navigator.userAgent);

function setState(state) {
  document.body.classList.remove('state-empty', 'state-picked', 'state-running', 'state-done');
  document.body.classList.add('state-' + state);
}

function setRunning(value) {
  running = value;
  document.body.setAttribute('aria-busy', value ? 'true' : 'false');
  els.btnRun.disabled = value || !targetRoot;
  els.btnRun.hidden = value;
  els.btnCancel.hidden = !value;
  els.btnPick.disabled = value;
  els.btnSetup.disabled = value;
  els.btnMask.disabled = value || !targetRoot;
  els.btnLearn.disabled = value;
  els.btnProfileDelete.disabled = value;
  els.runProgress.hidden = !value;
  if (!value) {
    els.runProgressBar.style.width = '0%';
    els.runProgressText.textContent = '';
  }
}

function overwriteMode() {
  const checked = els.outputRow.querySelector('input[name=output-mode]:checked');
  return checked && checked.value === 'overwrite';
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

function readSettings() {
  try {
    return JSON.parse(localStorage.getItem(SETTINGS_KEY)) || {};
  } catch (_) {
    return {};
  }
}

function applySettings() {
  const s = readSettings();
  if (s.outputMode === 'overwrite') {
    const radio = els.outputRow.querySelector('input[name=output-mode][value=overwrite]');
    if (radio) radio.checked = true;
  }
  if (typeof s.anyPosition === 'boolean') els.anyPosition.checked = s.anyPosition;
  if (typeof s.refine === 'boolean') els.refine.checked = s.refine;
  if (typeof s.inverse === 'boolean') els.inverse.checked = s.inverse;
  if (typeof s.keepWork === 'boolean') els.keepWork.checked = s.keepWork;
  // 档案 id 待 refreshProfiles() 后校验；已被删除的会在 renderProfiles 里回落成自动
  if (s.profileId) selectedProfileId = s.profileId;
  if (s.outputDir) outputDirOverride = s.outputDir;
  renderOutputHint();
  renderOutputDir();
}

function persistSettings() {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify({
      outputMode: overwriteMode() ? 'overwrite' : 'save',
      anyPosition: els.anyPosition.checked,
      refine: els.refine.checked,
      inverse: els.inverse.checked,
      keepWork: els.keepWork.checked,
      profileId: selectedProfileId || '',
      outputDir: outputDirOverride || '',
    }));
  } catch (_) { /* 隐私模式等禁用存储时忽略 */ }
}

function rememberRoot(root) {
  if (isMobile || !root) return;
  try { localStorage.setItem(LAST_ROOT_KEY, root); } catch (_) {}
}

// 启动时恢复上次目录；目录已被删/移动就清掉记录，不打扰用户
async function restoreLastFolder() {
  if (isMobile) return;
  const lastRoot = localStorage.getItem(LAST_ROOT_KEY);
  if (!lastRoot) return;
  try {
    const names = await invoke('list_pngs', { root: lastRoot, lang: window.i18n.lang });
    targetRoot = lastRoot;
    els.btnRefresh.disabled = false;
    clearMaskSelection();
    renderFolderPath();
    renderFiles(names);
  } catch (_) {
    try { localStorage.removeItem(LAST_ROOT_KEY); } catch (_) {}
  }
}

const STAGE_LABEL = () => ({ prepare: t('stagePrepare'), inpaint: t('stageInpaint'), save: t('stageSave') });

function setRunProgress(stage, done, total, name) {
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;
  els.runProgressBar.style.width = pct + '%';
  const labels = STAGE_LABEL();
  const stageText = labels[stage] || stage;
  els.runProgressText.textContent = t('progressLine')(stageText, done, total, name);
}

function logLine(text) {
  els.log.textContent += text + '\n';
  els.log.scrollTop = els.log.scrollHeight;
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
      img.addEventListener('click', () => openLightbox(dataUrl));
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

  stage.addEventListener('pointerdown', (ev) => {
    if (els.lightboxImg.hidden) return;
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
    if (!dragging) return;
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
    if (Math.abs(dx) > 4 || Math.abs(dy) > 4) moved = true;
    lbTx = startTx + dx;
    lbTy = startTy + dy;
    setLightboxTransform();
  });
  const endDrag = (ev) => {
    if (!dragging) return;
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
  window.addEventListener('resize', () => {
    if (els.lightbox.classList.contains('open') && !els.lightboxImg.hidden) resetLightboxView();
  });
}

// Android 返回键（MainActivity 经 evaluateJavascript 调用）：
// 返回 true = 已消费（关闭了弹层或正在处理），返回 false = 交由原生退出应用。
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
  // 处理中拦下返回，避免进程被杀导致任务中断、输出半成品
  if (running) return true;
  return false;
};

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

async function refreshEnv() {
  try {
    const status = await invoke('env_status');
    // Keep data-i18n in sync so a later applyI18n() (e.g. language toggle)
    // preserves the real state instead of resetting to "检查环境中…".
    els.envBadge.dataset.i18n = status.ready ? 'envReady' : 'envNotReady';
    els.envBadge.textContent = t(status.ready ? 'envReady' : 'envNotReady');
    els.envBadge.className = 'badge ' + (status.ready ? 'ok' : 'bad');
    els.envBadge.hidden = isMobile && status.ready;
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
    els.envBadge.dataset.i18n = 'envFailed';
    els.envBadge.textContent = t('envFailed');
    els.envBadge.className = 'badge bad';
    els.envBadge.hidden = false;
    logLine(t('logEnv')(String(err)));
    return false;
  }
}

async function refreshFiles() {
  if (!targetRoot) return;
  renderFolderPath();
  try {
    const names = await invoke('list_pngs', { root: targetRoot, lang: window.i18n.lang });
    renderFiles(names);
  } catch (err) {
    els.fileList.innerHTML = `<div class="placeholder">${escapeHtml(t('readFailed')(String(err)))}</div>`;
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
    els.fileList.innerHTML =
      '<div class="empty-state">' +
      '<div class="empty-icon">' + PICK_ICON + '</div>' +
      '<p class="empty-title">' + title + '</p>' +
      '<p class="empty-sub">' + t('emptyMobileSub') + '</p>' +
      '<button class="btn primary" data-action="pick" type="button">' + t('pickPhotos') + '</button>' +
      '</div>';
  } else {
    els.fileList.innerHTML = '<div class="placeholder">' + title + '</div>';
  }
}

function renderFiles(names) {
  els.fileList.innerHTML = '';
  setState(names.length > 0 ? 'picked' : 'empty');
  els.fileCount.textContent = t('fileCount')(names.length);
  els.btnSelectAll.disabled = names.length === 0;
  els.btnSelectNone.disabled = names.length === 0;
  els.btnRun.disabled = running || names.length === 0;
  if (names.length === 0) {
    renderEmptyState(isMobile ? 'emptyNoPngMobile' : 'emptyNoPngDesktop');
    updateRunButton();
    return;
  }
  for (const name of names) {
    const item = document.createElement('label');
    item.className = 'file-item';
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.value = name;
    checkbox.checked = true;
    checkbox.addEventListener('change', updateRunButton);
    item.appendChild(checkbox);
    const label = document.createElement('span');
    label.textContent = name;
    item.appendChild(label);
    els.fileList.appendChild(item);
  }
  updateRunButton();
}

function selectedFiles() {
  return [...els.fileList.querySelectorAll('input[type=checkbox]:checked')].map((el) => el.value);
}

function updateRunButton() {
  const count = selectedFiles().length;
  els.btnRun.textContent = count > 0 ? `${t('run')}（${count}）` : t('run');
  els.btnRun.disabled = running || !targetRoot || count === 0;
  els.btnMask.disabled = running || !targetRoot || count === 0;
  updateMaskUi();
}

function clearMaskSelection() {
  manualMask = null;
  updateMaskUi();
}

function setPickLabel() {
  els.btnPick.textContent = isMobile ? t('pickPhotos') : t('pickFolder');
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
  els.profileLabel.placeholder = t('profileLabelPlaceholder');
  els.fileCount.textContent = t('fileCount')(els.fileList.querySelectorAll('input[type=checkbox]').length);
  if (lastModelPath) els.setupHintPath.textContent = t('modelSavePath')(lastModelPath);
  updateRunButton();
  if (document.body.classList.contains('state-empty')) renderEmptyState(lastEmptyKey);
  if (lastExitPayload) renderExitBanner(lastExitPayload);
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

async function learnProfile() {
  const files = selectedFiles();
  if (!targetRoot) {
    els.learnStatus.textContent = t('learnNeedRoot');
    return;
  }
  if (files.length < 2) {
    els.learnStatus.textContent = t('learnNeedFiles');
    return;
  }
  const paths = files.map((name) => `${targetRoot}/${name}`);
  els.btnLearn.disabled = true;
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
    els.btnLearn.disabled = running;
  }
}

function updateMaskUi() {
  const has = !!manualMask;
  els.btnMaskClear.hidden = !has;
  els.maskStatus.textContent = has
    ? t('maskSelectedLabel')(`${manualMask.dx1}, ${manualMask.dy1}, ${manualMask.dx2}, ${manualMask.dy2}`)
    : t('maskAuto');
  els.maskStatus.classList.toggle('manual', has);
}

// —— 手动框选水印区域 ——
let maskSel = null;
// 预览可能被缩小（大图），框选坐标要按它映射回原图尺寸
let maskOrigin = null; // 原图像素坐标 { x1, y1, x2, y2 }

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
  const rect = img.getBoundingClientRect();
  const scale = rect.width / img.naturalWidth;
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
  clearResultList();
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
const notif = !isMobile && window.__TAURI__.notification ? window.__TAURI__.notification : null;
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
  if (document.hasFocus()) return; // 用户正看着窗口就不打扰
  try {
    await notif.sendNotification({ title, body: body || '' });
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

/// 结果列表：逐张缩略图（点击大图预览，桌面端可定位文件）。
function renderResultList(paths) {
  clearResultList();
  const list = (paths || []).filter(Boolean);
  if (list.length === 0) return;
  resultPaths = list;
  els.resultList.hidden = false;
  els.resultListCount.textContent = t('resultListCount')(list.length);
  list.forEach((path) => {
    const item = document.createElement('div');
    item.className = 'result-item';

    const img = document.createElement('img');
    img.className = 'result-thumb pending';
    img.alt = basename(path);
    img.title = t('viewLarge');
    img.addEventListener('click', () => openLightboxPath(path));
    item.appendChild(img);
    thumbQueue.push({ path, img });
    pumpThumbQueue();

    const name = document.createElement('div');
    name.className = 'result-item-name';
    name.textContent = basename(path);
    name.title = path;
    item.appendChild(name);

    if (!isMobile) {
      const actions = document.createElement('div');
      actions.className = 'result-item-actions';
      const reveal = document.createElement('button');
      reveal.className = 'result-item-btn';
      reveal.type = 'button';
      reveal.textContent = t('viewInFolder');
      reveal.addEventListener('click', () => {
        invoke('reveal_path', { path, lang: window.i18n.lang })
          .catch((err) => logLine(t('logError')(String(err))));
      });
      actions.appendChild(reveal);
      item.appendChild(actions);
    }
    els.resultListGrid.appendChild(item);
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
}

function startModelDownload(auto) {
  if (running) return;
  setRunning(true);
  taskKind = 'model';
  logLine(auto ? t('logModelAuto') : t('logModelStart'));
  invoke('setup_model', { lang: window.i18n.lang }).catch((err) => {
    logLine(t('logError')(String(err)));
    taskKind = null;
    els.modelProgress.hidden = true;
    setRunning(false);
  });
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
    const title = document.createElement('div');
    title.className = 'banner-title';
    title.textContent = `✓ ${t('doneBanner')} · ${t('fileCount')(written)}`;
    const sub = document.createElement('div');
    sub.className = 'banner-sub';
    sub.textContent = payload.overwritten ? t('doneSubOverwrite') : t('doneSubSave')(dir);
    main.appendChild(title);
    main.appendChild(sub);
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
      const mk = (label, cmd, doneText, failText, noneText) => {
        const btn = document.createElement('button');
        btn.className = 'btn small primary';
        btn.type = 'button';
        btn.textContent = label;
        btn.disabled = paths.length === 0;
        btn.addEventListener('click', async () => {
          btn.disabled = true;
          try {
            const n = await invoke(cmd, { paths, lang: window.i18n.lang });
            logLine(n > 0 ? doneText(n) : noneText);
          } catch (err) {
            logLine(failText(String(err)));
          } finally {
            btn.disabled = false;
          }
        });
        return btn;
      };
      wrap.appendChild(mk(t('exportGallery'), 'export_results', t('exportDone'), t('exportFailed'), t('exportNone')));
      wrap.appendChild(mk(t('shareResults'), 'share_results', t('shareDone'), t('shareFailed'), t('shareNone')));
      els.resultBanner.appendChild(wrap);
    }
    // 覆盖模式：备份保留在 original-watermark-backup/，给一个"后悔药"
    if (payload.overwritten && targetRoot) {
      const restore = document.createElement('button');
      restore.className = 'btn small danger ghost';
      restore.type = 'button';
      restore.textContent = t('restoreOriginal');
      restore.title = t('restoreHint');
      restore.addEventListener('click', async () => {
        const ok = await confirm(t('restoreConfirm'), { title: t('restoreOriginal'), kind: 'warning' });
        if (!ok) return;
        try {
          const n = await invoke('restore_backup', { root: targetRoot, lang: window.i18n.lang });
          logLine(t('restoreDone')(n));
          clearResultList();
          els.resultBanner.className = 'result-banner ok';
          els.resultBanner.innerHTML = '';
          const box = document.createElement('div');
          box.className = 'banner-main';
          box.textContent = `✓ ${t('restoreDone')(n)}`;
          els.resultBanner.appendChild(box);
          await refreshFiles();
        } catch (err) {
          logLine(t('restoreFailed')(String(err)));
        }
      });
      els.resultBanner.appendChild(restore);
    }
  } else if (payload.cancelled) {
    els.resultBanner.textContent = t('cancelledBanner');
  } else {
    els.resultBanner.textContent = `${t('failedBanner')}：${payload.error || 'exit ' + payload.code}`;
  }
}

function handleExit(payload) {
  const kind = taskKind;
  taskKind = null;
  els.modelProgress.hidden = true;
  setRunning(false);
  if (kind === 'model') {
    if (payload.success) {
      logLine(t('logModelDone'));
    } else {
      logLine(t('logModelFailed')(payload.error || ''));
    }
    notifyDone(
      payload.success ? t('notifyModelDoneTitle') : t('notifyModelFailTitle'),
      payload.success ? t('notifyModelDoneBody') : (payload.error || '')
    );
    refreshEnv();
    return;
  }
  logLine(payload.success ? t('logRunDone') : t('logRunFailed')(payload.code));
  if (!payload.success) els.logBox.open = true;
  lastExitPayload = payload;
  renderExitBanner(payload);
  renderResultList(payload.success ? payload.outputs : []);
  setState('done');
  if (!payload.cancelled) {
    notifyDone(
      payload.success ? t('notifyDoneTitle') : t('notifyFailTitle'),
      payload.success
        ? t('notifyDoneBody')(typeof payload.processed === 'number' ? payload.processed : lastRunCount)
        : (payload.error || `exit ${payload.code}`)
    );
  }
  const logText = els.log.textContent;
  const lastMatch = (re) => [...logText.matchAll(re)].pop();
  const sourceMatch = lastMatch(/source review: (.+)/g);
  const finalMatch = lastMatch(/final review: (.+)/g);
  if (sourceMatch) showReview(els.reviewCandidate, sourceMatch[1].trim());
  if (finalMatch) showReview(els.reviewFinal, finalMatch[1].trim());
  if (sourceMatch && finalMatch) {
    setupCompare(sourceMatch[1].trim(), finalMatch[1].trim());
  }
  refreshEnv();
}

async function init() {
  applySettings();
  const ready = await refreshEnv();
  await refreshProfiles();

  // 主题：默认跟随系统，手动切换后记忆
  const btnTheme = document.getElementById('btn-theme');
  const applyTheme = (mode) => {
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    const dark = mode === 'dark' || (mode !== 'light' && prefersDark);
    document.documentElement.dataset.theme = dark ? 'dark' : 'light';
    document.getElementById('btn-theme-icon').textContent = dark ? t('themeLight') : t('themeDark');
  };
  const themeMode = () => localStorage.getItem('wm-theme') || 'auto';
  applyTheme(themeMode());
  btnTheme.addEventListener('click', () => {
    const dark = document.documentElement.dataset.theme === 'dark';
    const next = dark ? 'light' : 'dark';
    localStorage.setItem('wm-theme', next);
    applyTheme(next);
  });
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => applyTheme(themeMode()));

  // 语言切换
  const btnLang = document.getElementById('btn-lang');
  // 系统菜单文案在 Rust 侧：装了菜单就把当前语言同步过去（页面加载与 JS 初始化先后
  // 不确定，故既在这里主动调一次，也留给原生回调 __WM_MENU_READY 再调一次）。
  const syncMenuLang = () => {
    if (window.__WM_HAS_MENU) {
      invoke('set_menu_lang', { lang: window.i18n.lang }).catch((err) => {
        // 菜单重建失败（加速键/平台）只影响菜单文案，不影响处理；记一行便于排查
        logLine(t('logMenuLangFailed')(String(err)));
      });
    }
  };
  window.__WM_MENU_READY = syncMenuLang;
  const applyLang = () => {
    btnLang.textContent = window.i18n.lang === 'zh' ? 'EN' : '中';
    window.i18n.applyI18n();
    applyDynamicLabels();
    syncMenuLang();
  };
  applyLang();
  btnLang.addEventListener('click', () => {
    window.i18n.setLang(window.i18n.lang === 'zh' ? 'en' : 'zh');
    applyLang();
  });

  els.btnDisclaimerOk.addEventListener('click', () => {
    localStorage.setItem('wm-disclaimer-ok', '1');
    els.disclaimerOverlay.hidden = true;
  });
  if (!localStorage.getItem('wm-disclaimer-ok')) {
    els.disclaimerOverlay.hidden = false;
  }
  els.btnSetupInline.addEventListener('click', () => {
    if (!running) startModelDownload(false);
  });

  let compareDragging = false;
  const comparePct = (ev) => {
    const rect = els.compareBox.getBoundingClientRect();
    return ((ev.clientX - rect.left) / rect.width) * 100;
  };
  els.compareBox.addEventListener('pointerdown', (ev) => {
    if (els.compareBox.hidden) return;
    compareDragging = true;
    els.compareBox.setPointerCapture(ev.pointerId);
    setComparePos(comparePct(ev));
  });
  els.compareBox.addEventListener('pointermove', (ev) => {
    if (compareDragging) setComparePos(comparePct(ev));
  });
  els.compareBox.addEventListener('pointerup', () => { compareDragging = false; });
  els.compareBox.addEventListener('pointercancel', () => { compareDragging = false; });

  listen('pipeline-log', (event) => logLine(event.payload));
  listen('pipeline-exit', (event) => handleExit(event.payload));
  // 处理中尝试关窗被原生拦下：把窗口带回前台并说明原因
  listen('quit-blocked', () => {
    els.logBox.open = true;
    logLine(t('quitBlockedLog'));
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
    if (id === 'menu-pick') {
      if (!running && !els.btnPick.disabled) els.btnPick.click();
    } else if (id === 'menu-start') {
      if (!els.btnRun.hidden && !els.btnRun.disabled) els.btnRun.click();
    } else if (id === 'menu-export-log') {
      if (!els.btnExportLog.disabled) els.btnExportLog.click();
    }
  });

  if (isMobile) {
    setPickLabel();
    els.keepWork.parentElement.style.display = 'none';
    document.body.classList.add('mobile');
  } else {
    els.logBox.open = true;
  }

  els.fileList.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action="pick"]');
    if (btn) els.btnPick.click();
  });

  bindLightbox();
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeLightbox();
      if (!els.maskOverlay.hidden) closeMaskEditor();
      return;
    }
    const mod = e.metaKey || e.ctrlKey;
    if (!mod) return;
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
    renderMaskRect();
    setMaskButtons();
  });
  els.btnMaskOk.addEventListener('click', confirmMaskSelection);

  let maskDragging = false;
  els.maskCanvasWrap.addEventListener('pointerdown', (ev) => {
    if (els.maskOverlay.hidden || !els.maskImg.naturalWidth) return;
    ev.preventDefault();
    maskDragging = true;
    els.maskCanvasWrap.setPointerCapture(ev.pointerId);
    const p = maskImgPoint(ev);
    maskSel = { x1: p.x, y1: p.y, x2: p.x, y2: p.y };
    renderMaskRect();
    setMaskButtons();
  });
  els.maskCanvasWrap.addEventListener('pointermove', (ev) => {
    if (!maskDragging) return;
    const p = maskImgPoint(ev);
    maskSel.x2 = p.x;
    maskSel.y2 = p.y;
    renderMaskRect();
    setMaskButtons();
  });
  const endMaskDrag = () => {
    maskDragging = false;
  };
  els.maskCanvasWrap.addEventListener('pointerup', endMaskDrag);
  els.maskCanvasWrap.addEventListener('pointercancel', endMaskDrag);

  els.btnClearLog.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    els.log.textContent = '';
  });

  els.btnExportLog.addEventListener('click', async (e) => {
    e.preventDefault();
    e.stopPropagation();
    const text = els.log.textContent || '';
    if (!text.trim()) return;
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

  async function importPaths(paths) {
    try {
      const imported = await invoke('import_files', { paths, lang: window.i18n.lang });
      targetRoot = imported.dir;
      els.btnRefresh.disabled = false;
      clearReviews();
      clearMaskSelection();
      logLine(`[${t('importedLog')}] ${paths.length}`);
      await refreshFiles();
    } catch (err) {
      logLine(`[${t('importFailed')}] ` + String(err));
      await message(String(err), { title: t('importFailed') }).catch(() => {});
    }
  }

  els.btnPick.addEventListener('click', async () => {
    if (isMobile) {
      const picked = await open({
        multiple: true,
        filters: [{ name: t('imageFilter'), extensions: ['png', 'jpg', 'jpeg', 'webp'] }],
        title: t('pickPhotosTitle'),
      });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      if (paths.length === 0) return;
      await importPaths(paths);
      return;
    }
    const picked = await open({ directory: true, multiple: false, title: t('pickFolderTitle') });
    if (!picked) return;
    targetRoot = picked;
    rememberRoot(picked);
    els.btnRefresh.disabled = false;
    clearReviews();
    clearMaskSelection();
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
            await refreshFiles();
          } else if (images.length > 0) {
            importPaths(images);
          } else if (all.length > 0) {
            logLine(t('dropNoImage'));
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
      }).catch(() => false);
      if (!ok) return;
    }
    lastRunCount = files.length;
    persistSettings();
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
    els.btnCancel.disabled = true;
    logLine(t('cancelLog'));
    try {
      await invoke('cancel_pipeline');
    } catch (err) {
      logLine(t('logCancelFailed')(String(err)));
    } finally {
      els.btnCancel.disabled = false;
    }
  });

  if (!ready) {
    startModelDownload(true);
  }
  setState('empty');
  renderEmptyState();
  await restoreLastFolder();
}

init();
