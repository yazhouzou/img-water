const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open, message, confirm } = window.__TAURI__.dialog;
const t = (key) => window.i18n.t(key);

const els = {
  envBadge: document.getElementById('env-badge'),
  btnSetup: document.getElementById('btn-setup'),
  btnPick: document.getElementById('btn-pick'),
  btnRefresh: document.getElementById('btn-refresh'),
  btnSelectAll: document.getElementById('btn-select-all'),
  btnSelectNone: document.getElementById('btn-select-none'),
  btnRun: document.getElementById('btn-run'),
  btnCleanup: document.getElementById('btn-cleanup'),
  btnClearLog: document.getElementById('btn-clear-log'),
  folderPath: document.getElementById('folder-path'),
  fileList: document.getElementById('file-list'),
  fileCount: document.getElementById('file-count'),
  keepWork: document.getElementById('keep-work'),
  anyPosition: document.getElementById('any-position'),
  refine: document.getElementById('refine'),
  log: document.getElementById('log'),
  reviewCandidate: document.getElementById('review-candidate'),
  reviewFinal: document.getElementById('review-final'),
  modelProgress: document.getElementById('model-progress'),
  modelProgressBar: document.getElementById('model-progress-bar'),
  modelProgressText: document.getElementById('model-progress-text'),
  logBox: document.getElementById('log-box'),
  resultBanner: document.getElementById('result-banner'),
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
};

let targetRoot = null;
let running = false;
let taskKind = null;
let lastRunCount = 0;
let lastModelPath = null;
let manualMask = null; // 相对右下角偏移 { dx1, dy1, dx2, dy2 }
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
  els.btnRun.disabled = value || !targetRoot;
  els.btnRun.hidden = value;
  els.btnCancel.hidden = !value;
  els.btnPick.disabled = value;
  els.btnCleanup.disabled = value || !targetRoot;
  els.btnSetup.disabled = value;
  els.btnMask.disabled = value || !targetRoot;
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
    .then((dataUrl) => {
      container.innerHTML = '';
      const img = document.createElement('img');
      img.src = dataUrl;
      img.title = t('viewLarge');
      img.addEventListener('click', () => openLightbox(dataUrl));
      container.appendChild(img);
      container.classList.add('has-image');
    })
    .catch((err) => {
      container.innerHTML = `<div class="placeholder">${escapeHtml(t('previewFailed')(String(err)))}</div>`;
    });
}

function openLightbox(dataUrl) {
  const lightbox = document.getElementById('lightbox');
  const img = lightbox.querySelector('img');
  img.src = dataUrl;
  lightbox.classList.add('open');
}

function closeLightbox() {
  const lightbox = document.getElementById('lightbox');
  lightbox.classList.remove('open');
  lightbox.querySelector('img').src = '';
}

