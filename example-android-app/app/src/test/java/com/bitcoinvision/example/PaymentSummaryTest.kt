package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Test

class PaymentSummaryTest {
    @Test
    fun leaving_is_payments_plus_fee_including_multiple_recipients() {
        assertEquals(41_000uL, totalLeaving(listOf(40_000uL), 1_000uL))
        assertEquals(51_000uL, totalLeaving(listOf(40_000uL, 10_000uL), 1_000uL))
    }

    @Test
    fun all_change_still_pays_a_fee() {
        assertEquals(1_000uL, totalLeaving(emptyList(), 1_000uL))
        assertEquals(0uL, totalLeaving(emptyList(), 0uL))
    }

    @Test
    fun amounts_use_grouped_satoshis_without_losing_precision() {
        assertEquals("2,100,000,000,000,000 sats", sats(2_100_000_000_000_000uL))
        assertEquals("1,000 sats", sats(1_000uL))
    }
}
