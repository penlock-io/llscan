# Full Android example app

This is the full bitcoin-vision example application, not a reduced scanner demo.
It includes photo capture/import, scan progress, word review/editing, layout
overrides, wallet storage, generated-seed photo backup, Penlock share-card recovery
and PSBT review/signing. It uses the adjacent `bitcoin-vision` Rust library and
models without a copied scanner implementation.

**This is a mainnet wallet and can sign real Bitcoin transactions.** Treat it as
experimental software. Never fund or use any phrase shown in this repository's
examples; those seeds are public. Do not import a real wallet just to try the
example. Release builds protect the screen from screenshots/recents; debug builds
remain capturable for development. No new upload service or network permission
is added by this example.

## Separate builds

Running Cargo in the repository root builds/tests only `bitcoin-vision`. It does
not invoke Gradle, build this app, generate Kotlin bindings, or need an Android SDK.
The app's support code lives in its own workspace under `rust/`.

For the Android app, install a JDK compatible with Gradle 8.11.1/AGP 8.7.3 (JDK 21
is used for verification), Android SDK platform 35, and Android NDK. The existing
app targets ARM64 devices running Android 10/API 29 or newer. Configure
`ANDROID_HOME` (or your local `local.properties` SDK path). Install the Rust
`aarch64-linux-android` target and `cargo-ndk` for your chosen Rust toolchain.
CI selects NDK 28.2.13676358; builds here have also used r29. No toolchain
installer runs implicitly. If a second Cargo is ahead of rustup's
on your `PATH` -- a package manager's, typically -- `:app:cargoNdk` fails
asking you to install a target you already have, because the target belongs to
the toolchain that Cargo did not run.

```sh
cd example-android-app
./gradlew :app:assembleDebug :app:testDebugUnitTest
```

Gradle builds `rust/bitcoin-vision-mobile`, generates Kotlin bindings with UniFFI, and
copies the shared model files/fonts. Generated outputs stay in `rust/target/`
and `app/build/`. The APK is `app/build/outputs/apk/debug/app-debug.apk`.
For a release build use `:app:assembleRelease`; no release signing key is bundled.

The application ID is `com.bitcoinvision.example`. **Installing it can update an
existing installation with that ID**, so do not install over a wallet you rely on.
It installs beside, not over, an older build that still carried the Penlock
identity; wallets saved under that identity stay with that install. Building
alone does not install anything. A separate debug-only camera diagnostic can be
built with `-PcaptureDiagnostic=true`; it uses `com.bitcoinvision.example.capturediag`.
There is no fixture benchmark variant in the public project.

Native support tests can be run explicitly with:

```sh
cargo test --locked --manifest-path rust/Cargo.toml --workspace --lib
```

Private photo corpora, PSBT fixtures, training tools and device dumps are not
required or included. The full-corpus regression harness stays in the research
repository; ordinary app builds do not copy private test images.

## Continuous integration

`.github/workflows/ci.yml` has an `android` job beside the crate's: it builds
the native library through `cargo-ndk`, generates the UniFFI bindings, runs the
JVM unit tests, assembles the debug APK and compiles the instrumentation
sources. A green run therefore means the app compiles, its unit tests pass and
it packages. **It does not mean the app was run.** Nothing on the runner starts
it, so anything that only shows up on a device -- the camera, a real scan, the
review journeys -- is not covered.

The instrumentation test that ships here is deliberately not run in CI. Doing
so needs three things: a second native target, since the app is `arm64-v8a`
only and hosted runners are x86_64 (about a minute to build, and a 35 MB
library to carry); and an emulator to run it on, driven either by a
third-party action with KVM access or by scripting one ourselves, in a workflow
whose only actions are two first-party ones pinned by digest. The first is
cheap; the second is where the minutes and the flakiness are, and the
convenient way to get it adds a dependency this repository has not needed. If that trade ever
looks different, the recipe is: add `x86_64` to the `cargo ndk -t` list and to
`abiFilters`, then run `:app:connectedDebugAndroidTest`.

## Licenses and assets

Project code and project-authored models use MIT OR Apache-2.0. Paddle/RapidOCR
models retain Apache-2.0; fonts retain their bundled OFL notices. These notices
are included in APK assets under `licenses/`. The two IBM Plex faces serve the UI;
the current worksheet renderer registers its three template faces, including
Patrick Hand and Caveat. Those files and their notices are retained to preserve
rendering behavior, not to include a training corpus.
