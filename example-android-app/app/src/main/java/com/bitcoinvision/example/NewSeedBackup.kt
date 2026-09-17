package com.bitcoinvision.example

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.bitcoin_vision_mobile.InitialOrder
import uniffi.bitcoin_vision_mobile.PhraseScan

enum class NewSeedStage { Idle, Writing, Camera, Reading, Review, Verified }

/** Immutable, blind scan selections; never a user-editable PhraseDraft. */
internal class BackupReading(
    val sequences: Map<Order, List<String>>,
    val initial: Order,
    val requiresOrderReview: Boolean,
    val entries: Map<Order, List<Entry>> = emptyMap(),
    val frame: PhotoFrame? = null,
) {
    companion object {
        fun from(scan: PhraseScan): BackupReading {
            fun words(indices: List<UInt>): List<String> = indices
                .map { scan.words[it.toInt()] }.filterNot { it.stray }.map { it.read }
            val orders = mutableMapOf(Order.COLUMNS to words(scan.columns), Order.ROWS to words(scan.rows))
            if (scan.numbering is uniffi.bitcoin_vision_mobile.Numbering.Held) orders[Order.NUMBERS] = words(scan.numbered)
            val initial = when (scan.initialOrder) {
                InitialOrder.COLUMNS -> Order.COLUMNS
                InitialOrder.ROWS -> Order.ROWS
                InitialOrder.NUMBERS -> Order.NUMBERS
            }
            fun entries(indices: List<UInt>) = indices.filterNot { scan.words[it.toInt()].stray }.map {
                val source = scan.words[it.toInt()].reviewSource()
                Entry(it.toInt(), source.read, source, false, source.number)
            }
            val entries = mutableMapOf(Order.COLUMNS to entries(scan.columns), Order.ROWS to entries(scan.rows))
            if (Order.NUMBERS in orders) entries[Order.NUMBERS] = entries(scan.numbered)
            return BackupReading(orders, initial, scan.orderRequiresReview, entries,
                PhotoFrame(scan.width.toInt(), scan.height.toInt()))
        }
    }
}

/** Memory-only authority: exact matches or explicit acknowledgements of blind misreads.
 * Acknowledgement never changes the generated seed or the scanner's selections. */
internal class NewSeedBackup {
    private val attempts = Attempts<BackupReading>()
    private var expected: List<String> = emptyList()
    private var reading: BackupReading? = null
    private var orderReviewed = false
    var acknowledged by mutableStateOf<Set<Int>>(emptySet())
        private set
    var stage by mutableStateOf(NewSeedStage.Idle)
        private set
    var order by mutableStateOf<Order?>(null)
        private set
    var message by mutableStateOf<String?>(null)
        private set
    val words: List<String> get() = expected.toList()
    val attempt: Attempt<BackupReading>? get() = attempts.current
    val availableOrders: Set<Order> get() = reading?.sequences?.keys ?: emptySet()
    val readWords: List<String> get() = reading?.sequences?.get(order) ?: emptyList()
    val entries: List<Entry> get() = reading?.entries?.get(order)
        ?: readWords.mapIndexed { i, word -> Entry(i, word, null, false) }
    val frame: PhotoFrame? get() = reading?.frame

    fun start(words: List<String>) {
        require(words.size == 12)
        cancel()
        expected = words.toList()
        stage = NewSeedStage.Writing
    }

    fun retake() {
        if (stage == NewSeedStage.Idle) return
        attempts.cancel()
        reading = null
        order = null
        orderReviewed = false
        acknowledged = emptySet()
        message = null
        stage = NewSeedStage.Camera
    }

    fun showWords() {
        if (stage == NewSeedStage.Idle) return
        retake()
        stage = NewSeedStage.Writing
    }

    fun begin(): Int {
        if (stage != NewSeedStage.Camera || attempt != null) return -1
        return attempts.begin()
    }

    fun deliver(token: Int, photo: PhotoInput?): Boolean {
        if (stage != NewSeedStage.Camera || attempt?.token != token) return false
        // Reject file/gallery data even if a caller manages to reach this method.
        val fresh = photo?.takeIf { it.camera != null }
        val accepted = attempts.deliverPhoto(token, fresh)
        stage = if (accepted) NewSeedStage.Reading else NewSeedStage.Review
        if (!accepted) message = "Take a fresh camera photo of your written backup."
        return accepted
    }

    fun prepared(token: Int, photo: ResolvedPhoto) = attempts.prepared(token, photo)

    fun finish(token: Int, result: BackupReading): Boolean {
        if (!attempts.finish(token, result)) return false
        reading = result
        acknowledged = emptySet()
        orderReviewed = !result.requiresOrderReview
        order = result.initial
        stage = NewSeedStage.Review
        assess()
        return true
    }

    fun chooseOrder(value: Order) {
        if (stage !in setOf(NewSeedStage.Review, NewSeedStage.Verified) || value !in availableOrders) return
        if (order != value) acknowledged = emptySet()
        order = value
        orderReviewed = true
        assess()
    }

    fun acknowledge(position: Int) {
        if (stage != NewSeedStage.Review || !orderReviewed || readWords.size != 12 ||
            position !in expected.indices || readWords[position] == expected[position]) return
        acknowledged = acknowledged + position
        assess()
    }

    private fun assess() {
        reading ?: return
        stage = NewSeedStage.Review
        if (!orderReviewed) {
            message = "Choose how you numbered or arranged the words on paper."
            return
        }
        val read = readWords
        if (read.size != 12) {
            message = "Read ${read.size} words; all 12 are required. Retake the whole written list."
        } else {
            val mismatches = read.indices.filter { read[it] != expected[it] }
            if (mismatches.all { it in acknowledged }) {
                message = if (mismatches.isEmpty()) "All 12 words match your generated phrase."
                    else "${mismatches.size} misread words acknowledged. The generated phrase is unchanged."
                stage = NewSeedStage.Verified
            } else message = "Words differ at positions ${mismatches.joinToString { (it + 1).toString() }}. " +
                "Tap each wrong word to compare it with the generated word and check your paper."
        }
    }

    fun fail(token: Int) {
        if (attempts.fail(token, "The photo could not be read. Please retake it.")) {
            message = "The photo could not be read. Please retake it."
            stage = NewSeedStage.Review
        }
    }

    fun consumeVerified(): List<String>? {
        if (stage != NewSeedStage.Verified) return null
        val verified = expected.toList()
        cancel()
        return verified
    }

    fun cancel() {
        attempts.cancel()
        expected = emptyList()
        reading = null
        order = null
        orderReviewed = false
        acknowledged = emptySet()
        message = null
        stage = NewSeedStage.Idle
    }
}
