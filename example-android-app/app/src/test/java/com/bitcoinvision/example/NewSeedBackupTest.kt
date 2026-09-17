package com.bitcoinvision.example

import org.junit.Assert.*
import org.junit.Test
import uniffi.bitcoin_vision_mobile.*

class NewSeedBackupTest {
    // Public, non-secret BIP39 test vectors; never funds.
    private val words = List(11) { "abandon" } + "about"
    private val other = "legal winner thank year wave sausage worth useful legal winner thank yellow".split(" ")
    private fun photo() = PhotoInput.fromCamera(byteArrayOf(1),
        CameraPhotoMetadata(0, 10, 10, 256, 0, 0))
    private fun reading(words: List<String>, review: Boolean = false) = BackupReading(
        mapOf(Order.COLUMNS to words, Order.ROWS to words.reversed()), Order.COLUMNS, review)
    private fun processing(flow: NewSeedBackup): Int {
        flow.retake()
        val token = flow.begin()
        val input = photo()
        assertTrue(flow.deliver(token, input))
        assertTrue(flow.prepared(token, ResolvedPhoto(input, 1, 10, 10)))
        return token
    }

    @Test fun exact_current_camera_read_is_reviewable_and_completion_is_one_use() {
        val flow = NewSeedBackup()
        flow.start(words)
        assertNull(flow.consumeVerified())
        assertEquals(-1, flow.begin())
        val token = processing(flow)
        assertNull(flow.consumeVerified())
        assertTrue(flow.finish(token, reading(words)))
        assertEquals(NewSeedStage.Verified, flow.stage)
        assertNotNull(flow.attempt) // retained for word/crop review until completion
        assertEquals(words, flow.consumeVerified())
        assertNull(flow.consumeVerified())
        assertTrue(flow.words.isEmpty())
        assertNull(flow.attempt)
    }

    @Test fun wrong_valid_phrase_missing_extra_and_transposed_are_not_accepted() {
        for (read in listOf(other, words.dropLast(1), words + "about", words.reversed())) {
            val flow = NewSeedBackup()
            flow.start(words)
            flow.finish(processing(flow), reading(read))
            assertEquals(NewSeedStage.Review, flow.stage)
            assertNull(flow.consumeVerified())
            assertEquals(words, flow.words)
        }
    }

    @Test fun each_misread_requires_acknowledgement_and_never_changes_the_seed_or_scan() {
        val read = words.toMutableList().apply { this[0] = "ability"; this[11] = "above" }
        val flow = NewSeedBackup()
        flow.start(words)
        flow.finish(processing(flow), reading(read))
        flow.acknowledge(-1)
        flow.acknowledge(12)
        flow.acknowledge(1) // matching words do not need acknowledgement
        assertTrue(flow.acknowledged.isEmpty())
        flow.acknowledge(0)
        assertNull(flow.consumeVerified())
        flow.acknowledge(11)
        assertEquals(NewSeedStage.Verified, flow.stage)
        assertEquals(read, flow.readWords)
        assertEquals(words, flow.consumeVerified())
        assertNull(flow.consumeVerified())
    }

    @Test fun acknowledgement_does_not_bypass_missing_extra_order_or_failed_capture() {
        for (read in listOf(words.dropLast(1), words + "about")) {
            val flow = NewSeedBackup()
            flow.start(words)
            flow.finish(processing(flow), reading(read))
            (0..12).forEach(flow::acknowledge)
            assertNull(flow.consumeVerified())
        }
        val flow = NewSeedBackup()
        flow.start(words)
        val read = words.toMutableList().apply { this[0] = "ability" }
        flow.finish(processing(flow), reading(read, review = true))
        flow.acknowledge(0)
        assertTrue(flow.acknowledged.isEmpty())
        flow.chooseOrder(Order.COLUMNS)
        flow.acknowledge(0)
        assertEquals(NewSeedStage.Verified, flow.stage)
        flow.chooseOrder(Order.ROWS)
        assertTrue(flow.acknowledged.isEmpty())
        assertNull(flow.consumeVerified())
        flow.retake()
        flow.acknowledge(0)
        assertNull(flow.consumeVerified())
        assertTrue(flow.acknowledged.isEmpty())
    }

