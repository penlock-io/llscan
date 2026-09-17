package com.bitcoinvision.example

import android.app.Application
import android.net.Uri
import androidx.annotation.VisibleForTesting
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.bitcoin_vision_mobile.InitialOrder
import uniffi.bitcoin_vision_mobile.Numbering
import uniffi.bitcoin_vision_mobile.VisionException
import uniffi.bitcoin_vision_mobile.PhraseScan
import uniffi.bitcoin_vision_mobile.PhraseScanner
import uniffi.bitcoin_vision_mobile.DiagnosedScan
import uniffi.bitcoin_vision_mobile.ScanProgressObserver
import uniffi.bitcoin_vision_mobile.checksumPasses
import java.io.IOException

private sealed interface Picked {
    class Photo(val bytes: ByteArray) : Picked

    class Refused(val message: String) : Picked
}

sealed interface SheetStage {
    data object Idle : SheetStage

    data object Scanning : SheetStage

    data object Reviewing : SheetStage
}

/** What reads a photographed page into words. */
fun interface PhraseReader {
    /** A successful scan must emit every stage and its terminal mapping. No silent overload. */
    fun scan(photo: ResolvedPhoto, observer: ScanProgressObserver): PhraseScan
    /** Explicit diagnostic attempt; legacy/test readers report no recorded files. */
    fun scanDiagnosed(photo: ResolvedPhoto, observer: ScanProgressObserver): DiagnosedScan =
        DiagnosedScan(scan(photo, observer), emptyList())
}

/** One shared native pipeline and asset set for recovery and generated backups. */
internal class NativePhraseReader(app: Application) : PhraseReader {
    private val scanner by lazy {
        ScanTrace.mark("asset_load_start")
        fun asset(name: String) = app.assets.open(name).use { it.readBytes() }
        val detector = asset("word-detector.onnx")
        val model = asset("word-reference.onnx")
        val calibration = String(asset("word-reference.calibration"))
        val recogniser = asset("text-recogniser.onnx")
        val dictionary = String(asset("text-recogniser.dict.txt"))
        ScanTrace.mark("asset_load_end")
        ScanTrace.mark("constructor_ffi_start")
        PhraseScanner(detector, model, calibration, recogniser, dictionary).also {
            ScanTrace.mark("constructor_ffi_end")
        }
    }

    override fun scan(photo: ResolvedPhoto, observer: ScanProgressObserver): PhraseScan {
        val ready = scanner
        ScanTrace.mark("scan_ffi_start")
        return ready.scanWithPhotoUpAndProgress(photo.source.copyBytes(), photo.orientation.toUByte(), photo.photoUp, observer)
            .also { ScanTrace.mark("scan_ffi_end") }
    }
    override fun scanDiagnosed(photo: ResolvedPhoto, observer: ScanProgressObserver): DiagnosedScan {
        val ready = scanner
        ScanTrace.mark("scan_ffi_start")
        return ready.scanWithPhotoUpAndDiagnostics(photo.source.copyBytes(), photo.orientation.toUByte(), photo.photoUp, observer)
            .also { ScanTrace.mark("scan_ffi_end") }
    }
}

/** User-editable recovery review; generated backups never consume this draft. */
class SheetViewModel(app: Application) : AndroidViewModel(app) {
    /** Shared native reader also used by generated-seed photo verification. */
    @VisibleForTesting var reader: PhraseReader = NativePhraseReader(app)

    /** Debug-only, one-shot opt-in. Ordinary scans, including debug scans, stay lean. */
    var traceNextScan by mutableStateOf(false)
        private set
    private var tracedAttempt: Int? = null
    private var savedDiagnosis: SavedScanDiagnosis? = null

    fun toggleDecisionTrace() {
        if (BuildConfig.DEBUG && !BuildConfig.SCAN_BENCHMARK && attempts.current == null) {
            traceNextScan = !traceNextScan
        }
    }

    /** The photographs taken, one current at a time. */
    val attempts = Attempts<PhraseScan>()
    @VisibleForTesting var preparer: PhotoPreparer = PhotoPreparer(::resolvePhoto)

    // The scanner is one object reading one photo at a time; a retake
    // while it reads waits its turn rather than running beside it.
    private val scanning = Mutex()

    // The read of the current attempt, and no other: an attempt
    // cancelled or replaced takes its read with it, whether the read
    // is waiting its turn or under way. A read under way in native
    // code cannot be interrupted; it holds the scanner until it
    // returns, and returns into nothing.
    private var reading: Job? = null
    private var progressJob: Job? = null
    private var progressHandoff: ScanProgressHandoff? = null
    var progress by mutableStateOf<ScanProgressState?>(null)
        private set

