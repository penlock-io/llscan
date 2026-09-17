package com.bitcoinvision.example

import android.app.Application
import android.util.Base64
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import uniffi.bitcoin_vision_mobile.VisionException
import uniffi.bitcoin_vision_mobile.Review
import uniffi.bitcoin_vision_mobile.ShareText
import uniffi.bitcoin_vision_mobile.WalletDescriptor
import uniffi.bitcoin_vision_mobile.descriptor
import uniffi.bitcoin_vision_mobile.reviewPsbt
import uniffi.bitcoin_vision_mobile.signReview
import uniffi.bitcoin_vision_mobile.splitPhrase
import java.io.File
import java.time.LocalDate
import java.util.UUID

/** Where a loaded key came from. */
sealed interface KeySource {
    data object New : KeySource

    data object WrittenPhrase : KeySource

    data class Strips(val first: UByte, val second: UByte) : KeySource
}

/** A wallet's key while it is in memory. */
class LoadedKey(val words: List<String>, val source: KeySource)

/** The next task suggested by a wallet's current state. */
enum class WalletAction { Review, BackUp, Sign, LoadKey }

/** What two photographed strips read back against the key: the first
 * row that differs, or none. */
class StripCheck(val read: List<String>, val mismatch: Int?)

/** A Penlock backup under way for one wallet. */
class Backup(val walletId: String, val shares: List<ShareText>, val check: StripCheck?)

/** A line for one wallet's page: a refusal, or a note on what just happened. */
class Notice(val walletId: String, val text: String, val refusal: Boolean)

/** A PSBT that came in, as far as it got. */
sealed interface Signing {
    val walletId: String?

    /** Reviewed against a saved wallet, waiting for the word. */
    data class Pending(override val walletId: String, val review: Review) : Signing

    data class Refused(override val walletId: String?, val reason: String) : Signing

    data class Signed(override val walletId: String, val review: Review, val bytes: ByteArray) : Signing
}

/**
 * The wallets: descriptors saved on disk, keys held in memory for as
 * long as they are wanted. The saved descriptor is the oracle for
 * whether a phrase is a wallet's key, and what a PSBT is held to.
 */
class WalletsViewModel(app: Application) : AndroidViewModel(app) {
    private val store = WalletStore(app)

    val wallets = mutableStateListOf<WalletRecord>().apply { addAll(store.load()) }
    val keys = mutableStateMapOf<String, LoadedKey>()

    /** The wallet whose phrase is on screen. */
    var revealed by mutableStateOf<String?>(null)
    var backup by mutableStateOf<Backup?>(null)
        private set
    var notice by mutableStateOf<Notice?>(null)
        private set
    var signing by mutableStateOf<Signing?>(null)
        private set

    fun wallet(id: String): WalletRecord? = wallets.firstOrNull { it.id == id }

    /** Names are public labels, not wallet identities. At most 40 Unicode code points. */
    fun rename(id: String, name: String): Boolean {
        val trimmed = normalizedWalletName(name) ?: return false
        val index = wallets.indexOfFirst { it.id == id }
        if (index < 0) return false
        wallets[index] = wallets[index].copy(name = trimmed)
        store.save(wallets)
        return true
    }

    fun needsBackupWarning(id: String): Boolean =
        key(id)?.source == KeySource.New && wallet(id)?.penlockBackup == null

    fun nextAction(id: String): WalletAction = when {
        (signing as? Signing.Pending)?.walletId == id -> WalletAction.Review
        needsBackupWarning(id) -> WalletAction.BackUp
        key(id) != null -> WalletAction.Sign
        else -> WalletAction.LoadKey
    }

    fun key(id: String): LoadedKey? = keys[id]

    private fun derive(words: List<String>): WalletDescriptor? =
        try {
            descriptor(words)
        } catch (e: VisionException) {
            null
        }

    /**
     * Takes `words` as a wallet: the saved one with this descriptor,
     * its key now loaded, or a new one saved with its key loaded. The
     * wallet's id, or null when the words are not a phrase.
     */
    fun take(words: List<String>, source: KeySource): String? {
        if (source == KeySource.New) return null
        return saveKey(words, source)
    }

    private fun saveKey(words: List<String>, source: KeySource): String? {
        val derived = derive(words) ?: return null
        val existing = wallets.firstOrNull { it.descriptor == derived.descriptor }
        if (existing != null) {
            keys[existing.id] = LoadedKey(words, source)
            notice = Notice(existing.id, "Already here as ${existing.name}. Its key is loaded.", refusal = false)
            return existing.id
        }
        val record = WalletRecord(
            id = UUID.randomUUID().toString(),
            name = "Wallet ${nextNumber()}",
            fingerprint = derived.fingerprint,
            descriptor = derived.descriptor,
            created = LocalDate.now().toString(),
            penlockBackup = null,
        )
        wallets.add(record)
        store.save(wallets)
        keys[record.id] = LoadedKey(words, source)
        // Generation reaches here only after its photographed backup is verified.
        notice = null
        return record.id
    }

