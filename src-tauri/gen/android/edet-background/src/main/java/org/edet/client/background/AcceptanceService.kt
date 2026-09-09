// edet: the foreground service that keeps this app's process — and the WebView
// inside it — alive while the member is elsewhere.
//
// **It cannot sign, and it holds no state that could.** The acceptance rule is
// JavaScript in the WebView (`ui/src/lib/autosign.ts`), signing with the seed
// that lives in that page; this class is a process priority and a permanent
// notification, nothing more. Two Android facts make it necessary:
//
//   1. a backgrounded app with no foreground service is a cached process the
//      system kills at will, and App Standby throttles its network;
//   2. `WryActivity.onPause()` pauses the WebView. `MainActivity` re-resumes it
//      while `BackgroundMode.armed` is true, and that flag is set here.
//
// `START_NOT_STICKY` for the same reason: a service the system restarted after
// killing the process would have no WebView behind it, so its notification
// would claim a rule that is not running. Reopening the app re-arms.
//
// Doze is not fought. An unplugged, stationary device with the screen off has
// its network suspended for every app outside the battery-optimisation
// allowlist, foreground service or not; the app acquires no wake lock and asks
// for no exemption — `BackgroundPlugin.openBatterySettings` takes the member to
// the OS screen where granting one is theirs to do. (`WAKE_LOCK` is in the
// merged manifest, declared by `tauri-plugin-notification` for the scheduled
// notifications this client does not use. Nothing here takes one.)

package org.edet.client.background

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.graphics.drawable.toBitmap

/**
 * Whether background mode is armed on this device.
 *
 * Read by `MainActivity.onPause()`, which is the whole point: the flag says
 * whether to undo wry's pause of the WebView. It lives beside the service
 * rather than inside it because the activity must be able to ask without
 * binding to anything.
 */
object BackgroundMode {
    @Volatile
    @JvmStatic
    var armed: Boolean = false
        internal set
}

class AcceptanceService : Service() {

    companion object {
        private const val CHANNEL_ID = "edet-acceptance"
        private const val NOTIFICATION_ID = 4201
        /** `icon.svg`'s ground, so the tinted silhouette reads as edet's. */
        private const val ACCENT = 0xFF7B1FA2.toInt()
        const val EXTRA_TITLE = "title"
        const val EXTRA_BODY = "body"

        /**
         * Start the service in the foreground, with text the client already
         * localized. Idempotent: a second start updates the notification.
         *
         * Called while the app IS in the foreground (the member turned the
         * setting on, unlocked the vault, or opened the app with both already
         * true), which is what makes starting a foreground service legal.
         */
        fun arm(context: Context, title: String, body: String) {
            val intent = Intent(context, AcceptanceService::class.java)
                .putExtra(EXTRA_TITLE, title)
                .putExtra(EXTRA_BODY, body)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
            BackgroundMode.armed = true
        }

        /** Stop it. The WebView pauses again at the next `onPause`. */
        fun disarm(context: Context) {
            BackgroundMode.armed = false
            context.stopService(Intent(context, AcceptanceService::class.java))
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val title = intent?.getStringExtra(EXTRA_TITLE) ?: "edet"
        val body = intent?.getStringExtra(EXTRA_BODY) ?: ""
        val notification = build(title, body)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            ServiceCompat.startForeground(
                this,
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
            )
        } else {
            @Suppress("DEPRECATION")
            startForeground(NOTIFICATION_ID, notification)
        }
        BackgroundMode.armed = true
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        // Whatever ended this — `disarm`, a swipe from the task switcher, the
        // system reclaiming the process — the flag must not outlive it, or
        // `MainActivity` would go on re-resuming a WebView on behalf of a
        // service that is gone.
        BackgroundMode.armed = false
        super.onDestroy()
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        // Swiping the app away takes the WebView with it, and a rule with no
        // WebView decides nothing. Stopping here is what keeps the notification
        // honest.
        BackgroundMode.armed = false
        stopSelf()
        super.onTaskRemoved(rootIntent)
    }

    private fun build(title: String, body: String): Notification {
        ensureChannel()
        // Tapping it reopens the app. Resolved through the package manager
        // rather than naming `MainActivity`, so this module stays independent
        // of the app module that consumes it.
        val launch = packageManager.getLaunchIntentForPackage(packageName)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0
        val pending = launch?.let { PendingIntent.getActivity(this, 0, it, flags) }
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            // **Two icons, because Android draws them differently.** The small
            // one is masked to its alpha and tinted, so the launcher icon —
            // opaque edge to edge — would render as a white block; it gets the
            // silhouette instead (`ic_edet_notification`, the same mesh). The
            // large one is drawn as it is, so it gets the real app icon. A
            // permanent notification a member cannot recognise at a glance is
            // one they swipe away, taking the service's visibility with it.
            .setSmallIcon(R.drawable.ic_edet_notification)
            .setLargeIcon(appIcon())
            .setColor(ACCENT)
            .setContentIntent(pending)
            .setOngoing(true)
            .setShowWhen(false)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .build()
    }

    /**
     * The app's own launcher icon as a bitmap, or null.
     *
     * Through the package manager rather than `BitmapFactory.decodeResource`:
     * on API 26+ the icon resource is an adaptive-icon XML, which decodes to
     * null, and a notification that silently lost its icon is exactly the kind
     * of failure nobody reports.
     */
    private fun appIcon(): Bitmap? = try {
        packageManager.getApplicationIcon(packageName).toBitmap()
    } catch (e: Exception) {
        null
    }

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (manager.getNotificationChannel(CHANNEL_ID) != null) return
        // LOW: it is permanent, and a permanent notification that makes a
        // sound is one the member turns off, taking the service's visibility
        // with it.
        val channel = NotificationChannel(CHANNEL_ID, "edet", NotificationManager.IMPORTANCE_LOW)
        channel.setShowBadge(false)
        manager.createNotificationChannel(channel)
    }
}
