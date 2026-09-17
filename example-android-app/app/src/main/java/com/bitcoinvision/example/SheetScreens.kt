package com.bitcoinvision.example

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import kotlinx.coroutines.launch
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Photo
import androidx.compose.material.icons.outlined.Photo
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.SuggestionChip
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import uniffi.bitcoin_vision_mobile.Layout
import uniffi.bitcoin_vision_mobile.DetectionSource
import uniffi.bitcoin_vision_mobile.listWords
import java.util.Locale

// The verdict colours: the same on both themes, since they carry
// meaning the theme must not restyle.
private val Accepted = Color(0xFF2E7D32)
private val Uncertain = Color(0xFFF9A825)
private val LeftOut = Color(0xFF9E9E9E)
private val LowConfidence = Color(0xFFB3261E)

/** One palette for row confidence and the corresponding polygon on the photo. */
internal fun confidenceColour(entry: Entry): Color = when {
    entry.modelConfidence == null -> LeftOut
    entry.modelConfidence!! >= 0.85f -> Accepted
    entry.modelConfidence!! >= 0.5f -> Uncertain
    else -> LowConfidence
}

@Composable
fun SheetScanScreen(
    viewModel: SheetViewModel,
    onReviewed: () -> Unit,
    onExit: () -> Unit,
    // The fixture route reads a directory only adb can fill. Everything
    // else a debug build offers is as useful on a phone as here.
    fixtureRoute: Boolean = BuildConfig.DEBUG && onEmulator,
) {
    val context = LocalContext.current
    // The read ends in the view model; this screen takes the one
    // navigation it asks for and clears the request.
    LaunchedEffect(viewModel.reviewed) {
        if (viewModel.reviewed) {
            viewModel.reviewed = false
            ScanTrace.mark("review_navigation")
            onReviewed()
        }
    }
    when (val attempt = viewModel.attempts.current) {
        is Attempt.Preparing -> CapturedPhotoScreen(
            title = "Preparing your photo", photo = null, error = null,
            onRetake = { viewModel.retake() },
            progress = viewModel.progress?.takeIf { it.token == attempt.token },
            onProgressPresented = viewModel::progressPresented,
        )
        is Attempt.Processing -> CapturedPhotoScreen(
            title = "Reading your photo",
            photo = attempt.photo,
            error = null,
            onRetake = { viewModel.retake() },
            progress = viewModel.progress?.takeIf { it.token == attempt.token },
            onProgressPresented = viewModel::progressPresented,
        )
        is Attempt.Failed -> CapturedPhotoScreen(
            title = "No words read",
            photo = attempt.photo,
            error = attempt.message,
            onRetake = { viewModel.retake() },
        ) {
            SavePhotoForDiagnosis { viewModel.savePhoto() }
            if (attempt.photo != null) SavePhotoForDiagnosis(canonical = true) { viewModel.saveCanonicalPhoto() }
        }
        else -> {
            BackHandler { onExit() }
            var noFixture by remember { mutableStateOf(false) }
            val fromFile: () -> Unit = {
                ScanTrace.mark("from_file_click")
                val page = latestPage(context)
                noFixture = page == null
                page?.let {
                    ScanTrace.mark("photo_read")
                    viewModel.deliver(viewModel.begin(), it)
                }
            }
            val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
                // A cancelled picker is not a failed scan: nothing was chosen.
                uri?.let(viewModel::deliverDocument)
            }
            val choosePhoto = { picker.launch(arrayOf("image/*")) }
            if (BuildConfig.SCAN_BENCHMARK) {
                // The isolated benchmark has no camera permission and only
                // exercises the existing From-file delivery path.
                TaskPage(title = "Fixture input", onBack = onExit, actions = {}) {
                    Button(onClick = fromFile, modifier = Modifier.testTag("from-file")) {
                        Text("From file")
                    }
                    if (noFixture) EmptyFixtureNote()
                }
            } else CameraCapture(
                shutterEnabled = attempt == null,
                onShutter = { viewModel.begin() },
                onJpeg = { token, photo -> viewModel.deliverPhoto(token, photo) },
                onExit = onExit,
                bar = {
                    Text(
                        "1 of 2 · Take the photo",
                        style = MaterialTheme.typography.titleMedium,
                        color = Color.White,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(onClick = choosePhoto, modifier = Modifier.testTag("choose-file")) {
                        Text("Choose photo", color = Color.White)
                    }
                    if (BuildConfig.DEBUG) {
                        TextButton(onClick = viewModel::toggleDecisionTrace, modifier = Modifier.testTag("trace-next-scan")) {
                            Text(if (viewModel.traceNextScan) "Trace next: on" else "Trace next: off", color = Color.White)
                        }
                    }
                    // The emulator's camera cannot resolve a written page; the
                    // gates hand a photo in through the app's files directory.
                    if (fixtureRoute) {
                        TextButton(
                            onClick = fromFile,
                            modifier = Modifier.testTag("from-file"),
                        ) { Text("From file", color = Color.White) }
                    }
                },
                instead = {
                    // Refusing the camera leaves a photo already on the phone
                    // as the only way in, so it cannot live behind the preview.
                    Button(onClick = choosePhoto, modifier = Modifier.testTag("choose-file")) {
                        Text("Choose a photo")
                    }
                },
                overlay = {
                    Guidance()
                    if (noFixture) {
                        Box(Modifier.align(Alignment.TopCenter).statusBarsPadding().padding(top = 64.dp)) {
                            EmptyFixtureNote()
                        }
                    }
                },
            )
        }
    }
}

