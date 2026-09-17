package com.bitcoinvision.example

import uniffi.bitcoin_vision_mobile.QrScan

/** One camera frame's luma plane, rows `rowStride` bytes apart. */
class QrFrame(val width: Int, val height: Int, val rowStride: Int, val luma: ByteArray)

/**
 * One scanning session's whole lifetime under one lock: which frames
 * are admitted, what is delivered, and when the native decoder is
 * released.
 *
 * One frame decodes at a time and the rest are dropped. The first
 * decoded PSBT is the last result delivered. [retire] ends the session
 * at once: nothing admitted afterwards, nothing already queued for
 * delivery reaches [onScan], and a decode under way is finished quietly.
 * [release] runs exactly once, only when no decode is admitted — on the
 * retiring thread if the worker is idle, otherwise on the worker when it
 * finishes — so the decoder is never destroyed between a frame's
 * admission and its native call. A decode that throws counts as a
 * dropped frame.
 */
class QrScanController(
    private val decode: (QrFrame) -> QrScan,
    private val release: () -> Unit,
    private val post: (Runnable) -> Unit,
    private val onScan: (QrScan) -> Unit,
) {
    private val lock = Any()
    private var busy = false
    private var latched = false
    private var closed = false
    private var released = false

    val retired: Boolean
        get() = synchronized(lock) { closed }

    /** True when the frame was taken, false when dropped. */
    fun offer(frame: QrFrame): Boolean {
        synchronized(lock) {
            if (closed || latched || busy) return false
            busy = true
        }
        var scan: QrScan? = null
        try {
            scan = try {
                decode(frame)
            } catch (_: Exception) {
                null
            }
        } finally {
            val releaseNow: Boolean
            synchronized(lock) {
                busy = false
                if (closed) scan = null
                if (scan is QrScan.Decoded) latched = true
                releaseNow = closed && !released
                if (releaseNow) released = true
            }
            if (releaseNow) release()
        }
        val result = scan ?: return true
        post {
            synchronized(lock) { if (closed) return@post }
            onScan(result)
        }
        return true
    }

    fun retire() {
        val releaseNow: Boolean
        synchronized(lock) {
            if (closed) return
            closed = true
            releaseNow = !busy && !released
            if (releaseNow) released = true
        }
        if (releaseNow) release()
    }
}

/** What the viewfinder shows about the stream: the ring and one notice. */
data class QrScanState(
    val fraction: Float = 0f,
    val partsSeen: Int = 0,
    val partsExpected: Int = 0,
    val notice: String? = null,
) {
    fun after(scan: QrScan): QrScanState = when (scan) {
        is QrScan.Progress -> QrScanState(scan.fraction, scan.partsSeen.toInt(), scan.partsExpected.toInt(), null)
        is QrScan.Decoded -> copy(fraction = 1f, notice = null)
        // The stream in progress is untouched by a stray code; only the notice changes.
        is QrScan.Rejected -> copy(notice = scan.reason)
    }
}
