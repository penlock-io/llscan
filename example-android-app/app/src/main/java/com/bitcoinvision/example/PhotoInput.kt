package com.bitcoinvision.example

import java.io.ByteArrayInputStream
import java.io.OutputStream

/** Immutable bounds copied from a view or ImageProxy; no Android object is retained. */
data class CaptureBounds(val left: Int, val top: Int, val right: Int, val bottom: Int)

/** View geometry at the shutter, not when CameraX eventually delivers the image. */
data class CapturePreviewMetadata(
    val bounds: CaptureBounds,
    val scaleType: String,
    val layoutDirection: Int,
)

/** Capture-time facts, not a later reading of the phone's attitude. */
data class CameraPhotoMetadata(
    val rotationDegrees: Int,
    val width: Int,
    val height: Int,
    val format: Int,
    val displayRotationAtShutter: Int?,
    val sensorToDisplayDegreesAtShutter: Int?,
    // Diagnostic only: verify per device that FIT_CENTER delivers a full-raster crop.
    // The preview fits the capture; these fields do not transform the scanned pixels.
    val cropRect: CaptureBounds? = null,
    val previewAtShutter: CapturePreviewMetadata? = null,
) {
    init {
        require(rotationDegrees in listOf(0, 90, 180, 270))
        require(width > 0 && height > 0)
    }
}

/**
 * An immutable encoded photo and its provenance. CameraX's correction
 * cannot be recovered from the bytes alone. No transform is applied here.
 * Neither callers nor readers can mutate the retained diagnostic original.
 */
class PhotoInput private constructor(private val encoded: ByteArray, val camera: CameraPhotoMetadata?) {
    val isEmpty: Boolean get() = encoded.isEmpty()

    fun copyBytes(): ByteArray = encoded.copyOf()
    fun openStream(): ByteArrayInputStream = ByteArrayInputStream(encoded)
    fun writeTo(output: OutputStream) { openStream().use { it.copyTo(output) } }

    companion object {
        fun fromFile(bytes: ByteArray): PhotoInput = PhotoInput(bytes.copyOf(), null)
        fun fromCamera(bytes: ByteArray, metadata: CameraPhotoMetadata): PhotoInput = PhotoInput(bytes.copyOf(), metadata)
        /** Only for the callback's fresh private plane copy: caller relinquishes ownership. */
        internal fun ownCameraBytes(bytes: ByteArray, metadata: CameraPhotoMetadata): PhotoInput = PhotoInput(bytes, metadata)
    }
}

/** A resolved input is reused as-is; consumers never read EXIF or compose another turn. */
class ResolvedPhoto internal constructor(
    val source: PhotoInput,
    val orientation: Int,
    val rawWidth: Int,
    val rawHeight: Int,
    // Never infer this from orientation == 1, or the presence of an orientation argument.
    val photoUp: PhotoUpSource = PhotoUpSource.UNKNOWN,
) {
    init { require(orientation in 1..8 && rawWidth > 0 && rawHeight > 0) }
    val width: Int get() = if (orientation >= 5) rawHeight else rawWidth
    val height: Int get() = if (orientation >= 5) rawWidth else rawHeight
    val isEmpty: Boolean get() = source.isEmpty
}

fun interface PhotoPreparer {
    fun prepare(photo: PhotoInput): ResolvedPhoto
}

class PhotoPreparationException(message: String) : Exception(message)

/** Provenance of canonical photo-up, not proof the paper was photographed upright. */
typealias PhotoUpSource = uniffi.bitcoin_vision_mobile.PhotoUpSource

/** Preserve provenance before missing EXIF collapses to the identity transform. */
internal fun photoUpFromMetadata(camera: CameraPhotoMetadata?, exifOrientation: Int?): PhotoUpSource =
    when {
        camera != null -> PhotoUpSource.CAMERA
        exifOrientation != null && exifOrientation in 1..8 -> PhotoUpSource.EXIF
        else -> PhotoUpSource.UNKNOWN
    }

/** EXIF-numbered transform; camera zero is authoritative, not a missing value. */
internal fun orientationFromMetadata(camera: CameraPhotoMetadata?, exifOrientation: Int?): Int =
    if (camera != null) {
        when (camera.rotationDegrees) {
            0 -> 1
            90 -> 6
            180 -> 3
            270 -> 8
            else -> error("Invalid camera rotation")
        }
    } else {
        exifOrientation?.takeIf { it in 1..8 } ?: 1
    }
