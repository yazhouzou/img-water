const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open, ask } = window.__TAURI__.dialog;

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
};

let targetRoot = null;
let running = false;

function setRunning(value) {
  running = value;
  els.btnRun.disabled = value || !targetRoot;
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
      container.appendChild(img);
    })
    .catch((err) => {
      container.innerHTML = `<div class="placeholder">预览失败：${escapeHtml(String(err))}</div>`;
    });
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
    els.btnSetup.hidden = status.ready;
    if (!status.ready) logLine('[环境] ' + status.hint);
    return status.ready;
  } catch (err) {
    els.envBadge.textContent = '环境检查失败';
    els.envBadge.className = 'badge bad';
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

function renderFiles(names) {
  els.fileList.innerHTML = '';
  els.fileCount.textContent = `${names.length} 张`;
  els.btnSelectAll.disabled = names.length === 0;
  els.btnSelectNone.disabled = names.length === 0;
  els.btnRun.disabled = running || names.length === 0;
  if (names.length === 0) {
    els.fileList.innerHTML = '<div class="placeholder">该文件夹没有 PNG 图片</div>';
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
}

function selectedFiles() {
  return [...els.fileList.querySelectorAll('input[type=checkbox]:checked')].map((el) => el.value);
}

function updateRunButton() {
  els.btnRun.disabled = running || !targetRoot || selectedFiles().length === 0;
}

function resetReviews() {
  els.reviewCandidate.innerHTML = '<div class="placeholder">处理中…</div>';
  els.reviewFinal.innerHTML = '<div class="placeholder">处理中…</div>';
}

function handleExit(payload) {
  setRunning(false);
  logLine(payload.success ? '[完成] 流水线执行成功' : `[失败] 退出码 ${payload.code}`);
  const logText = els.log.textContent;
  const candidateMatch = logText.match(/candidate review: (.+)/);
  const finalMatch = logText.match(/final review: (.+)/);
  if (candidateMatch) showReview(els.reviewCandidate, candidateMatch[1].trim());
  if (finalMatch) showReview(els.reviewFinal, finalMatch[1].trim());
  refreshEnv();
}

async function init() {
  const ready = await refreshEnv();

  listen('pipeline-log', (event) => logLine(event.payload));
  listen('pipeline-exit', (event) => handleExit(event.payload));

  els.btnPick.addEventListener('click', async () => {
    const picked = await open({ directory: true, multiple: false, title: '选择包含 PNG 的文件夹' });
    if (!picked) return;
    targetRoot = picked;
    els.btnRefresh.disabled = false;
    els.btnCleanup.disabled = running;
    resetReviews();
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
  els.btnClearLog.addEventListener('click', () => { els.log.textContent = ''; });

  els.btnSetup.addEventListener('click', async () => {
    if (running) return;
    const ok = await ask('将在线下载约 200MB 的修复模型（国内镜像，支持断点续传），无需 Python，是否继续？', {
      title: '下载修复模型',
      kind: 'info',
    });
    if (!ok) return;
    setRunning(true);
    logLine('[模型] 开始下载修复模型…');
    try {
      await invoke('setup_model');
    } catch (err) {
      logLine('[错误] ' + String(err));
      setRunning(false);
    }
  });

  els.btnRun.addEventListener('click', async () => {
    const files = selectedFiles();
    if (files.length === 0 || running) return;
    setRunning(true);
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
    logLine('[提示] 点击右上角“一键初始化修复环境”，或在项目根目录运行 ./tools/ensure-inpaint-env.sh');
  }
}

init();
