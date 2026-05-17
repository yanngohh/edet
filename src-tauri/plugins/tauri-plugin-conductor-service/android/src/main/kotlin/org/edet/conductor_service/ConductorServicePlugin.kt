package org.edet.conductor_service

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * Tauri plugin that manages the edet conductor foreground service.
 *
 * The service is started automatically when the plugin loads (`load()` hook),
 * which runs during Tauri application initialization — before the WebView
 * renders. This guarantees the conductor process is marked as foreground
 * before any conductor operations begin.
 *
 * Commands exposed to Rust/TS:
 *  - start_service                  — start the foreground service
 *  - stop_service                   — stop the foreground service
 *  - request_battery_exemption      — prompt user to whitelist from battery optimization
 *  - is_battery_optimized           — query whether battery optimization is active
 *  - is_shared_conductor_available  — detect android-service-runtime package
 */
@TauriPlugin
class ConductorServicePlugin(private val activity: Activity) : Plugin(activity) {

    override fun load(webView: WebView) {
        super.load(webView)
        // Auto-start the foreground service as soon as the plugin loads.
        // This is the earliest safe point to start a foreground service
        // (the activity is in the foreground at this moment).
        startConductorService()
    }

    // ── Commands ──────────────────────────────────────────────────────────────

    @Command
    fun startService(invoke: Invoke) {
        startConductorService()
        invoke.resolve()
    }

    @Command
    fun stopService(invoke: Invoke) {
        activity.stopService(Intent(activity, EdetConductorService::class.java))
        invoke.resolve()
    }

    @Command
    fun requestBatteryExemption(invoke: Invoke) {
        BatteryHelper.requestExemption(activity)
        invoke.resolve()
    }

    @Command
    fun isBatteryOptimized(invoke: Invoke) {
        val optimized = BatteryHelper.isOptimized(activity)
        val ret = JSObject()
        ret.put("optimized", optimized)
        invoke.resolve(ret)
    }

    /**
     * Check whether the holochain/android-service-runtime app is installed
     * on this device. Used for optional Phase 2 shared-conductor detection.
     */
    @Command
    fun isSharedConductorAvailable(invoke: Invoke) {
        val available = try {
            activity.packageManager.getPackageInfo(
                "org.holochain.androidserviceruntime",
                0
            )
            true
        } catch (e: PackageManager.NameNotFoundException) {
            false
        }
        val ret = JSObject()
        ret.put("available", available)
        invoke.resolve(ret)
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    private fun startConductorService() {
        val intent = Intent(activity, EdetConductorService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            activity.startForegroundService(intent)
        } else {
            activity.startService(intent)
        }
    }
}
