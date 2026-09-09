// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT
//
// edet: Android Keystore custody for the identity vault's device key.
//
// **The device key is not the Keystore key.** Keystore keys are
// non-exportable by design and the vault needs the raw 32 bytes for
// XChaCha20-Poly1305 (@noble/ciphers), so the Keystore holds a WRAPPING key —
// AES-256-GCM, generated in the Keystore and never leaving it — and what
// reaches disk is only the wrapped blob.
//
// **The blob lives under `noBackupFilesDir`, and that is the whole point of
// this class.** Android's default cloud backup and device-to-device transfer
// both copy an app's ordinary files and shared preferences, and neither can
// copy the Keystore key that wraps them. A wrapped key restored onto a new
// phone is therefore a blob nothing on that phone can open — which reads as a
// corrupt keystore unless the code says otherwise. `noBackupFilesDir` is the
// one directory both mechanisms skip, so a restored phone lands as "no key",
// the recovery-phrase path runs, and custody is re-established rather than
// downgraded.
//
// Plain Android, no Tauri types: this is what the instrumented test drives on
// a device, where a `Plugin` needs an `Activity` and an IPC bridge.

package org.edet.client.keystore

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** A blob exists but this device cannot open it — restored, reset, or the
 *  Keystore key invalidated. NOT the same as "no key": the member has to
 *  restore from their recovery phrase, and telling them their keystore is
 *  broken instead would send them looking for a fault that is not there.
 *
 *  The message prefix is load-bearing: `ui/src/common/vault.ts` reads it to
 *  tell this case apart from every other keystore failure, which fall back to
 *  browser storage. */
class UnwrapFailed(cause: Throwable?) : Exception("unwrap-failed: ${cause?.message ?: "the wrapped device key could not be opened"}", cause)

class DeviceKeyStore(private val context: Context) {
    companion object {
        /** The Keystore alias of the wrapping key. */
        const val KEY_ALIAS = "edet_vault_device_key_wrap"
        /** The wrapped blob, under the one directory backup and transfer skip. */
        const val BLOB_NAME = "edet_device_key.bin"

        private const val ANDROID_KEYSTORE = "AndroidKeyStore"
        private const val TRANSFORM = "AES/GCM/NoPadding"
        /** `[version 0x01][12-byte IV][ciphertext+tag]`. */
        private const val VERSION: Byte = 0x01
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
        private const val DEVICE_KEY_BYTES = 32
    }

    /** Where the blob lives. Public so the instrumented test can assert the
     *  directory rather than trust this comment. */
    fun blobFile(): File = File(context.noBackupFilesDir, BLOB_NAME)

    /**
     * The device key as 64 lowercase hex characters, or `null` when this
     * device holds none.
     *
     * Throws [UnwrapFailed] when a blob exists and cannot be opened. **Fail
     * closed, never null**: returning null there would tell the vault "fresh
     * device", and the vault would mint a NEW device key and overwrite the
     * blob — destroying the only thing that could still have decrypted the
     * sealed seed ring if the fault were transient.
     */
    fun get(): String? {
        val file = blobFile()
        if (!file.exists()) return null
        val blob = file.readBytes()
        if (blob.size < 1 + IV_BYTES + 1 || blob[0] != VERSION) {
            throw UnwrapFailed(IllegalStateException("device key blob is not a version ${VERSION.toInt()} blob"))
        }
        val key = existingKey() ?: throw UnwrapFailed(IllegalStateException("the wrapping key is gone from the Keystore"))
        return try {
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(
                Cipher.DECRYPT_MODE,
                key,
                GCMParameterSpec(TAG_BITS, blob, 1, IV_BYTES),
            )
            val plain = cipher.doFinal(blob, 1 + IV_BYTES, blob.size - 1 - IV_BYTES)
            if (plain.size != DEVICE_KEY_BYTES) {
                throw UnwrapFailed(IllegalStateException("device key is ${plain.size} bytes, not $DEVICE_KEY_BYTES"))
            }
            plain.joinToString("") { "%02x".format(it) }
        } catch (e: UnwrapFailed) {
            throw e
        } catch (e: Exception) {
            // AEADBadTagException, KeyPermanentlyInvalidatedException, a
            // provider that cannot use the alias — every one of them means the
            // same thing to the member.
            throw UnwrapFailed(e)
        }
    }

