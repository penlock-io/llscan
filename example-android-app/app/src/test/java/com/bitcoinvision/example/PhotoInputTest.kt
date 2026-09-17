package com.bitcoinvision.example

import org.junit.Assert.*
import org.junit.Test

class PhotoInputTest {
    private fun camera(rotation: Int) = CameraPhotoMetadata(rotation, 6, 4, 256, 0, 90)

    @Test
    fun camera_zero_is_present_and_file_has_no_camera_rotation() {
        val bytes = byteArrayOf(1, 2, 3)
        for (rotation in listOf(0, 90, 180, 270)) {
            val input = PhotoInput.fromCamera(bytes, camera(rotation))
            assertEquals(rotation, input.camera!!.rotationDegrees)
            assertArrayEquals(bytes, input.copyBytes())
        }
        assertNull(PhotoInput.fromFile(bytes).camera)
    }

    @Test
    fun caller_and_reader_mutation_cannot_change_the_retained_original() {
        val original = byteArrayOf(1, 2, 3)
        val input = PhotoInput.fromCamera(original, camera(90))
        original[0] = 8
        input.copyBytes()[1] = 9
        assertArrayEquals(byteArrayOf(1, 2, 3), input.openStream().readBytes())
    }

    @Test
    fun attempts_keep_their_own_framing_snapshot_without_applying_it() {
        val attempts = Attempts<String>()
        val metadata = camera(0).copy(
            cropRect = CaptureBounds(1, 0, 5, 4),
            previewAtShutter = CapturePreviewMetadata(CaptureBounds(0, 0, 720, 1612), "FILL_CENTER", 0),
        )
        val firstPhoto = PhotoInput.fromCamera(byteArrayOf(1), metadata)
        val first = attempts.begin()
        assertTrue(attempts.deliverPhoto(first, firstPhoto))
        val secondPhoto = PhotoInput.fromCamera(byteArrayOf(2), camera(90).copy(
            cropRect = CaptureBounds(0, 1, 6, 3),
            previewAtShutter = CapturePreviewMetadata(CaptureBounds(0, 0, 480, 800), "FIT_CENTER", 0),
        ))
        val second = attempts.begin()
        assertFalse(attempts.deliverPhoto(first, firstPhoto))
        assertTrue(attempts.deliverPhoto(second, secondPhoto))
        assertSame(secondPhoto, attempts.photo)
        assertEquals(CaptureBounds(0, 1, 6, 3), attempts.photo!!.camera!!.cropRect)
        assertEquals(CaptureBounds(1, 0, 5, 4), firstPhoto.camera!!.cropRect)
        val resolved = ResolvedPhoto(secondPhoto, 6, 6, 4)
        assertEquals(4, resolved.width)
        assertEquals(6, resolved.height)
        attempts.cancel()
        assertNull(attempts.photo)
    }

    @Test
    fun capture_metadata_follows_success_and_failure_and_is_dropped_on_cancel() {
        val attempts = Attempts<String>()
        val input = PhotoInput.fromCamera(byteArrayOf(1), camera(270))
        val first = attempts.begin()
        assertTrue(attempts.deliverPhoto(first, input))
        assertTrue(attempts.prepared(first, ResolvedPhoto(input, 8, 6, 4)))
        assertTrue(attempts.finish(first, "read"))
        assertSame(input, attempts.photo)
        val second = attempts.begin()
        assertTrue(attempts.deliverPhoto(second, input))
        assertTrue(attempts.fail(second, "blurred"))
        assertEquals(270, attempts.photo!!.camera!!.rotationDegrees)
        attempts.cancel()
        assertNull(attempts.photo)
        assertFalse(attempts.deliverPhoto(second, input))
        val third = attempts.begin()
        assertFalse(attempts.deliverPhoto(second, input))
        assertEquals(Attempt.Capturing(third), attempts.current)
    }

    @Test(expected = IllegalArgumentException::class)
    fun invalid_camera_rotation_is_not_silently_treated_as_zero() {
        camera(45)
    }

    @Test
    fun transforms_are_selected_once_not_added_together() {
        val expected = mapOf(0 to 1, 90 to 6, 180 to 3, 270 to 8)
        for ((rotation, orientation) in expected) {
            for (exif in 0..9) assertEquals(orientation, orientationFromMetadata(camera(rotation), exif))
        }
        for (exif in 1..8) assertEquals(exif, orientationFromMetadata(null, exif))
        for (exif in listOf(null, -1, 0, 9)) assertEquals(1, orientationFromMetadata(null, exif))
    }

    @Test
    fun identical_identity_transforms_keep_distinct_photo_up_provenance() {
        for ((camera, exif, source) in listOf(
            Triple(camera(0), null, PhotoUpSource.CAMERA),
            Triple(null, 1, PhotoUpSource.EXIF),
            Triple(null, null, PhotoUpSource.UNKNOWN),
            Triple(null, 0, PhotoUpSource.UNKNOWN),
            Triple(null, 9, PhotoUpSource.UNKNOWN),
        )) {
            assertEquals(1, orientationFromMetadata(camera, exif))
            assertEquals(source, photoUpFromMetadata(camera, exif))
        }
        for (rotation in listOf(0, 90, 180, 270)) {
            for (exif in listOf(null, -1, 0, 1, 6, 9)) {
                assertEquals(PhotoUpSource.CAMERA, photoUpFromMetadata(camera(rotation), exif))
            }
        }
        for (exif in 1..8) assertEquals(PhotoUpSource.EXIF, photoUpFromMetadata(null, exif))
        assertEquals(PhotoUpSource.UNKNOWN, photoUpFromMetadata(null, -1))
    }

    @Test
    fun resolved_photo_retains_provenance_without_inference_or_another_transform() {
        val input = PhotoInput.fromCamera(byteArrayOf(1), camera(90))
        // Legacy explicit-transform callers have not supplied a provenance hint.
        assertEquals(PhotoUpSource.UNKNOWN, ResolvedPhoto(input, 6, 6, 4).photoUp)
        val resolved = ResolvedPhoto(input, 6, 6, 4, PhotoUpSource.CAMERA)
        assertEquals(PhotoUpSource.CAMERA, resolved.photoUp)
        assertSame(input, resolved.source)
        assertEquals(6, resolved.orientation)
        assertEquals(4, resolved.width)
        assertEquals(6, resolved.height)
    }
}
