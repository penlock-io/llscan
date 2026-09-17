package com.bitcoinvision.example

enum class Order { NUMBERS, COLUMNS, ROWS }

/** What the item numbers written beside the words made of the page. */
sealed interface Numbers {
    data object None : Numbers

    /**
     * The numbers read as a run 1..[count]: [missing] are the numbers no
     * word carries, [outOfPlace] the numbers not where the page puts
     * them, [unresolved] how many words carry a number that could not
     * be placed in the run.
     */
    data class Held(val count: Int, val missing: List<Int>, val outOfPlace: List<Int>, val unresolved: Int) : Numbers

    data class Inconsistent(val detail: String) : Numbers
}

/** Original keep state before an extent expansion hid this observation. */
data class ExpansionParent(val id: Int, val stray: Boolean)

data class OriginalOcr(val corners: List<Float>, val cropPng: ByteArray, val raw: String)
data class MatchingText(val text: String, val original: Boolean, val removedPrefixBytes: Int)
data class ObservedLabel(val literal: String, val ordinal: Int?, val origin: String, val ambiguous: Boolean,
    val corners: List<Float> = emptyList())

/** A word box the sheet was read from, as the review needs it. */
class Source(
    /** Four corners in photo pixels, x1, y1, ... x4, y4. */
    val corners: List<Float>,
    /** The model's ranked list words with probabilities, best first. */
    val ranked: List<Pair<String, Float>>,
    val accepted: Boolean,
    val cropPng: ByteArray,
    /** The word the phone shows first: the pick over the recogniser's
     * read and the model's five. */
    val read: String,
    /** The recogniser's raw read, empty without one. */
    val raw: String = "",
    /** Not a word by the phone's reckoning: left out until put back. */
    val stray: Boolean = false,
    /** The item number the page's numbering gives the word, if any. */
    val number: Int? = null,
    /** A word by its read but far from the phrase: a stray for where it sits. */
    val apart: Boolean = false,
    /** Original pieces of this union; mutually exclusive with the joined word. */
    val joinedFrom: List<Int> = emptyList(),
    /** One or two originals of an expanded crop; distinct from a native join. */
    val expandedFrom: List<ExpansionParent> = emptyList(),
    val originalOcr: OriginalOcr? = null,
    val matching: MatchingText? = null,
    val labels: List<ObservedLabel> = emptyList(),
    val labelConflict: Boolean = false,
) {
    val replacementParents: List<Int> get() = if (expandedFrom.isNotEmpty()) expandedFrom.map { it.id } else joinedFrom

    val probability: Float get() = ranked.first().second

    /** The pick came from the recogniser's read, not the model's top. */
    val fromRead: Boolean get() = read != ranked.first().first
}

/** One word of the phrase under review: read from a box, or typed. A
 * typed word may carry the item number of the gap it fills. */
data class Entry(val id: Int, val word: String, val source: Source?, val checked: Boolean, val number: Int? = null) {
    val typed: Boolean get() = source == null
    val corrected: Boolean get() = source != null && word != source.read
}

private class Typed(val id: Int, val after: Int, val number: Int? = null)

/**
 * The phrase as the person is shaping it from a read sheet. A draft
 * is immutable: every edit returns the next draft, so a check is
 * only ever true of the exact word it was given.
 *
 * A read word keeps its box's identity through every edit. A typed
 * word is anchored behind the box it was inserted after (or at the
 * start), so it keeps its place when the reading order switches.
 */
