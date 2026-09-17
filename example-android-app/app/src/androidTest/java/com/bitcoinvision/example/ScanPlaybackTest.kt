package com.bitcoinvision.example

import android.graphics.Bitmap
import android.graphics.Color
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import kotlinx.coroutines.delay
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import uniffi.bitcoin_vision_mobile.*
import java.io.ByteArrayOutputStream
import java.util.concurrent.CopyOnWriteArrayList

/** Synthetic preview only: no wallet access, corpus, camera permission or model calls. */
class ScanPlaybackTest {
    @get:Rule val compose = createComposeRule()

    @Test fun fast_completion_plays_real_canvas_frames_before_review() {
        val bitmap = Bitmap.createBitmap(320, 240, Bitmap.Config.ARGB_8888).apply { eraseColor(Color.WHITE) }
        val png = ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
        bitmap.recycle()
        val photo = resolvePhoto(PhotoInput.fromFile(png))
        val handoff = ScanProgressHandoff(42)
        val drawn = CopyOnWriteArrayList<ScanProgressState>()
        compose.setContent {
            var progress by remember { mutableStateOf(handoff.nextFrame()!!) }
            var review by remember { mutableStateOf(false) }
            LaunchedEffect(handoff) {
                while (!handoff.playbackComplete()) {
                    progress = handoff.nextFrame()!!
                    delay(PROGRESS_FRAME_MS)
                }
                review = true
            }
            MaterialTheme {
                if (review) Text("Review", Modifier.testTag("test-review"))
                else CapturedPhotoScreen("Reading", photo, null, {}, progress,
                    onProgressPresented = { _, sequence ->
                        if (progress.sequence == sequence && drawn.lastOrNull()?.sequence != sequence) drawn += progress
                        handoff.presented(sequence)
                    })
            }
        }
        compose.onNodeWithTag("reading-note").assertDoesNotExist()
        val bar = compose.onNodeWithTag("reading-progress").fetchSemanticsNode()
        assertNotEquals(ProgressBarRangeInfo.Indeterminate, bar.config[SemanticsProperties.ProgressBarRangeInfo])
        var seq = 0uL
        fun send(update: ScanProgressUpdate) = handoff.onProgress(ScanProgressEvent(++seq, update))
        fun region(state: ScanRegionState) = ScanRegion(0u, listOf(10f, 20f, 100f, 20f, 100f, 60f, 10f, 60f), state)
        send(ScanProgressUpdate.Photo(320u, 240u))
        for (phase in ScanWorkPhase.entries) {
            send(ScanProgressUpdate.Work(phase, 0u, if (phase == ScanWorkPhase.READING) 1u else null))
            if (phase == ScanWorkPhase.GEOMETRY) send(ScanProgressUpdate.Region(region(ScanRegionState.FOUND)))
            if (phase == ScanWorkPhase.READING) {
                send(ScanProgressUpdate.Region(region(ScanRegionState.READING)))
                send(ScanProgressUpdate.Region(region(ScanRegionState.READ)))
                send(ScanProgressUpdate.Work(phase, 1u, 1u))
            }
        }
        send(ScanProgressUpdate.Finished(listOf(ScanRegionMapping(0u, 0u))))
        handoff.sealSuccess()
        compose.onNodeWithTag("test-review").assertDoesNotExist()
        compose.waitUntil(20_000) { handoff.playbackComplete() }
        compose.waitUntil(5_000) {
            compose.onAllNodesWithTag("test-review").fetchSemanticsNodes().isNotEmpty()
        }
        compose.onNodeWithTag("test-review").assertIsDisplayed()
        assertEquals(ScanWorkPhase.entries.toSet(), drawn.map { it.phase }.toSet())
        assertTrue(drawn.any { it.regions[0u]?.state == ScanRegionState.FOUND })
        assertTrue(drawn.any { it.regions[0u]?.state == ScanRegionState.READING })
        assertTrue(drawn.any { it.regions[0u]?.state == ScanRegionState.READ })
        assertEquals(ProgressOutcome.Finished, drawn.last().outcome)
        handoff.detach()
    }
}
