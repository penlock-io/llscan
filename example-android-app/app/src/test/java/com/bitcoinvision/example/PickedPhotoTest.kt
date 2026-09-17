package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.InputStream

class PickedPhotoTest {
    @Test
    fun a_document_within_the_limit_is_read_whole() {
        val bytes = ByteArray(200) { it.toByte() }
        assertTrue(readAtMost(ByteArrayInputStream(bytes), limit = 256)!!.contentEquals(bytes))
    }

    @Test
    fun a_document_of_exactly_the_limit_is_still_a_photo() {
        assertEquals(64, readAtMost(ByteArrayInputStream(ByteArray(64)), limit = 64)!!.size)
    }

    @Test
    fun one_byte_past_the_limit_is_refused_rather_than_held() {
        assertNull(readAtMost(ByteArrayInputStream(ByteArray(65)), limit = 64))
    }

    @Test
    fun an_enormous_document_is_never_read_into_memory() {
        // A stream with no end: returning at all is the assertion.
        val endless = object : InputStream() {
            override fun read(): Int = 0
            override fun read(b: ByteArray, off: Int, len: Int): Int = len
        }
        assertNull(readAtMost(endless, limit = 1024))
    }

    @Test
    fun an_empty_document_reads_as_empty_not_as_too_large() {
        assertEquals(0, readAtMost(ByteArrayInputStream(ByteArray(0)), limit = 64)!!.size)
    }

    @Test
    fun the_emulators_the_suite_runs_on_are_recognised() {
        assertTrue(
            looksLikeEmulator(
                "google/sdk_gphone64_arm64/emu64a:15/AE3A.240806.043/12448234:userdebug/dev-keys",
                "ranchu", "sdk_gphone64_arm64", "sdk_gphone64_arm64",
            ),
        )
        assertTrue(looksLikeEmulator("generic/vbox86p/vbox86p:9/PI/x:userdebug/test-keys", "vbox86", "vbox86p", "x"))
    }

    @Test
    fun a_phone_is_not_one_so_the_fixture_route_stays_off_it() {
        assertFalse(
            looksLikeEmulator(
                "google/husky/husky:15/AP4A.241205.013/12621605:user/release-keys",
                "husky", "husky", "Pixel 8 Pro",
            ),
        )
        assertFalse(
            looksLikeEmulator(
                "samsung/dm3qxxx/dm3q:14/UP1A.231005.007/S918BXXU4CXA1:user/release-keys",
                "qcom", "dm3qxxx", "SM-S918B",
            ),
        )
    }
}