function escapeHtml(text) {
  return text.replace(/[&<>"']/g, (ch) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  })[ch]);
}

// 前后对比 slider：左拖右滑看处理前/后差异（两张复查拼图尺寸一致）
async function setupCompare(sourcePath, finalPath) {
  try {
    const [sourceUrl, finalUrl] = await Promise.all([
      invoke('read_image_base64', { path: sourcePath, lang: window.i18n.lang }),
      invoke('read_image_base64', { path: finalPath, lang: window.i18n.lang }),
    ]);
    els.compareSource.src = sourceUrl;
    els.compareFinal.src = finalUrl;
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
  els.fileCount.textContent = t('fileCount')(els.fileList.querySelectorAll('input[type=checkbox]').length);
  if (lastModelPath) els.setupHintPath.textContent = t('modelSavePath')(lastModelPath);
  updateRunButton();
  if (document.body.classList.contains('state-empty')) renderEmptyState(lastEmptyKey);
  if (lastExitPayload) renderExitBanner(lastExitPayload);
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
let maskSel = null; // 原图像素坐标 { x1, y1, x2, y2 }

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
    const w = Math.round(Math.abs(maskSel.x2 - maskSel.x1));
    const h = Math.round(Math.abs(maskSel.y2 - maskSel.y1));
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
  setMaskButtons();
  els.maskImg.src = '';
  els.maskOverlay.hidden = false;
  try {
    els.maskImg.src = await invoke('read_image_base64', { path, lang: window.i18n.lang });
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
  if (!maskSel) return;
  const img = els.maskImg;
  manualMask = {
    dx1: Math.round(Math.min(maskSel.x1, maskSel.x2) - img.naturalWidth),
    dy1: Math.round(Math.min(maskSel.y1, maskSel.y2) - img.naturalHeight),
    dx2: Math.round(Math.max(maskSel.x1, maskSel.x2) - img.naturalWidth),
    dy2: Math.round(Math.max(maskSel.y1, maskSel.y2) - img.naturalHeight),
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
  els.resultBanner.className = 'result-banner ' + (payload.success ? 'ok' : 'err');
  els.resultBanner.innerHTML = '';
  if (payload.success) {
    const dir = payload.outputDir || 'watermark-cleaned/';
    const main = document.createElement('div');
    main.className = 'banner-main';
    const title = document.createElement('div');
    title.className = 'banner-title';
    title.textContent = `✓ ${t('doneBanner')} · ${t('fileCount')(lastRunCount)}`;
    const sub = document.createElement('div');
    sub.className = 'banner-sub';
    sub.textContent = payload.overwritten ? t('doneSubOverwrite') : t('doneSubSave')(dir);
    main.appendChild(title);
    main.appendChild(sub);
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
    refreshEnv();
    return;
  }
  logLine(payload.success ? t('logRunDone') : t('logRunFailed')(payload.code));
  if (!payload.success) els.logBox.open = true;
  lastExitPayload = payload;
  renderExitBanner(payload);
  setState('done');
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
  const ready = await refreshEnv();

  // 主题：默认跟随系统，手动切换后记忆
  const btnTheme = document.getElementById('btn-theme');
  const applyTheme = (mode) => {
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    const dark = mode === 'dark' || (mode !== 'light' && prefersDark);
    document.documentElement.dataset.theme = dark ? 'dark' : 'light';
    btnTheme.textContent = dark ? t('themeLight') : t('themeDark');
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
  const applyLang = () => {
    btnLang.textContent = window.i18n.lang === 'zh' ? 'EN' : '中';
    window.i18n.applyI18n();
    applyDynamicLabels();
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
  listen('model-progress', (event) => setModelProgress(event.payload.done, event.payload.total));
  listen('pipeline-progress', (event) => {
    const p = event.payload;
    setRunProgress(p.stage, p.done, p.total, p.name);
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

  document.getElementById('lightbox').addEventListener('click', closeLightbox);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeLightbox();
      if (!els.maskOverlay.hidden) closeMaskEditor();
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

  async function importPaths(paths) {
    try {
      const imported = await invoke('import_files', { paths, lang: window.i18n.lang });
      targetRoot = imported.dir;
      els.btnRefresh.disabled = false;
      els.btnCleanup.disabled = running;
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
    els.btnRefresh.disabled = false;
    els.btnCleanup.disabled = running;
    clearReviews();
    clearMaskSelection();
    await refreshFiles();
  });

  // 桌面拖拽导入：把图片文件拖进窗口即导入
  if (!isMobile && window.__TAURI__.window) {
    try {
      const { getCurrentWindow } = window.__TAURI__.window;
      getCurrentWindow().onDragDropEvent((event) => {
        if (running) return;
        const payload = event.payload;
        if (payload.type === 'drop' && Array.isArray(payload.paths)) {
          const paths = payload.paths.filter((p) => /\.(png|jpe?g|webp)$/i.test(p));
          if (paths.length > 0) importPaths(paths);
        }
      });
    } catch (_) { /* 旧版本 API 缺失时忽略 */ }
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

  els.outputRow.addEventListener('change', renderOutputHint);

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

  els.btnCleanup.addEventListener('click', async () => {
    els.btnCleanup.disabled = true;
    try {
      await invoke('cleanup_pipeline', { lang: window.i18n.lang });
      logLine(t('logCleanupDone'));
    } catch (err) {
      logLine(t('logCleanupFailed')(String(err)));
    } finally {
      els.btnCleanup.disabled = running;
    }
  });

  if (!ready) {
    startModelDownload(true);
  }
  setState('empty');
  renderEmptyState();
}

init();
