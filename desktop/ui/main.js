const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open, message } = window.__TAURI__.dialog;

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
  log: document.getElementById('log'),
  reviewCandidate: document.getElementById('review-candidate'),
  reviewFinal: document.getElementById('review-final'),
  modelProgress: document.getElementById('model-progress'),
  modelProgressBar: document.getElementById('model-progress-bar'),
  modelProgressText: document.getElementById('model-progress-text'),
  logBox: document.getElementById('log-box'),
  resultBanner: document.getElementById('result-banner'),
};

let targetRoot = null;
let running = false;
let taskKind = null;
let lastRunCount = 0;
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
  els.btnRun.textContent = value ? '处理中…' : '开始处理';
  els.btnPick.disabled = value;
  els.btnCleanup.disabled = value || !targetRoot;
  els.btnSetup.disabled = value;
}

function logLine(text) {
  els.log.textContent += text + '\n';
  els.log.scrollTop = els.log.scrollHeight;
}

function showReview(container, path) {
  invoke('read_image_base64', { path })
    .then((dataUrl) => {
      container.innerHTML = '';
      const img = document.createElement('img');
      img.src = dataUrl;
      img.title = '点击查看大图';
      img.addEventListener('click', () => openLightbox(dataUrl));
      container.appendChild(img);
      container.classList.add('has-image');
    })
    .catch((err) => {
      container.innerHTML = `<div class="placeholder">预览失败：${escapeHtml(String(err))}</div>`;
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

async function refreshEnv() {
  try {
    const status = await invoke('env_status');
    els.envBadge.textContent = status.ready ? '修复环境就绪' : '修复环境未就绪';
    els.envBadge.className = 'badge ' + (status.ready ? 'ok' : 'bad');
    els.envBadge.hidden = isMobile && status.ready;
    els.btnSetup.hidden = status.ready;
    if (!status.ready) logLine('[环境] ' + status.hint);
    return status.ready;
  } catch (err) {
    els.envBadge.textContent = '环境检查失败';
    els.envBadge.className = 'badge bad';
    els.envBadge.hidden = false;
    logLine('[环境] ' + String(err));
    return false;
  }
}

async function refreshFiles() {
  if (!targetRoot) return;
  els.folderPath.textContent = targetRoot;
  try {
    const names = await invoke('list_pngs', { root: targetRoot });
    renderFiles(names);
  } catch (err) {
    els.fileList.innerHTML = `<div class="placeholder">读取失败：${escapeHtml(String(err))}</div>`;
  }
}

const PICK_ICON =
  '<svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">' +
  '<rect x="6" y="10" width="36" height="28" rx="4"/><circle cx="17" cy="20" r="3.5"/>' +
  '<path d="M6 33l10-9 7 6 8-8 11 11"/></svg>';

function renderEmptyState(message) {
  if (isMobile) {
    els.fileList.innerHTML =
      '<div class="empty-state">' +
      '<div class="empty-icon">' + PICK_ICON + '</div>' +
      '<p class="empty-title">' + (message || '还没有选择图片') + '</p>' +
      '<p class="empty-sub">支持批量选择 PNG，自动去除右下角“豆包AI生成”水印</p>' +
      '<button class="btn primary" data-action="pick" type="button">选择图片</button>' +
      '</div>';
  } else {
    els.fileList.innerHTML = '<div class="placeholder">' + (message || '先选择图片或文件夹') + '</div>';
  }
}

function renderFiles(names) {
  els.fileList.innerHTML = '';
  setState(names.length > 0 ? 'picked' : 'empty');
  els.fileCount.textContent = `${names.length} 张`;
  els.btnSelectAll.disabled = names.length === 0;
  els.btnSelectNone.disabled = names.length === 0;
  els.btnRun.disabled = running || names.length === 0;
  if (names.length === 0) {
    renderEmptyState(isMobile ? '没有找到 PNG 图片' : '该文件夹没有 PNG 图片');
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
  els.btnRun.textContent = count > 0 ? `开始处理（${count} 张）` : '开始处理';
  els.btnRun.disabled = running || !targetRoot || count === 0;
}

function resetReviews() {
  els.resultBanner.hidden = true;
  els.reviewCandidate.innerHTML = '<div class="placeholder"><span class="spinner"></span>正在处理，请稍候…</div>';
  els.reviewFinal.innerHTML = '<div class="placeholder"><span class="spinner"></span>正在处理，请稍候…</div>';
}

// 选新文件夹/导入后清空旧预览，不显示"正在处理"
function clearReviews() {
  els.resultBanner.hidden = true;
  els.reviewCandidate.innerHTML = '<div class="placeholder">开始处理后，这里显示修复前预览</div>';
  els.reviewFinal.innerHTML = '<div class="placeholder">开始处理后，这里显示修复后预览</div>';
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
  logLine(auto ? '[模型] 未检测到修复模型，开始自动下载（约 200MB，多连接加速）…' : '[模型] 开始下载修复模型…');
  invoke('setup_model').catch((err) => {
    logLine('[错误] ' + String(err));
    taskKind = null;
    els.modelProgress.hidden = true;
    setRunning(false);
  });
}

function handleExit(payload) {
  const kind = taskKind;
  taskKind = null;
  els.modelProgress.hidden = true;
  setRunning(false);
  if (kind === 'model') {
    if (payload.success) {
      logLine('[模型] 修复模型下载完成，已就绪');
    } else {
      logLine('[模型] 下载失败：' + (payload.error || '') + '，可点击右上角按钮重试');
    }
    refreshEnv();
    return;
  }
  logLine(payload.success ? '[完成] 流水线执行成功' : `[失败] 退出码 ${payload.code}`);
  if (!payload.success) els.logBox.open = true;
  els.resultBanner.hidden = false;
  els.resultBanner.className = 'result-banner ' + (payload.success ? 'ok' : 'err');
  els.resultBanner.textContent = payload.success
    ? `处理完成，${lastRunCount} 张图片已覆盖保存`
    : `处理失败：${payload.error || '退出码 ' + payload.code}`;
  setState('done');
  const logText = els.log.textContent;
  const lastMatch = (re) => [...logText.matchAll(re)].pop();
  const sourceMatch = lastMatch(/source review: (.+)/g);
  const finalMatch = lastMatch(/final review: (.+)/g);
  if (sourceMatch) showReview(els.reviewCandidate, sourceMatch[1].trim());
  if (finalMatch) showReview(els.reviewFinal, finalMatch[1].trim());
  refreshEnv();
}

async function init() {
  const ready = await refreshEnv();

  listen('pipeline-log', (event) => logLine(event.payload));
  listen('pipeline-exit', (event) => handleExit(event.payload));
  listen('model-progress', (event) => setModelProgress(event.payload.done, event.payload.total));

  if (isMobile) {
    els.btnPick.textContent = '选择图片';
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
    if (e.key === 'Escape') closeLightbox();
  });

  els.btnClearLog.addEventListener('click', (e) => {
    e.preventDefault();
    e.stopPropagation();
    els.log.textContent = '';
  });

  els.btnPick.addEventListener('click', async () => {
    if (isMobile) {
      const picked = await open({
        multiple: true,
        filters: [{ name: 'PNG', extensions: ['png'] }],
        title: '选择要处理的图片',
      });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      if (paths.length === 0) return;
      try {
        const imported = await invoke('import_files', { paths });
        targetRoot = imported.dir;
        els.btnRefresh.disabled = false;
        els.btnCleanup.disabled = running;
        clearReviews();
        await refreshFiles();
      } catch (err) {
        logLine('[导入失败] ' + String(err));
        await message(String(err), { title: '导入失败' }).catch(() => {});
      }
      return;
    }
    const picked = await open({ directory: true, multiple: false, title: '选择包含 PNG 的文件夹' });
    if (!picked) return;
    targetRoot = picked;
    els.btnRefresh.disabled = false;
    els.btnCleanup.disabled = running;
    clearReviews();
    await refreshFiles();
  });

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

  els.btnRun.addEventListener('click', async () => {
    const files = selectedFiles();
    if (files.length === 0 || running) return;
    lastRunCount = files.length;
    setRunning(true);
    setState('running');
    resetReviews();
    logLine(`[开始] 处理 ${files.length} 张图片…`);
    try {
      await invoke('run_pipeline', {
        root: targetRoot,
        files,
        keepWork: els.keepWork.checked,
      });
    } catch (err) {
      logLine('[错误] ' + String(err));
      setRunning(false);
    }
  });

  els.btnCleanup.addEventListener('click', async () => {
    els.btnCleanup.disabled = true;
    try {
      await invoke('cleanup_pipeline');
      logLine('[清理] 已删除 original-watermark-backup 和临时复查产物');
    } catch (err) {
      logLine('[清理] ' + String(err));
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
