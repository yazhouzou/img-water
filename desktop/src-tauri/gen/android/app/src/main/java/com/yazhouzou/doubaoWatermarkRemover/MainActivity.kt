package com.yazhouzou.doubaoWatermarkRemover

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
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
  }
}
