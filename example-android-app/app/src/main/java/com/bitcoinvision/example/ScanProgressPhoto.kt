package com.bitcoinvision.example

import android.graphics.Bitmap
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import uniffi.bitcoin_vision_mobile.ScanRegionState as RegionState

/** No live labels or confidence colours: only geometry and native processing state. */
@Composable
internal fun ScanProgressPhoto(
    photo: ResolvedPhoto,
    progress: ScanProgressState,
    bitmap: Bitmap? = rememberPhotoBitmap(photo).value,
    onPresented: () -> Unit = {},
) {
    val image = bitmap ?: return // Never draw boxes over a missing or previous preview.
    val colours = MaterialTheme.colorScheme
    val photoHeightLimit = 420.dp / LocalDensity.current.fontScale.coerceAtLeast(1f)
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val height = minOf(photoHeightLimit, maxWidth * photo.height / photo.width)
        Canvas(Modifier.fillMaxWidth().height(height).background(colours.surfaceContainer)
            .testTag("captured-photo").semantics { contentDescription = "The photo just taken" }) {
            val fit = fitPhoto(PhotoFrame(photo.width, photo.height), size.width, size.height) ?: return@Canvas
            drawImage(image.asImageBitmap(), dstOffset = IntOffset(fit.left, fit.top),
                dstSize = IntSize(fit.width, fit.height))
            if (progressMatchesPhoto(progress, photo)) for (region in progress.regions.values) {
                if (region.state == RegionState.EXCLUDED) continue
                val path = Path().apply {
                    for (k in 0..3) {
                        val x = fit.x(region.corners[2 * k])
                        val y = fit.y(region.corners[2 * k + 1])
                        if (k == 0) moveTo(x, y) else lineTo(x, y)
                    }
                    close()
                }
                val reading = region.state == RegionState.READING
                val read = region.state == RegionState.READ
                val width = if (reading) 3.dp.toPx() else 1.5.dp.toPx()
                val dash = if (region.state == RegionState.FOUND)
                    PathEffect.dashPathEffect(floatArrayOf(6.dp.toPx(), 4.dp.toPx())) else null
                if (read) drawPath(path, colours.primary.copy(alpha = 0.12f))
                // A surface halo remains visible on light or dark paper. Stronger
                // double outline / dashed / filled shapes also work without colour.
                drawPath(path, colours.surface.copy(alpha = 0.9f), style = Stroke(width + 2.dp.toPx(), pathEffect = dash))
                drawPath(path, if (reading || read) colours.primary else colours.onSurfaceVariant,
                    style = Stroke(width, pathEffect = dash))
            }
            // Only acknowledge after the preview and this snapshot's boxes were drawn.
            onPresented()
        }
    }
}
