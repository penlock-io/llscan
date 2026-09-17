package com.bitcoinvision.example

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class AttemptsTest {
    private val photo = byteArrayOf(1, 2, 3)

    @Test
    fun an_attempt_moves_from_the_shutter_through_the_photo_to_its_result() {
        val attempts = Attempts<String>()
        assertNull(attempts.current)
        val token = attempts.begin()
        assertEquals(Attempt.Capturing(token), attempts.current)
        assertTrue(attempts.deliver(token, photo))
        assertTrue(attempts.current is Attempt.Preparing)
        assertTrue(attempts.prepared(token, ResolvedPhoto(attempts.photo!!, 1, 3, 1)))
        assertTrue(attempts.current is Attempt.Processing)
        assertTrue(attempts.finish(token, "words"))
        assertEquals("words", (attempts.current as Attempt.Done).result)
        assertTrue(attempts.photo!!.copyBytes().contentEquals(photo))
    }

    @Test
    fun a_photo_that_never_arrived_fails_in_the_words_of_whoever_went_looking() {
        val attempts = Attempts<String>()
        val token = attempts.begin()
        // The file route's reasons are not the camera's, and a person
        // who picked a file is not told to try taking it again.
        assertFalse(attempts.missing(token, "That file could not be opened. Choose another."))
        val failed = attempts.current as Attempt.Failed
        assertEquals("That file could not be opened. Choose another.", failed.message)
        assertNull(failed.photo)
    }

    @Test
    fun a_stale_token_cannot_fail_the_attempt_that_replaced_it() {
        val attempts = Attempts<String>()
        val stale = attempts.begin()
        val current = attempts.begin()
        assertFalse(attempts.missing(stale, "gone"))
        assertEquals(Attempt.Capturing(current), attempts.current)
    }

    @Test
    fun a_failed_read_keeps_the_photo_and_a_camera_that_gave_nothing_fails_without_one() {
        val attempts = Attempts<String>()
        val token = attempts.begin()
        attempts.deliver(token, photo)
        assertTrue(attempts.prepared(token, ResolvedPhoto(attempts.photo!!, 1, 3, 1)))
        assertTrue(attempts.fail(token, "blurred"))
        val failed = attempts.current as Attempt.Failed
        assertEquals("blurred", failed.message)
        assertTrue(failed.photo!!.source.copyBytes().contentEquals(photo))
        val again = attempts.begin()
        assertFalse(attempts.deliver(again, ByteArray(0)))
        assertNull((attempts.current as Attempt.Failed).photo)
        assertFalse(attempts.finish(again, "words"))
    }

    @Test
    fun a_cancelled_attempt_delivers_nothing_and_neither_does_one_replaced() {
        val attempts = Attempts<String>()
        val first = attempts.begin()
        attempts.cancel()
        assertNull(attempts.current)
        // the camera's late photo for a cancelled attempt
        assertFalse(attempts.deliver(first, photo))
        assertNull(attempts.current)
        // a read that ends after its attempt was cancelled
        val second = attempts.begin()
        attempts.deliver(second, photo)
        attempts.cancel()
        assertFalse(attempts.finish(second, "words"))
        assertFalse(attempts.fail(second, "late"))
        assertNull(attempts.current)
        // a read that ends after another attempt replaced it
        val third = attempts.begin()
        attempts.deliver(third, photo)
        val fourth = attempts.begin()
        assertFalse(attempts.finish(third, "stale"))
        assertEquals(Attempt.Capturing(fourth), attempts.current)
        assertFalse(attempts.deliver(third, photo))
        assertEquals(Attempt.Capturing(fourth), attempts.current)
    }

    @Test
    fun preparation_cannot_publish_after_cancel_replacement_or_twice() {
        val attempts = Attempts<String>()
        val first = attempts.begin()
        attempts.deliver(first, photo)
        val resolved = ResolvedPhoto(attempts.photo!!, 6, 3, 1)
        assertFalse(attempts.finish(first, "too early"))
        attempts.cancel()
        assertFalse(attempts.prepared(first, resolved))
        val second = attempts.begin()
        attempts.deliver(second, photo)
        assertFalse(attempts.prepared(first, resolved))
        assertFalse(attempts.prepared(second, resolved)) // Another input, even with the right token.
        val next = ResolvedPhoto(attempts.photo!!, 6, 3, 1)
        assertTrue(attempts.prepared(second, next))
        assertFalse(attempts.prepared(second, next))
        assertSame(next, attempts.resolvedPhoto)
        assertTrue(attempts.finish(second, "read"))
        assertSame(next, attempts.resolvedPhoto)
        assertFalse(attempts.deliver(second, photo))
    }
}
