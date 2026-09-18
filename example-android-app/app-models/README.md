# Additional app model

`cell-reference.onnx` is the project-authored 29-symbol classifier used for
Penlock share cards. It is distributed under MIT OR Apache-2.0, like the project's
own code. It is separate from the BIP39 word classifier, and it lives here rather than in
the repository's `models/` directory because `penlock-scan` is the crate that
loads it: the root `bitcoin-vision` crate neither embeds nor exposes it.

The Android build copies this file and the five existing files from the parent
`models/` directory into APK assets. It never downloads replacements. The
Paddle/RapidOCR artifacts retain the licenses and attribution in
[models/NOTICE](../../models/NOTICE); the app also includes those notices in assets.
