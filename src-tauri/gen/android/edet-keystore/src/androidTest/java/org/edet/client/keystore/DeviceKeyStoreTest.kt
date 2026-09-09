// The only thing in this tree that RUNS Android custody.
//
// Everything else about the device key is reasoning: a Keystore key and a file
// under `noBackupFilesDir` exist on a device or an emulator and nowhere else,
// so a unit test on the host would be a test of a mock. Run it with
// `just android-keystore-test`, which starts the emulator first, and in CI by
// `.github/workflows/android-custody.yaml` on every push.
//
// Four claims, and the two that matter are the last two: WHERE the blob lives
// (backup and device transfer both skip `noBackupFilesDir`, and that is what
// makes a restored phone land as "no key" rather than as a corrupt keystore)
// and what happens when a blob outlives its wrapping key (fail closed, so the
// member is sent to their recovery phrase instead of having a fresh key minted
// over the only thing that could still open their vault).

package org.edet.client.keystore

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.security.KeyStore
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class DeviceKeyStoreTest {
    private val context = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val store = DeviceKeyStore(context)
    private val hex = "0123456789abcdef".repeat(4)

    @Before
    fun clean() = store.clear()

    @After
    fun tidy() = store.clear()

    /** The ordinary path: what goes in comes back out, wrapped in between. */
    @Test
    fun setThenGetRoundTrips() {
        store.set(hex)
        assertEquals(hex, store.get())
        // A second store instance, because the app builds one per call — a
        // round trip that only worked through a cached cipher would say
        // nothing about a process that was killed and restarted.
        assertEquals(hex, DeviceKeyStore(context).get())
    }

    /** A device that holds no key says so, and says it with `null` rather than
     *  by throwing — this is the state every first run is in. */
    @Test
    fun getWithoutBlobIsNull() {
        assertNull(store.get())
    }

    /**
     * **The blob lives where backup and device transfer never look.**
     *
     * Android's cloud backup and device-to-device transfer both copy an app's
     * ordinary files and shared preferences, and neither can copy the Keystore
     * key that wraps them — so a wrapped key restored onto a new phone is a
     * blob nothing on that phone can open. `noBackupFilesDir` is the one
     * directory both skip.
     *
     * Mutation that bites: write under `context.filesDir` instead.
     */
    @Test
    fun theBlobLivesUnderNoBackupFilesDir() {
        store.set(hex)
        val path = store.blobFile().absolutePath
        assertTrue("the blob is at $path", path.startsWith(context.noBackupFilesDir.absolutePath))
        assertTrue("the blob exists at $path", store.blobFile().exists())
        assertTrue(
            "and it is not under filesDir, which backup copies",
            !path.startsWith(context.filesDir.absolutePath + "/") || path.startsWith(context.noBackupFilesDir.absolutePath),
        )
    }

    /**
     * **A blob whose key is gone fails CLOSED.**
     *
     * This is the restored-phone case, simulated exactly: the blob survives,
     * the Keystore entry does not. Returning null here would tell the vault
     * "fresh device", and the vault would mint a new device key and overwrite
     * the blob — destroying the only thing that could still have decrypted the
     * sealed seed ring.
     *
     * Mutation that bites: catch the decrypt failure in `DeviceKeyStore.get`
     * and return null.
     */
    @Test
    fun aBlobWithoutItsKeyFailsClosed() {
        store.set(hex)
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(DeviceKeyStore.KEY_ALIAS)
        assertTrue("the blob is still there", store.blobFile().exists())
        try {
            store.get()
            fail("a blob this device cannot open must not read as no key")
        } catch (e: UnwrapFailed) {
            assertTrue(
                "the message carries the prefix vault.ts routes on: ${e.message}",
                e.message!!.startsWith("unwrap-failed:"),
            )
        }
    }

    /** A blob that is not a version-1 blob is unopenable, not silently
     *  reinterpreted — a format change has to be a decision. */
    @Test
    fun aBlobOfAnotherFormatFailsClosed() {
        store.set(hex)
        store.blobFile().writeBytes(byteArrayOf(0x02, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13))
        try {
            store.get()
            fail("a blob of an unknown version must not be opened")
        } catch (e: UnwrapFailed) {
            assertTrue(e.message!!.startsWith("unwrap-failed:"))
        }
    }

    /** Overwriting replaces the whole blob, so a rotated device key does not
     *  leave the old one recoverable beside it. */
    @Test
    fun setReplacesTheBlob() {
        store.set(hex)
        val other = "fedcba9876543210".repeat(4)
        store.set(other)
        assertEquals(other, store.get())
    }

    /** The one argument check: a device key is 32 bytes and the vault sends it
     *  as 64 lowercase hex characters. */
    @Test
    fun aMalformedKeyIsRefused() {
        for (bad in listOf("", "00", hex.uppercase(), hex + "00", "zz".repeat(32))) {
            try {
                store.set(bad)
                fail("must refuse a device key of ${bad.length} characters")
            } catch (e: IllegalArgumentException) {
                // the refusal
            }
        }
    }
}
