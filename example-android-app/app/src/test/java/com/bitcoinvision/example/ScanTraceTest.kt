package com.bitcoinvision.example

import androidx.compose.ui.Modifier
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

class ScanTraceTest {
    @Test fun only_the_explicit_benchmark_build_delivers_numeric_events() {
        var calls = 0
        try {
            ScanTrace.sink = { calls++; true }
            ScanTrace.mark("test")
            assertEquals(if (BuildConfig.SCAN_BENCHMARK) 1 else 0, calls)
        } finally {
            ScanTrace.sink = null
        }
    }

    @Test fun ordinary_builds_do_not_add_a_drawing_modifier() {
        if (!BuildConfig.SCAN_BENCHMARK) {
            assertSame(Modifier, Modifier.traceReviewFrame())
            assertEquals("com.bitcoinvision.example", BuildConfig.APPLICATION_ID)
        }
    }
}
