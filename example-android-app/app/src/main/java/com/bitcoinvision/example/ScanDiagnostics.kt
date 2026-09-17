package com.bitcoinvision.example

import java.io.File
import java.nio.file.Files
import java.security.MessageDigest
import uniffi.bitcoin_vision_mobile.DiagnosedScan

private val diagnosticNames = setOf("manifest.json", "final-boxes.jsonl", "word-links.jsonl",
    "readings.jsonl", "sources.jsonl", "numbering.jsonl", "ordering.jsonl", "joins.jsonl")

/** Explicit-save writer: immutable native result, already encoded crops, and a
 * canonical-photo encoder supplied by the caller. Never calls a scanner. */
internal fun writeScanDiagnostic(raw: File, result: DiagnosedScan, canonical: () -> ByteArray): File {
    check(BuildConfig.DEBUG)
    require(result.files.map { it.name }.toSet() == diagnosticNames && result.files.size == diagnosticNames.size)
    val directory = requireNotNull(raw.parentFile) { "Raw capture must have a directory" }
    val destination = File(directory, raw.name + ".scan")
    check(!destination.exists()) { "Diagnostic destination already exists" }
    val staging = Files.createTempDirectory(directory.toPath(), ".scan-writing-").toFile()
    try {
        val stage = File(staging, "stage").apply { check(mkdir()) }
        val crops = File(stage, "crops").apply { check(mkdir()) }
        File(staging, "photo.png").writeBytes(canonical())
        File(raw.path + ".json").copyTo(File(staging, "capture.json"))
        result.files.forEach { File(stage, it.name).writeText(it.text) }
        result.scan.words.forEachIndexed { index, word ->
            val name = "photo.png~w$index.png"
            File(crops, name).writeBytes(word.cropPng)
            word.originalOcr?.let { original ->
                val originals = File(stage, "original-crops").apply { mkdirs() }
                File(originals, name).writeBytes(original.cropPng)
            }
        }
        // Bind every exported byte; capture.json also binds the raw image saved
        // beside this bundle. No hashes, pixels or recognized words enter logs.
        val sums = staging.walkTopDown().filter { it.isFile }.sortedBy { it.relativeTo(staging).path }.map { file ->
            val digest = MessageDigest.getInstance("SHA-256")
            file.inputStream().use { input ->
                val bytes = ByteArray(8192)
                while (true) {
                    val count = input.read(bytes)
                    if (count < 0) break
                    digest.update(bytes, 0, count)
                }
            }
            digest.digest().joinToString("") { "%02x".format(it) } + "  " + file.relativeTo(staging).invariantSeparatorsPath
        }.joinToString("\n", postfix = "\n")
        File(staging, "SHA256SUMS").writeText(sums)
        Files.move(staging.toPath(), destination.toPath()) // no overwrite
        return destination
    } catch (error: Exception) {
        staging.deleteRecursively() // only this call's private, newly created staging directory
        throw error
    }
}
