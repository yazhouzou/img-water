package com.yazhouzou.doubaoWatermarkRemover

import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.MediaStore
import android.view.View
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File

class MainActivity : TauriActivity() {
  private var webViewRef: WebView? = null

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
  fun shareFiles(paths: Array<String>, mime: String) {
    val uris = ArrayList<Uri>()
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
