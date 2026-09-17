package com.bitcoinvision.example

import org.junit.Assert.*
import org.junit.Test
import uniffi.bitcoin_vision_mobile.DetectionPart
import uniffi.bitcoin_vision_mobile.DetectionSource
import uniffi.bitcoin_vision_mobile.DetectionUnion

class DetectionReviewTest {
    private val source = DetectionSource(7u, listOf(0f, 0f, 4f, 0f, 4f, 3f, 0f, 3f),
        emptyList(), "no_ink_after_rules", null, "owned_descender", 20f, emptyList(), null, null)

    @Test fun geometry_only_ink_is_not_a_reading_but_a_recovery_is() {
        assertTrue(source.unread)
        assertEquals("Small ink belonging to a nearby word", source.reviewDescription)
        assertFalse(source.copy(recoveredWord = 4u).unread)
        assertFalse(source.copy(parts = listOf(DetectionPart(0u, source.corners, 4u))).unread)
        assertTrue(source.copy(parts = listOf(DetectionPart(0u, source.corners, null))).unread)
        assertEquals("Region that could not be read", source.copy(decision = "too_few_supports").reviewDescription)
    }

    @Test fun both_selected_and_inactive_union_readings_account_for_the_ink() {
        for (selected in listOf(true, false)) {
            val union = DetectionUnion(source.corners, source.corners, 4u, selected,
                if (selected) "selected" else "uncertain")
            assertFalse(source.copy(decision = "detached_ending", union = union).unread)
            assertTrue(source.copy(decision = "detached_ending",
                union = union.copy(wordIndex = null, selected = false, reason = "read_budget")).unread)
        }
    }
}
