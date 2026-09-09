// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT
//
// edet: the Tauri command boundary over `DeviceKeyStore`.
//
// Everything about custody is in that class, deliberately: it takes a plain
// `Context` and no Tauri types, so the instrumented test drives the real
// wrapping and the real file on a real device without an Activity or an IPC
// bridge. This file is the adapter — the same shape as the desktop
// `keychain_get_device_key` / `keychain_set_device_key` commands
// (src-tauri/src/lib.rs), so `ui/src/common/vault.ts` needs one branch and not
// a new call convention: a hex-encoded 32-byte device key in, the same hex
// string back out, `null` when this device holds none.

package org.edet.client.keystore

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONObject

@InvokeArg
class SetDeviceKeyArgs {
    lateinit var hex: String
}

@TauriPlugin
class KeystorePlugin(private val activity: Activity) : Plugin(activity) {
    // Built per call rather than held: a Keystore that was transiently
    // unavailable at plugin-load time must not make every later call
    // permanently unusable for the lifetime of the process.
    private fun store() = DeviceKeyStore(activity.applicationContext)

    @Command
    fun getDeviceKey(invoke: Invoke) {
        try {
            val ret = JSObject()
            // JSONObject.NULL (not Kotlin null) so the key round-trips as a
            // literal JSON `null` rather than being dropped from the object
            // entirely (org.json's `put` removes the key on a plain null).
            ret.put("hex", store().get() ?: JSONObject.NULL)
            invoke.resolve(ret)
        } catch (e: UnwrapFailed) {
            // **Distinct from every other failure, and the message says so.**
            // A blob this device cannot open means the phone was restored or
            // reset: the member needs their recovery phrase, not a fallback to
            // browser storage. `vault.ts` reads the `unwrap-failed:` prefix.
            invoke.reject(e.message)
        } catch (e: Exception) {
            // Keystore unavailable, an API level without the expected
            // primitives: fail closed here so the Rust side's
            // `Result<_, String>` carries the error back to `vault.ts`, which
            // falls back to browser storage and says so loudly.
            invoke.reject(e.message ?: "keystore get failed")
        }
    }

    @Command
    fun setDeviceKey(invoke: Invoke) {
        try {
            store().set(invoke.parseArgs(SetDeviceKeyArgs::class.java).hex)
            invoke.resolve()
        } catch (e: IllegalArgumentException) {
            invoke.reject(e.message ?: "device key must be 64 lowercase hex characters")
        } catch (e: Exception) {
            invoke.reject(e.message ?: "keystore set failed")
        }
    }
}
