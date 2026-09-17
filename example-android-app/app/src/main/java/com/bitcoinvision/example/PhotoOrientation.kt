package com.bitcoinvision.example

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.produceState
import androidx.compose.runtime.key
import androidx.exifinterface.media.ExifInterface
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import java.io.IOException
import kotlin.math.max

internal val photoPreparationLock = Mutex()

/** Called on the attempt worker, once, before any pixels are displayed or scanned. */
internal fun resolvePhoto(photo: PhotoInput): ResolvedPhoto = try {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    photo.openStream().use { BitmapFactory.decodeStream(it, null, bounds) }
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
        throw PhotoPreparationException("The photo could not be opened. Try another.")
    }
    val exif = if (photo.camera == null) {
        try {
            photo.openStream().use { ExifInterface(it).getAttributeInt(ExifInterface.TAG_ORIENTATION, 0) }
        } catch (_: IOException) { null } catch (_: RuntimeException) { null }
    } else null
    ResolvedPhoto(
        photo, orientationFromMetadata(photo.camera, exif), bounds.outWidth, bounds.outHeight,
        photoUpFromMetadata(photo.camera, exif),
    )
} catch (_: RuntimeException) {
    throw PhotoPreparationException("The photo could not be prepared. Try another.")
}

/** Same eight-entry transform table as mobile/photo.rs, with no interpolation. */
internal fun orientBitmap(bitmap: Bitmap, orientation: Int): Bitmap {
    require(orientation in 1..8)
    if (orientation == 1) return bitmap
    val axes = when (orientation) {
        2 -> floatArrayOf(-1f, 0f, 0f, 1f)
        3 -> floatArrayOf(-1f, 0f, 0f, -1f)
        4 -> floatArrayOf(1f, 0f, 0f, -1f)
        5 -> floatArrayOf(0f, 1f, 1f, 0f)
        6 -> floatArrayOf(0f, -1f, 1f, 0f)
        7 -> floatArrayOf(0f, -1f, -1f, 0f)
        else -> floatArrayOf(0f, 1f, -1f, 0f)
    }
    val matrix = Matrix().apply {
        setValues(floatArrayOf(axes[0], axes[1], 0f, axes[2], axes[3], 0f, 0f, 0f, 1f))
    }
    return Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, false)
}

/** No encoded-byte copy and no full-resolution Kotlin bitmap, including for quarter turns. */
internal fun decodeSampled(photo: ResolvedPhoto, longSide: Int): Bitmap? {
    require(longSide > 0)
    var sample = 1
    while (max(photo.rawWidth, photo.rawHeight) / sample > longSide) sample *= 2
    val options = BitmapFactory.Options().apply { inSampleSize = sample }
    val raw = photo.source.openStream().use { BitmapFactory.decodeStream(it, null, options) } ?: return null
    return try {
        orientBitmap(raw, photo.orientation).also { if (it !== raw) raw.recycle() }
    } catch (e: RuntimeException) {
        raw.recycle()
        throw e
    }
}

@Composable
internal fun rememberPhotoBitmap(photo: ResolvedPhoto?): State<Bitmap?> = key(photo) {
    // Reset the state itself, not only the producer: no frame may show the
    // previous photo while a replacement producer is waiting to launch.
    produceState<Bitmap?>(null) {
        var decoded: Bitmap? = null
        try {
            withContext(Dispatchers.IO) { decoded = photo?.let { decodeSampled(it, 1280) } }
            value = decoded
            // Published bitmaps remain GC-owned while Compose may still draw them.
        } catch (e: CancellationException) {
            decoded?.recycle() // Never published: a replaced/disposed attempt owns no bitmap.
            throw e
        } catch (_: RuntimeException) {
            decoded?.recycle()
            value = null
        }
    }
}