    @Test fun order_is_not_target_searched_and_an_explicit_transpose_is_allowed() {
        val flow = NewSeedBackup()
        flow.start(words)
        flow.finish(processing(flow), reading(words.reversed()))
        assertEquals(NewSeedStage.Review, flow.stage)
        flow.chooseOrder(Order.NUMBERS) // not supplied by this scan
        assertNull(flow.consumeVerified())
        flow.chooseOrder(Order.ROWS)
        assertEquals(words, flow.consumeVerified())
    }

    @Test fun order_review_requires_an_explicit_choice_even_when_initial_read_matches() {
        val flow = NewSeedBackup()
        flow.start(words)
        flow.finish(processing(flow), reading(words, review = true))
        assertNull(flow.consumeVerified())
        flow.chooseOrder(Order.COLUMNS)
        assertEquals(words, flow.consumeVerified())
    }

    @Test fun gallery_data_missing_photos_and_completion_before_preparation_fail_closed() {
        for (input in listOf(null, PhotoInput.fromFile(byteArrayOf(1)))) {
            val flow = NewSeedBackup()
            flow.start(words)
            flow.retake()
            val token = flow.begin()
            assertFalse(flow.finish(token, reading(words)))
            assertFalse(flow.deliver(token, input))
            assertNull(flow.consumeVerified())
            assertEquals(NewSeedStage.Review, flow.stage)
        }
    }

    @Test fun retake_cancel_and_new_seed_reject_every_late_result() {
        val flow = NewSeedBackup()
        flow.start(words)
        val old = processing(flow)
        flow.showWords()
        assertEquals(words, flow.words)
        assertFalse(flow.finish(old, reading(words)))
        val next = processing(flow)
        assertFalse(flow.finish(old, reading(words)))
        flow.cancel()
        assertFalse(flow.finish(next, reading(words)))
        assertTrue(flow.words.isEmpty())
        flow.start(other)
        assertFalse(flow.finish(next, reading(other)))
        assertNull(flow.consumeVerified())
        assertEquals(NewSeedStage.Idle, NewSeedBackup().stage) // no process-death resume
    }

    @Test fun failed_attempt_retries_without_replacing_the_seed() {
        val flow = NewSeedBackup()
        flow.start(words)
        flow.fail(processing(flow))
        assertEquals(NewSeedStage.Review, flow.stage)
        assertEquals(words, flow.words)
        flow.finish(processing(flow), reading(words))
        assertEquals(words, flow.consumeVerified())
    }

    @Test fun native_selection_is_used_not_confidence_checks_or_edited_drafts() {
        val readings = words.map { word -> WordReading(
            corners = List(8) { 0f }, ranked = emptyList(), accepted = false,
            read = word, raw = "uncertain independent OCR", originalOcr = null,
            matching = null, labels = emptyList(), labelConflict = false, stray = false,
            number = null, apart = false, cropPng = byteArrayOf(), joinedFrom = emptyList(), expandedFrom = emptyList(),
        ) }
        val indices = readings.indices.map { it.toUInt() }
        val scan = PhraseScan(false, Layout.PAGE, 100u, 200u, readings, indices, indices.reversed(),
            false, false, Numbering.None, indices, false, InitialOrder.COLUMNS, false, "longer_dimension", emptyList())
        val flow = NewSeedBackup()
        flow.start(words)
        flow.finish(processing(flow), BackupReading.from(scan))
        assertEquals(words, flow.consumeVerified())
        assertTrue(scan.words.none { it.accepted }) // no confidence promotion
    }
}
