package com.bitcoinvision.example

import org.junit.Assert.*
import org.junit.Test

class PhotoFitTest {
    @Test fun all_eight_canonical_marker_positions_are_fitted_without_another_transform() {
        // Native/preview already oriented the raw 3x2 marker. The overlay must
        // use these canonical coordinates as-is, including mirrored transforms.
        val points = listOf(.5f to .5f, 2.5f to .5f, 2.5f to 1.5f, .5f to 1.5f,
            .5f to .5f, 1.5f to .5f, 1.5f to 2.5f, .5f to 2.5f)
        val pixels = listOf(100f to 200f, 500f to 200f, 500f to 400f, 100f to 400f,
            200f to 100f, 400f to 100f, 400f to 500f, 200f to 500f)
        for (orientation in 1..8) {
            val photo = ResolvedPhoto(PhotoInput.fromFile(byteArrayOf(1)), orientation, 3, 2)
            val frame = PhotoFrame(photo.width, photo.height)
            val fit = fitPhoto(frame, 600f, 600f)!!
            assertEquals(pixels[orientation - 1].first, fit.x(points[orientation - 1].first), .001f)
            assertEquals(pixels[orientation - 1].second, fit.y(points[orientation - 1].second), .001f)
            assertTrue(progressMatchesPhoto(ScanProgressState(1, frame = frame), photo))
        }
    }

    @Test fun photo_and_quads_share_the_same_integer_destination_at_every_aspect_ratio() {
        for (frame in listOf(PhotoFrame(1281, 547), PhotoFrame(547, 1281))) {
            for ((width, height) in listOf(359.5f to 420f, 800f to 240f)) {
                val fit = fitPhoto(frame, width, height)!!
                assertEquals(fit.left.toFloat(), fit.x(0f), .001f)
                assertEquals((fit.left + fit.width).toFloat(), fit.x(frame.width.toFloat()), .001f)
                assertEquals(fit.top.toFloat(), fit.y(0f), .001f)
                assertEquals((fit.top + fit.height).toFloat(), fit.y(frame.height.toFloat()), .001f)
                assertTrue(fit.width <= width + 1 && fit.height <= height + 1)
            }
        }
    }

    @Test fun replacement_and_unresolved_frames_cannot_draw_old_geometry() {
        val photo = ResolvedPhoto(PhotoInput.fromFile(byteArrayOf(1)), 6, 640, 480)
        assertFalse(progressMatchesPhoto(null, photo))
        assertFalse(progressMatchesPhoto(ScanProgressState(1), photo))
        assertFalse(progressMatchesPhoto(ScanProgressState(1, frame = PhotoFrame(640, 480)), photo))
        assertTrue(progressMatchesPhoto(ScanProgressState(2, frame = PhotoFrame(480, 640)), photo))
        assertNull(fitPhoto(PhotoFrame(0, 10), 300f, 300f))
    }
}
