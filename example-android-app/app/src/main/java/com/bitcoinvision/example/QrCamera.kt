package com.bitcoinvision.example

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.LifecycleOwner
import uniffi.bitcoin_vision_mobile.PsbtQrDecoder
import uniffi.bitcoin_vision_mobile.QrScan
import java.util.concurrent.Executors

/**
 * The viewfinder that reads a PSBT off a screen: single or animated
 * `ur:crypto-psbt`, with a ring for the animated kind. [onPsbt] fires
 * once per visit, on the main thread, and never after the person has
 * left: the session is retired before Back navigates, when the page
 * pauses (another PSBT arriving, the app going behind), and on disposal
 * as the fallback.
 */
@Composable
fun QrScanScreen(title: String, onPsbt: (ByteArray) -> Unit, onExit: () -> Unit) {
    CameraPermissionGate(onExit) {
        val context = LocalContext.current
        val lifecycleOwner = LocalLifecycleOwner.current
        val main = remember(context) { ContextCompat.getMainExecutor(context) }
        val deliver by rememberUpdatedState(onPsbt)
        // A paused page's session is spent; coming back starts a fresh one.
        var generation by remember { mutableIntStateOf(0) }
        var state by remember(generation) { mutableStateOf(QrScanState()) }
        val controller = remember(generation) {
            val decoder = PsbtQrDecoder()
            QrScanController(
                decode = { frame ->
                    decoder.feedLuma(frame.width.toUInt(), frame.height.toUInt(), frame.rowStride.toUInt(), frame.luma)
                },
                release = { decoder.close() },
                post = { main.execute(it) },
                onScan = { scan ->
                    state = state.after(scan)
                    if (scan is QrScan.Decoded) deliver(scan.psbt)
                },
            )
        }
        val current by rememberUpdatedState(controller)
        val exit = {
            controller.retire()
            onExit()
        }
        BackHandler(onBack = exit)
        DisposableEffect(lifecycleOwner, controller) {
            val observer = LifecycleEventObserver { _, event ->
                when (event) {
                    Lifecycle.Event.ON_PAUSE -> controller.retire()
                    Lifecycle.Event.ON_RESUME -> if (controller.retired) generation += 1
                    else -> {}
                }
            }
            lifecycleOwner.lifecycle.addObserver(observer)
            onDispose {
                lifecycleOwner.lifecycle.removeObserver(observer)
                controller.retire()
            }
        }
        val session = remember(context) { QrCameraSession(context) }
        DisposableEffect(session, lifecycleOwner) {
            session.start(lifecycleOwner) { frame -> current.offer(frame) }
            onDispose { session.close() }
        }
        Box(Modifier.fillMaxSize()) {
            AndroidView(factory = { session.view }, modifier = Modifier.fillMaxSize().testTag("qr-preview"))
            Row(
                Modifier.fillMaxWidth().background(CameraScrim).statusBarsPadding().padding(12.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = exit) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back", tint = Color.White)
                }
                Text(title, style = MaterialTheme.typography.titleMedium, color = Color.White)
            }
            session.error?.let { Text(it, color = Color.White, modifier = Modifier.align(Alignment.Center)) }
            ScanRing(state, Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(32.dp))
        }
    }
}

@Composable
private fun ScanRing(state: QrScanState, modifier: Modifier) {
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (state.partsExpected > 0) {
            CircularProgressIndicator(
                progress = { state.fraction },
                color = Color.White,
                trackColor = CameraScrim,
                modifier = Modifier.size(72.dp).testTag("qr-progress"),
            )
            Text(
                "${state.partsSeen} of ${state.partsExpected} parts",
                color = Color.White,
                style = MaterialTheme.typography.labelLarge.copy(fontFamily = PlexMono),
                modifier = Modifier.testTag("qr-parts"),
            )
        }
        state.notice?.let {
            Text(
                it, color = Color.White, style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.background(CameraScrim).padding(horizontal = 12.dp, vertical = 8.dp)
                    .semantics { liveRegion = LiveRegionMode.Polite }.testTag("qr-notice"),
            )
        }
    }
}

// The same gate CameraCapture carries inline; that file is mid-change, so
// the two fold together once it lands.
@Composable
private fun CameraPermissionGate(onExit: () -> Unit, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val check = {
        ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
    }
    var granted by remember { mutableStateOf(check()) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    LaunchedEffect(Unit) { if (!granted) permission.launch(Manifest.permission.CAMERA) }
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) granted = check() }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }
    if (granted) {
        content()
        return
    }
    Column(
        Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("The camera reads the code on Sparrow's screen.", style = MaterialTheme.typography.bodyLarge)
        Button(onClick = { permission.launch(Manifest.permission.CAMERA) }) { Text("Allow camera") }
        TextButton(onClick = {
            context.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)),
            )
        }) { Text("Open app settings") }
        TextButton(onClick = onExit) { Text("Back") }
    }
}

/** Preview plus analysis on the back camera; every frame is closed, whatever happens to it. */
internal class QrCameraSession(private val context: Context) {
    val view = PreviewView(context).apply { scaleType = PreviewView.ScaleType.FIT_CENTER }
    private val preview = Preview.Builder().build()
    private val analysis = ImageAnalysis.Builder()
        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
        .build()
    private val executor = Executors.newSingleThreadExecutor { Thread(it, "VisionQr") }
    var error by mutableStateOf<String?>(null)
        private set
    private var provider: ProcessCameraProvider? = null
    private var active = false

    fun start(owner: LifecycleOwner, onFrame: (QrFrame) -> Unit) {
        active = true
        preview.setSurfaceProvider(view.surfaceProvider)
        analysis.setAnalyzer(executor) { image ->
            try {
                image.lumaFrame()?.let(onFrame)
            } catch (_: Exception) {
                // A malformed frame is a dropped frame; the worker must outlive it.
            } finally {
                image.close()
            }
        }
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            if (!active) return@addListener
            try {
                val provider = future.get()
                this.provider = provider
                provider.unbind(preview, analysis)
                provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
                error = null
            } catch (_: Exception) {
                error = "The camera could not start. Go back and try again."
            }
        }, ContextCompat.getMainExecutor(context))
    }

    fun close() {
        active = false
        analysis.clearAnalyzer()
        provider?.unbind(preview, analysis)
        preview.setSurfaceProvider(null)
        executor.shutdown()
    }
}

/** The Y plane inside the crop rect, packed to one byte per pixel; null when the buffer is short. */
internal fun ImageProxy.lumaFrame(): QrFrame? {
    val plane = planes.firstOrNull() ?: return null
    val buffer = plane.buffer.duplicate()
    val crop = cropRect
    val width = crop.width()
    val height = crop.height()
    if (width <= 0 || height <= 0) return null
    val pixelStride = plane.pixelStride
    val rowBytes = (width - 1) * pixelStride + 1
    val last = (crop.top + height - 1).toLong() * plane.rowStride + crop.left.toLong() * pixelStride + rowBytes
    if (last > buffer.limit()) return null
    val out = ByteArray(width * height)
    val row = ByteArray(rowBytes)
    for (y in 0 until height) {
        buffer.position((crop.top + y) * plane.rowStride + crop.left * pixelStride)
        buffer.get(row, 0, rowBytes)
        if (pixelStride == 1) {
            System.arraycopy(row, 0, out, y * width, width)
        } else {
            for (x in 0 until width) out[y * width + x] = row[x * pixelStride]
        }
    }
    return QrFrame(width, height, width, out)
}
