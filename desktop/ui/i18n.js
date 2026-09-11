// 轻量 i18n：静态元素用 data-i18n 标记，动态文案用 t(key)
(function () {
  const dict = {
    zh: {
      subtitle: '批量去除 AI 生成图片水印 · 纯本地处理，图片不上传',
      setupModel: '一键下载修复模型',
      checkUpdate: '检查更新',
      envChecking: '检查环境中…',
      envReady: '修复环境就绪',
      envNotReady: '修复环境未就绪',
      filesTitle: '图片文件',
      pickFolder: '选择文件夹',
      refresh: '刷新',
      selectAll: '全选',
      selectNone: '清空',
      noFolder: '未选择文件夹',
      setupHintTitle: '首次使用需要下载修复模型（约 200MB，仅此一次）：',
      setupHintPrivacy: '所有处理均在本地完成，图片不会上传到任何服务器',
      setupHintWifi: '当前为移动网络环境，建议连接 Wi-Fi 后下载',
      pickMask: '框选水印',
      maskAuto: '未框选，自动检测水印',
      maskClear: '清除框选',
      outputLabel: '输出方式',
      outputSave: '另存到文件夹（推荐）',
      outputOverwrite: '覆盖原图',
      keepWork: '处理后保留备份与复查图（便于人工检查）',
      run: '开始处理',
      cancel: '取消',
      cleanup: '清理本轮产物',
      resultTitle: '处理结果',
      beforePreview: '处理前预览（原水印位置）',
      afterPreview: '处理后预览',
      placeholderBefore: '开始处理后，这里显示修复前预览',
      placeholderAfter: '开始处理后，这里显示修复后预览',
      tagAfter: '处理后',
      tagBefore: '处理前',
      logTitle: '运行日志',
      clearLog: '清空',
      disclaimerTitle: '使用须知',
      disclaimerBody1: '<b>请仅处理你拥有版权或已获授权的图片。</b>去除他人作品上的水印可能构成侵权，由此产生的法律责任由使用者自行承担。',
      disclaimerBody2: '<b>隐私说明：</b>所有图片处理均在本机完成，图片与数据不会上传到任何服务器；应用仅联网下载修复模型与检查新版本。',
      disclaimerAgree: '我已阅读并同意',
      disclaimerOnly: '同意后方可使用',
      maskHead: '拖拽框选水印区域（以第一张选中图为例，框会按右下角偏移应用到所有选中图）',
      maskCancel: '取消',
      maskHint: '按住鼠标拖出一个矩形，覆盖住水印文字即可',
      maskReset: '重画',
      maskOk: '确认框选',
      themeDark: '🌙',
      themeLight: '☀️',
      // 动态
      doneBanner: '处理完成',
      doneOverwritten: '原图已覆盖（备份已清理）',
      doneSavedTo: '已另存到',
      cancelledBanner: '已取消：已处理的图片保持有效',
      failedBanner: '处理失败',
      openFolder: '打开文件夹',
      processing: (n) => `处理 ${n} 张`,
      confirmOverwrite: (n) =>
        `将直接覆盖选中的 ${n} 张原图（自动备份到 original-watermark-backup/，可用"清理本轮产物"还原删除）。确定继续吗？`,
      overwriteTitle: '覆盖原图确认',
      startLog: (n, mode) => `[开始] 处理 ${n} 张图片（${mode}）…`,
      modeSave: '另存到 watermark-cleaned/',
      modeOverwrite: '覆盖原图',
      cancelLog: '[取消] 正在取消，当前图片完成后停止…',
      stagePrepare: '分析水印',
      stageInpaint: '修复中',
      stageSave: '保存结果',
      updateTitle: '检查更新',
      updateFound: (latest, current) =>
        `发现新版本 v${latest}（当前 v${current}）。\n请到 GitHub Releases 页面下载：\nhttps://github.com/yazhouzou/img-water/releases/latest`,
      updateNone: (current) => `当前已是最新版本 v${current}`,
      updateCheckFailed: '[更新检查失败] ',
      maskSetLog: '已设定手动遮罩',
      maskClearedLog: '已清除手动遮罩，恢复自动检测',
      importedLog: '已导入',
      importFailed: '导入失败',
      maskSelectedLabel: (box) => `已手动框选（右下角偏移 ${box}）`,
    },
    en: {
      subtitle: 'Batch-remove AI watermark · 100% local, images never uploaded',
      setupModel: 'Download Model',
      checkUpdate: 'Check Updates',
      envChecking: 'Checking…',
      envReady: 'Inpainting model ready',
      envNotReady: 'Model not ready',
      filesTitle: 'Images',
      pickFolder: 'Choose Folder',
      refresh: 'Refresh',
      selectAll: 'All',
      selectNone: 'None',
      noFolder: 'No folder selected',
      setupHintTitle: 'First run requires the inpainting model (~200MB, one-time):',
      setupHintPrivacy: 'All processing is done locally; images are never uploaded',
      setupHintWifi: 'On mobile data — Wi-Fi recommended for the download',
      pickMask: 'Select Watermark',
      maskAuto: 'Not selected, auto-detect watermark',
      maskClear: 'Clear Selection',
      outputLabel: 'Output',
      outputSave: 'Save to folder (recommended)',
      outputOverwrite: 'Overwrite originals',
      keepWork: 'Keep backup & review sheets after processing',
      run: 'Start',
      cancel: 'Cancel',
      cleanup: 'Cleanup',
      resultTitle: 'Result',
      beforePreview: 'Before (watermark location)',
      afterPreview: 'After',
      placeholderBefore: 'Before preview appears here after processing',
      placeholderAfter: 'After preview appears here after processing',
      tagAfter: 'After',
      tagBefore: 'Before',
      logTitle: 'Run Log',
      clearLog: 'Clear',
      disclaimerTitle: 'Terms of Use',
      disclaimerBody1: '<b>Only process images you own or are licensed to use.</b> Removing watermarks from other people\'s work may infringe copyright; you bear all legal responsibility.',
      disclaimerBody2: '<b>Privacy:</b> All processing happens locally — images and data are never uploaded. The app only accesses the network to download the model and check for updates.',
      disclaimerAgree: 'I have read and agree',
      disclaimerOnly: 'Agreement required to continue',
      maskHead: 'Drag a box over the watermark (on the first selected image; applied to all by bottom-right offset)',
      maskCancel: 'Cancel',
      maskHint: 'Hold and drag a rectangle over the watermark text',
      maskReset: 'Redraw',
      maskOk: 'Apply',
      themeDark: '🌙',
      themeLight: '☀️',
      doneBanner: 'Done',
      doneOverwritten: 'originals overwritten (backup cleaned)',
      doneSavedTo: 'saved to',
      cancelledBanner: 'Cancelled: processed images remain valid',
      failedBanner: 'Failed',
      openFolder: 'Open Folder',
      processing: (n) => `Process ${n}`,
      confirmOverwrite: (n) =>
        `This will overwrite ${n} original image(s) in place (a backup is kept in original-watermark-backup/). Continue?`,
      overwriteTitle: 'Confirm Overwrite',
      startLog: (n, mode) => `[Start] Processing ${n} image(s) (${mode})…`,
      modeSave: 'save to watermark-cleaned/',
      modeOverwrite: 'overwrite',
      cancelLog: '[Cancel] Cancelling, will stop after current image…',
      stagePrepare: 'Analyzing',
      stageInpaint: 'Inpainting',
      stageSave: 'Saving',
      updateTitle: 'Check Updates',
      updateFound: (latest, current) =>
        `New version v${latest} available (current v${current}).\nDownload from GitHub Releases:\nhttps://github.com/yazhouzou/img-water/releases/latest`,
      updateNone: (current) => `You are on the latest version v${current}`,
      updateCheckFailed: '[Update check failed] ',
      maskSetLog: 'Manual mask set',
      maskClearedLog: 'Manual mask cleared, back to auto-detect',
      importedLog: 'Imported',
      importFailed: 'Import failed',
      maskSelectedLabel: (box) => `Manual mask set (bottom-right offset ${box})`,
    },
  };

  let lang = localStorage.getItem('wm-lang');
  if (!lang) {
    lang = (navigator.language || 'zh').toLowerCase().startsWith('zh') ? 'zh' : 'en';
  }

  function t(key) {
    const table = dict[lang] || dict.zh;
    return table[key] !== undefined ? table[key] : dict.zh[key];
  }

  function applyI18n() {
    document.querySelectorAll('[data-i18n]').forEach((el) => {
      const text = t(el.dataset.i18n);
      if (typeof text === 'string') el.textContent = text;
    });
    document.querySelectorAll('[data-i18n-html]').forEach((el) => {
      const text = t(el.dataset.i18nHtml);
      if (typeof text === 'string') el.innerHTML = text;
    });
    document.title = lang === 'en' ? 'Watermark Cleaner' : '图片水印清理助手';
  }

  function setLang(next) {
    lang = next;
    localStorage.setItem('wm-lang', next);
    applyI18n();
  }

  window.i18n = { t, applyI18n, setLang, get lang() { return lang; } };
})();
