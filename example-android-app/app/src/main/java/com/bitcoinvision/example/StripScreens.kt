package com.bitcoinvision.example

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import uniffi.bitcoin_vision_mobile.StripReading

/** The camera and its result are separate in-page states; no Dialog window is needed. */
@Composable
fun StripScanScreen(
    viewModel: StripsViewModel,
    onRecovered: () -> Unit,
    onExit: () -> Unit,
) {
    // a photograph being read, or read and failed, is shown as the
    // photograph; a strip read, or a combination that failed, is the
    // result card
    when (val attempt = viewModel.attempts.current) {
        is Attempt.Preparing -> {
            CapturedPhotoScreen("Preparing your photo", null, null, onRetake = { viewModel.retake() })
            return
        }
        is Attempt.Processing -> {
            CapturedPhotoScreen("Reading the strip", attempt.photo, null, onRetake = { viewModel.retake() })
            return
        }
        is Attempt.Failed -> {
            CapturedPhotoScreen("No strip read", attempt.photo, attempt.message, onRetake = { viewModel.retake() }) {
                SavePhotoForDiagnosis { viewModel.savePhoto() }
                if (attempt.photo != null) SavePhotoForDiagnosis(canonical = true) { viewModel.saveCanonicalPhoto() }
            }
            return
        }
        else -> {}
    }
    val capture = viewModel.capture
    if (capture is Capture.Found || capture is Capture.Failed) {
        CaptureResult(viewModel, capture, onRecovered)
        return
    }
    BackHandler(onBack = onExit)
    val scanning = viewModel.scan as? StripScan.Scanning
    val heldNumber = scanning?.held?.let { viewModel.shareNumber(it) }
    CameraCapture(
        shutterEnabled = capture == null,
        onShutter = { viewModel.begin() },
        onJpeg = { token, photo -> viewModel.deliverPhoto(token, photo) },
        onExit = onExit,
        bar = {
            Column(Modifier.weight(1f)) {
                Text(
                    if (heldNumber == null) "1 of 2 · Scan first strip" else "2 of 2 · Scan second strip",
                    style = MaterialTheme.typography.titleMedium, color = Color.White,
                    modifier = Modifier.testTag("strip-scan-stage"),
                )
                if (heldNumber != null) Text(
                    "Share $heldNumber read", style = MaterialTheme.typography.bodyMedium, color = Color.White,
                )
            }
        },
    )
}

private fun readingSummary(strip: StripReading): String {
    val unread = strip.unread()
    return "${strip.rows()} rows read" + if (unread > 0u) ", $unread cells unclear" else ""
}

@Composable
private fun CaptureResult(viewModel: StripsViewModel, capture: Capture, onRecovered: () -> Unit) {
    val found = capture as? Capture.Found
    val next = found?.let { viewModel.usable(it.strips) }
    val retake = { viewModel.retake() }
    BackHandler(onBack = retake)
    TaskPage(
        title = if (next != null) "Review strip" else "Strip needs attention",
        onBack = retake,
        actions = {
            if (next != null) {
                Button(
                    onClick = { if (viewModel.use(next)) onRecovered() },
                    modifier = Modifier.fillMaxWidth().testTag("use"),
                ) { Text("Use share ${viewModel.shareNumber(next)}") }
                TextButton(onClick = retake, modifier = Modifier.fillMaxWidth().testTag("retake")) { Text("Retake") }
            } else {
                Button(onClick = retake, modifier = Modifier.fillMaxWidth().testTag("retake")) { Text("Retake") }
            }
        },
    ) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        if (next != null) "Share ${viewModel.shareNumber(next)}" else if (found != null) "Not this strip" else "No strip read",
                        style = MaterialTheme.typography.headlineSmall,
                    )
                    Text(
                        when {
                            next != null -> readingSummary(next)
                            found != null -> viewModel.refusal(found.strips)
                            capture is Capture.Failed -> capture.message
                            else -> ""
                        },
                        modifier = Modifier.testTag("strip-read-result").semantics {
                            liveRegion = LiveRegionMode.Polite
                            stateDescription = if (next != null) "Ready to use" else "Retake needed"
                        },
                    )
                }
            }
            if (next != null) Text("Check the share number against your paper before using it.")
            SavePhotoForDiagnosis { viewModel.savePhoto() }
            if (viewModel.attempts.resolvedPhoto != null) {
                SavePhotoForDiagnosis(canonical = true) { viewModel.saveCanonicalPhoto() }
            }
        }
    }
}
