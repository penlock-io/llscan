package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WalletPageTest {
    @Test
    fun the_primary_button_follows_the_next_action_with_review_first() {
        assertEquals(Primary.Review, primaryFor(WalletAction.Review))
        assertEquals(Primary.BackUp, primaryFor(WalletAction.BackUp))
        assertEquals(Primary.Sign, primaryFor(WalletAction.Sign))
        assertEquals(Primary.Scan, primaryFor(WalletAction.LoadKey))
    }

    @Test
    fun the_unloaded_primary_is_the_camera_under_the_old_load_tag() {
        assertEquals("Scan phrase", Primary.Scan.label)
        assertEquals("load-wallet-key", Primary.Scan.tag)
        assertEquals("Sign a transaction", Primary.Sign.label)
        assertEquals("Back up this wallet", Primary.BackUp.label)
        assertEquals("Review transaction", Primary.Review.label)
    }

    @Test
    fun provenance_and_backup_labels_are_short_facts_not_sentences() {
        assertEquals("from written phrase", KeySource.WrittenPhrase.provenance())
        assertEquals("from strips 1 and 3", KeySource.Strips(3u, 1u).provenance())
        assertEquals("made on this phone", KeySource.New.provenance())
        assertEquals("Backup not checked", backupLabel(null))
        assertEquals("Backup checked 3 Sep 2026", backupLabel("2026-09-03"))
    }

    @Test
    fun every_fixed_line_on_the_wallet_pages_is_at_most_one_sentence() {
        val lines = listOf(BACKUP_WARNING_LINE, SAFETY_LINE) + Primary.entries.map { it.label } +
            listOf(backupLabel(null), KeySource.New.provenance())
        for (line in lines) {
            val sentences = line.trim().trimEnd('.').split(". ").size
            assertTrue("'$line' has $sentences sentences", sentences <= if (line == SAFETY_LINE) 2 else 1)
        }
    }

    @Test
    fun unload_is_authorised_by_travel_at_release_not_by_the_proposed_target() {
        val width = 1000
        assertTrue(unloadRequested(-1000f, width, loaded = true))
        assertTrue(unloadRequested(-(width * UNLOAD_TRAVEL), width, loaded = true))
        assertFalse("a 40 % drag", unloadRequested(-400f, width, loaded = true))
        assertFalse("a short fast fling", unloadRequested(-250f, width, loaded = true))
        assertFalse("rightward travel", unloadRequested(1000f, width, loaded = true))
        assertFalse("no key to unload", unloadRequested(-1000f, width, loaded = false))
        assertFalse("before the first layout", unloadRequested(-1000f, 0, loaded = true))
        assertTrue(UNLOAD_TRAVEL >= 0.85f)
    }
}
