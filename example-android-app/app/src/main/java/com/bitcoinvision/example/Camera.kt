package com.bitcoinvision.example

import android.Manifest
import android.content.Context
import android.graphics.ImageFormat
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.ui.draw.drawWithContent
import kotlinx.coroutines.delay
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.ImageProxy
import androidx.compose.foundation.background
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.PhotoCamera
import androidx.compose.material3.Button
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import java.util.concurrent.Executors

/**
 * The camera step both scanners share: the permission gate, the
 * preview, a bar over it with a back button and [bar]'s content, the
 * shutter and [overlay] on top. A tap on the shutter names a new
 * attempt through [onShutter] before the picture is taken, and the
 * camera's photo comes back through [onJpeg] with that name, null
 * when the camera delivered nothing; what is done with it, and the
 * screen that shows it being read, is the caller's.
 */
@Composable
fun CameraCapture(
    shutterEnabled: Boolean,
    onShutter: () -> Int,
    onJpeg: (Int, PhotoInput?) -> Unit,
    onExit: () -> Unit,
    bar: @Composable RowScope.() -> Unit,
    overlay: @Composable BoxScope.() -> Unit = {},
    instead: @Composable ColumnScope.() -> Unit = {},
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    var granted by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED,
        )
    }
    val permission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted = it }
    LaunchedEffect(Unit) {
        if (!granted) permission.launch(Manifest.permission.CAMERA)
    }
    // Granting from the system settings page happens outside the
    // launcher callback; coming back must notice it.
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) {
                granted = ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.CAMERA,
                ) == PackageManager.PERMISSION_GRANTED
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    if (!granted) {
        Column(
            Modifier
                .fillMaxSize()
                .statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState())
                .padding(24.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(
                "The camera reads what is on paper.",
                style = MaterialTheme.typography.bodyLarge,
            )
            Button(onClick = { permission.launch(Manifest.permission.CAMERA) }) {
                Text("Allow camera")
            }
            instead()
            // After a second refusal Android stops showing the dialog;
            // the settings page is the only way back in.
            TextButton(onClick = {
                context.startActivity(
                    Intent(
                        Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                        Uri.fromParts("package", context.packageName, null),
                    ),
                )
            }) { Text("Open app settings") }
            TextButton(onClick = onExit) { Text("Back") }
        }
        return
    }

    val session = remember(context) { DisplayCamera(context) }
    DisposableEffect(session, lifecycleOwner) {
        session.bind(lifecycleOwner)
        onDispose { session.close() }
    }

    Box(Modifier.fillMaxSize()) {
        AndroidView(
            factory = { session.view },
            modifier = Modifier.fillMaxSize(),
        )
        Row(
            Modifier
                .fillMaxWidth()
                // The preview behind can be any brightness; the bar
                // carries its own ground.
                .background(CameraScrim)
                .statusBarsPadding()
                .padding(12.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onExit) {
                Icon(
                    Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = "Back",
                    tint = Color.White,
                )
            }
            bar()
        }
        overlay()
        session.error?.let { Text(it, color = Color.White, modifier = Modifier.align(Alignment.Center)) }
        FilledIconButton(
            enabled = shutterEnabled && session.camera != null,
            onClick = {
                // A tap before the camera finishes initializing, or
                // while a photo is being read, is a shrug, not a crash.
                if (shutterEnabled) {
                    val token = onShutter()
                    try {
                        val displayRotation = session.shutterRotation()
                        val sensorToDisplay = session.camera?.cameraInfo?.getSensorRotationDegrees(displayRotation)
                        val preview = session.shutterPreview()
                        takePicture(session.capture, context, displayRotation, sensorToDisplay, preview) { onJpeg(token, it) }
                    } catch (_: IllegalStateException) {
                        onJpeg(token, null)
                    }
                }
            },
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .navigationBarsPadding()
                .padding(32.dp)
                .size(72.dp)
                .testTag("shutter"),
        ) {
            Icon(Icons.Filled.PhotoCamera, contentDescription = "Take photo")
        }
    }
}

// A single bounded callback worker; only attempt delivery returns to Main.
private val captureExecutor = Executors.newSingleThreadExecutor { Thread(it, "VisionCapture") }

private fun takePicture(
    capture: ImageCapture,
    context: Context,
    displayRotation: Int?,
    sensorToDisplay: Int?,
    previewAtShutter: CapturePreviewMetadata,
    onJpeg: (PhotoInput?) -> Unit,
) {
    val main = ContextCompat.getMainExecutor(context)
    capture.takePicture(
        captureExecutor,
        object : ImageCapture.OnImageCapturedCallback() {
            override fun onCaptureSuccess(image: ImageProxy) {
                val photo = copyCapturedPhoto(image, displayRotation, sensorToDisplay, previewAtShutter)
                photo?.let(::logPhotoInput)
                main.execute { onJpeg(photo) }
            }

            override fun onError(e: ImageCaptureException) {
                main.execute { onJpeg(null) }
            }
        },
    )
}

