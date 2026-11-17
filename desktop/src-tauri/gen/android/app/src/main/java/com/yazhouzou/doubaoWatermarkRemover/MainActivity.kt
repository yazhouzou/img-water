package com.yazhouzou.doubaoWatermarkRemover

import android.content.ContentValues
import android.content.Intent
import android.graphics.drawable.ColorDrawable
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.view.MotionEvent
import android.view.View
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import org.json.JSONArray
import java.io.File

class MainActivity : TauriActivity() {
  private var webViewRef: WebView? = null

  private companion object {
    const val TAG = "WmDebug"
  }

  /** 系统图片选择器进行中：防重复启动（返回结果后复位）。 */
  private var pickInFlight = false

  /**
   * 从系统 picker/弹窗返回后的 WebView「解冻」兜底：ColorOS 12 实测返回后触摸分发
   * 不恢复（画面与 JS 正常、所有点击无响应）。onResume/onWindowFocusChanged/pick 回调
   * 三个时机都补一次 onResume + requestFocus + 重绘轻推，覆盖各 ROM 的回调顺序差异。
   */
  private fun thawWebView(delayMs: Long) {
    val wv = webViewRef ?: return
    wv.postDelayed({
      if (isFinishing || isDestroyed) return@postDelayed
      if (BuildConfig.DEBUG) android.util.Log.i(TAG, "thawWebView(+${delayMs}ms)")
      wv.onResume()
      wv.requestFocus()
      wv.evaluateJavascript(
        "(function(){try{var b=document.body;if(!b)return;" +
          "b.style.opacity='0.999';" +
          "setTimeout(function(){b.style.opacity='';},60);" +
          "}catch(e){}})()",
        null
      )
    }, delayMs)
  }

  /**
   * 向 WebView 补发一次 ACTION_CANCEL，清掉可能残留的"未收尾手势序列"：
   * ColorOS 12 实机从系统 picker 返回后单指点击被吞吐（长按仍触发选词、页面还被误
   * 解释成 pinch 缩放）——符合"Activity 在触摸序列中被切走，WebView 内部手势状态
   * 卡在未结束"的形态。CANCEL 是手势序列的标准终止信号；无残留时对孤立 CANCEL
   * 为安全 no-op。
   */
  private fun flushStaleTouch() {
    val wv = webViewRef ?: return
    try {
      val now = SystemClock.uptimeMillis()
      val cancel = MotionEvent.obtain(now, now, MotionEvent.ACTION_CANCEL, 0f, 0f, 0)
      wv.dispatchTouchEvent(cancel)
      cancel.recycle()
      if (BuildConfig.DEBUG) android.util.Log.i(TAG, "flushStaleTouch")
    } catch (_: Throwable) {
    }
  }

  /**
   * Android 相册选择器：ActivityResultContracts.PickMultipleVisualMedia 在 Android 13+
   * 走系统 Photo Picker（相册网格），更早版本自动回退 ACTION_OPEN_DOCUMENT/GET_CONTENT
   * （无 GMS 的国产 ROM 会落到系统图库/文档界面）。
   * 多选上限在注册时固定（系统上限内取 100），结果经 __onAndroidPickResult 回传前端
   * （与 __onAndroidBack 同模式：Kotlin → evaluateJavascript，不走 Rust IPC）。
   */
  private val pickImagesLauncher = registerForActivityResult(
    ActivityResultContracts.PickMultipleVisualMedia(100)
  ) { uris ->
    pickInFlight = false
    // 部分 ROM 的图片/文档选择器是 translucent Activity：宿主可能没有经历完整的
    // onResume 回调，返回后 WebView 触摸分发不恢复。拿到结果后补一次解冻 + 清残留手势。
    thawWebView(250)
    flushStaleTouch()
    if (uris.isEmpty()) {
      webViewRef?.evaluateJavascript(
        "window.__onAndroidPickResult && window.__onAndroidPickResult([])",
        null
      )
      return@registerForActivityResult
    }
    // 拷贝在后台线程进行（见下），可能持续数秒~十几秒；先通知前端"正在导入"，
    // 否则回到 App 后界面毫无动静，用户以为选了图没反应。
    webViewRef?.evaluateJavascript(
      "window.__onAndroidPickCopyStart && window.__onAndroidPickCopyStart(${uris.size})",
      null
    )
    // 拷贝放后台线程：content:// 交给 ContentResolver 直读（Android 标准路径，不给
    // WebView 主线程/跨 JNI 管道增负担），完成后才回调 JS；回调传的是本地路径，
    // Rust 侧 copy_one 走 fs::copy 快分支（不再经 content:// JNI 拷贝链路）。
    Thread {
      val staging = File(cacheDir, "import-pick")
      staging.deleteRecursively()
      staging.mkdirs()
      val copied = mutableListOf<String>()
      uris.forEachIndexed { index, uri ->
        try {
          val rawName = queryDisplayName(uri) ?: "image-$index.png"
          var dst = File(staging, rawName)
          var n = 1
          while (dst.exists()) {
            val dot = rawName.lastIndexOf('.')
            val stem = if (dot > 0) rawName.substring(0, dot) else rawName
            val ext = if (dot > 0) rawName.substring(dot) else ""
            dst = File(staging, "$stem-$n$ext")
            n++
          }
          contentResolver.openInputStream(uri)?.use { input ->
            dst.outputStream().use { out -> input.copyTo(out) }
            copied.add(dst.absolutePath)
          } ?: android.util.Log.w("WmImport", "openInputStream 返回 null: $uri")
        } catch (t: Throwable) {
          android.util.Log.w("WmImport", "拷贝相册文件失败: $uri", t)
        }
      }
      runOnUiThread {
        val arr = JSONArray()
        copied.forEach { arr.put(it) }
        // JSONArray.toString() 是合法 JSON 字面量（自动转义特殊字符），可直接内嵌
        webViewRef?.evaluateJavascript(
          "window.__onAndroidPickResult && window.__onAndroidPickResult($arr)",
          null
        )
      }
    }.start()
  }

