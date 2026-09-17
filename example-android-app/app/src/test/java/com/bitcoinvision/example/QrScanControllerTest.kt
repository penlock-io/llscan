package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.bitcoin_vision_mobile.QrScan
import java.util.ArrayDeque
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

class QrScanControllerTest {
    private val frame = QrFrame(4, 4, 4, ByteArray(16))
    private val psbt = "psbtÿ".toByteArray(Charsets.ISO_8859_1)

    /** The main thread as a queue the test drains by hand. */
    private class Harness(decode: (QrFrame) -> QrScan) {
        val queue = ArrayDeque<Runnable>()
        val delivered = mutableListOf<QrScan>()
        var releases = 0
        val controller = QrScanController(decode, { releases += 1 }, { queue.add(it) }, { delivered.add(it) })
        fun drain() {
            while (queue.isNotEmpty()) queue.poll().run()
        }
    }

    private fun blocking(result: QrScan, started: CountDownLatch, release: CountDownLatch): (QrFrame) -> QrScan = {
        started.countDown()
        release.await(5, TimeUnit.SECONDS)
        result
    }

    @Test
    fun a_frame_arriving_while_one_decodes_is_dropped() {
        val started = CountDownLatch(1)
        val release = CountDownLatch(1)
        val h = Harness(blocking(QrScan.Progress(1u, 4u, 0.14f), started, release))
        val worker = thread { assertTrue(h.controller.offer(frame)) }
        started.await(5, TimeUnit.SECONDS)
        assertFalse("the second frame is dropped, not queued", h.controller.offer(frame))
        release.countDown()
        worker.join(5_000)
        h.drain()
        assertEquals(listOf<QrScan>(QrScan.Progress(1u, 4u, 0.14f)), h.delivered)
        assertTrue(h.controller.offer(frame))
        assertEquals(0, h.releases)
    }

    @Test
    fun the_first_decoded_psbt_is_the_last_thing_delivered() {
        var decodes = 0
        val h = Harness {
            decodes += 1
            QrScan.Decoded(psbt)
        }
        assertTrue(h.controller.offer(frame))
        assertFalse(h.controller.offer(frame))
        assertFalse(h.controller.offer(frame))
        h.drain()
        assertEquals(listOf<QrScan>(QrScan.Decoded(psbt)), h.delivered)
        assertEquals("frames after the latch never reach the decoder", 1, decodes)
    }

    @Test
    fun a_decode_queued_for_delivery_is_dropped_when_the_session_retires_first() {
        val h = Harness { QrScan.Decoded(psbt) }
        assertTrue(h.controller.offer(frame))
        assertEquals("delivery is waiting on the main queue", 1, h.queue.size)
        h.controller.retire()
        h.drain()
        assertTrue("nothing reached the screen after Back", h.delivered.isEmpty())
        assertEquals(1, h.releases)
        assertFalse(h.controller.offer(frame))
    }

    @Test
    fun retiring_between_admission_and_the_native_call_waits_for_it_to_finish() {
        val started = CountDownLatch(1)
        val release = CountDownLatch(1)
        val h = Harness(blocking(QrScan.Decoded(psbt), started, release))
        val worker = thread { h.controller.offer(frame) }
        started.await(5, TimeUnit.SECONDS)
        h.controller.retire()
        assertEquals("the decoder stays alive while a frame is inside it", 0, h.releases)
        assertTrue(h.controller.retired)
        release.countDown()
        worker.join(5_000)
        assertEquals("released once, by the worker, after the call returned", 1, h.releases)
        h.drain()
        assertTrue(h.delivered.isEmpty())
        h.controller.retire()
        assertEquals("retire is idempotent", 1, h.releases)
    }

    @Test
    fun a_decode_that_throws_is_a_dropped_frame_and_cleanup_still_happens() {
        var attempts = 0
        val h = Harness {
            attempts += 1
            if (attempts == 1) throw IllegalStateException("decoder gone")
            QrScan.Progress(1u, 2u, 0.29f)
        }
        assertTrue(h.controller.offer(frame))
        h.drain()
        assertTrue(h.delivered.isEmpty())
        assertTrue("the worker is free again", h.controller.offer(frame))
        h.drain()
        assertEquals(listOf<QrScan>(QrScan.Progress(1u, 2u, 0.29f)), h.delivered)
        h.controller.retire()
        assertEquals(1, h.releases)
    }

    @Test
    fun the_ring_follows_progress_and_a_stray_code_only_adds_a_notice() {
        val ring = QrScanState().after(QrScan.Progress(3u, 8u, 0.21f))
        assertEquals(QrScanState(0.21f, 3, 8, null), ring)
        val strayed = ring.after(QrScan.Rejected("QR is a ur:crypto-seed, not a PSBT"))
        assertEquals(ring.copy(notice = "QR is a ur:crypto-seed, not a PSBT"), strayed)
        val resumed = strayed.after(QrScan.Progress(4u, 8u, 0.29f))
        assertNull("the next real part clears the notice", resumed.notice)
        assertEquals(1f, resumed.after(QrScan.Decoded(psbt)).fraction)
    }
}
