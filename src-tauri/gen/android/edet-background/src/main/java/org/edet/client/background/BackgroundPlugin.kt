// edet: the Tauri command boundary over `AcceptanceService`.
//
// The same shape as `KeystorePlugin`: everything real is in a class that takes
// a plain `Context` and no Tauri types, and this file is the adapter. Five
// commands, called from `src-tauri/src/android_background.rs`:
//
//   arm(title, body)      start the foreground service with localized text
//   disarm()              stop it
//   batteryOptimised()    is edet still subject to Doze?
//   openBatterySettings() the OS screen where the exemption is granted
//   systemInsets()        how much of the screen the system bars are using
//
// None of them signs anything. This module is what the PLATFORM does to the
// page — keeps it running, sleeps it, and covers its edges — which is why the
// last one lives here rather than in a module of its own for one getter.

package org.edet.client.background

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class ArmArgs {
    lateinit var title: String
    lateinit var body: String
}

@TauriPlugin
class BackgroundPlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun arm(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(ArmArgs::class.java)
            AcceptanceService.arm(activity.applicationContext, args.title, args.body)
            invoke.resolve()
        } catch (e: Exception) {
            // Fail closed and say why: the client turns `backgroundArmed` off
            // on any rejection, so the settings screen tells the member this
            // device will stop deciding when they put it down rather than
            // letting them believe otherwise.
            invoke.reject(e.message ?: "could not start the background service")
        }
    }

    @Command
    fun disarm(invoke: Invoke) {
        try {
            AcceptanceService.disarm(activity.applicationContext)
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject(e.message ?: "could not stop the background service")
        }
    }

    /**
     * How much of the screen the system bars and the cutout are covering, in
     * CSS pixels.
     *
     * **A pull, not a push, and the difference is a measurement.** Publishing
     * these from `MainActivity.onWebViewCreate` reaches a document the real
     * page then replaces: on a handset the custom properties read back
     * empty. Asked for by the page, when the page is ready to use them, there
     * is nothing to race.
     *
     * The conversion is exact rather than approximate: the page declares
     * `width=device-width, initial-scale=1`, so one CSS pixel IS one
     * density-independent pixel and `density` is the whole of it.
     *
     * **`getInsetsIgnoringVisibility`, and the difference is not academic.**
     * `getInsets` counts only sources that are visible RIGHT NOW, and a
     * navigation bar hides transiently — behind the notification shade, during
     * its own animation. Asked in that moment it answers 0, truthfully, and a
     * page that believed it would put its footer back under the buttons a
     * second later with nothing to say so. Measured on a handset, where
     * the system reported the bar as `visible=false` with an inset hint of
     * 144 px, and this command returned zero. What the layout wants is the
     * room the bar takes when it is there, which is what "ignoring visibility"
     * means; this client never goes immersive, so the two only differ while
     * something is animating.
     *
     * A window with no insets to report is an ERROR, not a zero: the page
     * falls back to `env()` and asks again, rather than reserving nothing
     * because it asked too early.
     */
    @Command
    fun systemInsets(invoke: Invoke) {
        try {
            val insets = ViewCompat.getRootWindowInsets(activity.window.decorView)
                ?: throw IllegalStateException("the window has no insets yet")
            val bars = insets.getInsetsIgnoringVisibility(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout(),
            )
            val d = activity.resources.displayMetrics.density
            val ret = JSObject()
            ret.put("top", bars.top / d)
            ret.put("bottom", bars.bottom / d)
            ret.put("left", bars.left / d)
            ret.put("right", bars.right / d)
            invoke.resolve(ret)
        } catch (e: Exception) {
            invoke.reject(e.message ?: "could not read the window insets")
        }
    }

    /**
     * Is this app STILL subject to battery optimisation?
     *
     * A read, and it needs no permission — unlike asking to be exempted, which
     * needs one this app does not hold. It is what keeps the briefing from
     * firing at a member who has already granted the exemption: an app that
     * asks again for something it was already given teaches people to dismiss
     * it unread. Below API 23 there is no Doze and nothing to grant.
     */
    @Command
    fun batteryOptimised(invoke: Invoke) {
        try {
            val exempt = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                val power = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
                power.isIgnoringBatteryOptimizations(activity.packageName)
            } else {
                true
            }
            val ret = JSObject()
            ret.put("optimised", !exempt)
            invoke.resolve(ret)
        } catch (e: Exception) {
            invoke.reject(e.message ?: "could not read the battery optimisation state")
        }
    }

    /**
     * Android's battery-optimisation list.
     *
     * The LIST, not `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`: that one
     * needs a permission that lets an app ask to be exempted, and this app
     * holds none. Doze is documented, not fought — the exemption is the
     * member's own act, in the OS's own screen.
     */
    @Command
    fun openBatterySettings(invoke: Invoke) {
        try {
            val intent = Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            activity.applicationContext.startActivity(intent)
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject(e.message ?: "no battery optimisation settings on this device")
        }
    }
}