  /** 查询 content:// URI 的显示名（OpenableColumns.DISPLAY_NAME）；失败返回 null。 */
  private fun queryDisplayName(uri: Uri): String? {
    return try {
      contentResolver.query(uri, null, null, null, null)?.use { cursor ->
        val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (idx >= 0 && cursor.moveToFirst()) cursor.getString(idx) else null
      }
    } catch (_: Throwable) {
      null
    }
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // targetSdk 36 强制 edge-to-edge，WebView 内容会画进系统栏；
    // 给 content 加四边避让 padding（Android WebView 不支持 CSS env(safe-area-inset-*)）
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.statusBars()
          or WindowInsetsCompat.Type.displayCutout()
          or WindowInsetsCompat.Type.navigationBars()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }

    // 返回键接管：优先交给前端关闭弹层（框选/大图/免责声明），前端返回 false 时才退出。
    // Tauri 的 TauriActivity 关闭了自带返回处理（handleBackNavigation=false），若不接管
    // 弹层打开时按返回会直接杀掉进程（处理中还会中断任务）。
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        val wv = webViewRef
        if (wv == null) {
          finish()
          return
        }
        wv.evaluateJavascript("(window.__onAndroidBack && window.__onAndroidBack()) === true") { result ->
          if (result != "true") finish()
        }
      }
    })
  }

  override fun onWebViewCreate(webView: WebView) {
    webViewRef = webView
  }

  /**
   * P0 兜底：从系统图片选择器返回后，Android WebView 偶发“画面定格/触摸失效”——渲染或
   * 输入分发被系统暂停但 JS 照常执行（实测定格 3.5 分钟的极端个例，触摸才恢复）。
   * 生命周期本身是配对的（wry 在 onPause/onResume 里调 WebView.onResume/onPause），
   * 这里在 onResume 与窗口重新获焦两个时机各补一次解冻，代价约零、无可见副作用。
   */
  override fun onResume() {
    super.onResume()
    if (BuildConfig.DEBUG) android.util.Log.i(TAG, "onResume")
    thawWebView(350)
  }

  override fun onPause() {
    if (BuildConfig.DEBUG) android.util.Log.i(TAG, "onPause")
    super.onPause()
  }

  override fun onWindowFocusChanged(hasFocus: Boolean) {
    super.onWindowFocusChanged(hasFocus)
    if (BuildConfig.DEBUG) android.util.Log.i(TAG, "focus=$hasFocus")
    // 从 picker 返回时窗口重新获焦是最可靠的恢复时机（部分 ROM onResume 先于焦点恢复）
    if (hasFocus) thawWebView(200)
  }

  /**
   * 系统栏（状态栏/导航栏）外观跟随 App 主题：edge-to-edge 下两栏透明，图标颜色与
   * 露出的背景色都由系统按"系统主题"决定；App 内手动切换主题时须同步，否则：
   * ① 深色 App + 浅色系统 → 深色图标压在深色背景上看不清（图标深浅）；
   * ② 浅色 App + 深色系统 → 状态栏/导航栏区（content 的避让 padding 区露出的是
   *    窗口背景，跟随 Android DayNight 主题）仍然是黑的（背景色，反之露白）。
   */
  fun setSystemBarAppearance(dark: Boolean) {
    val content = findViewById<View>(android.R.id.content)
    WindowInsetsControllerCompat(window, content).apply {
      isAppearanceLightStatusBars = !dark
      isAppearanceLightNavigationBars = !dark
    }
    // 色值与 ui/styles.css 的 --bg 对齐（深 #12141a / 浅 #f4f5f9）。
    window.setBackgroundDrawable(
      ColorDrawable(if (dark) 0xFF12141A.toInt() else 0xFFF4F5F9.toInt())
    )
  }

  /**
   * 从系统 picker 返回后的最终修复手段：ColorOS 12（Android 12 + WebView 97）实测在
   * picker 内发生过触摸后返回，WebView 输入分发会整体卡死（轻量信号 onResume/
   * requestFocus/ACTION_CANCEL 均无法恢复，只有等价冷启动的 WebView 重建有效）。
   * 由前端在系统 picker/文件对话框的导入流程完成后经 Rust 命令调用（取消返回不触发）。
   *
   * Android 13+（API 33）未复现该问题（系统 Photo Picker + 新版 WebView），而 recreate
   * 有可见代价（闪屏、页面重载回空态数秒），故不再作为常规路径；若新系统复现再放宽。
   */
  fun rebuildWebView() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      if (BuildConfig.DEBUG) android.util.Log.i(TAG, "rebuildWebView skipped (SDK >= 33)")
      return
    }
    runOnUiThread {
      if (isFinishing || isDestroyed) return@runOnUiThread
      if (BuildConfig.DEBUG) android.util.Log.i(TAG, "rebuildWebView")
      recreate()
    }
  }

  /**
   * 处理/模型下载期间的前台服务 + 唤醒锁（WatermarkService）：切后台/锁屏时系统不
   * 冻结、不回收本进程（处理单张 ~10s、整批可达数分钟）。文案由 Rust 侧按 App 语言
   * 传入。Android 12+ 在 App 处于后台时不允许启动前台服务，前端只在用户点「开始/
   * 下载」（App 前台）时调用，失败仅记录日志、处理照常。
   */
  fun startProcessingService(text: String) {
    try {
      val intent = Intent(this, WatermarkService::class.java).putExtra(WatermarkService.EXTRA_TEXT, text)
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        startForegroundService(intent)
      } else {
        startService(intent)
      }
    } catch (t: Throwable) {
      android.util.Log.w(TAG, "startProcessingService failed", t)
    }
  }

  fun stopProcessingService() {
    try {
      stopService(Intent(this, WatermarkService::class.java))
    } catch (t: Throwable) {
      android.util.Log.w(TAG, "stopProcessingService failed", t)
    }
  }

  /**
   * 启动系统图片选择器（多选）。结果在 pickImagesLauncher 回调里经
   * window.__onAndroidPickResult 回传前端；用户取消时同样回调（传空数组）。
   */
  fun pickImages(max: Int) {
    if (pickInFlight) return
    pickInFlight = true
    try {
      pickImagesLauncher.launch(
        PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)
      )
    } catch (e: Exception) {
      pickInFlight = false
      // 极端情况（无任何可处理选择器的设备）：回传空数组，让前端按"已取消"处理
      webViewRef?.evaluateJavascript(
        "window.__onAndroidPickResult && window.__onAndroidPickResult([])",
        null
      )
    }
  }

  /**
   * 把处理结果写入系统相册 Pictures/WatermarkCleaner。
   * 返回写入后的 content:// URI；失败（或 Android 9 及以下无权限）返回空串，由前端回退到分享。
   */
  fun exportToGallery(path: String, displayName: String, mime: String): String {
    val src = File(path)
    if (!src.isFile) return ""
    // RELATIVE_PATH/IS_PENDING 自 Android 10（API 29）起可用且免存储权限；
    // 更早版本需 WRITE_EXTERNAL_STORAGE，这里不申请，让前端回退到系统分享。
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return ""
    val values = ContentValues().apply {
      put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)
      put(MediaStore.MediaColumns.MIME_TYPE, mime)
      put(MediaStore.MediaColumns.RELATIVE_PATH, "Pictures/WatermarkCleaner")
      put(MediaStore.MediaColumns.IS_PENDING, 1)
    }
    val resolver = contentResolver
    val uri: Uri = resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) ?: return ""
    return try {
      val out = resolver.openOutputStream(uri) ?: throw IllegalStateException("no output stream")
      out.use { o -> src.inputStream().use { it.copyTo(o) } }
      val done = ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }
      resolver.update(uri, done, null, null)
      uri.toString()
    } catch (e: Exception) {
      runCatching { resolver.delete(uri, null, null) }
      ""
    }
  }

  /** 系统分享（无需权限；接收方应用可直接"保存到相册/发送"）。 */
  fun shareFiles(paths: Array<String>, mime: String) {    val uris = ArrayList<Uri>()
    for (p in paths) {
      val f = File(p)
      if (f.isFile) {
        runCatching {
          uris.add(FileProvider.getUriForFile(this, "$packageName.fileprovider", f))
        }
      }
    }
    if (uris.isEmpty()) return
    val intent = if (uris.size == 1) {
      Intent(Intent.ACTION_SEND).putExtra(Intent.EXTRA_STREAM, uris[0])
    } else {
      Intent(Intent.ACTION_SEND_MULTIPLE).putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
    }
    intent.type = mime
    intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    startActivity(Intent.createChooser(intent, null))
  }
}