class PhraseDraft private constructor(
    private val boxes: Map<Int, Source>,
    private val columns: List<Int>,
    private val rows: List<Int>,
    private val byNumber: List<Int>,
    /** What the scan made of the page's numbers, as read: provenance,
     * not the state of the draft. */
    val scanNumbers: Numbers,
    val listNumbered: Boolean,
    val order: Order,
    val orderRequiresReview: Boolean,
    private val removed: Set<Int>,
    private val chosen: Map<Int, String>,
    private val typed: List<Typed>,
    private val checked: Set<Int>,
    private val nextId: Int,
) {
    /**
     * Strays start left out; `leftOut` lists them and `restore` puts one
     * back. A word the model is sure of starts ticked; the person's work
     * is the uncertain ones, and every tick stays theirs to undo.
     */
    constructor(
        boxes: List<Source>,
        columns: List<Int>,
        rows: List<Int>,
        order: Order,
        byNumber: List<Int> = columns,
        numbers: Numbers = Numbers.None,
        listNumbered: Boolean = numbers != Numbers.None,
        orderRequiresReview: Boolean = false,
    ) : this(
        boxes.withIndex().associate { it.index to it.value },
        columns,
        rows,
        byNumber,
        numbers,
        listNumbered,
        order,
        orderRequiresReview,
        boxes.withIndex().filter { it.value.stray }.map { it.index }.toSet(),
        emptyMap(),
        emptyList(),
        if (orderRequiresReview) emptySet() else confident(boxes.withIndex().associate { it.index to it.value }),
        boxes.size,
    )

    /** Whether the page's numbers were read as a run, so `Order.NUMBERS` means something. */
    val numbered: Boolean get() = scanNumbers is Numbers.Held

    private val sequence: List<Int>
        get() = when (order) {
            Order.NUMBERS -> byNumber
            Order.COLUMNS -> columns
            Order.ROWS -> rows
        }

    val entries: List<Entry> = buildList {
        fun entry(id: Int, source: Source?, number: Int?) =
            Entry(id, chosen[id] ?: source!!.read, source, id in checked, number)
        typed.filter { it.after == START }.forEach { add(entry(it.id, null, it.number)) }
        for (box in sequence) {
            if (box !in removed) add(entry(box, boxes.getValue(box), boxes.getValue(box).number))
            typed.filter { it.after == box }.forEach { add(entry(it.id, null, it.number)) }
        }
    }

    /** The page's numbers as the draft now stands: a number the scan
     * found no word for is missing until a typed word fills its gap,
     * and missing again when that word goes. */
    val numbers: Numbers
        get() = when (val scanned = scanNumbers) {
            is Numbers.Held -> {
                val carried = entries.mapNotNull { it.number }.toSet()
                scanned.copy(missing = scanned.missing.filter { it !in carried })
            }
            else -> scanned
        }

    val words: List<String> get() = entries.map { it.word }

    /** The boxes left out, strays and removed words alike, in the
     * current order, each as it would return. */
    val leftOut: List<Entry> = sequence
        .filter { it in removed }
        .map { Entry(it, chosen[it] ?: boxes.getValue(it).read, boxes.getValue(it), false) }

    /**
     * Where each number the page's run lacks belongs among the entries,
     * read by number: the index at which a word carrying it would be
     * inserted, before the first entry whose number is higher. Several
     * missing numbers at one place name the lowest. Under either
     * geometric order the place is no one's to say, and there is none.
     */
    fun missingAt(): Map<Int, Int> {
        val held = numbers as? Numbers.Held ?: return emptyMap()
        if (order != Order.NUMBERS) return emptyMap()
        val out = mutableMapOf<Int, Int>()
        for (k in held.missing.sorted()) {
            val at = entries.indexOfFirst { (it.number ?: Int.MAX_VALUE) > k }
                .let { if (it < 0) entries.size else it }
            out.putIfAbsent(at, k)
        }
        return out
    }

    val allChecked: Boolean get() = entries.isNotEmpty() && entries.all { it.checked }

    fun change(id: Int, word: String): PhraseDraft =
        copy(chosen = chosen + (id to word), checked = checked - id)

    fun remove(id: Int): PhraseDraft = copy(
        removed = if (id in boxes) removed + id else removed,
        typed = typed.filter { it.id != id },
        chosen = chosen - id,
        checked = checked - id,
    )

    /**
     * Inserts a typed word so that it becomes entry `at`, unchecked.
     * With [number], the word fills that gap in the page's run and
     * carries the number from then on; without, it carries none, even
     * at a gap's place.
     */
    fun insert(at: Int, word: String, number: Int? = null): PhraseDraft {
        val id = nextId
        val previous = entries.getOrNull(at - 1)
        val list = typed.toMutableList()
        when {
            previous == null -> list.add(0, Typed(id, START, number))
            previous.typed -> {
                val i = list.indexOfFirst { it.id == previous.id }
                list.add(i + 1, Typed(id, list[i].after, number))
            }
            else -> {
                val i = list.indexOfFirst { it.after == previous.id }
                list.add(if (i < 0) list.size else i, Typed(id, previous.id, number))
            }
        }
        return copy(typed = list, chosen = chosen + (id to word), nextId = nextId + 1)
    }

    /** Restore an interpretation, unchecked, never a replacement and its originals together. */
    fun restore(id: Int): PhraseDraft {
        val source = boxes[id] ?: return this
        if (source.replacementParents.isNotEmpty()) {
            val pieces = source.replacementParents.toSet()
            return copy(removed = (removed + pieces) - id, checked = checked - pieces - id)
        }
        val replacement = replacementOf(id)
        if (replacement != null) {
            val pieces = replacement.value.replacementParents.toSet()
            // Once the original interpretation is selected, an explicitly
            // restored stray may be kept individually, just like other strays.
            // Native joins retain their existing whole-pair restoration rule.
            if (replacement.value.expandedFrom.isNotEmpty() && replacement.key in removed) {
                return copy(removed = removed - id, checked = checked - pieces - replacement.key)
            }
            val originallyStray = replacement.value.expandedFrom.filter { it.stray }.map { it.id }.toSet()
            return copy(
                removed = ((removed + replacement.key) - pieces) + originallyStray,
                checked = checked - pieces - replacement.key,
            )
        }
        return copy(removed = removed - id, checked = checked - id)
    }

    private fun replacementOf(id: Int) = boxes.entries.firstOrNull { id in it.value.replacementParents }

    /** Whether a box belongs to mutually exclusive original/replacement interpretations. */
    fun isAlternative(id: Int): Boolean = boxes[id]?.replacementParents?.isNotEmpty() == true ||
        replacementOf(id) != null

    /** Name the full effect of restoration when it swaps interpretations. */
    fun restoreLabel(id: Int): String {
        val source = boxes[id]
        if (source?.expandedFrom?.isNotEmpty() == true) return "Use expanded word"
        if (source?.joinedFrom?.isNotEmpty() == true) return "Use joined word"
        val replacement = replacementOf(id) ?: return "Keep as a word"
        return when {
            replacement.value.expandedFrom.isEmpty() -> "Use original parts"
            replacement.key in removed -> "Keep as a word"
            replacement.value.expandedFrom.size == 1 -> "Use original word"
            else -> "Use original parts"
        }
    }

    /** Recognition confidence cannot confirm a new position. Preserve checks
     * only for entries that stay in exactly the same position; never re-tick. */
    fun reorder(order: Order): PhraseDraft {
        if (order == this.order) return this
        val reordered = copy(order = order, orderRequiresReview = true)
        val unchanged = reordered.entries.mapIndexedNotNull { position, entry ->
            entry.id.takeIf { entries.getOrNull(position)?.id == entry.id }
        }.toSet()
        return reordered.copy(checked = checked.intersect(unchanged))
    }

    fun check(id: Int, on: Boolean): PhraseDraft =
        copy(checked = if (on) checked + id else checked - id)

    private fun copy(
        order: Order = this.order,
        orderRequiresReview: Boolean = this.orderRequiresReview,
        removed: Set<Int> = this.removed,
        chosen: Map<Int, String> = this.chosen,
        typed: List<Typed> = this.typed,
        checked: Set<Int> = this.checked,
        nextId: Int = this.nextId,
    ) = PhraseDraft(boxes, columns, rows, byNumber, scanNumbers, listNumbered, order, orderRequiresReview, removed, chosen, typed, checked, nextId)

    private companion object {
        const val START = -1

        /** The boxes that open ticked: read with confidence, and words at all. */
        fun confident(boxes: Map<Int, Source>): Set<Int> =
            boxes.filterValues { it.accepted && !it.stray }.keys
    }
}

