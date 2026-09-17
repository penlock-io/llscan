package com.bitcoinvision.example

import uniffi.bitcoin_vision_mobile.WordReading
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.max

/** The scanner's existing verdict, unless the person has replaced that read. */
val Entry.needsAttention: Boolean
    get() = source?.accepted == false && !corrected

/** Support for this displayed word, never the probability of a different top pick. */
val Entry.modelConfidence: Float?
    get() = source?.ranked?.firstOrNull { it.first == word }?.second
        ?.takeIf { it.isFinite() && it in 0f..1f }

private val numberLabel = Regex("([0-9]{1,2})[.)]")

/** Only suppress already-excluded, unambiguous standalone labels, never word prefixes. */
internal val Entry.isObviousNumberLabel: Boolean
    get() {
        val s = source ?: return false
        if (!s.stray || corrected || s.labelConflict || s.labels.any { it.ambiguous }) return false
        val ordinal = numberLabel.matchEntire(s.raw.trim())?.groupValues?.get(1)?.toIntOrNull()
            ?: return false
        return ordinal in 1..24
    }

internal val PhraseDraft.reviewLeftOut: List<Entry>
    get() = leftOut.filterNot { it.isObviousNumberLabel && !isAlternative(it.id) }

internal data class ReviewPhotoBounds(val left: Int, val top: Int, val width: Int, val height: Int)

/** Display-only crop in canonical photo coordinates. Never changes recognition input. */
internal fun reviewPhotoBounds(frame: PhotoFrame, quads: List<List<Float>>): ReviewPhotoBounds {
    val width = frame.width.coerceAtLeast(1)
    val height = frame.height.coerceAtLeast(1)
    val full = ReviewPhotoBounds(0, 0, width, height)
    val valid = quads.filter { it.size == 8 && it.all(Float::isFinite) }
    if (valid.isEmpty()) return full
    val xs = valid.flatMap { q -> listOf(q[0], q[2], q[4], q[6]) }
    val ys = valid.flatMap { q -> listOf(q[1], q[3], q[5], q[7]) }
    val minX = xs.min().coerceIn(0f, width.toFloat())
    val maxX = xs.max().coerceIn(0f, width.toFloat())
    val minY = ys.min().coerceIn(0f, height.toFloat())
    val maxY = ys.max().coerceIn(0f, height.toFloat())
    if (maxX <= minX || maxY <= minY) return full
    val margin = max(4f, max(maxX - minX, maxY - minY) * .04f)
    val left = floor(minX - margin).toInt().coerceIn(0, width - 1)
    val top = floor(minY - margin).toInt().coerceIn(0, height - 1)
    val right = ceil(maxX + margin).toInt().coerceIn(left + 1, width)
    val bottom = ceil(maxY + margin).toInt().coerceIn(top + 1, height)
    return ReviewPhotoBounds(left, top, right - left, bottom - top)
}

internal fun WordReading.reviewSource() = Source(
    corners, ranked.map { it.word to it.probability }, accepted, cropPng,
    read = read, raw = raw, stray = stray, number = number?.toInt(), apart = apart,
    joinedFrom = joinedFrom.map { it.toInt() },
    expandedFrom = expandedFrom.map { ExpansionParent(it.wordIndex.toInt(), it.stray) },
    originalOcr = originalOcr?.let { OriginalOcr(it.corners, it.cropPng, it.raw) },
    matching = matching?.let { MatchingText(it.text, it.original, it.removedPrefixBytes.toInt()) },
    labels = labels.map { ObservedLabel(it.literal, it.ordinal?.toInt(), it.origin, it.ambiguous, it.corners) },
    labelConflict = labelConflict,
)
