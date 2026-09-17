package com.bitcoinvision.example

import android.app.Activity
import android.view.WindowManager
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner

@Composable
fun NewSeedScreen(model: NewSeedViewModel, onCancel: () -> Unit, onComplete: () -> Unit) {
    val window = (LocalContext.current as Activity).window
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle, model) {
        val observer = LifecycleEventObserver { _, event ->
            // CameraX may abandon a shutter callback when its activity stops.
            // Revoke that capture so resume offers a usable shutter, not a stale photo.
            if (event == Lifecycle.Event.ON_STOP && model.capturing) model.retake()
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    DisposableEffect(window) {
        val alreadySecure = window.attributes.flags and WindowManager.LayoutParams.FLAG_SECURE != 0
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        onDispose { if (!alreadySecure) window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) }
    }
    BackHandler { onCancel() }
    when (model.stage) {
        NewSeedStage.Idle -> WalletPage("Setup ended", onCancel) {
            Text("No new wallet was saved. If Android closed the app, start again with a new phrase.")
            Button(onClick = onCancel) { Text("Back to wallets") }
        }
        NewSeedStage.Writing -> WalletPage("Write down your phrase", onCancel, titleTag = "write-key") {
            Text("Write all 12 words in order on paper, then photograph the whole list. " +
                "Review the photo's reading before saving the wallet, and check any misread words against your paper.")
            Text("Keep this screen private. The pending key and photo stay in memory only. " +
                "Cancelling or the app being closed by Android abandons setup. Do not use this wallet yet.")
            PhraseBody(model.words)
            Button(onClick = model::retake, modifier = Modifier.fillMaxWidth().testTag("photograph-backup")) {
                Text("Photograph my written backup")
            }
        }
        NewSeedStage.Camera -> CameraCapture(
            shutterEnabled = !model.capturing,
            onShutter = model::begin,
            onJpeg = model::deliver,
            onExit = model::showWords,
            bar = { Text("Photograph all 12 written words", color = Color.White) },
        )
        NewSeedStage.Reading -> CapturedPhotoScreen(
            title = "Checking your written backup", photo = model.photo, error = null,
            onRetake = model::retake, progress = model.progress,
            onProgressPresented = model::progressPresented,
        )
        NewSeedStage.Review, NewSeedStage.Verified -> BackupWordReview(model, onCancel, onComplete)
    }
}

/** Same word rows and confidence display as recovery, with a known-word comparison.
 * Tap reviews a discrepancy, rather than editing the immutable generated seed. */
@Composable
private fun BackupWordReview(model: NewSeedViewModel, onCancel: () -> Unit, onComplete: () -> Unit) {
    var inspecting by remember(model.order, model.entries) { mutableStateOf<Int?>(null) }
    val ready = model.stage == NewSeedStage.Verified
    TaskPage("Review the words", onCancel, actions = {
        Text(model.message ?: "Retake your written backup.",
            color = if (ready) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.error,
            modifier = Modifier.testTag("backup-status"))
        Button(onClick = onComplete, enabled = ready,
            modifier = Modifier.fillMaxWidth().testTag("complete-new-wallet")) { Text("Continue to wallet") }
    }) {
        LazyColumn(Modifier.fillMaxSize().testTag("review-list")) {
            item {
                Column(Modifier.padding(horizontal = 24.dp, vertical = 12.dp)) {
                    val photo = model.photo
                    val frame = model.frame
                    if (photo != null && frame != null) {
                        PhotoWithBoxes(photo, frame.width, frame.height, model.entries, emptyList(),
                            null, emptyList(), null)
                    }
                    Text("Compare the photo with your generated phrase. Tap a word to inspect it. " +
                        "If a word is wrong, make its handwriting clearer on paper and retake, " +
                        "or acknowledge the misread after checking the paper.")
                    Text("Reading order (choose the layout on your paper)")
                    model.availableOrders.forEach { order ->
                        FilterChip(selected = model.order == order, onClick = { model.chooseOrder(order) },
                            modifier = Modifier.testTag("order-${order.name.lowercase()}"),
                            label = { Text(when (order) {
                                Order.NUMBERS -> "By detected numbers"
                                Order.COLUMNS -> "Down columns"
                                Order.ROWS -> "Across rows"
                            }) })
                    }
                }
            }
            itemsIndexed(model.entries) { i, entry ->
                WordRow(i + 1, entry, showCrop = false, onClick = { inspecting = i },
                    expectedWord = model.words.getOrNull(i) ?: "No word expected",
                    acknowledged = i in model.acknowledged)
            }
            item {
                Column(Modifier.padding(24.dp)) {
                    Button(onClick = model::retake, modifier = Modifier.testTag("backup-retake")) { Text("Retake photo") }
                    TextButton(onClick = model::showWords) { Text("Show the same generated phrase") }
                }
            }
        }
    }
    inspecting?.let { position ->
        val entry = model.entries.getOrNull(position) ?: return@let
        val expected = model.words.getOrNull(position)
        val wrong = entry.word != expected
        AlertDialog(onDismissRequest = { inspecting = null },
            title = { Text("Word ${position + 1}") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    entry.source?.let { Crop(it.cropPng, Modifier.fillMaxWidth().height(72.dp)) }
                    Text("Expected: ${expected ?: "No word expected"}")
                    Text("Photo read: ${entry.word}")
                    if (wrong) Text("Check that your paper says the expected word. Try making the handwriting " +
                        "clearer and retaking the photo. Acknowledging this misread does not change your seed.")
                }
            },
            confirmButton = {
                if (wrong && position !in model.acknowledged && model.readWords.size == 12) {
                    TextButton(onClick = { model.acknowledge(position); inspecting = null },
                        modifier = Modifier.testTag("acknowledge-word")) { Text("I checked the paper · acknowledge misread") }
                } else TextButton(onClick = { inspecting = null }) { Text("Close") }
            },
            dismissButton = { TextButton(onClick = { inspecting = null; model.retake() }) { Text("Retake photo") } })
    }
}
