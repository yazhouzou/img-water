# docs/android.md — Android 适配（按需加载）

只在改 Android 时读。根 `AGENTS.md`/`desktop/AGENTS.md` 只留指针。

- 适配就绪：路径重定向、相册 `content://` 经 JNI/ContentResolver 导入（`android_uri.rs`）、移动端单列 UI。APK 由 CI 出（debug 签名 ~70MB）；问题优先真机复现 + 截图。
- v0.3.5 起 `gen/android` 入库（`.gitignore`：`gen/*` + `!gen/android/`）。
- `MainActivity.kt` 用 `WindowInsetsCompat` 加四边避让 padding：targetSdk 36 强制 edge-to-edge，且 Android WebView 不支持 CSS `env(safe-area-inset-*)`；去掉 `enableEdgeToEdge()` 无效。
- 本地 `pnpm tauri android build` 的 gradle rustBuild 有 WebSocket 环境问题（CI 正常）；仅验 Kotlin 时用 `./gradlew :app:compileUniversalDebugKotlin`。
- 工具链（2026-09 已装）：brew `android-commandlinetools`（`/opt/homebrew/share/android-commandlinetools`）+ `openjdk@17`（`/opt/homebrew/opt/openjdk@17`）+ NDK 26.3.11579264；环境变量在 `~/.zshrc`。
- 推送前跑 `./tools/android-check.sh`（cargo check + 测试编译，aarch64-linux-android，NDK 工具链、`tools/android-test-stubs.c` 桩）。
- 真机：`adb devices` 后 `cd desktop && pnpm tauri android dev`。
- 导出/分享：`android_export.rs` 经 JNI 调 `MainActivity.exportToGallery`（MediaStore 写 `Pictures/WatermarkCleaner`，`RELATIVE_PATH`+`IS_PENDING`，API 29 以下直接返回空串让前端回退分享）与 `MainActivity.shareFiles`（FileProvider + `ACTION_SEND`/`ACTION_SEND_MULTIPLE`，`file_paths.xml` 的 `cache-path` 覆盖暂存目录 `<cache>/share/<ts>`，每次调用先清空旧暂存防累积）；结果区按钮只在 `isMobile` 时渲染，数据来自 pipeline-exit 的 `outputs`。
- 返回键：`TauriActivity.handleBackNavigation=false`，`MainActivity.onBackPressed` 把返回转发给 JS `window.__onAndroidBack`（main.js 定义：优先关大图/框选/免责声明，处理中拦下防中断并 Toast 提示，**空闲且有导入批次时二次确认——2.5 秒内连按两次才退出**（退出丢整个会话），返回 `true` 消费；否则原生 `finish()`）。
- WebView 定格兜底（P0）：从系统 picker 返回后 WebView 偶发"画面定格、JS 照常跑、触摸才恢复"（极端个例定格 3.5 分钟）。生命周期本身配对（wry `WryActivity.onPause/onResume` 调 `WebView.onPause/onResume`），`MainActivity.onResume` 延迟 350ms 再补调一次 `webView.onResume()` + evaluateJavascript 轻推一帧（opacity 0.999→恢复）强制重绘，代价约零、无可见副作用。
- 交互与反馈（v0.5.7，详见 desktop/AGENTS.md「移动端交互」）：Toast 浮层反馈（导出/分享/导入/返回键）；导入先拷后删（防失败毁旧批）+ 逐张进度 + 魔数校验（HEIC 拒收）；移动端固定副本模式（无「替换原图」）；系统栏图标深浅随 App 主题（`set_system_bars` → `MainActivity.setSystemBarAppearance`，`WindowInsetsControllerCompat`）；灯箱支持双指缩放。
- **App 化改版（v0.6.0）**：四视图（空态/列表/处理中/结果）整页自然滚动，废弃桌面"固定一屏+面板内滚"；「添加图片」可追加（`import_files` 带 `dir` 追加到当前批，`remove_import_file`/`clear_import_batch` 单删/清批）；高级选项与精确模式收进设置页（DOM 仅 `isMobile` 时搬移，桌面结构不动）。
  - **系统相册 Picker**：`pick_images` 命令经 `android_pick.rs` JNI 调 `MainActivity.pickImages(max)`（`ActivityResultContracts.PickMultipleVisualMedia`，Android 13+ 系统 Photo Picker、旧版自动回退），不阻塞 Rust 线程；结果由 Kotlin `evaluateJavascript("window.__onAndroidPickResult(JSON)")` 回传（与返回键同模式，不走 IPC）；启动失败前端回退系统文件选择器。真机 Android 12 无系统 Picker 时回退的是 legacy 图库选择器（可多选）。
  - **content:// 拷贝在 Kotlin 侧直拷（v0.6.0 真机修复）**：picker 回调里后台线程 `contentResolver.openInputStream(uri)` 拷到 `cacheDir/import-pick/`（每次清目录重建、`queryDisplayName`（OpenableColumns）取名、重名 `-1/-2` 后缀），完成后 `runOnUiThread` 回传**本地路径数组**；JS 零改动（`__onAndroidPickResult` 收路径），Rust `copy_one` 对本地路径走 `fs::copy`。**旧链路（Rust 侧 `copy_content_uri`：`with_webview`→`MainPipe`(bounded 8)→`exec`→`copy_from_fd`）在 Ace 2 上稳定卡死**——批次目录已建但 >6 分钟不返回（远超 120s `recv_timeout` 理论上限；`content read` 独立验证 MediaProvider 读同一文件正常、主线程与消费线程均无卡点，根因未 100% 锁定，架构绕开）；`android_uri.rs` 旧链保留，仅桌面/历史路径使用。
  - **picker 返回后触摸卡死 → rebuild（v0.6.0 真机修复）**：ColorOS 上从 picker 返回后 WebView 输入分发整体卡死（轻量 `onResume`/解冻均无效），导入完成（或追加导入完成）后 JS `invoke('rebuild_webview')` → `android_pick.rs` JNI → `MainActivity.rebuildWebView()`（`recreate()`，28ms）重建 Activity/WebView；重建后由 `localStorage['wm-mobile-batch']`（导入成功时写、清批时删）恢复已导入批次——冷启动进程重启同靠它。Ace 2 实测全链路（picker→直拷→copy→rebuild→恢复）2.5 秒；页面重建后前几秒可能仍显示空态（页面加载中），>8 秒属异常慢待评估。
  - **处理中全屏进度页**：进度环（SVG `stroke-dashoffset`，JS `setRunProgress` 同步 `pipeline-progress` 事件）+ 张数 + 当前文件名 + 取消按钮。
