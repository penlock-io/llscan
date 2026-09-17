package com.bitcoinvision.example

import android.app.Application
import androidx.annotation.VisibleForTesting
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.bitcoin_vision_mobile.VisionException
import uniffi.bitcoin_vision_mobile.Scanner
import uniffi.bitcoin_vision_mobile.StripIdentity
import uniffi.bitcoin_vision_mobile.StripReading
import uniffi.bitcoin_vision_mobile.recover

sealed interface Capture {
    data object Reading : Capture

    data class Found(val strips: List<StripReading>) : Capture

    data class Failed(val message: String) : Capture
}

sealed interface StripScan {
    data object Idle : StripScan

    /// The wizard: step one while nothing is held, step two after.
    data class Scanning(val held: StripReading?) : StripScan

    /// The phrase two shares recovered, for whoever asked for it.
    data class Recovered(val words: List<String>, val first: UByte, val second: UByte) : StripScan
}

/** What reads a photographed strip into its cells. */
fun interface StripReader {
    fun read(photo: ResolvedPhoto): List<StripReading>
}

/// The strip wizard: photograph two share strips and recover the
/// phrase they hold. What happens to the phrase is the caller's:
/// loading the key, or checking a backup against it.
class StripsViewModel(app: Application) : AndroidViewModel(app) {
    private val scanner by lazy {
        Scanner(app.assets.open("cell-reference.onnx").use { it.readBytes() })
    }

    /** The reader every attempt goes through; a test puts a slower or a failing one here. */
    @VisibleForTesting
    var reader: StripReader = StripReader { scanner.readOriented(it.source.copyBytes(), it.orientation.toUByte()) }

    /** The photographs taken, one current at a time. */
    val attempts = Attempts<List<StripReading>>()
    @VisibleForTesting var preparer: PhotoPreparer = PhotoPreparer(::resolvePhoto)
    private val scanning = Mutex()
    private var reading: Job? = null

    var scan by mutableStateOf<StripScan>(StripScan.Idle)
        private set

    // Combining two shares can fail after both photographs succeeded;
    // that failure is the wizard's, not an attempt's.
    private var combined by mutableStateOf<Capture.Failed?>(null)

    /** What the current photograph came to, or the last combination's failure. */
    val capture: Capture?
        get() = when (val attempt = attempts.current) {
            is Attempt.Capturing, is Attempt.Preparing, is Attempt.Processing -> Capture.Reading
            is Attempt.Done -> Capture.Found(attempt.result)
            is Attempt.Failed -> Capture.Failed(attempt.message)
            null -> combined
        }

    fun shareNumber(strip: StripReading): UByte? =
        (strip.identity() as? StripIdentity.Share)?.index

    fun start() {
        scan = StripScan.Scanning(null)
        attempts.cancel()
        reading?.cancel()
        combined = null
    }

    fun cancel() {
        scan = StripScan.Idle
        attempts.cancel()
        reading?.cancel()
        combined = null
    }

    /// Clears the wizard once its phrase has been taken.
    fun finish() = cancel()

    /** A new attempt at the shutter, named for the photo the camera will deliver. */
    fun begin(): Int {
        combined = null
        return attempts.begin()
    }

    /** The camera's photo for the attempt [token], read on a worker and
     * offered only if the attempt is still current when the read ends. */
    fun deliver(token: Int, bytes: ByteArray?) {
        if (attempts.current?.token != token) return
        deliverPhoto(token, bytes?.let(PhotoInput::fromFile))
    }

    fun deliverPhoto(token: Int, photo: PhotoInput?) {
        if (!attempts.deliverPhoto(token, photo)) return
        photo ?: return
        reading?.cancel()
        reading = viewModelScope.launch {
            val strips = try {
                val resolved = withContext(Dispatchers.Default) {
                    photoPreparationLock.withLock { preparer.prepare(photo) }
                }
                if (!attempts.prepared(token, resolved)) return@launch
                scanning.withLock {
                    if (attempts.current?.token != token) return@launch
                    withContext(Dispatchers.Default) {
                        if (photo.camera == null) logPhotoInput(photo)
                        reader.read(resolved)
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
            attempts.finish(token, strips)
        }
    }

    suspend fun savePhoto(): String? = saveRawPhotoForDiagnosis(getApplication(), attempts.photo)
    suspend fun saveCanonicalPhoto(): String? = saveCanonicalPhotoForDiagnosis(getApplication(), attempts.resolvedPhoto)

    fun retake() {
        attempts.cancel()
        reading?.cancel()
        combined = null
    }

    /// The share the wizard can take from a capture: the first share
    /// strip whose number is not already held.
    fun usable(strips: List<StripReading>): StripReading? {
        val scanning = scan as? StripScan.Scanning ?: return null
        val held = scanning.held?.let(::shareNumber)
        return strips.firstOrNull {
            val number = shareNumber(it)
            number != null && number != held
        }
    }

    /// Why a capture offered nothing usable.
    fun refusal(strips: List<StripReading>): String {
        val held = (scan as? StripScan.Scanning)?.held?.let(::shareNumber)
        return when {
            strips.any { shareNumber(it) != null && shareNumber(it) == held } ->
                "That is share $held again — scan a different share."
            strips.any { it.identity() is StripIdentity.Seed } ->
                "That is the seed strip — loading needs two share strips."
            else -> "No share strip could be read in this photo."
        }
    }

    /// Takes a share into the wizard. True when the second share
    /// completed it and the phrase is recovered.
    fun use(strip: StripReading): Boolean {
        val scanning = scan as? StripScan.Scanning ?: return false
        attempts.cancel()
        reading?.cancel()
        val held = scanning.held
        if (held == null) {
            scan = StripScan.Scanning(strip)
            return false
        }
        val first = shareNumber(held) ?: return false
        val second = shareNumber(strip) ?: return false
        return try {
            scan = StripScan.Recovered(recover(held, strip), first, second)
            true
        } catch (e: VisionException) {
            val detail = e.message?.removePrefix("detail=")
            combined = Capture.Failed(detail ?: "The shares could not be combined.")
            false
        }
    }
}