/** Always relinquishes CameraX's image, including malformed/unsupported deliveries. */
internal fun copyCapturedPhoto(
    image: ImageProxy,
    displayRotation: Int?,
    sensorToDisplay: Int?,
    previewAtShutter: CapturePreviewMetadata? = null,
): PhotoInput? =
    try {
        require(image.format == ImageFormat.JPEG) { "Expected JPEG capture" }
        val crop = image.cropRect
        val metadata = CameraPhotoMetadata(
            image.imageInfo.rotationDegrees, image.width, image.height, image.format,
            displayRotation, sensorToDisplay,
            CaptureBounds(crop.left, crop.top, crop.right, crop.bottom), previewAtShutter,
        )
        val buffer = image.planes[0].buffer
        val jpeg = ByteArray(buffer.remaining()).also { buffer.get(it) }
        PhotoInput.ownCameraBytes(jpeg, metadata)
    } catch (_: RuntimeException) {
        null
    } finally {
        image.close()
    }

/**
 * The photograph just taken, while it is read and after a read that
 * failed: the photo as the scanner sees it, a progress bar under it
 * that promises nothing it cannot measure, and the way back to the
 * camera at once, without waiting for the read. With [error], the
 * read is over and the photo stays for the person to judge.
 */
@Composable
fun CapturedPhotoScreen(
    title: String,
    photo: ResolvedPhoto?,
    error: String?,
    onRetake: () -> Unit,
    progress: ScanProgressState? = null,
    onProgressPresented: (Int, ULong) -> Unit = { _, _ -> },
    extra: @Composable ColumnScope.() -> Unit = {},
) {
    BackHandler { onRetake() }
    TaskPage(
        title = title,
        onBack = onRetake,
        actions = {
            if (error == null) {
                ScanProcessingIndicator(progress)
                TextButton(
                    onClick = onRetake,
                    modifier = Modifier.fillMaxWidth().testTag("cancel-retake"),
                ) { Text("Cancel and retake") }
            } else {
                Button(
                    onClick = onRetake,
                    modifier = Modifier.fillMaxWidth().testTag("retake"),
                ) { Text("Retake") }
            }
        },
    ) {
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 24.dp, vertical = 12.dp)
                .testTag("captured"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (photo != null && progress != null && error == null) {
                ScanProgressPhoto(photo, progress, onPresented = {
                    onProgressPresented(progress.token, progress.sequence)
                })
            } else if (photo != null) {
                // The shell, the bar and the way back render at once; the
                // photo follows when a worker has decoded it, and a decode
                // still under way when the attempt goes is dropped with it.
                val bitmap by rememberPhotoBitmap(photo)
                if (bitmap != null) {
                    Image(
                        bitmap!!.asImageBitmap(),
                        contentDescription = "The photo just taken",
                        contentScale = ContentScale.Fit,
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 420.dp)
                            .testTag("captured-photo"),
                    )
                }
            }
            if (photo == null && progress != null && error == null) {
                // Preparation has no preview yet, but still needs an actual displayed frame.
                Box(Modifier.fillMaxWidth().size(1.dp).drawWithContent {
                    drawContent()
                    onProgressPresented(progress.token, progress.sequence)
                })
            }
            if (error != null) {
                Text(
                    error,
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.error,
                    modifier = Modifier.testTag("read-error"),
                )
                extra()
            }
        }
    }
}

@Composable
private fun ScanProcessingIndicator(progress: ScanProgressState?) {
    val meter = remember(progress?.token) { ScanProgressMeter(progressClockMs()) }
    var fraction by remember(meter) { mutableStateOf(0.01f) }
    val current by androidx.compose.runtime.rememberUpdatedState(progress)
    LaunchedEffect(meter) {
        while (true) {
            fraction = meter.advance(current, progressClockMs())
            delay(50)
        }
    }
    LinearProgressIndicator(progress = { fraction },
        modifier = Modifier.fillMaxWidth().testTag("reading-progress")
            .semantics { stateDescription = "Estimated scan progress" })
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
        modifier = Modifier.padding(vertical = 8.dp)) {
        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
        Text(progress?.label ?: "Preparing to read", style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.testTag("scan-phase").semantics { liveRegion = LiveRegionMode.Polite })
    }
}
