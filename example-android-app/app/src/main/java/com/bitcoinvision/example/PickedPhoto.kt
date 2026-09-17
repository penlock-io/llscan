package com.bitcoinvision.example

import android.os.Build
import java.io.ByteArrayOutputStream
import java.io.InputStream

/**
 * A photo a person picks out of their files comes from anywhere: a
 * remote provider, a scanner app, a video. The camera's photos are a
 * few megabytes; this is the point past which the file is not a
 * photograph of a page and is not worth holding in memory twice.
 */
const val PICKED_PHOTO_LIMIT = 32 * 1024 * 1024

/**
 * Up to [limit] bytes of [input], or null if it holds more. The stream
 * is never read into memory past the limit, so an enormous document
 * costs one buffer rather than the phone.
 */
fun readAtMost(input: InputStream, limit: Int = PICKED_PHOTO_LIMIT): ByteArray? {
    require(limit > 0)
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(64 * 1024)
    while (true) {
        val read = input.read(buffer)
        if (read < 0) return out.toByteArray()
        if (out.size() + read > limit) return null
        out.write(buffer, 0, read)
    }
}

/**
 * Whether this build is running on an emulator, by the same marks the
 * Android tooling itself uses.
 *
 * The debug fixture route reads a directory only `adb` can fill, so it
 * is of no use on a phone; the click-through suite that does use it
 * runs on the owned emulator alone.
 */
val onEmulator: Boolean by lazy {
    looksLikeEmulator(Build.FINGERPRINT, Build.HARDWARE, Build.PRODUCT, Build.MODEL)
}

internal fun looksLikeEmulator(fingerprint: String, hardware: String, product: String, model: String): Boolean =
    fingerprint.startsWith("generic") ||
        fingerprint.startsWith("unknown") ||
        fingerprint.contains("emulator") ||
        hardware == "goldfish" || hardware == "ranchu" || hardware.contains("goldfish") ||
        product.startsWith("sdk") || product.contains("_sdk") || product.contains("sdk_") ||
        model.contains("Emulator") || model.startsWith("Android SDK built for")
