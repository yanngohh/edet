package org.edet.conductor_service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * Android Foreground Service that keeps the edet conductor process alive.
 *
 * Foreground services are given high priority by the OS — they will not be
 * killed under normal memory pressure and survive the user swiping the app
 * away from the recents list (on stock Android ROMs).
 *
 * The persistent notification is required by Android to indicate to the user
 * that an app is running in the background. It is shown with low importance
 * (no sound, no heads-up) to minimise intrusiveness.
 *
 * Service type: `specialUse`
 *   - No time limit (unlike dataSync which caps at 6h on API 34+)
 *   - Required for P2P network nodes that don't fit other categories
 *   - Requires Play Store justification via PROPERTY_SPECIAL_USE_FGS_SUBTYPE
 *
 * OEM note: On Xiaomi, Huawei, Samsung, and OnePlus, the user may need to
 * whitelist edet in the manufacturer's battery/autostart settings to prevent
 * the service from being killed by the OEM task killer. The BatteryHelper
 * utility and the `requestBatteryExemption` command assist with this.
 */
class EdetConductorService : Service() {

    companion object {
        const val CHANNEL_ID = "edet_conductor"
        const val NOTIFICATION_ID = 1
    }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        val notification = buildNotification()

        // API 34+ requires the foreground service type to be passed at runtime
        // in addition to being declared in the manifest.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    /**
     * START_STICKY ensures the service is restarted by the OS if it is ever
     * killed due to extreme memory pressure. The conductor will then re-boot
     * on the next Tauri app launch (since edet only boots the conductor when
     * the activity is opened). A future enhancement could auto-restart the
     * conductor from within onStartCommand if needed.
     */
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        return START_STICKY
    }

    /**
     * Called when the user swipes the app from the recents list. On stock
     * Android this does NOT kill a foreground service — the service (and
     * therefore the conductor) keeps running. OEM task killers may override
     * this behaviour; the battery exemption prompt mitigates that.
     */
    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
        // Intentional no-op: conductor continues running.
    }

    // Foreground services do not support binding.
    override fun onBind(intent: Intent?): IBinder? = null

    // ── Notification ──────────────────────────────────────────────────────────

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "edet Network",
                // IMPORTANCE_LOW = no sound, no heads-up banner
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Keeps edet connected to the peer network"
                setShowBadge(false)
            }
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(channel)
        }
    }

    private fun buildNotification(): Notification {
        // Build without a PendingIntent for now; tapping the notification
        // does nothing. A future enhancement could re-open the main activity.
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
        }
        return builder
            .setContentTitle("edet")
            .setContentText("Connected to peer network")
            // ic_menu_share is a built-in Android system icon — replace with
            // a custom R.drawable.ic_notification once branding assets exist.
            .setSmallIcon(android.R.drawable.ic_menu_share)
            .setOngoing(true)
            .build()
    }
}