@Composable
private fun BoxScope.Guidance() {
    Column(
        Modifier
            .align(Alignment.BottomCenter)
            .navigationBarsPadding()
            .padding(bottom = 128.dp)
            .padding(horizontal = 24.dp)
            .background(CameraScrim)
            .padding(12.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            "Lay the sheet flat, every word in the frame, in good light. " +
                "A printed recovery phrase sheet is read field by field; any other page word by word.",
            style = MaterialTheme.typography.bodyMedium,
            color = Color.White,
        )
    }
}

// A bad read is diagnosed on the desk, from the photo the phone took:
// a debug build can write it where adb pulls from. A release build
// never writes a phrase's photo anywhere, so it gets no button.
@Composable
internal fun SavePhotoForDiagnosis(canonical: Boolean = false, save: suspend () -> String?) {
    if (!BuildConfig.DEBUG) return
    var saved by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    TextButton(onClick = {
        saving = true
        scope.launch {
            try {
                saved = save() ?: "Could not save photo"
            } finally {
                saving = false
            }
        }
    }, enabled = !saving, modifier = Modifier.testTag(if (canonical) "save-canonical-photo" else "save-photo")) {
        Text(if (canonical) "Save upright photo for diagnosis" else "Save raw photo + metadata for diagnosis")
    }
    saved?.let {
        Text(
            it,
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.testTag("saved-photo"),
        )
    }
}

/**
 * The fixture route reads a directory only `adb` can fill. Doing
 * nothing when it is empty is what made the button look dead.
 */
@Composable
internal fun EmptyFixtureNote() {
    Text(
        "No photo in the app's files/pages directory.",
        style = MaterialTheme.typography.bodySmall,
        color = Color.White,
        modifier = Modifier
            .background(CameraScrim)
            .padding(8.dp)
            .testTag("from-file-empty"),
    )
}

private fun latestPage(context: Context): ByteArray? =
    context.getExternalFilesDir("pages")
        ?.listFiles()
        ?.filter { it.isFile }
        ?.maxByOrNull { it.lastModified() }
        ?.readBytes()

/** What the edit sheet is open for. */
private sealed interface Editing {
    data class Word(val id: Int) : Editing

    /** A word typed in at entry `at`; with `number`, filling that gap in the page's run. */
    data class Insert(val at: Int, val number: Int? = null) : Editing
}