    /**
     * Wrap `hex` (64 lowercase hex characters) and persist it, replacing
     * whatever was there.
     *
     * Written to a temp file and renamed, because a half-written blob is a
     * vault nobody can open: `rename` is atomic within a directory, so the
     * file a later `get` finds is either the old one or the whole new one.
     */
    fun set(hex: String) {
        // ASCII digits and lowercase a-f, written out: `isDigit()` is true
        // of every Unicode digit and `lowercaseChar()` accepts an uppercase
        // key. The vault sends `bytesToHex` output, which is lowercase ASCII,
        // so anything else is a caller bug — and taking it would hide that
        // bug behind a key `get` answers in a spelling `set` never saw.
        require(hex.length == DEVICE_KEY_BYTES * 2 && hex.all { it in '0'..'9' || it in 'a'..'f' }) {
            "device key must be ${DEVICE_KEY_BYTES * 2} lowercase hex characters"
        }
        val raw = ByteArray(DEVICE_KEY_BYTES) { i -> hex.substring(i * 2, i * 2 + 2).toInt(16).toByte() }
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, existingKey() ?: generateKey())
        val sealed = cipher.doFinal(raw)
        val iv = cipher.iv
        check(iv.size == IV_BYTES) { "AES-GCM IV is ${iv.size} bytes, not $IV_BYTES" }

        val blob = ByteArray(1 + iv.size + sealed.size)
        blob[0] = VERSION
        System.arraycopy(iv, 0, blob, 1, iv.size)
        System.arraycopy(sealed, 0, blob, 1 + iv.size, sealed.size)

        val dir = context.noBackupFilesDir
        dir.mkdirs()
        val tmp = File.createTempFile("edet_device_key", ".tmp", dir)
        try {
            tmp.writeBytes(blob)
            if (!tmp.renameTo(blobFile())) {
                throw IllegalStateException("could not replace ${blobFile().absolutePath}")
            }
        } finally {
            tmp.delete()
        }
    }

    /** Forget the key and the blob both. For the instrumented test; nothing in
     *  the app calls it, because losing the device key loses the sealed vault. */
    fun clear() {
        blobFile().delete()
        keystore().deleteEntry(KEY_ALIAS)
    }

    private fun keystore(): KeyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }

    private fun existingKey(): SecretKey? = keystore().getKey(KEY_ALIAS, null) as? SecretKey

    /**
     * Mint the wrapping key inside the Keystore.
     *
     * StrongBox where the device has it — a separate security chip, so the key
     * is beyond even a compromised kernel — and the ordinary TEE-backed
     * Keystore where it does not. Attempted rather than required: most devices
     * have no StrongBox, and refusing them would mean no custody at all on a
     * phone that has perfectly good Keystore backing.
     *
     * No user authentication requirement: the vault's own passphrase is the
     * thing that gates use of the seed ring, and requiring a device unlock
     * here as well would make the key unavailable to a background refresh that
     * legitimately needs it.
     */
    private fun generateKey(): SecretKey {
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        val spec = {
            KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                // Randomized encryption: the IV is the provider's per-call
                // draw, never ours. A reused IV under one GCM key is a total
                // break of confidentiality and authenticity both.
                .setRandomizedEncryptionRequired(true)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            try {
                generator.init(spec().setIsStrongBoxBacked(true).build())
                return generator.generateKey()
            } catch (e: StrongBoxUnavailableException) {
                // Fall through: this device has no StrongBox.
            }
        }
        generator.init(spec().build())
        return generator.generateKey()
    }
}
