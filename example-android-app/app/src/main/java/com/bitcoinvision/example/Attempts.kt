package com.bitcoinvision.example

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** One photograph's journey from the shutter to its result. */
sealed interface Attempt<out R> {
    val token: Int

    /** The shutter was pressed; the camera has not delivered the photo yet. */
    data class Capturing(override val token: Int) : Attempt<Nothing>

    /** Raw bytes are in hand, but their orientation is not yet resolved. */
    data class Preparing(override val token: Int, val photo: PhotoInput) : Attempt<Nothing>

    /** Only resolved photos can be displayed or handed to a reader. */
    data class Processing(override val token: Int, val photo: ResolvedPhoto) : Attempt<Nothing>

    data class Done<R>(override val token: Int, val photo: ResolvedPhoto, val result: R) : Attempt<R>

    /** The read failed, or the camera delivered nothing, in which case there is no photo. */
    data class Failed(
        override val token: Int, val photo: ResolvedPhoto?, val message: String,
        val original: PhotoInput? = photo?.source,
    ) : Attempt<Nothing>
}

/**
 * The attempts a scanner makes, one current at a time, owned by a
 * view model so an attempt outlives the camera on screen. Every step
 * names the attempt it belongs to, and only the current attempt may
 * move: what a cancelled or replaced attempt delivers later is dropped.
 */
class Attempts<R> {
    var current by mutableStateOf<Attempt<R>?>(null)
        private set
    private var issued = 0

    /** Starts a new attempt at the shutter, replacing any other, and names it. */
    fun begin(): Int {
        issued += 1
        current = Attempt.Capturing(issued)
        return issued
    }

    /**
     * The camera's photo for the attempt [token]. True when the read
     * may start; false when the attempt is no longer current, or the
     * camera delivered nothing, which fails it.
     */
    fun deliver(token: Int, photo: ByteArray?): Boolean {
        if (current?.token != token) return false
        return deliverPhoto(token, photo?.let(PhotoInput::fromFile))
    }

    fun deliverPhoto(token: Int, photo: PhotoInput?): Boolean {
        if (current !is Attempt.Capturing || current?.token != token) return false
        if (photo == null || photo.isEmpty) return missing(token, "The camera gave no photo. Try again.")
        current = Attempt.Preparing(token, photo)
        return true
    }

    /**
     * No photo ever arrived for [token], and why. Always false: there
     * is nothing to read. A photo can fail to arrive for reasons the
     * camera has no words for — a file the phone would not open — so
     * the reason is the caller's to give.
     */
    fun missing(token: Int, message: String): Boolean {
        if (current !is Attempt.Capturing || current?.token != token) return false
        current = Attempt.Failed(token, null, message)
        return false
    }

    fun prepared(token: Int, photo: ResolvedPhoto): Boolean {
        val preparing = current as? Attempt.Preparing ?: return false
        if (preparing.token != token || preparing.photo !== photo.source) return false
        current = Attempt.Processing(token, photo)
        return true
    }

    /** The result of the attempt [token]; false, and nothing changes, when it is no longer current. */
    fun finish(token: Int, result: R): Boolean {
        val processing = current as? Attempt.Processing ?: return false
        if (processing.token != token) return false
        current = Attempt.Done(token, processing.photo, result)
        return true
    }

    fun fail(token: Int, message: String): Boolean {
        val attempt = current ?: return false
        if (attempt.token != token) return false
        current = when (attempt) {
            is Attempt.Preparing -> Attempt.Failed(token, null, message, attempt.photo)
            is Attempt.Processing -> Attempt.Failed(token, attempt.photo, message)
            else -> return false
        }
        return true
    }

    /** Drops the current attempt; anything it still delivers is dropped too. */
    fun cancel() {
        issued += 1
        current = null
    }

    /** The photo of the current attempt, once the camera delivered it. */
    val photo: PhotoInput?
        get() = when (val attempt = current) {
            is Attempt.Preparing -> attempt.photo
            is Attempt.Processing -> attempt.photo.source
            is Attempt.Done -> attempt.photo.source
            is Attempt.Failed -> attempt.original
            else -> null
        }

    val resolvedPhoto: ResolvedPhoto?
        get() = when (val attempt = current) {
            is Attempt.Processing -> attempt.photo
            is Attempt.Done -> attempt.photo
            is Attempt.Failed -> attempt.photo
            else -> null
        }
}
