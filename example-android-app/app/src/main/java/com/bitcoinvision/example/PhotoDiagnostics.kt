package com.bitcoinvision.example

import android.content.Context
import android.graphics.BitmapFactory
import android.util.Log
import androidx.exifinterface.media.ExifInterface
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.security.MessageDigest
import uniffi.bitcoin_vision_mobile.canonicalPhoto
import uniffi.bitcoin_vision_mobile.VisionException
import uniffi.bitcoin_vision_mobile.DiagnosedScan
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** Metadata only: never log a photo, its hash, OCR output or other EXIF fields. */
internal fun photoDiagnosticMetadata(photo: PhotoInput): JSONObject {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    photo.openStream().use { BitmapFactory.decodeStream(it, null, bounds) }
    var exifStatus = "read"
    val orientation = try {
        photo.openStream().use {
            ExifInterface(it).getAttribute(ExifInterface.TAG_ORIENTATION)?.toIntOrNull()
        }
    } catch (_: IOException) {
        exifStatus = "unreadable"
        null
    } catch (_: RuntimeException) {
        exifStatus = "unreadable"
        null
    }
    return JSONObject().apply {
        put("schema", 2)
        put("source", if (photo.camera == null) "file" else "camera")
        put("pixelTransformApplied", false)
        put("encodedWidth", bounds.outWidth)
        put("encodedHeight", bounds.outHeight)
        put("mimeType", bounds.outMimeType ?: JSONObject.NULL)
        put("exifOrientation", orientation ?: JSONObject.NULL)
        put("exifStatus", exifStatus)
        put("orientationToCaptureTarget", orientationFromMetadata(photo.camera, orientation))
        photo.camera?.let {
            put("rotationDegrees", it.rotationDegrees)
            put("proxyWidth", it.width)
            put("proxyHeight", it.height)
            put("imageFormat", it.format)
            put("displayRotationAtShutter", it.displayRotationAtShutter ?: JSONObject.NULL)
            put("sensorToDisplayDegreesAtShutter", it.sensorToDisplayDegreesAtShutter ?: JSONObject.NULL)
            put("proxyCropRect", it.cropRect?.json() ?: JSONObject.NULL)
            put("previewAtShutter", it.previewAtShutter?.let { preview ->
                JSONObject().apply {
                    put("boundsInParent", preview.bounds.json())
                    put("scaleType", preview.scaleType)
                    put("layoutDirection", preview.layoutDirection)
                }
            } ?: JSONObject.NULL)
            put("captureCropApplied", false)
        }
    }
}

private fun CaptureBounds.json(): JSONObject = JSONObject().apply {
    put("left", left)
    put("top", top)
    put("right", right)
    put("bottom", bottom)
}

internal fun logPhotoInput(photo: PhotoInput) {
    if (!BuildConfig.DEBUG) return
    // Diagnostics must not prevent the callback reaching its attempt.
    try {
        Log.i("VisionCapture", photoDiagnosticMetadata(photo).toString())
    } catch (_: RuntimeException) {
        Log.i("VisionCapture", "Capture metadata unavailable")
    }
}

/** Native result tied to the exact resolved capture, before any review edits. */
internal data class SavedScanDiagnosis(val photo: ResolvedPhoto, val result: DiagnosedScan)

/** Only the explicit debug button calls this; capture itself never writes files. */
internal suspend fun saveRawPhotoForDiagnosis(context: Context, photo: PhotoInput?, diagnosis: SavedScanDiagnosis? = null): String? {
    if (!BuildConfig.DEBUG || photo == null) return null
    return withContext(Dispatchers.IO) {
        val dir = context.getExternalFilesDir("captures") ?: return@withContext null
        try {
            val saved = writeRawPhotoDiagnostic(photo, dir)
            val message = "Saved raw photo as captures/${saved.name} and ${saved.name}.json"
            if (diagnosis != null && diagnosis.photo.source === photo) {
                try {
                    val bundle = writeScanDiagnostic(saved, diagnosis.result) {
                        canonicalPhoto(photo.copyBytes(), diagnosis.photo.orientation.toUByte())
                    }
                    "$message; decisions and crops in captures/${bundle.name}"
                } catch (_: Exception) {
                    "$message; decision export failed (no complete bundle saved)"
                }
            } else "$message. Processing decisions not included."
        } catch (_: IOException) {
            null
        } catch (_: RuntimeException) {
            null
        }
    }
}

private val canonicalExportLock = Mutex()

/** Expensive full-resolution encoding is confined to this explicit debug action. */
internal suspend fun saveCanonicalPhotoForDiagnosis(context: Context, photo: ResolvedPhoto?): String? {
    if (!BuildConfig.DEBUG || photo == null) return null
    return withContext(Dispatchers.IO) {
        canonicalExportLock.withLock {
            var output: File? = null
            try {
                val dir = context.getExternalFilesDir("captures") ?: return@withLock null
                val png = canonicalPhoto(photo.source.copyBytes(), photo.orientation.toUByte())
                output = File.createTempFile("upright-scan-", ".png", dir)
                output.writeBytes(png)
                "Saved upright photo as captures/${output.name}"
            } catch (_: IOException) {
                output?.delete(); null
            } catch (_: VisionException) {
                output?.delete(); null
            }
        }
    }
}

/** A uniquely named pair; the sidecar's digest binds capture-only metadata to these bytes. */
internal fun writeRawPhotoDiagnostic(photo: PhotoInput, directory: File): File {
    check(BuildConfig.DEBUG)
    val metadata = photoDiagnosticMetadata(photo)
    val extension = when (metadata.optString("mimeType")) {
        "image/jpeg" -> ".jpg"
        "image/png" -> ".png"
        else -> ".bin"
    }
    val image = File.createTempFile("raw-scan-", extension, directory)
    val sidecar = File(directory, image.name + ".json")
    try {
        image.outputStream().use(photo::writeTo)
        val digest = MessageDigest.getInstance("SHA-256")
        photo.openStream().use { input ->
            val chunk = ByteArray(8192)
            while (true) {
                val count = input.read(chunk)
                if (count < 0) break
                digest.update(chunk, 0, count)
            }
        }
        metadata.put("file", image.name)
        metadata.put("sha256", digest.digest().joinToString("") { "%02x".format(it) })
        sidecar.writeText(metadata.toString(2) + "\n")
        return image
    } catch (e: Exception) {
        // Only these newly created files: never leave a misleading half-pair.
        sidecar.delete()
        image.delete()
        throw e
    }
}
