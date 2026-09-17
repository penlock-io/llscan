package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PhraseDraftTest {
    @Test
    fun passing_checksum_does_not_tick_nine_accepted_words_when_order_requires_review() {
        val words = listOf("close", "transfer", "orange", "absorb", "figure", "between",
            "pen", "tobacco", "exist", "outside", "equip", "spare")
        val d = PhraseDraft(words.map { source(it, accepted = it !in setOf("close", "pen", "spare")) },
            columns = listOf(0, 2, 4, 6, 8, 10, 1, 3, 5, 7, 9, 11),
            rows = words.indices.toList(), order = Order.ROWS, orderRequiresReview = true)
        assertTrue(d.entries.none { it.checked })
        assertFalse(d.allChecked)
        assertEquals("12 words · checksum passes · 0 of 12 ticked · check word order against paper",
            phraseStatus(12, true, ticked = 0, orderNeedsReview = d.orderRequiresReview && !d.allChecked))
        val checked = d.checkAll()
        assertTrue(checked.allChecked)
        assertEquals(checked.entries, checked.reorder(Order.ROWS).entries)
        assertFalse(checked.reorder(Order.COLUMNS).allChecked)
        assertTrue(checked.reorder(Order.COLUMNS).entries.first().checked)
        assertTrue(checked.reorder(Order.COLUMNS).entries.last().checked)
        assertTrue(checked.reorder(Order.COLUMNS).entries.drop(1).dropLast(1).none { it.checked })
    }

    @Test
    fun page22_geometric_default_preserves_word_uncertainty_and_manual_transpose() {
        val words = listOf("close", "orange", "figure", "pen", "exist", "equip",
            "transfer", "absorb", "between", "tobacco", "outside", "spare")
        val d = PhraseDraft(words.map { source(it, accepted = it !in setOf("close", "pen", "spare")) },
            columns = words.indices.toList(), rows = listOf(0, 6, 1, 7, 2, 8, 3, 9, 4, 10, 5, 11),
            order = Order.COLUMNS, orderRequiresReview = false)
        assertEquals(words, d.words)
        assertEquals(9, d.entries.count { it.checked })
        assertFalse(d.allChecked)
        assertEquals("12 words · checksum fails: review the words", phraseStatus(12, false))
        val transposed = d.reorder(Order.ROWS)
        assertEquals(listOf("close", "transfer", "orange", "absorb", "figure", "between",
            "pen", "tobacco", "exist", "outside", "equip", "spare"), transposed.words)
        assertTrue(transposed.entries.none { it.checked })
        assertTrue(transposed.orderRequiresReview)
    }

    @Test
    fun near_tie_default_opens_unticked_and_manual_transpose_stays_available() {
        val words = listOf("fun", "soldier", "lock", "fix", "onion", "afford",
            "sibling", "order", "all", "report", "injury", "habit")
        val d = PhraseDraft(words.map { source(it, accepted = it != "all") },
            columns = words.indices.toList(), rows = listOf(0, 4, 8, 1, 5, 9, 2, 6, 10, 3, 7, 11),
            order = Order.COLUMNS, orderRequiresReview = true)
        assertEquals(words, d.words) // Longest-dimension default is NOT changed.
        assertTrue(d.entries.none { it.checked })
        assertEquals("12 words · checksum fails: review the words · check word order against paper",
            phraseStatus(12, false, orderNeedsReview = d.orderRequiresReview))
        val rows = d.reorder(Order.ROWS)
        assertEquals(listOf("fun", "onion", "all", "soldier", "afford", "report",
            "lock", "sibling", "injury", "fix", "order", "habit"), rows.words)
        assertTrue(rows.entries.none { it.checked })
        assertFalse(rows.allChecked)
        assertTrue(rows.checkAll().allChecked)
    }

    @Test
    fun selected_word_confidence_controls_story_and_unlock_ticks_in_both_orders() {
        val story = Source(List(8) { 0f }, listOf("history" to .9947837f, "victory" to .002336808f),
            false, ByteArray(0), read = "story", raw = "12.SToRy",
            matching = MatchingText("12.SToRy", false, 0))
        val unlock = Source(List(8) { 1f }, listOf("unlock" to .9999974f, "under" to .0000009745443f),
            true, ByteArray(0), read = "unlock", raw = "unlock")
        val draft = PhraseDraft(listOf(story, unlock), listOf(0, 1), listOf(1, 0), Order.COLUMNS)
        for (d in listOf(draft, draft.reorder(Order.ROWS))) {
            assertFalse(d.entries.single { it.id == 0 }.checked)
            assertEquals(d.order == Order.COLUMNS, d.entries.single { it.id == 1 }.checked)
            assertEquals("story", d.entries.single { it.id == 0 }.word)
            assertFalse(d.allChecked)
        }
        assertTrue(draft.check(0, true).allChecked)
        // Existing reorder policy rechecks uncertain words; editing invalidates a check.
        assertFalse(draft.check(0, true).reorder(Order.ROWS).entries.single { it.id == 0 }.checked)
        assertFalse(draft.change(1, "unfold").entries.single { it.id == 1 }.checked)
    }

    @Test
    fun list_mode_and_ocr_provenance_survive_edits_without_inventing_number_order() {
        val original = OriginalOcr(List(8) { 1f }, byteArrayOf(1), "9.Hobby")
        val word = Source(List(8) { 2f }, listOf("hobby" to .9f), true, byteArrayOf(2),
            "hobby", raw = ". Hobby", originalOcr = original,
            matching = MatchingText("Hobby", true, 2), labels = listOf(ObservedLabel("9.Hobby",9,"prefix",false)))
        val draft = PhraseDraft(listOf(word, source("state")), listOf(0,1), listOf(1,0),
            Order.COLUMNS, listNumbered = true)
        assertTrue(draft.listNumbered)
        assertFalse(draft.numbered)
        assertTrue(numberingNote(draft.numbers, draft.listNumbered)!!.contains("check the reading order"))
        val changed = draft.change(0, "happy").reorder(Order.ROWS)
        assertTrue(changed.listNumbered)
        val entry = changed.entries.first { it.id == 0 }
        assertFalse(entry.checked)
        assertEquals(". Hobby", entry.source!!.raw)
        assertEquals(original, entry.source.originalOcr)
        assertEquals("Hobby", entry.source.matching!!.text)
        assertEquals(null, entry.number)
    }

    @Test
    fun joined_words_and_original_pieces_are_reversible_but_never_active_together() {
        val joined = Source(List(8) { 0f }, listOf("transfer" to 0.999f), true,
            ByteArray(0), read = "transfer", joinedFrom = listOf(0, 2))
        val sources = listOf(source("track", accepted = false, stray = true), joined,
            source("fee", accepted = false, stray = true), source("last"))
        val d = PhraseDraft(sources, listOf(0, 1, 2, 3), listOf(3, 0, 1, 2), Order.COLUMNS)
        assertEquals(listOf("transfer", "last"), d.words)
        assertEquals("Use original parts", d.restoreLabel(0))
        assertEquals("Use joined word", d.restoreLabel(1))
        assertEquals("Keep as a word", d.restoreLabel(3))
        assertTrue(d.isAlternative(2))
        for (piece in listOf(0, 2)) {
            val split = d.restore(piece)
            assertEquals(listOf("track", "fee", "last"), split.words)
            assertEquals(listOf(1), split.leftOut.map { it.id })
            assertTrue(split.entries.take(2).none { it.checked })
            assertEquals(listOf("last", "track", "fee"), split.reorder(Order.ROWS).words)
            val back = split.change(0, "train").restore(1)
            assertEquals(listOf("transfer", "last"), back.words)
            assertFalse(back.entries.first().checked)
            assertEquals(listOf("train", "fee", "last"), back.restore(2).words)
            // Removing a union does not silently select a different interpretation.
            assertEquals(listOf("last"), d.remove(1).words)
            assertEquals(listOf("track", "fee", "last"), d.remove(1).restore(piece).words)
        }
    }

    private fun source(word: String, accepted: Boolean = true, stray: Boolean = false, read: String = word, number: Int? = null) =
        Source(List(8) { 0f }, listOf(word to 0.9f, "zoo" to 0.05f), accepted, ByteArray(0), read = read, raw = read, stray = stray, number = number)

    @Test
    fun expanded_pairs_restore_prior_strays_both_orders_and_keep_corrections_without_ticks() {
        for (originalStrays in listOf(listOf(false, false), listOf(false, true), listOf(true, true))) {
            val expanded = Source(List(8) { 4f }, listOf("later" to 0.7f), false,
                byteArrayOf(1, 2), read = "later", raw = "laler",
                expandedFrom = listOf(ExpansionParent(0, originalStrays[0]), ExpansionParent(2, originalStrays[1])))
            val sources = listOf(source("ten", accepted = false, stray = true), expanded,
                source("fee", accepted = false, stray = true), source("last"))
            for (order in listOf(Order.COLUMNS, Order.ROWS)) {
                val d = PhraseDraft(sources, listOf(0, 1, 2, 3), listOf(3, 0, 1, 2), order)
                assertFalse(d.entries.single { it.id == 1 }.checked)
                assertTrue(d.isAlternative(0) && d.isAlternative(1) && d.isAlternative(2))
                assertEquals("Use original parts", d.restoreLabel(0))
                assertEquals("Use expanded word", d.restoreLabel(1))
                assertTrue(expanded.joinedFrom.isEmpty())
                val originals = d.check(1, true).restore(0)
                val expectedIds = listOf(0, 2).filterIndexed { i, _ -> !originalStrays[i] }
                assertEquals(expectedIds.toSet() + 3, originals.entries.map { it.id }.toSet())
                assertTrue(originals.entries.filter { it.id != 3 }.none { it.checked })
                assertEquals(if (order == Order.COLUMNS) expectedIds + 3 else listOf(3) + expectedIds,
                    originals.entries.map { it.id })
                assertEquals("Keep as a word", originals.restoreLabel(2))
                // A previously stray original remains available for an explicit
                // keep after the group switch, including an all-stray group.
                val corrected = originals.restore(2).change(2, "fire").check(2, true)
                val back = corrected.restore(1)
                assertEquals(setOf(1, 3), back.entries.map { it.id }.toSet())
                assertFalse(back.entries.single { it.id == 1 }.checked)
                assertTrue(back.entries.single { it.id == 1 }.source === expanded)
                assertFalse(back.reorder(if (order == Order.ROWS) Order.COLUMNS else Order.ROWS)
                    .entries.single { it.id == 1 }.checked)
                val again = back.restore(2).restore(2)
                assertEquals("fire", again.entries.single { it.id == 2 }.word)
                assertFalse(again.entries.single { it.id == 2 }.checked)
                assertTrue(again.entries.single { it.id == 2 }.source === sources[2])
                assertEquals(listOf("last"), d.remove(1).words)
            }
        }
    }

    @Test
    fun singleton_expansion_uses_its_own_verdict_and_clears_checks_on_each_switch() {
        for (accepted in listOf(false, true)) {
            val original = source("later", accepted = true, stray = true)
            val expanded = Source(List(8) { 5f }, listOf("later" to 0.97f), accepted,
                byteArrayOf(3), read = "later", expandedFrom = listOf(ExpansionParent(0, false)))
            val d = PhraseDraft(listOf(original, expanded), listOf(0, 1), listOf(0, 1), Order.COLUMNS)
            assertEquals(accepted, d.entries.single().checked)
            assertEquals("Use original word", d.restoreLabel(0))
            val restored = d.restore(0)
            assertTrue(restored.entries.single().source === original)
            assertFalse(restored.entries.single().checked)
            val back = restored.check(0, true).restore(1)
            assertTrue(back.entries.single().source === expanded)
            assertFalse(back.entries.single().checked)
        }
    }

    // Four boxes laid out two by two: columns read a c b d, rows a b c d.
    private fun draft() = PhraseDraft(
        listOf(source("a"), source("b"), source("c"), source("d")),
        columns = listOf(0, 2, 1, 3),
        rows = listOf(0, 1, 2, 3),
        order = Order.COLUMNS,
    )

    private fun PhraseDraft.checkAll() = entries.fold(this) { d, e -> d.check(e.id, true) }

    @Test
    fun the_order_chooses_which_words_follow_which() {
        assertEquals(listOf("a", "c", "b", "d"), draft().words)
        assertEquals(listOf("a", "b", "c", "d"), draft().reorder(Order.ROWS).words)
    }

    @Test
    fun changing_a_word_clears_only_its_check() {
        val all = draft()
        assertTrue(all.allChecked)
        val changed = all.change(2, "cat")
        assertEquals(listOf("a", "cat", "b", "d"), changed.words)
        assertEquals(listOf(true, false, true, true), changed.entries.map { it.checked })
        assertTrue(changed.entries[1].corrected)
        assertTrue(changed.check(2, true).allChecked)
    }

    @Test
    fun an_inserted_word_is_typed_unchecked_and_keeps_its_place_across_orders() {
        val d = draft().checkAll().insert(1, "x")
        assertEquals(listOf("a", "x", "c", "b", "d"), d.words)
        val x = d.entries[1]
        assertTrue(x.typed)
        assertFalse(x.checked)
        assertFalse(d.allChecked)
        // anchored after a: in row order it still follows a
        assertEquals(listOf("a", "x", "b", "c", "d"), d.reorder(Order.ROWS).words)
        // at the very start, and between two typed words
        assertEquals(listOf("s", "a", "x", "c", "b", "d"), d.insert(0, "s").words)
        assertEquals(listOf("a", "x", "y", "c", "b", "d"), d.insert(2, "y").words)
        assertEquals(listOf("a", "c", "b", "d", "e"), draft().insert(4, "e").words)
    }

    @Test
    fun removing_a_word_drops_its_check_and_typed_words_can_go_too() {
        val d = draft().checkAll().remove(2)
        assertEquals(listOf("a", "b", "d"), d.words)
        assertTrue(d.allChecked)
        val typed = d.insert(1, "x")
        val id = typed.entries[1].id
        assertEquals(listOf("a", "b", "d"), typed.remove(id).words)
        assertTrue(typed.remove(id).allChecked)
    }

    @Test
    fun switching_order_preserves_only_checks_at_unchanged_positions() {
        val d = PhraseDraft(
            listOf(source("a"), source("b", accepted = false), source("c"), source("d")),
            columns = listOf(0, 2, 1, 3),
            rows = listOf(0, 1, 2, 3),
            order = Order.COLUMNS,
        )
        val ticked = d.checkAll().reorder(Order.ROWS)
        assertEquals(listOf(true, false, false, true), ticked.entries.map { it.checked })
        assertTrue(draft().checkAll().reorder(Order.COLUMNS).allChecked)
        // a corrected word stays the person's to tick, through a reorder and back
        val corrected = draft().change(0, "cat")
        assertFalse(corrected.entries[0].checked)
        val rows = corrected.reorder(Order.ROWS)
        assertFalse(rows.entries.first { it.id == 0 }.checked)
        assertFalse(rows.entries.single { it.id == 1 }.checked)
        assertFalse(rows.entries.single { it.id == 2 }.checked)
        assertTrue(rows.entries.single { it.id == 3 }.checked)
        assertFalse(rows.reorder(Order.COLUMNS).entries.first { it.id == 0 }.checked)
        // Returning to the model's spelling does not manufacture confirmation.
        assertFalse(corrected.change(0, "a").reorder(Order.ROWS).entries.first { it.id == 0 }.checked)
    }

    @Test
    fun a_stray_starts_left_out_and_comes_back_at_its_place_unchecked() {
        // a stray s between a and c in column order, and c read from the recogniser
        val d = PhraseDraft(
            listOf(source("a"), source("b"), source("s", stray = true, read = "sea"), source("c", read = "cat")),
            columns = listOf(0, 2, 1, 3),
            rows = listOf(0, 1, 2, 3),
            order = Order.COLUMNS,
        )
        assertEquals(listOf("a", "b", "cat"), d.words)
        assertEquals(listOf("sea"), d.leftOut.map { it.word })
        assertTrue(d.leftOut.single().source!!.stray)
        assertTrue(d.entries.last().source!!.fromRead)
        assertFalse(d.entries.first().source!!.fromRead)
        val back = d.checkAll().restore(2)
        assertEquals(listOf("a", "sea", "b", "cat"), back.words)
        assertFalse(back.entries[1].checked)
        assertFalse(back.allChecked)
        assertTrue(back.leftOut.isEmpty())
        assertEquals(listOf("a", "b", "sea", "cat"), back.reorder(Order.ROWS).words)
        // a word the person removed is left out the same way and comes back the same way
        val removed = back.remove(1)
        assertEquals(listOf("b"), removed.leftOut.map { it.word })
        assertEquals(listOf("a", "sea", "b", "cat"), removed.restore(1).words)
    }

    @Test
    fun a_numbered_page_reads_by_number_and_a_missing_number_is_placed_and_named() {
        // a, b, c, d on the page; the numbers beside them 2, none, 1, 4, so 3 is missing
        val d = PhraseDraft(
            listOf(source("a", number = 2), source("b"), source("c", number = 1), source("d", number = 4)),
            columns = listOf(0, 1, 2, 3),
            rows = listOf(0, 1, 2, 3),
            order = Order.NUMBERS,
            byNumber = listOf(2, 0, 3, 1),
            numbers = Numbers.Held(4, listOf(3), emptyList(), 0),
        )
        assertTrue(d.numbered)
        assertEquals(listOf("c", "a", "d", "b"), d.words)
        // word 3 belongs before d, the first word numbered higher
        assertEquals(mapOf(2 to 3), d.missingAt())
        assertEquals("4 words: word 3 was not found", phraseStatus(4, false, d.numbers))
        val filled = d.insert(2, "x")
        assertEquals(listOf("c", "a", "x", "d", "b"), filled.words)
        // the same words down columns, the numbers still on them, and
        // no place to say where 3 would go
        assertEquals(listOf("a", "b", "c", "d"), d.reorder(Order.COLUMNS).words)
        assertTrue(d.reorder(Order.COLUMNS).missingAt().isEmpty())
        // an unnumbered page has no numbered order to speak of
        assertFalse(draft().numbered)
        assertEquals(listOf("a", "c", "b", "d"), draft().reorder(Order.NUMBERS).words)
        assertTrue(draft().missingAt().isEmpty())
    }

    @Test
    fun a_typed_word_fills_the_gap_it_was_offered_and_reopens_it_when_it_goes() {
        // numbers 1..6 and 9..10 on eight boxes: 7 and 8 are missing
        val boxes = (1..10).filter { it != 7 && it != 8 }.map { source("w$it", number = it) }
        val d = PhraseDraft(
            boxes,
            columns = boxes.indices.toList(),
            rows = boxes.indices.toList(),
            order = Order.NUMBERS,
            byNumber = boxes.indices.toList(),
            numbers = Numbers.Held(10, listOf(7, 8), emptyList(), 0),
        )
        assertEquals(mapOf(6 to 7), d.missingAt())
        assertEquals("8 words: words 7 and 8 were not found", phraseStatus(8, false, d.numbers))
        // filling 7 at its place: the offer moves on to 8, after the new word
        val seven = d.insert(6, "seven", number = 7)
        assertEquals(listOf(7), seven.entries[6].number.let { listOf(it) })
        assertEquals(mapOf(7 to 8), seven.missingAt())
        assertEquals("9 words: word 8 was not found", phraseStatus(9, false, seven.numbers))
        val eight = seven.insert(7, "eight", number = 8)
        assertTrue(eight.missingAt().isEmpty())
        assertEquals(listOf(7, 8), eight.entries.subList(6, 8).map { it.number })
        assertEquals(Numbers.Held(10, emptyList(), emptyList(), 0), eight.numbers)
        assertEquals("10 words: this worksheet holds a 12-word phrase", phraseStatus(10, false, eight.numbers))
        // the number stays through a correction and either order, and the
        // scan's own reading is kept apart
        assertEquals(7, eight.change(seven.entries[6].id, "heaven").entries[6].number)
        assertEquals(listOf(7, 8), eight.reorder(Order.COLUMNS).entries.subList(6, 8).map { it.number })
        assertEquals(listOf(7, 8), (eight.scanNumbers as Numbers.Held).missing)
        // removing the word reopens its gap, and an ordinary insertion fills none
        val gone = eight.remove(seven.entries[6].id)
        assertEquals(mapOf(6 to 7), gone.missingAt())
        val plain = gone.insert(6, "plain")
        assertEquals(null, plain.entries[6].number)
        assertEquals(mapOf(6 to 7), plain.missingAt())
    }

    @Test
    fun the_numbering_note_says_what_the_page_s_numbers_left_open() {
        assertEquals(null, numberingNote(Numbers.None))
        assertEquals(null, numberingNote(Numbers.Held(12, emptyList(), emptyList(), 0)))
        assertEquals(
            "Number 5 is not where the page puts it: check the order.",
            numberingNote(Numbers.Held(12, emptyList(), listOf(5), 0)),
        )
        assertEquals(
            "Numbers 5, 6 are not where the page puts them: check the order. A number beside 1 word could not be read.",
            numberingNote(Numbers.Held(12, emptyList(), listOf(5, 6), 1)),
        )
        assertEquals(
            "The numbers on the page contradict each other, so the words are in the order they sit on the page.",
            numberingNote(Numbers.Inconsistent("two boxes are numbered 4")),
        )
        assertEquals("11 words: words 4 and 7 were not found", phraseStatus(11, false, Numbers.Held(12, listOf(4, 7), emptyList(), 0)))
        assertEquals("12 words · checksum passes", phraseStatus(12, true, Numbers.Held(12, emptyList(), emptyList(), 0)))
    }

    @Test
    fun supported_order_can_start_confident_but_reordering_never_regrants_ticks() {
        val d = PhraseDraft(
            listOf(source("a"), source("b", accepted = false), source("c"), source("s", stray = true)),
            columns = listOf(0, 2, 1, 3),
            rows = listOf(0, 1, 2, 3),
            order = Order.COLUMNS,
        )
        assertEquals(listOf(true, true, false), d.entries.map { it.checked })
        assertFalse(d.allChecked)
        // Edits take a tick back; reordered positions require fresh checks.
        assertEquals(listOf(false, true, false), d.change(0, "cat").entries.map { it.checked })
        val all = d.check(1, true)
        assertTrue(all.allChecked)
        assertEquals(listOf(true, false, false), all.reorder(Order.ROWS).entries.map { it.checked })
        assertEquals(listOf(false, false, false), all.check(0, false).reorder(Order.ROWS).entries.map { it.checked })
        // a stray put back and a typed word are the person's to tick
        assertFalse(all.restore(3).entries.first { it.id == 3 }.checked)
        assertFalse(all.insert(1, "x").entries[1].checked)
        assertEquals("12 words · checksum passes · 9 of 12 ticked", phraseStatus(12, true, ticked = 9))
        assertEquals("12 words · checksum fails: review the words", phraseStatus(12, false, ticked = 9))
    }

    @Test
    fun the_status_line_names_the_count_and_the_checksum() {
        assertEquals("12 words · checksum passes", phraseStatus(12, true))
        assertEquals(
            "12 words · checksum fails: review the words",
            phraseStatus(12, false),
        )
        assertEquals("11 words: a word is missing", phraseStatus(11, false))
        assertEquals("13 words: one word too many", phraseStatus(13, false))
        assertEquals("24 words: this worksheet holds a 12-word phrase", phraseStatus(24, true))
        assertEquals("0 words: this worksheet holds a 12-word phrase", phraseStatus(0, false))
    }
}
