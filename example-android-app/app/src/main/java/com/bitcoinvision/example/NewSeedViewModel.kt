package com.bitcoinvision.example

import android.app.Application
import androidx.annotation.VisibleForTesting
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.bitcoin_vision_mobile.newPhrase

/** Owns a pending seed, never a wallet/key available to signing. */
class NewSeedViewModel(app: Application) : AndroidViewModel(app) {
    private val backup = NewSeedBackup()
    @VisibleForTesting internal var reader: PhraseReader = NativePhraseReader(app)
    @VisibleForTesting internal var preparer: PhotoPreparer = PhotoPreparer(::resolvePhoto)
    @VisibleForTesting internal var generate: () -> List<String> = ::newPhrase
    private val scanning = Mutex()
    private var worker: Job? = null
    private var progressWorker: Job? = null
    private var handoff: ScanProgressHandoff? = null
    var progress by mutableStateOf<ScanProgressState?>(null)
        private set
    val stage get() = backup.stage
    val words get() = backup.words
    val message get() = backup.message
    val order get() = backup.order
    val availableOrders get() = backup.availableOrders
    val readWords get() = backup.readWords
    val entries get() = backup.entries
    val acknowledged get() = backup.acknowledged
    val frame get() = backup.frame
    val photo get() = when (val attempt = backup.attempt) {
        is Attempt.Processing -> attempt.photo
        is Attempt.Done -> attempt.photo
        is Attempt.Failed -> attempt.photo
        else -> null
    }
    val capturing get() = backup.attempt is Attempt.Capturing

    fun start(): Boolean {
        cancel()
        return try { backup.start(generate()); true } catch (_: Exception) { false }
    }

    private fun stopWork() {
        handoff?.detach()
        handoff = null
        progressWorker?.cancel()
        progressWorker = null
        progress = null
        worker?.cancel()
        worker = null
    }

    fun cancel() { stopWork(); backup.cancel() }
    fun retake() { stopWork(); backup.retake() }
    fun showWords() { stopWork(); backup.showWords() }
    fun begin() = backup.begin()
    fun chooseOrder(order: Order) = backup.chooseOrder(order)
    fun acknowledge(position: Int) = backup.acknowledge(position)
    fun progressPresented(token: Int, sequence: ULong) {
        handoff?.takeIf { it.token == token }?.presented(sequence)
    }
    internal fun consumeVerified(): List<String>? = backup.consumeVerified()

    fun deliver(token: Int, photo: PhotoInput?) {
        if (!backup.deliver(token, photo)) return
        photo ?: return
        val events = ScanProgressHandoff(token)
        handoff = events
        progress = events.nextFrame()
        progressWorker = viewModelScope.launch {
            while (isActive) {
                progress = events.nextFrame()
                delay(PROGRESS_FRAME_MS)
            }
        }
        worker = viewModelScope.launch {
            try {
                val resolved = withContext(Dispatchers.Default) {
                    photoPreparationLock.withLock { preparer.prepare(photo) }
                }
                if (!backup.prepared(token, resolved)) return@launch
                val result = scanning.withLock {
                    withContext(Dispatchers.Default) { reader.scan(resolved, events) }
                }
                events.awaitPlayback()
                backup.finish(token, BackupReading.from(result))
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // Never surface/log an exception payload containing OCR text or seed words.
                backup.fail(token)
            } finally {
                events.detach()
                if (handoff === events) {
                    worker = null
                    progressWorker?.cancel()
                    progressWorker = null
                    handoff = null
                    progress = null
                }
            }
        }
    }

    override fun onCleared() { cancel(); super.onCleared() }
}