    private fun stopProgress() {
        progressHandoff?.detach()
        progressHandoff = null
        progressJob?.cancel()
        progressJob = null
        progress = null
    }

    private fun startProgress(token: Int): ScanProgressHandoff {
        stopProgress()
        val handoff = ScanProgressHandoff(token)
        progressHandoff = handoff
        progress = handoff.snapshot()
        progressJob = viewModelScope.launch {
            while (isActive && progressHandoff === handoff && attempts.current?.token == token) {
                val latest = handoff.nextFrame() ?: break
                if (progress !== latest) progress = latest // Compose writes stay on Main.
                delay(PROGRESS_FRAME_MS)
            }
        }
        return handoff
    }

    fun progressPresented(token: Int, sequence: ULong) {
        progressHandoff?.takeIf { it.token == token }?.presented(sequence)
    }

    override fun onCleared() {
        stopProgress()
        super.onCleared()
    }

    var stage by mutableStateOf<SheetStage>(SheetStage.Idle)
        private set
    var photo by mutableStateOf<ResolvedPhoto?>(null)
        private set
    var scan by mutableStateOf<PhraseScan?>(null)
        private set
    var draft by mutableStateOf<PhraseDraft?>(null)
        private set

    /** Set once a read has produced words to review; the scan screen
     * takes the navigation and clears it. */
    var reviewed by mutableStateOf(false)

    /// The box the review is looking at, drawn heavier on the photo.
    var highlighted by mutableStateOf<Int?>(null)

    /** The current attempt's photo and capture metadata, whatever the read made of them. */
    val capture: PhotoInput? get() = attempts.photo

    fun start() {
        forget()
        stage = SheetStage.Scanning
    }

    fun forget() {
        traceNextScan = false
        tracedAttempt = null
        savedDiagnosis = null
        stopProgress()
        attempts.cancel()
        reading?.cancel()
        stage = SheetStage.Idle
        photo = null
        scan = null
        draft = null
        highlighted = null
        reviewed = false
    }

    /** A new attempt at the shutter, named for the photo the camera will deliver. */
    fun begin(): Int {
        stopProgress()
        reading?.cancel()
        savedDiagnosis = null
        return attempts.begin().also { token ->
            tracedAttempt = token.takeIf { BuildConfig.DEBUG && traceNextScan }
            traceNextScan = false
        }
    }

    /**
     * The camera's photo for the attempt [token]: read on a worker,
     * and its words offered for review only if the attempt is still
     * the current one when the read ends.
     */
    fun deliver(token: Int, bytes: ByteArray?) {
        ScanTrace.mark("deliver_entry")
        if (attempts.current?.token != token) return
        readPhoto(token, bytes?.let(PhotoInput::fromFile))
    }

    fun deliverPhoto(token: Int, photo: PhotoInput?) {
        ScanTrace.mark("deliver_entry")
        readPhoto(token, photo)
    }

    /**
     * A photo the person picked out of their own files. The document
     * is opened on a worker and bounded: the provider can be remote,
     * slow, or gone by the time it is read, and what it hands back is
     * not necessarily a photograph.
     */
    fun deliverDocument(uri: Uri) {
        val token = begin()
        viewModelScope.launch {
            when (val picked = withContext(Dispatchers.IO) { open(uri) }) {
                is Picked.Refused -> attempts.missing(token, picked.message)
                is Picked.Photo -> deliver(token, picked.bytes)
            }
        }
    }

    private fun open(uri: Uri): Picked {
        val unopened = Picked.Refused("That file could not be opened. Choose another.")
        val bytes = try {
            val stream = getApplication<Application>().contentResolver.openInputStream(uri) ?: return unopened
            stream.use { readAtMost(it) } ?: return Picked.Refused("That file is too large to be a photo of a page.")
        } catch (_: IOException) {
            return unopened
        } catch (_: SecurityException) {
            return unopened
        }
        return if (bytes.isEmpty()) Picked.Refused("That file is empty. Choose another.") else Picked.Photo(bytes)
    }

