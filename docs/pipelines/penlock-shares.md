# Reading a Penlock share

[Penlock](https://penlock.io) splits a BIP39 seed phrase into three shares with
a printable paper wheel, no electronics involved. Any two shares recover the
phrase; one on its own says nothing about it. Each share is twelve rows of six
symbols, written by hand on a worksheet and cut into a strip:

```text
penlock v1 share 2
 1. KV ABAN
 2. #= MOTH
```

The first two symbols of a row are that word's checksum; the last four are the
word itself. Symbols come from GF(29) — `=#ABCDEFGHIJKLMNOPQRSTUVWXYZ-` — which
is the alphabet every part of Penlock is written in, and the arithmetic that
recombines shares is arithmetic in that field.

This pipeline turns a photograph of such a strip into a distribution over the
29 symbols for every one of its 72 cells.

## Stages

**Find the strip.** Two worksheets are in circulation and the strip is found
differently on each.

*Upstream's original worksheet* prints no scan aids at all. What it does print
is two shaded checksum boxes per row — twenty-four mid-grey squares in two
columns at the row pitch, at known positions — whose centres fit the same
homography a fiducial would give. That lattice is its own image under a
half-turn, so which way up the strip is cannot come from the lattice: upstream
draws a dotted line along the bottom of each row's boxes and nothing along the
top, and which side and edge that line runs along is the orientation.

*The registered worksheet* this project renders prints four solid dark corner
squares, with a smaller solid square beside the top-left one saying which
corner that is. A quadruple is accepted only when its homography puts that mark
where the strip says it should be and gives every square its expected size, so
clutter elsewhere cannot pass for a corner.

**Read which strip it is — when the paper says.** The registered worksheet
prints an identity code at six positions, read by whether a blob is present at
each and decoded through a distance-4 codebook. One wrong mark is corrected;
two make the strip `Identity::Unknown` — never a different strip, which matters
because recovering with the wrong share number produces confident nonsense. The
original worksheet carries no such code, so on those strips the share number is
the caller's to supply, from the number written at the top.

**Cut the cells.** The rectified card is cut into one image per symbol, each
normalised the same way so the classifier sees the same framing whatever the
photograph was: ink is whatever is darker than a fraction of the *local* paper
white, which drops the grey cell borders and the checksum shading with it; the
ink's bounding box is scaled to fit a 51-pixel box, centred in a 128-pixel
cell, black on white.

**Classify.** `cell-reference.onnx` is a project-authored 29-way classifier
over those cell images, loaded from bytes the caller supplies. It is the only
recogniser here: the research harness keeps others to compare against, with the
harness, so nothing in this repository can be asked to send a cell image
anywhere. Every cell comes back as a distribution, not a letter;
cells it cannot read are returned as uniform and listed in `unread`, so a
caller can tell "I could not read this" from "I read a `Q`".

**Recover.** Two shares recombine to the phrase. The per-word checksum is what
catches a hand's slip: it can correct a single wrong symbol in a word, and
reports the correction rather than applying it silently.

## Where it lives

`example-android-app/rust/penlock-scan` owns the photograph-to-symbols half
(`locate`, `cells`, `model`, `scan`), and `example-android-app/rust/penlock`
owns the arithmetic (`split`, `recover`, `verify`, and the wheel the paper
version is). Both are plain Rust crates. `penlock-scan` also renders
worksheets and strips (`sheet`, `wordsheet`, `template`, `pdf`), which is how
the test fixtures are made.

Its entry point is shaped like the word-list pipeline's, because the two answer
the same kind of question:

```rust,no_run
use penlock_scan::{ModelBytes, ScanOptions, Scanner};

let scanner = Scanner::new(ModelBytes { cells: &model_bytes })?;
let result = scanner.scan(&photo_bytes, ScanOptions::default())?;
for strip in result.document().observations {
    // strip.kind, strip.share, strip.quad, strip.confidence, strip.text
    // and strip.words: twelve words of six cells, each with its probability
}
```

`scan` takes encoded bytes and applies EXIF orientation once; `scan_rgb` takes
canonical pixels. `scan_all` reads every strip on an uncut sheet. Decoding and
its limits — 64 MiB encoded, 40 million pixels — are the word-list pipeline's
`decode_image`, so both read the same files the same way rather than each
carrying its own decoder. `document()` projects decisions already made; it does
not rectify or reclassify anything.

The model is `example-android-app/app-models/cell-reference.onnx`; it belongs
to the app's workspace because `penlock-scan` is what loads it. The root
`bitcoin-vision` crate does not expose this pipeline.

## Limits

A share strip is small and its symbols are terse, so resolution matters more
than it does for a word list: a strip photographed at 1280x960 has been
unreadable where the same strip at phone resolution reads cleanly. On the
registered worksheet the identity code is the load-bearing part of getting this
right — an unrecognised strip is recoverable, a misrecognised one is not —
which is why its codebook refuses rather than guesses. On the original
worksheet nothing on the paper carries that, and a share number typed against
the wrong strip is the failure to watch for.
