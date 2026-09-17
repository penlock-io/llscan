package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WordReviewTest {
    @Test fun standalone_labels_are_not_unresolved_text_but_ambiguous_or_prefixed_words_remain() {
        fun leftover(raw: String, conflict: Boolean = false, ambiguous: Boolean = false, stray: Boolean = true) = Entry(
            0, "eight", Source(List(8) { 0f }, listOf("eight" to .1f), false, ByteArray(0), "eight",
                raw = raw, stray = stray, labelConflict = conflict,
                labels = if (ambiguous) listOf(ObservedLabel(raw, 8, "standalone", true)) else emptyList()), false)
        for (raw in listOf("8.", " 12. ", "24)")) assertTrue(leftover(raw).isObviousNumberLabel)
        for (raw in listOf("8. iron", "eight", "q", "8", "25.", "0.", "."))
            assertFalse(leftover(raw).isObviousNumberLabel)
        assertFalse(leftover("8.", conflict = true).isObviousNumberLabel)
        assertFalse(leftover("8.", ambiguous = true).isObviousNumberLabel)
        assertFalse(leftover("8.", stray = false).isObviousNumberLabel)
        assertFalse(leftover("8.").copy(word = "iron").isObviousNumberLabel)
        val sources = listOf(leftover("8.").source!!, leftover("something").source!!)
        val draft = PhraseDraft(sources, listOf(0, 1), listOf(0, 1), Order.COLUMNS)
        assertEquals(2, draft.leftOut.size) // diagnostic evidence is not removed
        assertEquals(listOf(1), draft.reviewLeftOut.map { it.id })
    }

    @Test fun preview_bounds_include_every_word_with_margin_and_clamp_to_photo() {
        fun quad(l: Float, t: Float, r: Float, b: Float) = listOf(l, t, r, t, r, b, l, b)
        val frame = PhotoFrame(1000, 800)
        assertEquals(ReviewPhotoBounds(184, 84, 432, 432),
            reviewPhotoBounds(frame, listOf(quad(200f, 100f, 400f, 200f), quad(450f, 400f, 600f, 500f))))
        assertEquals(ReviewPhotoBounds(0, 0, 1000, 800), reviewPhotoBounds(frame, emptyList()))
        assertEquals(ReviewPhotoBounds(0, 0, 1000, 800), reviewPhotoBounds(frame, listOf(List(8) { Float.NaN })))
        val edge = reviewPhotoBounds(frame, listOf(quad(-10f, -20f, 995f, 810f)))
        assertEquals(ReviewPhotoBounds(0, 0, 1000, 800), edge)
    }
    @Test fun confidence_belongs_to_displayed_word_and_unknown_is_not_zero_or_top_probability() {
        val source = Source(List(8) { 0f }, listOf("cat" to .8f, "dog" to .2f),
            false, ByteArray(0), "dog")
        val word = Entry(0, "dog", source, false)
        assertEquals(.2f, word.modelConfidence)
        assertEquals(.8f, word.copy(word = "cat").modelConfidence)
        assertNull(word.copy(word = "zoo").modelConfidence)
        assertNull(word.copy(source = null).modelConfidence)
        val invalid = Source(List(8) { 0f }, listOf("dog" to Float.NaN), false, ByteArray(0), "dog")
        assertNull(word.copy(source = invalid).modelConfidence)
    }
    private fun entry(id: Int, accepted: Boolean, read: String = "cat") = Entry(
        id, read,
        Source(List(8) { 0f }, listOf("cat" to 0.8f), accepted, ByteArray(0), read),
        checked = false,
    )

    @Test
    fun attention_uses_the_existing_scanner_verdict_and_correction_state() {
        assertTrue(entry(0, false).needsAttention)
        assertFalse(entry(0, true).needsAttention)
        assertFalse(entry(0, false).copy(word = "dog").needsAttention)
        assertFalse(Entry(2, "dog", null, false).needsAttention)
        // Recogniser-origin picks keep the scanner's verdict; no probability cutoff here.
        assertTrue(entry(0, false, "dog").needsAttention)
        assertFalse(entry(0, true, "dog").needsAttention)
    }
}
