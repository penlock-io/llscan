package com.bitcoinvision.example

import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.platform.LocalView

/** Optional numeric event sink for the separate fixture benchmark. Normal
 * builds do not call a sink, read a clock, or add a draw modifier. The sink
 * returns true only for its first observation of a marker. */
internal object ScanTrace {
    @Volatile var sink: ((String) -> Boolean)? = null

    fun mark(name: String) {
        if (BuildConfig.SCAN_BENCHMARK) sink?.invoke(name)
    }
}

/** Observe a frame that actually contains the review list. Frame commit means
 * rendered/submitted, not physical display presentation or completed animation.
 * Capture this attempt's sink so a late callback cannot mark a later scan. */
internal fun Modifier.traceReviewFrame(): Modifier = if (!BuildConfig.SCAN_BENCHMARK) this else composed {
    val view = LocalView.current
    drawWithContent {
        val sink = ScanTrace.sink
        val first = sink?.invoke("review_draw_start") == true
        if (first) {
            check(view.isHardwareAccelerated) { "Frame timing requires hardware rendering" }
            view.viewTreeObserver.registerFrameCommitCallback { sink?.invoke("review_frame_committed") }
        }
        drawContent()
        if (first) sink?.invoke("review_draw_end")
    }
}
