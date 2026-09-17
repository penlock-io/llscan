package com.bitcoinvision.example

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.File

/** A saved wallet: public data only, never the phrase. */
data class WalletRecord(
    val id: String,
    val name: String,
    val fingerprint: String,
    val descriptor: String,
    /** ISO date. */
    val created: String,
    /** The ISO date two written strips last read back as the key, if ever. */
    val penlockBackup: String?,
)

/** The wallets on disk: one JSON file under the app's files, written whole. */
class WalletStore(private val file: File) {
    constructor(context: Context) : this(File(context.filesDir, "wallets.json"))

    fun load(): List<WalletRecord> {
        if (!file.exists()) return emptyList()
        val array = JSONArray(file.readText())
        return (0 until array.length()).map { i ->
            val o = array.getJSONObject(i)
            WalletRecord(
                id = o.getString("id"),
                name = o.getString("name"),
                fingerprint = o.getString("fingerprint"),
                descriptor = o.getString("descriptor"),
                created = o.getString("created"),
                penlockBackup = if (o.isNull("penlockBackup")) null else o.getString("penlockBackup"),
            )
        }
    }

    fun save(wallets: List<WalletRecord>) {
        val array = JSONArray()
        for (w in wallets) {
            array.put(
                JSONObject()
                    .put("id", w.id)
                    .put("name", w.name)
                    .put("fingerprint", w.fingerprint)
                    .put("descriptor", w.descriptor)
                    .put("created", w.created)
                    .put("penlockBackup", w.penlockBackup ?: JSONObject.NULL),
            )
        }
        // Written beside and renamed over, so a crash mid-write leaves
        // the old list rather than half of the new one.
        val fresh = File(file.parentFile, file.name + ".new")
        fresh.writeText(array.toString())
        check(fresh.renameTo(file)) { "could not replace ${file.name}" }
    }

    fun clear() {
        file.delete()
    }
}