/** State removal never navigates a composing or outgoing page by itself. */
@Composable
private fun EndedWordReview(onBack: () -> Unit) {
    TaskPage(
        title = "Word review",
        onBack = onBack,
        actions = {
            Button(onClick = onBack, modifier = Modifier.fillMaxWidth().testTag("return-ended-review")) {
                Text("Return")
            }
        },
    ) {
        Text("This scan is no longer available.", modifier = Modifier.padding(24.dp).testTag("review-ended"))
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ReviewScreen(
    viewModel: SheetViewModel,
    action: String,
    onAccept: () -> Unit,
    onBack: () -> Unit,
) {
    BackHandler { onBack() }
    val draft = viewModel.draft
    val scan = viewModel.scan
    val photo = viewModel.photo
    if (draft == null || scan == null || photo == null) {
        EndedWordReview(onBack)
        return
    }
    var editing by remember { mutableStateOf<Editing?>(null) }
    var showLeftOut by remember { mutableStateOf(false) }
    var highlightedSource by remember(scan) { mutableStateOf<UInt?>(null) }
    val unreadSources = remember(scan) { scan.sources.filter { it.unread } }
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val passes = viewModel.passes()
    TaskPage(
        title = "Review the words",
        onBack = onBack,
        bar = {
            IconButton(
                onClick = {
                    scope.launch { list.scrollToItem(0) }
                },
                modifier = Modifier.testTag("toggle-photo")
                    .semantics { stateDescription = "Expanded" },
            ) {
                Icon(
                    Icons.Filled.Photo,
                    contentDescription = "Go to the source photo",
                )
            }
        },
        actions = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    viewModel.status(),
                    style = MaterialTheme.typography.bodyMedium,
                    color = if (passes) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.error,
                    modifier = Modifier.weight(1f).testTag("status")
                        .semantics { liveRegion = LiveRegionMode.Polite },
                )
            }
            Button(
                onClick = onAccept,
                enabled = passes,
                modifier = Modifier.fillMaxWidth().testTag("accept"),
            ) { Text(action) }
        },
    ) {
        LazyColumn(
            Modifier
                .fillMaxSize()
                .testTag("review-list")
                .traceReviewFrame(),
            state = list,
        ) {
            item {
                Column(Modifier.padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    run {
                        Spacer(Modifier.height(8.dp))
                        PhotoWithBoxes(
                            photo, scan.width.toInt(), scan.height.toInt(),
                            draft.entries, draft.reviewLeftOut, viewModel.highlighted,
                            unreadSources, highlightedSource,
                        )
                        val selected = draft.entries.indexOfFirst { it.id == viewModel.highlighted }
                        val source = unreadSources.firstOrNull { it.detectorId == highlightedSource }
                        if (source != null) {
                            Text(source.reviewDescription, modifier = Modifier.testTag("photo-source-selection"))
                            Text("No separate word was read here. If a word is missing, use + at its place in the list to type it.",
                                style = MaterialTheme.typography.bodyMedium)
                        } else if (selected >= 0) {
                            Text("Highlighted: word ${selected + 1}", modifier = Modifier.testTag("photo-selection"))
                        }
                    }
                    // a word sheet's fields have one order; a page has two,
                    // and a third when its numbers were read as a run
                    if (scan.layout == Layout.PAGE) {
                        Spacer(Modifier.height(4.dp))
                        val orders = Order.entries.filter { it != Order.NUMBERS || draft.numbered }
                        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                            orders.forEachIndexed { i, order ->
                                SegmentedButton(
                                    selected = draft.order == order,
                                    onClick = { viewModel.reorder(order) },
                                    shape = SegmentedButtonDefaults.itemShape(i, orders.size),
                                    modifier = Modifier.testTag("order-${order.name.lowercase()}"),
                                ) {
                                    Text(
                                        when (order) {
                                            Order.NUMBERS -> "By number"
                                            Order.COLUMNS -> "Down columns"
                                            Order.ROWS -> "Across rows"
                                        },
                                    )
                                }
                            }
                        }
                        numberingNote(draft.numbers, draft.listNumbered)?.let { note ->
                            Text(
                                note,
                                style = MaterialTheme.typography.bodyMedium,
                                color = if (draft.numbers is Numbers.Inconsistent) MaterialTheme.colorScheme.error
                                else MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.testTag("numbering"),
                            )
                        }
                    }
                }
            }
            val missingAt = draft.missingAt()
            itemsIndexed(draft.entries, key = { _, e -> e.id }) { i, entry ->
                InsertRow(i, missing = missingAt[i], onInsert = { editing = Editing.Insert(i, missingAt[i]) })
                WordRow(
                    number = i + 1,
                    entry = entry,
                    showCrop = false,
                    // by number, a row is named by the number written beside it
                    label = if (draft.order == Order.NUMBERS) entry.number?.let { "$it." } ?: "–" else "${i + 1}.",
                    onClick = {
                        highlightedSource = null
                        viewModel.highlighted = entry.source?.let { entry.id }
                        editing = Editing.Word(entry.id)
                    },
                )
            }
            item {
                InsertRow(
                    draft.entries.size,
                    missing = missingAt[draft.entries.size],
                    onInsert = { editing = Editing.Insert(draft.entries.size, missingAt[draft.entries.size]) },
                )
            }
            if (draft.reviewLeftOut.isNotEmpty() || unreadSources.isNotEmpty()) {
                item {
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 24.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            listOfNotNull(
                                leftOutLine(draft).takeIf { draft.reviewLeftOut.isNotEmpty() },
                                "${unreadSources.size} other detected regions".takeIf { unreadSources.isNotEmpty() },
                            ).joinToString(" · "),
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            modifier = Modifier
                                .weight(1f)
                                .testTag("left-out"),
                        )
                        TextButton(
                            onClick = { showLeftOut = !showLeftOut },
                            modifier = Modifier.testTag("show-left-out"),
                        ) { Text(if (showLeftOut) "Hide them" else "Show them") }
                    }
                }
                if (showLeftOut) {
                    itemsIndexed(draft.reviewLeftOut, key = { _, e -> "out-${e.id}" }) { i, entry ->
                        LeftOutRow(i, entry, draft.restoreLabel(entry.id), onKeep = { viewModel.restore(entry.id) })
                    }
                    itemsIndexed(unreadSources, key = { _, s -> "raw-${s.detectorId}" }) { _, source ->
                        Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp)
                            .testTag("unread-source-${source.detectorId}")) {
                            Text(source.reviewDescription, style = MaterialTheme.typography.bodyMedium)
                            Text("Not read as a separate word", style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant)
                            TextButton(onClick = {
                                highlightedSource = source.detectorId
                                viewModel.highlighted = source.owner?.wordIndex?.toInt()
                                scope.launch { list.scrollToItem(0) }
                            }, modifier = Modifier.testTag("locate-source-${source.detectorId}")) {
                                Text("Show on photo")
                            }
                        }
                    }
                }
            }
            item {
                // the desk's tool, at the end and on debug builds only
                Column(Modifier.padding(horizontal = 24.dp)) {
                    SavePhotoForDiagnosis { viewModel.savePhoto() }
                    SavePhotoForDiagnosis(canonical = true) { viewModel.saveCanonicalPhoto() }
                }
                Spacer(Modifier.height(24.dp))
            }
        }
    }
    editing?.let { what ->
        EditSheet(
            viewModel,
            what,
            onLocate = {
                editing = null
                scope.launch { list.scrollToItem(0) }
            },
            onDismiss = {
                editing = null
                viewModel.highlighted = null
            },
        )
    }
}

