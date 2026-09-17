package com.bitcoinvision.example

import java.io.File
import java.io.IOException
import java.nio.file.Files
import java.security.MessageDigest
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import uniffi.bitcoin_vision_mobile.*

/** Synthetic DTOs only: no native library, detector, OCR, or Android device. */
class ScanDiagnosticsTest {
    @get:Rule val temp = TemporaryFolder()

    private fun fixture(): DiagnosedScan {
        val word = WordReading(
            corners = List(8) { it.toFloat() }, ranked = emptyList(), accepted = false,
            read = "fox", raw = "x", originalOcr = OcrObservation(List(8) { 2f }, byteArrayOf(8, 9), "7."),
            matching = null, labels = emptyList(), labelConflict = false, stray = false,
            number = null, apart = false, cropPng = byteArrayOf(1, 2, 3),
            joinedFrom = emptyList(), expandedFrom = emptyList(),
        )
        val scan = PhraseScan(false, Layout.PAGE, 100u, 200u, listOf(word), listOf(0u), listOf(0u),
            false, false, Numbering.None, listOf(0u), false, InitialOrder.COLUMNS, false, "same_retained_traversal", emptyList())
        val files = listOf("manifest.json", "final-boxes.jsonl", "word-links.jsonl", "readings.jsonl",
            "sources.jsonl", "numbering.jsonl", "ordering.jsonl", "joins.jsonl")
            .map { DiagnosticFile(it, "{\"recorded\":\"$it\"}\n") }
        return DiagnosedScan(scan, files)
    }

    private fun raw(): File = temp.newFile("raw-scan.bin").also {
        it.writeBytes(byteArrayOf(4, 5, 6))
        File(it.path + ".json").writeText("{\"source\":\"fixture\"}\n")
    }

    @Test fun saves_existing_crop_and_trace_bytes_and_binds_every_file() {
        val raw = raw()
        val result = fixture()
        var encodes = 0
        val saved = writeScanDiagnostic(raw, result) { encodes++; byteArrayOf(11, 12) }
        assertEquals(1, encodes)
        assertArrayEquals(byteArrayOf(11, 12), File(saved, "photo.png").readBytes())
        assertArrayEquals(result.scan.words[0].cropPng, File(saved, "stage/crops/photo.png~w0.png").readBytes())
        assertArrayEquals(result.scan.words[0].originalOcr!!.cropPng, File(saved, "stage/original-crops/photo.png~w0.png").readBytes())
        assertEquals(File(raw.path + ".json").readText(), File(saved, "capture.json").readText())
        result.files.forEach { assertEquals(it.text, File(saved, "stage/${it.name}").readText()) }
        val covered = File(saved, "SHA256SUMS").readLines().map { line ->
            val (hash, name) = line.split("  ", limit = 2)
            val actual = MessageDigest.getInstance("SHA-256").digest(File(saved, name).readBytes())
                .joinToString("") { "%02x".format(it) }
            assertEquals(hash, actual)
            name
        }.toSet()
        assertEquals(saved.walkTopDown().filter { it.isFile && it.name != "SHA256SUMS" }
            .map { it.relativeTo(saved).invariantSeparatorsPath }.toSet(), covered)
        assertArrayEquals(byteArrayOf(4, 5, 6), raw.readBytes())
        // Existing evidence is never overwritten, including when Save is retried.
        assertThrows(IllegalStateException::class.java) { writeScanDiagnostic(raw, result) { error("must not encode") } }
    }

    @Test fun failed_save_leaves_only_the_raw_pair_and_invalid_names_never_write() {
        val raw = raw()
        val result = fixture()
        assertThrows(IOException::class.java) { writeScanDiagnostic(raw, result) { throw IOException("fixture failure") } }
        assertEquals(setOf(raw.name, raw.name + ".json"), temp.root.list()!!.toSet())
        for (bad in listOf(emptyList(), result.files + result.files.first(),
            result.files.dropLast(1) + DiagnosticFile("../escape.json", "{}"))) {
            assertThrows(IllegalArgumentException::class.java) {
                writeScanDiagnostic(raw, result.copy(files = bad)) { error("must not encode") }
            }
        }
        assertEquals(setOf(raw.name, raw.name + ".json"), temp.root.list()!!.toSet())
        // Failure after canonical encoding must clean its incomplete directory too.
        Files.delete(File(raw.path + ".json").toPath())
        assertThrows(IOException::class.java) { writeScanDiagnostic(raw, result) { byteArrayOf(11) } }
        assertEquals(setOf(raw.name), temp.root.list()!!.toSet())
    }
}
