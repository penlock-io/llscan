package com.bitcoinvision.example

import org.junit.Assert.*
import org.junit.Test
import uniffi.bitcoin_vision_mobile.ProgressDeliveryException
import uniffi.bitcoin_vision_mobile.ScanProgressEvent
import uniffi.bitcoin_vision_mobile.ScanProgressUpdate as Update
import uniffi.bitcoin_vision_mobile.ScanRegion
import uniffi.bitcoin_vision_mobile.ScanRegionMapping
import uniffi.bitcoin_vision_mobile.ScanRegionState as State
import uniffi.bitcoin_vision_mobile.ScanWorkPhase as Phase
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class ScanProgressTest {
    private class Stream(token: Int = 1) {
        val handoff = ScanProgressHandoff(token)
        var sequence = 0uL
        fun send(update: Update) { handoff.onProgress(ScanProgressEvent(++sequence, update)) }
        fun state() = handoff.snapshot()!!
        fun photo() = send(Update.Photo(640u, 480u))
        fun region(id: UInt, state: State = State.FOUND) = send(Update.Region(box(id, state)))
    }
    companion object {
        private fun box(id: UInt, state: State = State.READ) =
            ScanRegion(id, listOf(10f, 20f, 100f, 20f, 100f, 40f, 10f, 40f), state)
    }

    @Test fun counts_precede_boxes_and_are_phase_local_not_whole_scan_completion() {
        val s = Stream()
        s.photo()
        s.send(Update.Work(Phase.READING, 0u, 2u))
        assertTrue(s.state().regions.isEmpty())
        assertEquals(0f, s.state().fraction)
        s.region(0u)
        s.region(0u, State.READ)
        s.send(Update.Work(Phase.READING, 1u, 2u))
        assertEquals(0.5f, s.state().fraction)
        assertEquals("1 of 2 candidate regions read", s.state().countLabel)
        s.region(1u, State.READ)
        s.send(Update.Work(Phase.READING, 2u, 2u))
        assertEquals(1f, s.state().fraction)
        assertNull(s.state().outcome)
        s.send(Update.Work(Phase.CHECKING, 0u, null))
        assertNull(s.state().fraction)
        assertEquals("Checking readings", s.state().label)
    }

    @Test fun empty_reading_stage_never_divides_by_zero() {
        val s = Stream()
        s.send(Update.Work(Phase.READING, 0u, 0u))
        assertNull(s.state().fraction)
        assertEquals("No candidate regions to read", s.state().countLabel)
        s.send(Update.Finished(emptyList()))
        assertEquals(ProgressOutcome.Finished, s.state().outcome)
    }

    @Test fun accumulated_stream_preserves_replacements_removals_trims_and_terminal_mapping() {
        val s = Stream()
        s.photo()
        s.region(0u); s.region(1u); s.region(2u)
        s.send(Update.Replaced(box(3u), listOf(0u, 1u)))
        s.send(Update.Removed(2u))
        val narrowed = box(3u).copy(corners = listOf(30f, 20f, 100f, 20f, 100f, 40f, 30f, 40f))
        s.send(Update.Region(narrowed))
        s.send(Update.Finished(listOf(ScanRegionMapping(0u, 0u), ScanRegionMapping(3u, 1u), ScanRegionMapping(1u, 2u))))
        // Validation sees the full ordered stream even before visual playback starts.
        val state = s.state()
        assertEquals(setOf(3u), state.regions.keys)
        assertEquals(narrowed.corners, state.regions[3u]!!.corners)
        assertEquals(mapOf(0u to 0u, 3u to 1u, 1u to 2u), state.finalIndices)
        assertEquals(setOf(0u, 1u, 2u), state.retired)
        s.send(Update.Region(box(0u)))
        assertSame(state, s.state())
    }

    @Test fun generated_mutable_records_cannot_change_a_published_snapshot() {
        val s = Stream(); s.photo()
        val points = box(0u).corners.toMutableList()
        val region = ScanRegion(0u, points, State.FOUND)
        s.send(Update.Region(region))
        points[0] = 500f
        region.state = State.READ
        assertEquals(10f, s.state().regions[0u]!!.corners[0])
        assertEquals(State.FOUND, s.state().regions[0u]!!.state)
    }

    @Test fun overflow_bounds_ids_even_when_removed_and_keeps_counts_and_terminal() {
        val s = Stream(); s.photo()
        s.send(Update.Work(Phase.READING, 0u, 600u))
        repeat(MAX_PROGRESS_REGIONS + 1) { id -> s.region(id.toUInt()); s.send(Update.Removed(id.toUInt())) }
        assertTrue(s.state().overlaysDisabled)
        assertFalse(s.state().unavailable)
        assertTrue(s.state().regions.isEmpty() && s.state().seen.isEmpty() && s.state().retired.isEmpty())
        s.send(Update.Work(Phase.READING, 300u, 600u))
        assertEquals(0.5f, s.state().fraction)
        s.send(Update.Finished((0u..599u).map { ScanRegionMapping(it, it) }))
        assertEquals(ProgressOutcome.Finished, s.state().outcome)
        assertTrue(s.state().finalIndices.isEmpty())
    }

    @Test fun a_failed_attempt_clears_boxes_and_cannot_be_resurrected() {
        val s = Stream(); s.photo(); s.region(0u)
        s.send(Update.Failed)
        val failed = s.state()
        assertEquals(ProgressOutcome.Failed, failed.outcome)
        assertTrue(failed.regions.isEmpty())
        s.send(Update.Finished(emptyList()))
        assertSame(failed, s.state())
    }

    @Test fun detach_rejects_late_events_and_a_new_attempt_has_its_own_sequence() {
        val old = Stream(7); old.photo(); old.region(0u)
        old.handoff.detach()
        val next = Stream(9); next.send(Update.Work(Phase.FINDING, 0u, null))
        try { old.send(Update.Region(box(0u))); fail("detached callback accepted") }
        catch (_: ProgressDeliveryException.Detached) { }
        assertNull(old.handoff.snapshot())
        assertEquals(9, next.state().token)
        assertEquals(1uL, next.state().sequence)
        assertTrue(next.state().regions.isEmpty())
    }

    @Test fun detach_wins_against_an_in_flight_producer() {
        val s = Stream(); s.photo()
        val entered = CountDownLatch(1)
        val worker = Executors.newSingleThreadExecutor()
        try {
            val task = worker.submit {
                entered.countDown()
                try { repeat(2_000) { s.region(0u, State.READING) } }
                catch (_: ProgressDeliveryException.Detached) { }
            }
            assertTrue(entered.await(2, TimeUnit.SECONDS))
            s.handoff.detach()
            task.get(5, TimeUnit.SECONDS)
            assertNull(s.handoff.snapshot())
        } finally { worker.shutdownNow() }
    }

    @Test fun broken_sequence_or_count_disables_inconsistent_progress_but_not_terminal() {
        val s = Stream(); s.photo(); s.region(0u)
        s.handoff.onProgress(ScanProgressEvent(50u, Update.Region(box(1u))))
        assertTrue(s.state().unavailable)
        assertTrue(s.state().regions.isEmpty())
        s.handoff.onProgress(ScanProgressEvent(52u, Update.Finished(emptyList())))
        assertEquals(ProgressOutcome.Finished, s.state().outcome)
        val changing = Stream()
        changing.send(Update.Work(Phase.READING, 1u, 2u))
        changing.send(Update.Work(Phase.READING, 1u, 3u))
        assertTrue(changing.state().unavailable)
        assertNull(changing.state().fraction)
    }

    @Test fun geometry_revisions_do_not_reset_a_started_reading_counter() {
        val s = Stream(); s.photo()
        s.send(Update.Work(Phase.GEOMETRY, 0u, null)); s.region(0u); s.region(1u)
        s.send(Update.Removed(1u))
        s.send(Update.Work(Phase.GEOMETRY, 0u, null)); s.region(0u)
        s.send(Update.Work(Phase.READING, 0u, 1u))
        s.region(0u, State.READ); s.send(Update.Work(Phase.READING, 1u, 1u))
        assertFalse(s.state().unavailable)
        assertEquals(setOf(0u), s.state().regions.keys)
    }

    @Test fun fast_native_completion_must_replay_every_stage_and_box_before_review() {
        val s = Stream(); s.photo()
        for (phase in Phase.entries) {
            s.send(Update.Work(phase, 0u, if (phase == Phase.READING) 1u else null))
            if (phase == Phase.GEOMETRY) s.region(0u, State.FOUND)
            if (phase == Phase.READING) {
                s.region(0u, State.READING); s.region(0u, State.READ)
                s.send(Update.Work(phase, 1u, 1u))
            }
        }
        s.send(Update.Finished(listOf(ScanRegionMapping(0u, 0u))))
        s.handoff.sealSuccess()
        assertEquals(0uL, s.handoff.nextFrame(10_000)!!.sequence) // No draw, no advancement.
        assertFalse(s.handoff.playbackComplete(10_000))
        var now = 0L
        val phases = mutableSetOf<Phase>()
        val states = mutableSetOf<State>()
        var visible = s.handoff.nextFrame(now)!!
        while (visible.outcome == null) {
            phases += visible.phase
            visible.regions[0u]?.let { states += it.state }
            s.handoff.presented(visible.sequence, now)
            now += PROGRESS_STAGE_MS
            visible = s.handoff.nextFrame(now)!!
        }
        assertEquals(Phase.entries.toSet(), phases)
        assertEquals(setOf(State.FOUND, State.READING, State.READ), states)
        assertFalse(s.handoff.playbackComplete(now)) // Finished must itself be drawn and held.
        s.handoff.presented(visible.sequence, now)
        assertFalse(s.handoff.playbackComplete(now + PROGRESS_STAGE_MS - 1))
        assertTrue(s.handoff.playbackComplete(now + PROGRESS_STAGE_MS))
        s.handoff.detach()
        assertNull(s.handoff.nextFrame(now))
    }

    @Test fun a_silent_or_incomplete_reader_cannot_open_review() {
        for (s in listOf(Stream(), Stream().also { it.photo(); it.send(Update.Finished(emptyList())) })) {
            try { s.handoff.sealSuccess(); fail("missing stages accepted") }
            catch (_: ScanProgressException) { }
        }
    }

    @Test fun estimated_bar_moves_during_opaque_work_and_never_resets_or_finishes_early() {
        val meter = ScanProgressMeter(0)
        var previous = meter.advance(null, 0)
        assertTrue(previous > 0f)
        for (phase in Phase.entries) {
            val state = ScanProgressState(1, phase = phase)
            val start = phase.ordinal * 30_000L
            val first = meter.advance(state, start)
            val later = meter.advance(state, start + 20_000)
            assertTrue(first >= previous)
            assertTrue(later > first && later < 1f)
            previous = later
        }
        assertEquals(1f, meter.advance(ScanProgressState(1, outcome = ProgressOutcome.Finished), 200_000))
    }
}