private fun leftOutLine(draft: PhraseDraft): String {
    val alternatives = draft.reviewLeftOut.count { draft.isAlternative(it.id) }
    val leftOut = draft.reviewLeftOut.filterNot { draft.isAlternative(it.id) }
    val apart = leftOut.count { it.source?.apart == true }
    val strays = leftOut.count { it.source?.stray == true } - apart
    val removed = leftOut.size - strays - apart
    fun boxes(n: Int) = "$n ${if (n == 1) "box" else "boxes"}"
    return listOfNotNull(
        alternatives.takeIf { it > 0 }?.let { "${boxes(it)} kept as alternatives" },
        strays.takeIf { it > 0 }?.let { "${boxes(it)} left out as not words" },
        apart.takeIf { it > 0 }?.let { "${boxes(it)} left out as far from the words" },
        removed.takeIf { it > 0 }?.let { "$it ${if (it == 1) "word" else "words"} removed" },
    ).joinToString(" · ")
}

/** A box left out: what the recogniser read and what the word model
 * made of it, and a way back into the phrase. */
@Composable
private fun LeftOutRow(position: Int, entry: Entry, keepLabel: String, onKeep: () -> Unit) {
    val source = entry.source ?: return
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 24.dp, vertical = 8.dp)
            .testTag("left-out-$position"),
    ) {
        // The action gets its own line. Beside the fixed-width crop it used to
        // consume all remaining width at large fonts, hiding the source reading.
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Crop(source.cropPng, Modifier.height(36.dp).width(96.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    if (source.raw.isBlank()) "nothing read" else "read \"${source.raw}\"",
                    style = MaterialTheme.typography.bodyMedium,
                    fontFamily = PlexMono,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.testTag("left-out-$position-read"),
                )
                Text(
                    "as a word: ${entry.word}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.testTag("left-out-$position-word"),
                )
            }
        }
        TextButton(onClick = onKeep, modifier = Modifier.align(Alignment.End).testTag("keep-$position")) {
            Text(keepLabel, modifier = Modifier.testTag("keep-$position-label"))
        }
    }
}

