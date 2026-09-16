// 浏览器联调用 Tauri API mock。
// 仅在非 Tauri 环境（浏览器直接打开）生效；打包进应用后 window.__TAURI__ 已存在，自动跳过。
// URL 参数：?mobile=1 强制移动端视图；?mobile=0 强制桌面视图；?nomodel=1 模拟未下载模型（自动下载+进度条）
(function () {
  if (window.__TAURI__) return;

  const handlers = {};
  const FAKE_PNGS = ['1.png', '2.png', '3.png', '4.png', '5.png'];
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const noModel = new URLSearchParams(location.search).get('nomodel') === '1';

  function svgDataUrl(text, bg) {
    const svg =
      '<svg xmlns="http://www.w3.org/2000/svg" width="600" height="337">' +
      '<rect width="100%" height="100%" fill="' + bg + '"/>' +
      '<text x="50%" y="45%" font-size="28" fill="#fff" text-anchor="middle" font-family="sans-serif">' + text + '</text></svg>';
    return 'data:image/svg+xml;utf8,' + encodeURIComponent(svg);
  }

  function emit(name, payload) {
    (handlers[name] || []).forEach((cb) => cb({ payload }));
  }

  async function fakeRun() {
    const lines = [
      '[Start] processing ' + FAKE_PNGS.length + ' file(s)… (mock)',
      'source review: /tmp/mock/source.png',
      'final review: /tmp/mock/final.png',
    ];
    for (let i = 0; i < FAKE_PNGS.length; i++) {
      await sleep(600);
      emit('pipeline-progress', { stage: 'inpaint', done: i, total: FAKE_PNGS.length, name: FAKE_PNGS[i] });
      emit('pipeline-log', 'inpainting ' + FAKE_PNGS[i] + '... (mock)');
      emit('pipeline-progress', { stage: 'inpaint', done: i + 1, total: FAKE_PNGS.length, name: FAKE_PNGS[i] });
    }
    for (const line of lines) {
      await sleep(300);
      emit('pipeline-log', line);
    }
    emit('pipeline-exit', {
      code: 0,
      success: true,
      processed: FAKE_PNGS.length,
      overwritten: false,
      outputDir: '/mock/Pictures/watermark/watermark-cleaned',
    });
  }

  async function fakeSetup() {
    for (let p = 0; p <= 100; p += 10) {
      emit('model-progress', { done: p * 2000000, total: 200000000 });
      await sleep(250);
    }
    emit('pipeline-exit', { code: 0, success: true });
  }

  const invoke = (cmd, args) => {
    switch (cmd) {
      case 'env_status':
        return Promise.resolve({
          ready: !noModel,
          model_path: '/mock/lama_fp32.onnx',
          hint: noModel ? '(mock) inpainting model not downloaded' : '',
        });
      case 'list_pngs':
        return Promise.resolve(FAKE_PNGS);
      case 'import_files':
        return Promise.resolve({ dir: '(mock) /storage/emulated/0/Imported' });
      case 'read_image_base64':
        return new Promise((resolve) =>
          setTimeout(() => resolve(svgDataUrl('复查图预览（mock）', '#3b5bdb')), 400)
        );
      case 'list_watermarks':
        return Promise.resolve([
          { id: 'qwen', label: 'qwen', created: '', source: 'builtin', width: 82, height: 428, builtin: true },
          { id: 'learned', label: 'learned', created: '', source: 'auto', width: 148, height: 223, builtin: false },
        ]);
      case 'learn_watermark':
        return new Promise((resolve, reject) =>
          setTimeout(() => {
            if (args && args.files && args.files.length >= 2) {
              resolve({ id: 'mock', label: args.label || 'learned', path: '/mock/profiles/mock', mean_score: 0.62 });
            } else {
              reject('mock: need >= 2 images');
            }
          }, 600)
        );
      case 'delete_watermark':
        return Promise.resolve();
      case 'run_pipeline':
        fakeRun();
        return Promise.resolve();
      case 'setup_model':
        fakeSetup();
        return Promise.resolve();
      case 'cleanup_pipeline':
        return Promise.resolve();
      case 'cancel_pipeline':
        return Promise.resolve();
      case 'open_path':
        console.log('[mock] open_path:', args && args.path);
        return Promise.resolve();
      default:
        return Promise.reject('mock 未实现命令: ' + cmd);
    }
  };

  window.__TAURI__ = {
    core: { invoke },
    window: {
      getCurrentWindow: () => ({ onDragDropEvent: async () => {} }),
    },
    event: {
      listen: (name, cb) => {
        (handlers[name] = handlers[name] || []).push(cb);
        return Promise.resolve();
      },
    },
    dialog: {
      open: async (opts) => {
        if (opts && opts.directory) return '/mock/Pictures/watermark';
        return ['/mock/import/a.png', '/mock/import/b.png'];
      },
      confirm: async () => true,
      message: async () => {},
    },
  };

  console.log('[mock] Tauri API 已替换为浏览器 mock');
})();
