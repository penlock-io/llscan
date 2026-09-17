package com.bitcoinvision.example

import kotlin.math.roundToInt

/** The exact integer image destination and its matching full-image coordinate map. */
internal data class PhotoFit(
    val left: Int, val top: Int, val width: Int, val height: Int, val frame: PhotoFrame,
) {
    fun x(value: Float): Float = left + value * width / frame.width
    fun y(value: Float): Float = top + value * height / frame.height
}

/** Canonical coordinates only: this function never reads or reapplies orientation. */
internal fun fitPhoto(frame: PhotoFrame, width: Float, height: Float): PhotoFit? {
    if (frame.width <= 0 || frame.height <= 0 || !width.isFinite() || !height.isFinite() || width < 1 || height < 1)
        return null
    val scale = minOf(width / frame.width, height / frame.height)
    val drawnWidth = (frame.width * scale).roundToInt().coerceAtLeast(1)
    val drawnHeight = (frame.height * scale).roundToInt().coerceAtLeast(1)
    return PhotoFit(((width - drawnWidth) / 2).roundToInt(), ((height - drawnHeight) / 2).roundToInt(),
        drawnWidth, drawnHeight, frame)
}

internal fun progressMatchesPhoto(progress: ScanProgressState?, photo: ResolvedPhoto): Boolean =
    progress != null && progress.frame == PhotoFrame(photo.width, photo.height) &&
        !progress.overlaysDisabled && !progress.unavailable
