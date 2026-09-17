package com.bitcoinvision.example

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.delay
import uniffi.bitcoin_vision_mobile.PsbtQrEncoder
import uniffi.bitcoin_vision_mobile.QrModules
import uniffi.bitcoin_vision_mobile.qrModules

// Scanners want contrast, not the app's palette: ink on paper in every theme.
private val QrPaper = Color.White
private val QrInk = Color(0xFF17181A)
private const val QUIET_ZONE = 4

/** Frames of an animated code; the rate frostsnap and Sparrow's own display use. */
const val QR_FRAME_MS = 100L

/** A code drawn edge to edge in a square, with its quiet zone. */
@Composable
fun QrView(modules: QrModules, description: String, modifier: Modifier = Modifier) {
    val n = modules.size.toInt()
    Canvas(modifier.aspectRatio(1f).semantics { contentDescription = description }) {
        drawRect(QrPaper)
        val cell = size.minDimension / (n + 2 * QUIET_ZONE)
        // A hair of overlap keeps hairline gaps from appearing between modules at odd scales.
        val module = Size(cell + 0.5f, cell + 0.5f)
        for (y in 0 until n) {
            for (x in 0 until n) {
                if (modules.dark[y * n + x]) {
                    drawRect(QrInk, topLeft = Offset((x + QUIET_ZONE) * cell, (y + QUIET_ZONE) * cell), size = module)
                }
            }
        }
    }
}

/** The encoder's parts shown in turn while the screen is resumed; a single part stays still. */
@Composable
fun AnimatedQr(encoder: PsbtQrEncoder, description: String, modifier: Modifier = Modifier) {
    var modules by remember(encoder) { mutableStateOf(qrModules(encoder.nextPart())) }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var resumed by remember { mutableStateOf(lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, _ -> resumed = lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED) }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    if (resumed && encoder.partCount() > 1u) {
        LaunchedEffect(encoder) {
            while (true) {
                delay(QR_FRAME_MS)
                modules = qrModules(encoder.nextPart())
            }
        }
    }
    QrView(modules, description, modifier)
}

/** A static code for `text`, or nothing when it is too long for one. */
@Composable
fun StaticQr(text: String, description: String, tag: String, modifier: Modifier = Modifier) {
    val modules = remember(text) { runCatching { qrModules(text) }.getOrNull() } ?: return
    QrView(modules, description, modifier.testTag(tag))
}
