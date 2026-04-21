//! Android Storage Access Framework helpers.
//!
//! On Android the system file picker returns `content://` URIs instead
//! of real filesystem paths.  Rust's `std::fs` / `tokio::fs` cannot
//! open these URIs — they must be opened through Android's
//! `ContentResolver` API via JNI.
//!
//! Both functions create the `JavaVM` and attach the current thread
//! locally so that the `JNIEnv` and all `JObject`s live within a
//! single stack frame (avoiding lifetime escape issues).

use anyhow::{anyhow, Result};
use jni::{
    objects::{JByteArray, JObject, JString, JValue},
    JavaVM,
};

/// Write `bytes` to an Android `content://` URI.
///
/// Calls `ContentResolver.openOutputStream(uri)` and writes the full
/// buffer in one shot, then closes the stream.
pub fn write_content_uri(uri_str: &str, bytes: &[u8]) -> Result<()> {
    let ctx = ndk_context::android_context();
    // SAFETY: Tauri's Android runtime initialises the JVM and the
    // activity pointer in ndk_context before any Rust code runs.
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|e| anyhow!("JavaVM::from_raw: {e:?}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| anyhow!("attach_current_thread: {e:?}"))?;

    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let cr: JObject = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .map_err(|e| anyhow!("getContentResolver: {e:?}"))?
        .l()
        .map_err(|e| anyhow!("getContentResolver.l: {e:?}"))?;

    let uri = parse_uri(&mut env, uri_str)?;

    let os: JObject = env
        .call_method(
            &cr,
            "openOutputStream",
            "(Landroid/net/Uri;)Ljava/io/OutputStream;",
            &[JValue::Object(&uri)],
        )
        .map_err(|e| anyhow!("openOutputStream: {e:?}"))?
        .l()
        .map_err(|e| anyhow!("openOutputStream.l: {e:?}"))?;

    let jarr: JByteArray = env
        .byte_array_from_slice(bytes)
        .map_err(|e| anyhow!("byte_array_from_slice: {e:?}"))?;

    env.call_method(&os, "write", "([B)V", &[JValue::Object(&jarr)])
        .map_err(|e| anyhow!("OutputStream.write: {e:?}"))?;

    env.call_method(&os, "close", "()V", &[])
        .map_err(|e| anyhow!("OutputStream.close: {e:?}"))?;

    Ok(())
}

/// Read all bytes from an Android `content://` URI.
///
/// Calls `ContentResolver.openInputStream(uri)` and drains it in
/// 8 KiB chunks, then closes the stream.
pub fn read_content_uri(uri_str: &str) -> Result<Vec<u8>> {
    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }
        .map_err(|e| anyhow!("JavaVM::from_raw: {e:?}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| anyhow!("attach_current_thread: {e:?}"))?;

    let activity = unsafe { JObject::from_raw(ctx.context().cast()) };

    let cr: JObject = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .map_err(|e| anyhow!("getContentResolver: {e:?}"))?
        .l()
        .map_err(|e| anyhow!("getContentResolver.l: {e:?}"))?;

    let uri = parse_uri(&mut env, uri_str)?;

    let is: JObject = env
        .call_method(
            &cr,
            "openInputStream",
            "(Landroid/net/Uri;)Ljava/io/InputStream;",
            &[JValue::Object(&uri)],
        )
        .map_err(|e| anyhow!("openInputStream: {e:?}"))?
        .l()
        .map_err(|e| anyhow!("openInputStream.l: {e:?}"))?;

    let buf: JByteArray = env
        .new_byte_array(8192)
        .map_err(|e| anyhow!("new_byte_array: {e:?}"))?;

    let mut result: Vec<u8> = Vec::new();
    loop {
        let n: i32 = env
            .call_method(&is, "read", "([B)I", &[JValue::Object(&buf)])
            .map_err(|e| anyhow!("InputStream.read: {e:?}"))?
            .i()
            .map_err(|e| anyhow!("InputStream.read.i: {e:?}"))?;
        if n <= 0 {
            break;
        }
        // convert_byte_array returns the full 8 KiB buffer; take only
        // the `n` bytes that were actually filled.
        let chunk: Vec<u8> = env
            .convert_byte_array(&buf)
            .map_err(|e| anyhow!("convert_byte_array: {e:?}"))?;
        result.extend_from_slice(&chunk[..n as usize]);
    }

    env.call_method(&is, "close", "()V", &[])
        .map_err(|e| anyhow!("InputStream.close: {e:?}"))?;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// `Uri.parse(uri_str)` — returns a local `JObject` valid for the
/// duration of the caller's `JNIEnv` borrow.
fn parse_uri<'local>(
    env: &mut jni::JNIEnv<'local>,
    uri_str: &str,
) -> Result<JObject<'local>> {
    let jstr: JString = env
        .new_string(uri_str)
        .map_err(|e| anyhow!("new_string: {e:?}"))?;
    let cls = env
        .find_class("android/net/Uri")
        .map_err(|e| anyhow!("find_class Uri: {e:?}"))?;
    env.call_static_method(
        cls,
        "parse",
        "(Ljava/lang/String;)Landroid/net/Uri;",
        &[JValue::Object(&jstr)],
    )
    .map_err(|e| anyhow!("Uri.parse: {e:?}"))?
    .l()
    .map_err(|e| anyhow!("Uri.parse.l: {e:?}"))
}
