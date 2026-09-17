package com.bitcoinvision.example

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import uniffi.bitcoin_vision_mobile.ProgressDeliveryException
import uniffi.bitcoin_vision_mobile.ScanProgressEvent
import uniffi.bitcoin_vision_mobile.ScanProgressObserver
import uniffi.bitcoin_vision_mobile.ScanProgressUpdate as Update
import uniffi.bitcoin_vision_mobile.ScanRegion
import uniffi.bitcoin_vision_mobile.ScanRegionState as RegionState
import uniffi.bitcoin_vision_mobile.ScanWorkPhase as Phase

internal const val MAX_PROGRESS_REGIONS = 512
internal const val PROGRESS_FRAME_MS = 16L
internal const val PROGRESS_STAGE_MS = 240L
internal const val MAX_PROGRESS_EVENTS = 8192
internal fun progressClockMs() = System.nanoTime() / 1_000_000L
internal class ScanProgressException : Exception("Scan updates were interrupted. Please try again.")

data class PhotoFrame(val width: Int, val height: Int)
data class ProgressRegion(val id: UInt, val corners: List<Float>, val state: RegionState)
enum class ProgressOutcome { Finished, Failed }

/** Immutable snapshots only; no generated mutable records, images, words or UI references. */
data class ScanProgressState(
    val token: Int,
    val sequence: ULong = 0u,
    val phase: Phase = Phase.PREPARING,
    val completed: UInt = 0u,
    val total: UInt? = null,
    val frame: PhotoFrame? = null,
    val regions: Map<UInt, ProgressRegion> = emptyMap(),
    val seen: Set<UInt> = emptySet(),
    val retired: Set<UInt> = emptySet(),
    val finalIndices: Map<UInt, UInt> = emptyMap(),
    val overlaysDisabled: Boolean = false,
    val unavailable: Boolean = false,
    val outcome: ProgressOutcome? = null,
    val stages: Set<Phase> = emptySet(),
) {
    val fraction: Float? get() = if (!unavailable && outcome == null && total != null && total > 0u)
        completed.toFloat() / total.toFloat() else null

    val label: String get() = when {
        unavailable -> "Reading your photo"
        outcome == ProgressOutcome.Failed -> "The photo could not be read"
        outcome != null -> "Preparing review"
        phase == Phase.PREPARING -> "Preparing to read"
        phase == Phase.FINDING -> "Finding words"
        phase == Phase.GEOMETRY -> "Fitting columns and boxes"
        phase == Phase.READING -> "Reading words"
        phase == Phase.CHECKING -> "Checking readings"
        else -> "Preparing review"
    }

    val countLabel: String? get() = if (!unavailable && outcome == null && phase == Phase.READING && total != null)
        if (total == 0u) "No candidate regions to read" else "$completed of $total candidate regions read"
    else null

    private fun withoutOverlays() = copy(
        overlaysDisabled = true, regions = emptyMap(), seen = emptySet(), retired = emptySet(), finalIndices = emptyMap(),
    )

    private fun malformed() = withoutOverlays().copy(unavailable = true, total = null)

    /** Validate every native delta; the UI replays the same ordered stream. */
    internal fun reduce(event: ScanProgressEvent): ScanProgressState {
        if (outcome != null) return this // Terminal state cannot be overwritten by a late delta.
        val next = copy(sequence = event.sequence)
        val update = event.update
        // Even fallback observation consumes terminal outcomes, but never navigates.
        if (update is Update.Failed) return next.withoutOverlays().copy(outcome = ProgressOutcome.Failed)
        if (event.sequence != sequence + 1uL) return next.malformed().copy(
            outcome = if (update is Update.Finished) ProgressOutcome.Finished else null,
        )
        if (update is Update.Finished) {
            if (overlaysDisabled || update.regions.size > MAX_PROGRESS_REGIONS) {
                return next.withoutOverlays().copy(outcome = ProgressOutcome.Finished)
            }
            val mapping = update.regions.associate { it.id to it.wordIndex }
            if (mapping.size != update.regions.size || mapping.values.toSet().size != mapping.size ||
                mapping.values.any { it >= mapping.size.toUInt() } || mapping.keys.any { it !in seen }) {
                return next.malformed().copy(outcome = ProgressOutcome.Finished)
            }
            return next.copy(outcome = ProgressOutcome.Finished, finalIndices = mapping,
                regions = regions.filterKeys { it in mapping && it !in retired })
        }
        if (unavailable) return next
        return when (update) {
            is Update.Photo -> {
                val width = update.width.toInt()
                val height = update.height.toInt()
                if (frame != null || width <= 0 || height <= 0) next.malformed()
                else next.copy(frame = PhotoFrame(width, height))
            }
            is Update.Work -> {
                if (update.phase.ordinal < phase.ordinal ||
                    (update.total != null && update.completed > update.total!!) ||
                    (update.phase == phase && sequence != 0uL &&
                        (update.total != total || update.completed < completed))) next.malformed()
                else next.copy(phase = update.phase, completed = update.completed, total = update.total,
                    stages = stages + update.phase)
            }
            is Update.Region -> {
                if (overlaysDisabled) next else next.upsert(update.region)
            }
            is Update.Replaced -> {
                if (overlaysDisabled) next
                else if (update.parents.isEmpty() || update.parents.size > MAX_PROGRESS_REGIONS ||
                    update.parents.any { it !in regions || it in retired } ||
                    update.parents.toSet().size != update.parents.size || update.region.id in seen) next.malformed()
                else next.copy(regions = regions - update.parents.toSet(), retired = retired + update.parents)
                    .upsert(update.region)
            }
            is Update.Removed -> {
                if (overlaysDisabled) next
                else if (update.id !in seen) next.malformed()
                else next.copy(regions = regions - update.id, retired = retired + update.id)
            }
            else -> next // Both terminals were handled above.
        }
    }

    private fun upsert(region: ScanRegion): ScanProgressState {
        if (frame == null || region.corners.size != 8 || region.corners.any { !it.isFinite() } || region.id in retired)
            return malformed()
        if (region.id !in seen && seen.size == MAX_PROGRESS_REGIONS) return withoutOverlays()
        return copy(
            seen = seen + region.id,
            regions = regions + (region.id to ProgressRegion(region.id, region.corners.toList(), region.state)),
        )
    }
}