/**
 * The photo fitted into a fixed height, every entry's box drawn over
 * it in its verdict's colour, the highlighted one heavier.
 */
@Composable
internal fun PhotoWithBoxes(
    photo: ResolvedPhoto,
    width: Int,
    height: Int,
    entries: List<Entry>,
    leftOut: List<Entry>,
    highlighted: Int?,
    unreadSources: List<DetectionSource>,
    highlightedSource: UInt?,
) {
    val bitmap by rememberPhotoBitmap(photo)
    val primary = MaterialTheme.colorScheme.primary
    var wholePhoto by remember(photo) { mutableStateOf(false) }
    val relevant = entries.mapNotNull { it.source?.corners } +
        leftOut.filter { it.id == highlighted }.mapNotNull { it.source?.corners } +
        unreadSources.filter { it.detectorId == highlightedSource }.map { it.corners }
    val bounds = if (wholePhoto) ReviewPhotoBounds(0, 0, width, height)
        else reviewPhotoBounds(PhotoFrame(width, height), relevant)
    Column {
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(300.dp)
                .background(Color.Black)
                .testTag("photo")
                .semantics { contentDescription = if (wholePhoto) "Whole source photo with word confidence boxes"
                    else "Word area of source photo with word confidence boxes" },
        ) {
            val image = bitmap ?: return@Canvas
            val fit = fitPhoto(PhotoFrame(bounds.width, bounds.height), size.width, size.height) ?: return@Canvas
            // The decoded preview can be downsampled; geometry remains in canonical pixels.
            val sx = image.width.toFloat() / width
            val sy = image.height.toFloat() / height
            val left = (bounds.left * sx).toInt().coerceIn(0, image.width - 1)
            val top = (bounds.top * sy).toInt().coerceIn(0, image.height - 1)
            val right = ((bounds.left + bounds.width) * sx).toInt().coerceIn(left + 1, image.width)
            val bottom = ((bounds.top + bounds.height) * sy).toInt().coerceIn(top + 1, image.height)
            drawImage(image.asImageBitmap(), srcOffset = IntOffset(left, top), srcSize = IntSize(right - left, bottom - top),
                dstOffset = IntOffset(fit.left, fit.top), dstSize = IntSize(fit.width, fit.height))
            clipRect(fit.left.toFloat(), fit.top.toFloat(), (fit.left + fit.width).toFloat(), (fit.top + fit.height).toFloat()) {
                // Source geometry is not a word reading: show its own polygon, never
                // enlarge a parent's old crop or invent classifier/confirmation state.
                for (source in unreadSources) {
                    if (source.corners.size != 8) continue
                    val path = Path()
                    for (k in 0 until 4) {
                        val x = fit.x(source.corners[2 * k] - bounds.left); val y = fit.y(source.corners[2 * k + 1] - bounds.top)
                        if (k == 0) path.moveTo(x, y) else path.lineTo(x, y)
                    }
                    path.close()
                    val selected = source.detectorId == highlightedSource
                    drawPath(path, if (selected) primary else LeftOut,
                        style = Stroke(width = if (selected) 5.dp.toPx() else 1.dp.toPx()))
                }
                for ((entry, out) in entries.map { it to false } + leftOut.map { it to true }) {
                    val source = entry.source ?: continue
                    if (source.corners.size != 8 || source.corners.any { !it.isFinite() }) continue
                    val path = Path()
                    for (k in 0 until 4) {
                        val x = fit.x(source.corners[2 * k] - bounds.left)
                        val y = fit.y(source.corners[2 * k + 1] - bounds.top)
                        if (k == 0) path.moveTo(x, y) else path.lineTo(x, y)
                    }
                    path.close()
                    val colour = when {
                        out -> LeftOut
                        else -> confidenceColour(entry)
                    }
                    val stroke = if (entry.id == highlighted) 5.dp.toPx() else if (out) 1.dp.toPx() else 2.dp.toPx()
                    drawPath(path, colour, style = Stroke(width = stroke))
                }
            }
        }
        TextButton(onClick = { wholePhoto = !wholePhoto }, modifier = Modifier.testTag("photo-context")) {
            Text(if (wholePhoto) "Show word area" else "Show whole photo")
        }
    }
}

