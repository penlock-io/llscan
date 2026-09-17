plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val scannerRoot = rootDir.parentFile
val nativeRoot = rootDir.resolve("rust")
val nativeTarget = nativeRoot.resolve("target")
val captureDiagnostic = providers.gradleProperty("captureDiagnostic").orNull.let {
    require(it == null || it == "true") { "captureDiagnostic must be omitted or true" }
    it == "true"
}
if (captureDiagnostic) {
    require(gradle.startParameter.taskNames.none { it.contains("release", ignoreCase = true) }) {
        "Capture diagnostics are debug-only"
    }
    layout.buildDirectory.set(file("build-capture-diagnostic"))
}

android {
    namespace = "com.bitcoinvision.example"
    compileSdk = 35
    defaultConfig {
        // Preserve the full app identity and wallet storage namespace.
        applicationId = if (captureDiagnostic) "com.bitcoinvision.example.capturediag" else "com.bitcoinvision.example"
        buildConfigField("boolean", "SCAN_BENCHMARK", "false")
        minSdk = 29
        targetSdk = 34
        versionCode = 1
        versionName = "0.1"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += "arm64-v8a" }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true; buildConfig = true }
    sourceSets["main"].java.srcDir(layout.buildDirectory.dir("generated/uniffi"))
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("rust/jniLibs"))
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("generated/assets"))
    sourceSets["main"].res.srcDir(layout.buildDirectory.dir("generated/fontres"))
    if (captureDiagnostic) {
        sourceSets["debug"].manifest.srcFile("src/captureDiagnostic/AndroidManifest.xml")
        sourceSets["debug"].java.srcDir("src/captureDiagnostic/java")
    }
}
androidComponents {
    beforeVariants(selector().withBuildType("release")) { variant ->
        if (captureDiagnostic) variant.enable = false
    }
}

val cargoNdk = tasks.register<Exec>("cargoNdk") {
    workingDir = nativeRoot
    environment("CARGO_TARGET_DIR", nativeTarget.absolutePath)
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "-o",
        layout.buildDirectory.dir("rust/jniLibs").get().asFile.absolutePath,
        "build", "--locked", "--release", "-p", "bitcoin-vision-mobile")
    // cargo ndk copies every library in the target directory, so a crate's old name would ship beside it.
    doLast {
        fileTree(layout.buildDirectory.dir("rust/jniLibs")) { exclude("**/libbitcoin_vision_mobile.so") }.forEach { it.delete() }
    }
}
val uniffiBindings = tasks.register<Exec>("uniffiBindings") {
    dependsOn(cargoNdk)
    doFirst { delete(layout.buildDirectory.dir("generated/uniffi")) }
    workingDir = nativeRoot
    environment("CARGO_TARGET_DIR", nativeTarget.absolutePath)
    commandLine("cargo", "run", "--locked", "-p", "bitcoin-vision-mobile", "--features", "bindgen",
        "--bin", "uniffi-bindgen", "--", "generate", "--library",
        nativeTarget.resolve("aarch64-linux-android/release/libbitcoin_vision_mobile.so").absolutePath,
        "--language", "kotlin", "--out-dir",
        layout.buildDirectory.dir("generated/uniffi").get().asFile.absolutePath)
}
val copyModels = tasks.register<Copy>("copyModels") {
    from(rootDir.resolve("app-models/cell-reference.onnx"))
    for (name in listOf("word-detector.onnx", "word-reference.onnx", "word-reference.calibration",
        "text-recogniser.onnx", "text-recogniser.dict.txt")) {
        from(scannerRoot.resolve("models/$name"))
    }
    into(layout.buildDirectory.dir("generated/assets"))
}
val copyFonts = tasks.register<Copy>("copyFonts") {
    from(nativeRoot.resolve("penlock-scan/fonts/IBMPlexMono-Regular.ttf")) { rename { "ibm_plex_mono.ttf" } }
    from(nativeRoot.resolve("penlock-scan/fonts/IBMPlexSans-Variable.ttf")) { rename { "ibm_plex_sans.ttf" } }
    into(layout.buildDirectory.dir("generated/fontres/font"))
}
val copyNotices = tasks.register<Copy>("copyNotices") {
    from(scannerRoot.resolve("LICENSE"))
    from(scannerRoot.resolve("LICENSE-APACHE"))
    from(scannerRoot.resolve("models/NOTICE")) { rename { "PADDLE-NOTICE" } }
    from(nativeRoot.resolve("penlock-scan/fonts")) { include("OFL-*.txt") }
    into(layout.buildDirectory.dir("generated/assets/licenses"))
}
tasks.named("preBuild") { dependsOn(uniffiBindings, copyModels, copyFonts, copyNotices) }

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.navigation:navigation-compose:2.8.5")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    val camerax = "1.4.1"
    implementation("androidx.camera:camera-core:$camerax")
    implementation("androidx.camera:camera-camera2:$camerax")
    implementation("androidx.camera:camera-lifecycle:$camerax")
    implementation("androidx.camera:camera-view:$camerax")
    implementation("androidx.exifinterface:exifinterface:1.3.2")
    implementation("net.java.dev.jna:jna:5.15.0@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation(composeBom)
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test:rules:1.6.1")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.6.1")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