/** A worksheet strip holds one phrase of this many words. */
const val WORKSHEET_WORDS = 12

private fun named(numbers: List<Int>): String = when (numbers.size) {
    1 -> "word ${numbers[0]}"
    else -> "words " + numbers.dropLast(1).joinToString(", ") + " and " + numbers.last()
}

/** The review's status line. Passing is well formed, never right. A
 * number the page's run lacks is named before anything else; a phrase
 * that passes carries its ticks. */
fun phraseStatus(count: Int, passes: Boolean, numbers: Numbers = Numbers.None, ticked: Int? = null, orderNeedsReview: Boolean = false): String = (when {
    numbers is Numbers.Held && numbers.missing.isNotEmpty() ->
        "$count words: ${named(numbers.missing)} ${if (numbers.missing.size == 1) "was" else "were"} not found"
    count == WORKSHEET_WORDS && passes ->
        "$count words · checksum passes" + (ticked?.let { " · $it of $count ticked" } ?: "")
    count == WORKSHEET_WORDS ->
        "$count words · checksum fails: review the words"
    count == WORKSHEET_WORDS - 1 -> "$count words: a word is missing"
    count == WORKSHEET_WORDS + 1 -> "$count words: one word too many"
    else -> "$count words: this worksheet holds a $WORKSHEET_WORDS-word phrase"
}) + if (orderNeedsReview) " · check word order against paper" else ""

/** What the page's numbers left open, for the review to say under the
 * order; nothing when they were read as a complete run or not at all. */
fun numberingNote(numbers: Numbers, listNumbered: Boolean = numbers != Numbers.None): String? = when (numbers) {
    Numbers.None -> if (listNumbered) "Numbered list detected. Some numbers could not be resolved; check the reading order." else null
    is Numbers.Inconsistent ->
        "The numbers on the page contradict each other, so the words are in the order they sit on the page."
    is Numbers.Held -> listOfNotNull(
        numbers.outOfPlace.takeIf { it.isNotEmpty() }?.let {
            "${if (it.size == 1) "Number" else "Numbers"} ${it.joinToString(", ")} ${if (it.size == 1) "is" else "are"} not where the page puts ${if (it.size == 1) "it" else "them"}: check the order."
        },
        numbers.unresolved.takeIf { it > 0 }?.let {
            "A number beside $it ${if (it == 1) "word" else "words"} could not be read."
        },
    ).joinToString(" ").ifEmpty { null }
}