@Composable
private fun InsertRow(at: Int, onInsert: () -> Unit, missing: Int? = null) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 24.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (missing != null) {
            Text(
                "Word $missing was not found: insert it here",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.error,
                modifier = Modifier.testTag("missing-$missing"),
            )
        }
        HorizontalDivider(Modifier.weight(1f))
        IconButton(
            onClick = onInsert,
            modifier = Modifier
                .size(48.dp)
                .testTag("insert-$at"),
        ) {
            Icon(
                Icons.Filled.Add,
                contentDescription = "Insert a word at position ${at + 1}",
                tint = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(16.dp),
            )
        }
    }
}

@Composable
internal fun WordRow(
    number: Int,
    entry: Entry,
    showCrop: Boolean,
    onClick: () -> Unit,
    label: String = "$number.",
    expectedWord: String? = null,
    acknowledged: Boolean = false,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClickLabel = "Review word $number", onClick = onClick)
            .semantics { stateDescription = if (entry.needsAttention) "Needs attention" else "" }
            .padding(horizontal = 24.dp, vertical = 6.dp)
            .testTag("row-$number"),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            label,
            style = MaterialTheme.typography.titleMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.widthIn(min = 32.dp).testTag("row-$number-label"),
        )
        Column(Modifier.weight(1f)) {
            Text(
                entry.word,
                style = MaterialTheme.typography.titleLarge,
                fontFamily = PlexMono,
                modifier = Modifier.testTag("row-$number-word"),
            )
            val confidence = entry.modelConfidence
            val confidenceText = confidence?.let {
                "Model confidence ${String.format(Locale.ROOT, "%.1f%%", it * 100)}"
            } ?: "Model confidence unavailable"
            LinearProgressIndicator(
                progress = { confidence ?: 0f },
                color = confidenceColour(entry),
                modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp)
                    .testTag("confidence-$number").semantics { contentDescription = confidenceText },
            )
            Text(confidenceText, style = MaterialTheme.typography.labelSmall)
            if (showCrop) {
                val source = entry.source
                if (source != null) {
                    Crop(source.cropPng, Modifier.fillMaxWidth().height(56.dp).testTag("crop-$number"))
                } else {
                    Text("Typed, not read from paper", style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            if (!showCrop) {
                val label = when {
                    entry.needsAttention -> "Needs attention"
                    entry.corrected -> "Corrected"
                    entry.typed -> "Typed"
                    entry.source?.expandedFrom?.isNotEmpty() == true -> "Expanded word crop"
                    entry.source?.joinedFrom?.isNotEmpty() == true -> "Joined from two pieces"
                    else -> null
                }
                if (label != null) Text(label, style = MaterialTheme.typography.labelMedium,
                    modifier = Modifier.testTag("row-$number-note"))
                if (entry.source?.fromRead == true && !entry.corrected) {
                    Text("read: ${entry.source.raw}", style = MaterialTheme.typography.labelMedium)
                }
            }
        }
        if (expectedWord != null) {
            val correct = entry.word == expectedWord
            Text(
                if (correct) "✓ Correct" else if (acknowledged) "✗ Wrong\nAcknowledged" else "✗ Wrong",
                color = if (correct) Accepted else MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.labelMedium,
                modifier = Modifier.testTag("match-$number"),
            )
        }
    }
}

