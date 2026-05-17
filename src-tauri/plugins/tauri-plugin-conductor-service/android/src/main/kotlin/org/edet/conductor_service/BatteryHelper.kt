package org.edet.conductor_service

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings

/**
 * Utilities for Android battery optimization management.
 *
 * The edet conductor must communicate over the network while the screen is
 * off (to receive support transactions and participate in DHT gossip). Android
 * Doze mode restricts network access in these conditions unless the app is
 * whitelisted from battery optimization.
 *
 * Additionally, OEM battery savers on Xiaomi, Huawei, Samsung, and OnePlus
 * may aggressively kill the foreground service regardless of the standard
 * Android battery optimization setting. This helper provides both the
 * standard Android API and manufacturer-specific deep links.
 */
object BatteryHelper {

    /**
     * Returns true if Android battery optimization is active for edet
     * (i.e. edet is NOT on the optimization whitelist).
     * Returns false if edet is already exempt (good state — no action needed).
     */
    fun isOptimized(context: Context): Boolean {
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager
        return !pm.isIgnoringBatteryOptimizations(context.packageName)
    }

    /**
     * Show the system dialog asking the user to exempt edet from battery
     * optimization. This uses the standard Android API which is allowed by
     * Google Play policy for apps whose core function requires background
     * network access (P2P nodes qualify).
     *
     * After exemption: the device will allow network access during Doze mode
     * maintenance windows.
     */
    fun requestExemption(context: Context) {
        if (isOptimized(context)) {
            val intent = Intent(
                Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS
            ).apply {
                data = Uri.parse("package:${context.packageName}")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            context.startActivity(intent)
        }
    }

    /**
     * Attempt to open the OEM-specific battery/autostart settings for this
     * device. These settings are needed on manufacturer-customized Android
     * ROMs where aggressive task killers ignore the standard battery
     * optimization whitelist.
     *
     * Falls back to the generic Android battery settings if the OEM activity
     * is not found (e.g. on a future ROM version that changed the component).
     *
     * Callers should display an in-app guide with device-specific screenshots
     * alongside this intent, since the OEM settings UI changes frequently.
     */
    fun openOemSettings(context: Context) {
        val manufacturer = Build.MANUFACTURER.lowercase()

        val oemIntent: Intent? = when {
            manufacturer.contains("xiaomi") -> Intent().apply {
                setClassName(
                    "com.miui.securitycenter",
                    "com.miui.permcenter.autostart.AutoStartManagementActivity"
                )
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            manufacturer.contains("huawei") -> Intent().apply {
                setClassName(
                    "com.huawei.systemmanager",
                    "com.huawei.systemmanager.optimize.process.ProtectActivity"
                )
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            manufacturer.contains("samsung") -> Intent().apply {
                setClassName(
                    "com.samsung.android.lool",
                    "com.samsung.android.sm.battery.ui.BatteryActivity"
                )
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            manufacturer.contains("oneplus") || manufacturer.contains("oppo") -> Intent().apply {
                setClassName(
                    "com.coloros.safecenter",
                    "com.coloros.privacypermissionsentry.PermissionTopActivity"
                )
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            else -> null
        }

        val fallback = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }

        val toStart = if (oemIntent != null) {
            try {
                // Verify the OEM activity is actually present before launching
                context.packageManager.resolveActivity(oemIntent, 0)
                oemIntent
            } catch (e: Exception) {
                fallback
            }
        } else {
            fallback
        }

        try {
            context.startActivity(toStart)
        } catch (e: Exception) {
            // Final fallback: generic Android settings screen
            try {
                context.startActivity(fallback)
            } catch (ignored: Exception) {
                // Nothing to do if even the generic settings won't open
            }
        }
    }
}