    /** The only generated-key completion path consumes one verified photo session. */
    fun completeNew(creation: NewSeedViewModel): String? {
        val words = creation.consumeVerified() ?: return null
        return saveKey(words, KeySource.New)
    }

    /** Loads `words` as `id`'s key. Another wallet's phrase is refused by fingerprint. */
    fun loadInto(id: String, words: List<String>, source: KeySource): Boolean {
        if (source == KeySource.New) return false
        val wallet = wallet(id) ?: return false
        val derived = derive(words) ?: return false
        if (derived.descriptor != wallet.descriptor) {
            notice = Notice(
                id,
                "That is not this wallet's key: its fingerprint is ${derived.fingerprint}, " +
                    "this wallet's is ${wallet.fingerprint}.",
                refusal = true,
            )
            return false
        }
        keys[id] = LoadedKey(words, source)
        notice = null
        return true
    }

    fun unload(id: String) {
        keys.remove(id)
        if (revealed == id) revealed = null
        if (backup?.walletId == id) backup = null
        if (notice?.walletId == id) notice = null
        if (signing?.walletId == id) signing = null
    }

    fun forget(id: String) {
        unload(id)
        wallets.removeAll { it.id == id }
        store.save(wallets)
    }

    /** Splits `id`'s key. True when the shares are ready to show. */
    fun makeShares(id: String): Boolean {
        val key = keys[id] ?: return false
        return try {
            backup = Backup(id, splitPhrase(key.words), null)
            true
        } catch (e: VisionException) {
            false
        }
    }

    /** Compares what two strips recovered with the key being backed
     * up; a match is the backup checked, and recorded on the wallet. */
    fun checkStrips(read: List<String>) {
        val backup = backup ?: return
        val key = keys[backup.walletId] ?: return
        val mismatch = (0 until maxOf(key.words.size, read.size))
            .firstOrNull { key.words.getOrNull(it) != read.getOrNull(it) }
        this.backup = Backup(backup.walletId, backup.shares, StripCheck(read, mismatch))
        if (mismatch != null) return
        val i = wallets.indexOfFirst { it.id == backup.walletId }
        if (i >= 0) {
            wallets[i] = wallets[i].copy(penlockBackup = LocalDate.now().toString())
            store.save(wallets)
        }
    }

    fun endBackup() {
        backup = null
    }

    /**
     * Takes a PSBT that came in, binary or base64 text, reviewed
     * against every saved wallet. Whatever was pending is replaced.
     */
    fun receive(bytes: ByteArray) {
        signing = try {
            val review = reviewPsbt(wallets.map { it.descriptor }, asPsbtBytes(bytes))
            val wallet = wallets.firstOrNull { it.descriptor == review.descriptor() }
            if (wallet == null) {
                Signing.Refused(null, "No saved wallet has this transaction's descriptor.")
            } else {
                Signing.Pending(wallet.id, review)
            }
        } catch (e: VisionException) {
            Signing.Refused(null, e.message?.removePrefix("detail=") ?: "The transaction was refused.")
        }
    }

    /**
     * Signs `pending` with its wallet's loaded key, only while it is
     * still the request on the table: consent given to one transaction
     * does not carry to whatever replaced it. True when signed.
     */
    fun sign(pending: Signing.Pending): Boolean {
        if (signing !== pending) return false
        val key = keys[pending.walletId] ?: return false
        signing = try {
            val bytes = signReview(pending.review, key.words)
            if (BuildConfig.DEBUG) keepSignedForTheGate(bytes)
            Signing.Signed(pending.walletId, pending.review, bytes)
        } catch (e: VisionException) {
            Signing.Refused(pending.walletId, e.message?.removePrefix("detail=") ?: "The transaction could not be signed.")
        }
        return signing is Signing.Signed
    }

    fun dismissSigning() {
        signing = null
    }

    /** Opening a wallet's page drops a request that was another wallet's. */
    fun leaveSigningUnless(id: String) {
        val owner = signing?.walletId
        if (owner != null && owner != id) signing = null
    }

    // The emulator gate reads the signed transaction back from the
    // app's files; a release build keeps it in memory only.
    private fun keepSignedForTheGate(bytes: ByteArray) {
        val dir = getApplication<Application>().getExternalFilesDir("psbts") ?: return
        File(dir, "signed.psbt").writeBytes(bytes)
    }

    private fun nextNumber(): Int =
        (wallets.mapNotNull { it.name.removePrefix("Wallet ").toIntOrNull() }.maxOrNull() ?: 0) + 1
}

private val PSBT_MAGIC = byteArrayOf(0x70, 0x73, 0x62, 0x74, 0xFF.toByte())

/** The PSBT's bytes, whether they came as the file or as its base64 text. */
fun asPsbtBytes(bytes: ByteArray): ByteArray {
    if (bytes.size >= PSBT_MAGIC.size && bytes.copyOfRange(0, PSBT_MAGIC.size).contentEquals(PSBT_MAGIC)) {
        return bytes
    }
    return try {
        Base64.decode(String(bytes).trim(), Base64.DEFAULT)
    } catch (e: IllegalArgumentException) {
        bytes
    }
}