@Composable
internal fun Crop(png: ByteArray, modifier: Modifier = Modifier) {
    val bitmap = remember(png) { BitmapFactory.decodeByteArray(png, 0, png.size) }
    if (bitmap != null) {
        Image(
            bitmap.asImageBitmap(),
            contentDescription = "What was written",
            contentScale = ContentScale.Fit,
            modifier = modifier,
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun EditSheet(
    viewModel: SheetViewModel,
    what: Editing,
    onDismiss: () -> Unit,
    onLocate: (() -> Unit)? = null,
) {
    val draft = viewModel.draft ?: return
    val entry = (what as? Editing.Word)?.let { w -> draft.entries.firstOrNull { it.id == w.id } }
    val number = entry?.let { draft.entries.indexOf(it) + 1 }
    var query by remember { mutableStateOf("") }
    val matches = remember(query) {
        if (query.isBlank()) emptyList() else listWords(query.trim()).take(12)
    }
    fun choose(word: String) {
        when (what) {
            is Editing.Word -> viewModel.change(what.id, word)
            is Editing.Insert -> viewModel.insert(what.at, word, what.number)
        }
        onDismiss()
    }
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(
            Modifier.verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                if (entry == null) "Insert a word" else "Word $number",
                style = MaterialTheme.typography.titleMedium,
                modifier = Modifier.testTag("edit-word-title"),
            )
            val source = entry?.source
            if (entry != null && source != null) {
                Crop(
                    source.cropPng,
                    Modifier
                        .fillMaxWidth()
                        .height(72.dp)
                        .testTag("word-crop"),
                )
                OcrEvidence(source)
                Text(
                    "Model's top candidate: ${source.ranked.first().first} " +
                        String.format(Locale.ROOT, "%.2f%%", source.probability * 100) +
                        ". Initially shown as \"${source.read}\". Other possibilities:",
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.testTag("word-confidence"),
                )
                if (onLocate != null) {
                    TextButton(onClick = onLocate, modifier = Modifier.testTag("locate-word")) {
                        Text("Locate on source photo")
                    }
                }
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for ((word, p) in source.ranked.take(5)) {
                        FilterChip(
                            selected = entry.word == word,
                            onClick = { choose(word) },
                            label = { Text("$word ${(p * 100).toInt()}%") },
                            modifier = Modifier.testTag("alt-$word"),
                        )
                    }
                }
            } else if (entry != null) {
                Text(
                    "Typed, not read from the sheet.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                label = { Text("Find a word in the list") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("search"),
            )
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (word in matches) {
                    SuggestionChip(
                        onClick = { choose(word) },
                        label = { Text(word) },
                        modifier = Modifier.testTag("pick-$word"),
                    )
                }
            }
            if (entry != null) {
                TextButton(
                    onClick = {
                        viewModel.remove(entry.id)
                        onDismiss()
                    },
                    modifier = Modifier.testTag("remove"),
                ) {
                    Icon(Icons.Filled.Delete, contentDescription = null)
                    Spacer(Modifier.width(8.dp))
                    Text("Remove this word")
                }
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

/** Literal reads always appear with their own crop, not the matching transform. */
@Composable
private fun OcrEvidence(source: Source) {
    Text(if (source.raw.isBlank()) "No text read from this crop" else "Read as: ${source.raw}",
        style = MaterialTheme.typography.bodyMedium, modifier = Modifier.testTag("current-ocr"))
    source.matching?.let { m ->
        if (m.original || m.removedPrefixBytes > 0) {
            Text("Matching text: ${m.text}" +
                (if (m.removedPrefixBytes > 0) " · label prefix removed" else "") +
                (if (m.original) " · from original crop" else " · from this crop"),
                style = MaterialTheme.typography.bodySmall, modifier = Modifier.testTag("matching-evidence"))
        }
    }
    if (source.labels.isNotEmpty()) {
        Text("Label observations: " + source.labels.joinToString("; ") {
            "${it.literal} (${it.ordinal?.toString() ?: "number unknown"}${if (it.ambiguous) ", attachment unclear" else ""})"
        } + if (source.labelConflict) ". Conflicting numbers; check the order." else "",
            style = MaterialTheme.typography.bodySmall, modifier = Modifier.testTag("label-evidence"))
    }
    source.originalOcr?.let { original ->
        var expanded by remember(source) { mutableStateOf(false) }
        TextButton(onClick = { expanded = !expanded }, modifier = Modifier.testTag("original-ocr-toggle")) {
            Text(if (expanded) "Hide original OCR" else "Show original OCR")
        }
        if (expanded) {
            Crop(original.cropPng, Modifier.fillMaxWidth().height(72.dp).testTag("original-ocr-crop"))
            Text("Original crop read as: ${original.raw}", style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.testTag("original-ocr-text"))
        }
    }
}

/** Every word ticked against the paper, then [action] takes them. */
