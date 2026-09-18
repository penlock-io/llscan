# Model inventory

| File | Role | Source / status |
| --- | --- | --- |
| word-detector.onnx | Text-region proposals | RapidOCRv3.9.2 PP-OCRv4 Chinese/mobile detector export |
| text-recogniser.onnx | Literal English OCR | RapidOCRv3.9.2 PP-OCRv4 English/mobile recogniser export |
| text-recogniser.dict.txt | OCR class labels | Extracted from that recogniser's `character` metadata |
| word-reference.onnx | English BIP39 classifier | Project-authored reference CNN; MIT OR Apache-2.0 |
| word-reference.calibration | Bound thresholds / preprocessing | Thresholds bound to the classifier they were fitted for |

No candidate models, training examples, benchmark photos, metrics tables or
recogniser-hunt extras are bundled here.

These files are committed, so the commit says which bytes they are; nothing
here repeats that in a digest. The two digests this repository does keep do a
job a commit cannot: a `.calibration` names the classifier it was fitted for,
so a caller cannot hand the API one model's weights with another's thresholds,
and the three upstream artifacts below are checkable against the URLs they came
from.

Verified against their published exports:

```
d2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9  word-detector.onnx
e8770c967605983d1570cdf5352041dfb68fa0c21664f49f47b155abd3e0e318  text-recogniser.onnx
5662df9d2d03f0e8ca0d3b0649d6acbab904b6a14b3d3521463c71c37c668ce3  text-recogniser.dict.txt
```

The detector and recogniser bytes match the pinned RapidOCRv3.9.2 exports:
[detector](https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_mobile.onnx),
[recogniser](https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/rec/en_PP-OCRv4_rec_mobile.onnx).
Their upstream projects publish Apache-2.0 licenses:
[RapidOCRv3.9.2](https://github.com/RapidAI/RapidOCR/blob/v3.9.2/LICENSE),
[PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR/blob/main/LICENSE).
The [RapidOCR model repository](https://www.modelscope.cn/RapidAI/RapidOCR)
declares Apache-2.0, as do PaddlePaddle's model cards for the
[detector](https://huggingface.co/PaddlePaddle/PP-OCRv4_mobile_det) and
[English recogniser](https://huggingface.co/PaddlePaddle/en_PP-OCRv4_mobile_rec).
Retain [LICENSE-APACHE](LICENSE-APACHE) and [NOTICE](NOTICE) when redistributing
these artifacts. They are unmodified exports, byte-verified against the pinned
download URLs. The project's dual license does not replace their Apache-2.0 terms.

There is no runtime fetch and no script that downloads substitute weights if a
file is missing.
