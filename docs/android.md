# docs/android.md — Android 适配（按需加载）

只在改 Android 时读。根 `AGENTS.md`/`desktop/AGENTS.md` 只留指针。

- 适配就绪：路径重定向、相册 `content://` 经 JNI/ContentResolver 导入（`android_uri.rs`）、移动端单列 UI。APK 由 CI 出（debug 签名 ~70MB）；问题优先真机复现 + 截图。
- v0.3.5 起 `gen/android` 入库（`.gitignore`：`gen/*` + `!gen/android/`）。
- `MainActivity.kt` 用 `WindowInsetsCompat` 加四边避让 padding：targetSdk 36 强制 edge-to-edge，且 Android WebView 不支持 CSS `env(safe-area-inset-*)`；去掉 `enableEdgeToEdge()` 无效。
- 本地 `pnpm tauri android build` 的 gradle rustBuild 有 WebSocket 环境问题（CI 正常）；仅验 Kotlin 时用 `./gradlew :app:compileUniversalDebugKotlin`。
- 工具链（2026-09 已装）：brew `android-commandlinetools`（`/opt/homebrew/share/android-commandlinetools`）+ `openjdk@17`（`/opt/homebrew/opt/openjdk@17`）+ NDK 26.3.11579264；环境变量在 `~/.zshrc`。
- 推送前跑 `./tools/android-check.sh`（cargo check + 测试编译，aarch64-linux-android，NDK 工具链、`tools/android-test-stubs.c` 桩）。
- 真机：`adb devices` 后 `cd desktop && pnpm tauri android dev`。