    private fun readPhoto(token: Int, photo: PhotoInput?) {
        if (!attempts.deliverPhoto(token, photo)) return
        photo ?: return
        ScanTrace.mark("delivered")
        reading?.cancel()
        val handoff = startProgress(token)
        val recordDecisions = tracedAttempt == token
        reading = viewModelScope.launch {
            try {
                val result = try {
                    ScanTrace.mark("orientation_start")
                    val resolved = withContext(Dispatchers.Default) {
                        photoPreparationLock.withLock { preparer.prepare(photo) }
                    }
                    if (!attempts.prepared(token, resolved)) return@launch
                    ScanTrace.mark("orientation_end")
                    scanning.withLock {
                        if (attempts.current?.token != token) return@launch
                        withContext(Dispatchers.Default) {
                            ScanTrace.mark("worker_start")
                            if (photo.camera == null) logPhotoInput(photo)
                            (if (recordDecisions) reader.scanDiagnosed(resolved, handoff)
                             else DiagnosedScan(reader.scan(resolved, handoff), emptyList()))
                                .also { ScanTrace.mark("worker_end") }
                        }
                    }
                } catch (e: PhotoPreparationException) {
                    attempts.fail(token, e.message ?: "The photo could not be prepared. Try again.")
                    return@launch
                } catch (e: VisionException) {
                    // The generated exception message carries its field name.
                    val detail = e.message?.removePrefix("detail=")
                    attempts.fail(token, detail ?: "The photo could not be read.")
                    return@launch
                }
                ScanTrace.mark("result_received")
                try {
                    handoff.awaitPlayback()
                } catch (e: ScanProgressException) {
                    attempts.fail(token, e.message!!)
                    return@launch
                }
                if (attempts.finish(token, result.scan)) {
                    this@SheetViewModel.photo = attempts.resolvedPhoto
                    scan = result.scan
                    savedDiagnosis = attempts.resolvedPhoto?.let { prepared ->
                        result.takeIf { it.files.isNotEmpty() }?.let { SavedScanDiagnosis(prepared, it) }
                    }
                    ScanTrace.mark("draft_start")
                    draft = draftOf(result.scan)
                    ScanTrace.mark("draft_end")
                    stage = SheetStage.Reviewing
                    reviewed = true
                    ScanTrace.mark("review_ready")
                }
            } finally {
                // An old worker returning after retake must not clear the new attempt.
                handoff.detach()
                if (progressHandoff === handoff) stopProgress()
            }
        }
    }

    private fun draftOf(result: PhraseScan): PhraseDraft {
        val numbers = when (val n = result.numbering) {
            is Numbering.Held -> Numbers.Held(
                n.count.toInt(), n.missing.map { it.toInt() }, n.outOfPlace.map { it.toInt() }, n.unresolved.size,
            )
            is Numbering.Inconsistent -> Numbers.Inconsistent(n.detail)
            is Numbering.None -> Numbers.None
        }
        return PhraseDraft(
            result.words.map { it.reviewSource() },
            result.columns.map { it.toInt() },
            result.rows.map { it.toInt() },
            when (result.initialOrder) {
                InitialOrder.NUMBERS -> Order.NUMBERS
                InitialOrder.ROWS -> Order.ROWS
                InitialOrder.COLUMNS -> Order.COLUMNS
            },
            byNumber = result.numbered.map { it.toInt() },
            numbers = numbers,
            listNumbered = result.listNumbered,
            orderRequiresReview = result.orderRequiresReview,
        )
    }

    /** Drops the current attempt, read or unread, and returns to the camera. */
    fun retake() {
        tracedAttempt = null
        savedDiagnosis = null
        stopProgress()
        attempts.cancel()
        reading?.cancel()
    }

    /**
     * Explicitly exports raw bytes/metadata and, if recorded for this capture,
     * native decisions/crops off Main. Unavailable in a release build.
     */
    suspend fun savePhoto(): String? = saveRawPhotoForDiagnosis(getApplication(), capture, savedDiagnosis)
    suspend fun saveCanonicalPhoto(): String? = saveCanonicalPhotoForDiagnosis(getApplication(), attempts.resolvedPhoto)

    fun rescan() {
        tracedAttempt = null
        savedDiagnosis = null
        stopProgress()
        attempts.cancel()
        reading?.cancel()
        stage = SheetStage.Scanning
        photo = null
        scan = null
        draft = null
        highlighted = null
        reviewed = false
    }

    /// Whether the phrase fits a strip and passes the checksum.
    fun passes(): Boolean =
        draft?.let { it.words.size == WORKSHEET_WORDS && checksumPasses(it.words) } ?: false

    fun status(): String =
        draft?.let {
            if (passes()) "Checksum passes"
            else if (it.words.size == WORKSHEET_WORDS) "At least one word is wrong"
            else phraseStatus(it.words.size, false, it.numbers)
        } ?: ""

    fun change(id: Int, word: String) {
        draft = draft?.change(id, word)
    }

    fun insert(at: Int, word: String, number: Int? = null) {
        draft = draft?.insert(at, word, number)
    }

    fun remove(id: Int) {
        draft = draft?.remove(id)
    }

    fun restore(id: Int) {
        draft = draft?.restore(id)
    }

    fun reorder(order: Order) {
        draft = draft?.reorder(order)
    }

    fun check(id: Int, on: Boolean) {
        draft = draft?.check(id, on)
    }

    /// Only a complete phrase with a passing checksum can leave recovery review.
    fun confirmed(): List<String>? =
        draft?.takeIf { passes() }?.words
}
