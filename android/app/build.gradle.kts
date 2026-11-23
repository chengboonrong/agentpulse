// The Android app (docs/mobile.md, "Android app"): Compose, minimum SDK 26, on the
// phone side of android/link. AGP's built-in Kotlin compiles it.
plugins {
    id("com.android.application")
    kotlin("plugin.compose")
    kotlin("plugin.serialization")
}

// The workspace version, as the Mac and iPhone apps use.
val workspaceVersion = Regex("""(?m)^version = "([^"]+)"""")
    .find(rootDir.resolve("../Cargo.toml").readText())!!.groupValues[1]

android {
    namespace = "io.github.chengboonrong.agentpulse"
    compileSdk = 37
    androidResources { generateLocaleConfig = true }
    defaultConfig {
        applicationId = "io.github.chengboonrong.agentpulse"
        minSdk = 26
        targetSdk = 36
        // Play's version code (max over all tracks + 1), passed by fastlane (android/fastlane).
        versionCode = (findProperty("versionCode") as String?)?.toInt() ?: 1
        versionName = workspaceVersion
    }
    // The Play upload key, from the environment (android/README.md, "Google Play"). Without
    // it a release build is left unsigned.
    System.getenv("ANDROID_KEYSTORE_PATH")?.let { path ->
        val release = signingConfigs.create("release") {
            storeFile = file(path)
            storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
            keyAlias = System.getenv("ANDROID_KEY_ALIAS") ?: "upload"
            keyPassword = System.getenv("ANDROID_KEY_PASSWORD") ?: storePassword
        }
        buildTypes.getByName("release").signingConfig = release
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(project(":link"))
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.compose.material3.adaptive:adaptive-layout:1.3.0")
    implementation("androidx.compose.material3.adaptive:adaptive-navigation:1.3.0")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.13.0")
    // Scanning the pairing code: CameraX frames, ZXing's decoder (no Play services).
    implementation("androidx.camera:camera-camera2:1.6.2")
    implementation("androidx.camera:camera-view:1.6.2")
    implementation("com.google.zxing:core:3.5.4")
    // The home-screen widget, refreshed every 15 minutes.
    implementation("androidx.glance:glance-appwidget:1.2.0")
    implementation("androidx.work:work-runtime:2.12.0")
    testImplementation(kotlin("test-junit"))
}
