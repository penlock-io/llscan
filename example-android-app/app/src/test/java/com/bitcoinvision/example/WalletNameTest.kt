package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class WalletNameTest {
    @Test
    fun names_are_trimmed() {
        assertEquals("Travel wallet", normalizedWalletName("  Travel wallet \n"))
    }

    @Test
    fun empty_and_control_character_names_are_rejected() {
        for (name in listOf("", " \t\n", "bad\nname", "bad\tname", "bad\u0000name")) {
            assertNull(normalizedWalletName(name))
        }
    }

    @Test
    fun names_have_a_forty_code_point_limit() {
        assertEquals("x".repeat(40), normalizedWalletName("x".repeat(40)))
        assertNull(normalizedWalletName("x".repeat(41)))
    }

    @Test
    fun supplementary_characters_count_once() {
        val symbol = "\uD83C\uDFD5"
        assertEquals(symbol.repeat(40), normalizedWalletName(symbol.repeat(40)))
        assertNull(normalizedWalletName(symbol.repeat(41)))
    }
}