/** Bounded lossless deltas, not a latest-only mailbox. Native never waits on drawing.
 * Every displayed snapshot needs a draw acknowledgement before the next one.
 * Cancellation clears the queue; malformed/overflowing streams cannot enter review.
 */
internal class ScanProgressHandoff(val token: Int) : ScanProgressObserver {
    private var latest: ScanProgressState? = ScanProgressState(token)
    private var visible: ScanProgressState? = latest
    private val pending = ArrayDeque<ScanProgressEvent>()
    private var presentedAt: Long? = null
    private var dwellMs = PROGRESS_STAGE_MS
    private var sealed = false
    private var overflow = false

    @Synchronized fun snapshot(): ScanProgressState? = latest
    @Synchronized fun detach() {
        latest = null; visible = null; pending.clear(); presentedAt = null
    }

    @Synchronized fun presented(sequence: ULong, nowMs: Long = progressClockMs()) {
        if (visible?.sequence == sequence && presentedAt == null) presentedAt = nowMs
    }

    @Synchronized fun nextFrame(nowMs: Long = progressClockMs()): ScanProgressState? {
        val current = visible ?: return null
        val shown = presentedAt ?: return current
        if (nowMs - shown < dwellMs || pending.isEmpty()) return current
        val next = current.reduce(pending.removeFirst())
        dwellMs = if (next.phase != current.phase || next.outcome != null) PROGRESS_STAGE_MS else PROGRESS_FRAME_MS
        presentedAt = null
        visible = next
        return next
    }

    @Synchronized fun sealSuccess() {
        val state = latest ?: throw CancellationException("Scan detached")
        if (overflow || state.unavailable || state.overlaysDisabled || state.frame == null ||
            state.outcome != ProgressOutcome.Finished || state.stages != Phase.entries.toSet()) {
            throw ScanProgressException()
        }
        sealed = true
    }

    @Synchronized fun playbackComplete(nowMs: Long = progressClockMs()): Boolean =
        sealed && pending.isEmpty() && visible?.outcome == ProgressOutcome.Finished &&
            presentedAt?.let { nowMs - it >= dwellMs } == true

    suspend fun awaitPlayback() {
        sealSuccess()
        while (!playbackComplete()) {
            if (snapshot() == null) throw CancellationException("Scan detached")
            delay(PROGRESS_FRAME_MS)
        }
    }

    @Synchronized
    override fun onProgress(event: ScanProgressEvent) {
        val old = latest ?: throw ProgressDeliveryException.Detached()
        if (old.outcome != null) return
        if (pending.size == MAX_PROGRESS_EVENTS) {
            overflow = true
            throw ProgressDeliveryException.Failed()
        }
        // UniFFI records/lists are mutable. Never retain objects owned by the caller.
        fun region(r: ScanRegion) = r.copy(corners = r.corners.toList())
        val update = when (val u = event.update) {
            is Update.Photo -> u.copy()
            is Update.Work -> u.copy()
            is Update.Region -> Update.Region(region(u.region))
            is Update.Replaced -> Update.Replaced(region(u.region), u.parents.toList())
            is Update.Removed -> u.copy()
            is Update.Finished -> Update.Finished(u.regions.map { it.copy() })
            is Update.Failed -> Update.Failed
        }
        val frozen = ScanProgressEvent(event.sequence, update)
        latest = old.reduce(frozen)
        pending.addLast(frozen)
    }
}

/** A monotone display estimate, never reported as native work counts or model confidence. */
internal class ScanProgressMeter(startMs: Long) {
    private var phase = Phase.PREPARING
    private var phaseAt = startMs
    private var value = 0.01f
    fun advance(state: ScanProgressState?, nowMs: Long): Float {
        val next = state?.phase ?: Phase.PREPARING
        if (next != phase) { phase = next; phaseAt = nowMs }
        val bounds = floatArrayOf(0.01f, 0.10f, 0.22f, 0.42f, 0.80f, 0.94f, 0.99f)
        val start = bounds[phase.ordinal]
        val end = bounds[phase.ordinal + 1]
        val elapsed = (nowMs - phaseAt).coerceAtLeast(0L)
        val estimate = (1.0 - kotlin.math.exp(-elapsed / 10_000.0)).toFloat()
        val within = maxOf(state?.fraction ?: 0f, estimate * 0.95f)
        value = maxOf(value, start + (end - start) * within)
        if (state?.outcome == ProgressOutcome.Finished) value = 1f
        return value
    }
}
