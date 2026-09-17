# Reading a handwritten BIP39 word list

A recovery phrase written out by hand — twelve or twenty-four English BIP39
words, usually numbered, often in columns — photographed and turned back into
ordered words with the evidence for every one of them.

## Stages

**Decode the photograph.** JPEG, PNG or WebP, with EXIF orientation applied
exactly once; every polygon returned afterwards refers to that canonical image.
Accepted images are never silently resized. The limits are 64 MiB encoded and
40 million pixels.

**Find the writing.** `word-detector.onnx` proposes text regions, which are
then split and joined into one box per word.

**Work out the layout.** Printed or written numbers down the left of a column
establish where the list is and what order it runs in: the rows a guide's
numbers occupy bound its list, a guide may reach its words across blank paper
but never across writing, and marks cannot invent a label column on a page
whose numbers were never read. Reading order comes from that geometry. A
checksum never chooses an order — it is an output, not permission to reorder.

**Cut once.** The final polygon is the crop both models read: no padding, no
second opinion about where the word was.

**Read it two ways.** `text-recogniser.onnx` gives a literal OCR read of the
crop — whatever is written, word or not. `word-reference.onnx` gives a
probability for each of the 2048 BIP39 words. These are different questions and
the output keeps them apart.

**Select.** When the literal read is itself a BIP39 word, that is taken as
written. Otherwise the candidates are ranked by `log10(probability)` minus a
weighted edit distance from the read — one edit costs about one decade of
probability. A calibration file carries the temperature and the confidence
thresholds, bound to the classifier's SHA-256 so a model and the numbers fitted
to it cannot drift apart.

**Refuse.** The classifier has 2048 words and must answer with one of them, so
its confidence about a printed `Wallet name` says nothing about whether that is
a word. A read whose shape rules it out — too long for any list word, or
carrying more than one — is refused on that ground, but only once it is already
far from the list: a misspelling or a split read of a real word stays near the
list and never reaches the test, and a box the page's own geometry supports is
kept regardless. Refused observations are retained with the reason rather than
dropped.

## What comes out

Ordered words and their photo-coordinate polygons; the exact crop each model
read; the literal OCR text; every candidate with its probability and, where the
ranking ran, its hybrid score; the branch that selected the word and whether it
was confirmed; excluded observations with their reasons; and whether the
assembled phrase's checksum is valid.

`probability` is a model probability. `hybrid_score` is a ranking statistic and
is not one. A missing score means the executed branch did not rank that
candidate — not that its support was zero.

## Where it lives

The root `bitcoin-vision` crate: `Scanner::bundled()` loads the five bundled
artifacts, `scan` takes encoded bytes and `scan_rgb` takes canonical pixels,
and `ScanResult::document()` is a serializable projection of decisions already
made — it does not re-run inference or re-assign boxes.

The models are in [`models/`](../../models/README.md) with their digests.

## Limits

English BIP39 is the whole vocabulary; this is not generic handwriting OCR.
Low contrast, ambiguous letters, unusual layouts and missing strokes all cause
mistakes, and a confident wrong word is the failure mode to expect. A valid
checksum does not establish correct recognition: a twelve-word phrase carries
four checksum bits, so one wrong word still passes about one time in sixteen.
