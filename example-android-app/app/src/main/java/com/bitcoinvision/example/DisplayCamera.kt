package com.bitcoinvision.example

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Matrix
import android.graphics.RectF
import android.hardware.display.DisplayManager
import android.os.Handler
import android.os.Looper
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.FocusMeteringAction
import androidx.camera.core.ImageCapture
import androidx.camera.core.Preview
import androidx.camera.core.UseCaseGroup
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleOwner

/** Explicit use cases: no motion-sensor listener overrides the preview target. */
internal class DisplayCamera(private val context: Context) {
    // Set before the first ViewPort read: fitting is part of the bound camera geometry.
    val view = PreviewView(context).apply { scaleType = PreviewView.ScaleType.FIT_CENTER }
    val capture = ImageCapture.Builder()
        .setFlashMode(ImageCapture.FLASH_MODE_AUTO)
        .build()
    private val preview = Preview.Builder().build()
    var camera by mutableStateOf<Camera?>(null)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    private var provider: ProcessCameraProvider? = null
    private var owner: LifecycleOwner? = null
    private var active = false
    private var geometry: Triple<Int, Int, Int>? = null
    private val displays = context.getSystemService(DisplayManager::class.java)
    private val layout = View.OnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> bindIfReady() }
    private val displayListener = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) {}
        override fun onDisplayRemoved(displayId: Int) {}
        override fun onDisplayChanged(displayId: Int) {
            if (view.display?.displayId == displayId) bindIfReady()
        }
    }

    @SuppressLint("ClickableViewAccessibility")
    fun bind(lifecycleOwner: LifecycleOwner) {
        owner = lifecycleOwner
        active = true
        preview.setSurfaceProvider(view.surfaceProvider)
        view.addOnLayoutChangeListener(layout)
        displays.registerDisplayListener(displayListener, Handler(Looper.getMainLooper()))
        val scale = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
            override fun onScale(detector: ScaleGestureDetector): Boolean {
                val current = camera ?: return false
                val zoom = current.cameraInfo.zoomState.value ?: return false
                current.cameraControl.setZoomRatio(
                    (zoom.zoomRatio * detector.scaleFactor).coerceIn(zoom.minZoomRatio, zoom.maxZoomRatio),
                )
                return true
            }
        })
        val tap = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
            override fun onDown(e: MotionEvent): Boolean = true
            override fun onSingleTapUp(e: MotionEvent): Boolean {
                if (scale.isInProgress) return false
                val current = camera ?: return false
                if (!isInPreviewImage(view.outputTransform?.matrix, e.x, e.y)) return false
                val point = view.meteringPointFactory.createPoint(e.x, e.y)
                if (point.x !in 0f..1f || point.y !in 0f..1f) return false
                current.cameraControl.startFocusAndMetering(FocusMeteringAction.Builder(point).build())
                view.performClick()
                return true
            }
        })
        view.setOnTouchListener { _, event ->
            scale.onTouchEvent(event)
            tap.onTouchEvent(event)
            true
        }
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener({
            if (active) {
                try {
                    provider = future.get()
                    bindIfReady()
                } catch (_: Exception) {
                    error = "The camera could not start. Go back and try again."
                }
            }
        }, ContextCompat.getMainExecutor(context))
    }

    private fun bindIfReady() {
        if (!active) return
        val provider = provider ?: return
        val owner = owner ?: return
        val rotation = view.display?.rotation ?: return
        val viewport = view.viewPort ?: return
        val next = Triple(view.width, view.height, rotation)
        if (next == geometry) return
        try {
            setDisplayTarget(preview, capture, rotation)
            provider.unbind(preview, capture)
            camera = provider.bindToLifecycle(
                owner, CameraSelector.DEFAULT_BACK_CAMERA,
                UseCaseGroup.Builder().addUseCase(preview).addUseCase(capture).setViewPort(viewport).build(),
            )
            geometry = next
            error = null
        } catch (_: Exception) {
            camera = null
            error = "The camera could not start. Go back and try again."
        }
    }

    /** Snapshot in the same Main turn as takePicture, never at callback time. */
    fun shutterPreview(): CapturePreviewMetadata = CapturePreviewMetadata(
        CaptureBounds(view.left, view.top, view.right, view.bottom),
        view.scaleType.name,
        view.layoutDirection,
    )

    /** Keep the existing rotation authority; framing metadata does not change it. */
    fun shutterRotation(): Int {
        check(camera != null) { "Camera is not ready" }
        val rotation = checkNotNull(view.display).rotation
        setDisplayTarget(preview, capture, rotation)
        return rotation
    }

    fun close() {
        active = false
        view.removeOnLayoutChangeListener(layout)
        view.setOnTouchListener(null)
        displays.unregisterDisplayListener(displayListener)
        provider?.unbind(preview, capture)
        preview.setSurfaceProvider(null)
        camera = null
        owner = null
        geometry = null
    }
}

/** CameraX maps its normalized crop square to the actual visible image, including letterboxing. */
internal fun isInPreviewImage(transform: Matrix?, x: Float, y: Float): Boolean {
    if (transform == null || !x.isFinite() || !y.isFinite()) return false
    val image = RectF(-1f, -1f, 1f, 1f)
    transform.mapRect(image)
    return image.contains(x, y)
}

internal fun setDisplayTarget(preview: Preview, capture: ImageCapture, displayRotation: Int) {
    preview.targetRotation = displayRotation
    capture.targetRotation = displayRotation
}
