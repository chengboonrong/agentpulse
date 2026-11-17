// The Android app's port of link/ (docs/mobile.md, "Android app (planned)"): plain
// Kotlin/JVM, so its tests run without an emulator. tink-android is a JAR and runs on
// API 26 (pure-Java X25519 and ChaCha20-Poly1305 where the platform has none).
plugins {
    kotlin("jvm") version "2.4.21"
    kotlin("plugin.serialization") version "2.4.21"
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    // HpkeContext lives in Tink's internal package: keep this pinned.
    implementation("com.google.crypto.tink:tink-android:1.24.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.13.0")
    testImplementation(kotlin("test"))
}

tasks.test {
    useJUnitPlatform()
    // Shared with link/'s Swift tests; `-PwriteVectors` rewrites kotlin.json.
    val vectors = rootDir.resolve("../link/Tests/vectors")
    inputs.dir(vectors)
    systemProperty("vectors", vectors.path)
    systemProperty("writeVectors", (findProperty("writeVectors") != null).toString())
}
