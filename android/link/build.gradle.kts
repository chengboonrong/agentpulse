import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// The Android app's port of link/ (docs/mobile.md, "Android app (planned)"): plain
// Kotlin/JVM, so its tests run without an emulator. tink-android is a JAR and runs on
// API 26 (pure-Java X25519 and ChaCha20-Poly1305 where the platform has none).
plugins {
    kotlin("jvm")
    kotlin("plugin.serialization")
}

kotlin {
    jvmToolchain(17)
    compilerOptions.jvmTarget = JvmTarget.JVM_1_8
}

java {
    sourceCompatibility = JavaVersion.VERSION_1_8
    targetCompatibility = JavaVersion.VERSION_1_8
}

// Only the Java 8 API, roughly what Android 8 (API 26) has, so a newer JDK call fails here
// instead of on a phone. The tests may use the whole JDK.
tasks.compileKotlin {
    compilerOptions.freeCompilerArgs.add("-Xjdk-release=1.8")
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

// scripts/e2e-android.sh's phone: the pairing link on stdin.
tasks.register<JavaExec>("phoneSim") {
    classpath = sourceSets.test.get().runtimeClasspath
    mainClass = "io.github.chengboonrong.agentpulse.link.PhoneSimKt"
    standardInput = System.`in`
}
