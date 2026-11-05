plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.phonelink.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.phonelink.app"
        minSdk = 29 // Android 10
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        // Galaxy S24 FE (and nearly every phone) is arm64; x86_64 is for the emulator.
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }
}

dependencies {
    // Generated Rust bindings call into libphonelink_ffi.so through JNA.
    implementation("net.java.dev.jna:jna:5.17.0@aar")

    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-service:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")

    // QR scanning, fully offline, no Google Play Services.
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
}

// Fail early with a useful message instead of a crash at runtime.
val rustLib = file("src/main/jniLibs/arm64-v8a/libphonelink_ffi.so")
tasks.named("preBuild") {
    doFirst {
        if (!rustLib.exists()) {
            throw GradleException("Rust core not built. Run ./scripts/build-android.sh from the repository root.")
        }
    }
}
