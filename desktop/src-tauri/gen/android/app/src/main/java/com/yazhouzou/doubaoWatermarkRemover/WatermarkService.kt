package com.yazhouzou.doubaoWatermarkRemover

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.os.PowerManager

/**
 * 去水印处理/模型下载期间的前台服务 + PartialWakeLock。
 *
 * 为什么需要：单张 LaMa 推理 ~10s、整批最多 100 张可达十几分钟；没有前台服务时用户
 * 切后台或锁屏，Android 12+ 的 cached-app freezer 会冻结进程、LMK 也可能回收，任务
 * 半途中止（前端只有一句"保持前台"的文案，用户很难一直盯着）。前台服务把进程提到
 * 前台优先级，通知常驻展示"处理中"，状态一眼可见。
 *
 * 生命周期：MainActivity.startProcessingService() 启动，任务结束（Rust finish()）
 * 调 stopService 停止；START_NOT_STICKY（进程被杀不自动重启，避免留下僵尸通知）。
 */
class WatermarkService : Service() {
  private var wakeLock: PowerManager.WakeLock? = null

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val text = intent?.getStringExtra(EXTRA_TEXT) ?: ""
    startForeground(NOTIFICATION_ID, buildNotification(text))
    acquireWakeLock()
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    releaseWakeLock()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
      stopForeground(STOP_FOREGROUND_REMOVE)
    } else {
      @Suppress("DEPRECATION")
      stopForeground(true)
    }
    super.onDestroy()
  }

  private fun acquireWakeLock() {
    if (wakeLock == null) {
      val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
      wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "WatermarkCleaner:processing").apply {
        setReferenceCounted(false)
        // 上限 60 分钟：任何异常路径漏掉 release 也不会永久持锁
        acquire(60 * 60 * 1000L)
      }
    }
  }

  private fun releaseWakeLock() {
    wakeLock?.let { if (it.isHeld) it.release() }
    wakeLock = null
  }

  private fun buildNotification(text: String): Notification {
    ensureChannel()
    val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
    val pending = PendingIntent.getActivity(
      this,
      0,
      open,
      PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
    )
    val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      Notification.Builder(this, CHANNEL_ID)
    } else {
      @Suppress("DEPRECATION")
      Notification.Builder(this)
    }
    return builder
      .setContentTitle(getString(R.string.app_name))
      .setContentText(if (text.isNotEmpty()) text else getString(R.string.app_name))
      .setSmallIcon(android.R.drawable.stat_sys_download)
      .setOngoing(true)
      .setContentIntent(pending)
      .build()
  }

  private fun ensureChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    if (manager.getNotificationChannel(CHANNEL_ID) == null) {
      manager.createNotificationChannel(
        NotificationChannel(
          CHANNEL_ID,
          getString(R.string.app_name),
          NotificationManager.IMPORTANCE_LOW
        ).apply {
          description = "处理进行中"
          setShowBadge(false)
        }
      )
    }
  }

  companion object {
    const val EXTRA_TEXT = "text"
    const val CHANNEL_ID = "watermark-processing"
    const val NOTIFICATION_ID = 1001
  }
}
