plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.hectoralejandrg.plaintextchess"
    compileSdk = 35

    // Required in AGP 8.5.x: composeOptions alone no longer activates the
    // Compose compiler - AGP only adds androidx.compose.compiler:compiler to
    // the Kotlin task classpath when the compose build feature is on.
    buildFeatures {
        compose = true
    }

    defaultConfig {
        applicationId = "com.hectoralejandrg.plaintextchess"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    composeOptions {
        kotlinCompilerExtensionVersion = "1.5.14"
    }

    // The UniFFI-generated Kotlin binding (package uniffi.chess_core) is
    // produced by scripts/build-android.sh into ../../target/uniffi/android.
    // Compiling it into the main source set keeps the app self-contained:
    // nothing generated is committed to the repo.
    sourceSets["main"].kotlin.srcDir("../../target/uniffi/android")
}

dependencies {
    // BOM 2024.08.00 pins Compose UI 1.6.8, the runtime pairing for
    // Kotlin 1.9.24 + Compose compiler 1.5.14 (see D5 in the change design).
    val composeBom = platform("androidx.compose:compose-bom:2024.08.00")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.9.0")
    // The UniFFI 0.28 Kotlin bindings call Rust through JNA. The AAR packaging
    // ships the native dispatcher (libjnidispatch.so) for the Android ABIs,
    // which JNA loads via System.loadLibrary at startup (required on Android,
    // per the official UniFFI Kotlin/Gradle docs).
    implementation("net.java.dev.jna:jna:5.14.0@aar")
    debugImplementation("androidx.compose.ui:ui-tooling")
}

// ---------------------------------------------------------------------------
// ChessCore artifacts: jniLibs (src/main/jniLibs/<abi>/libchess_core.so) and
// the UniFFI Kotlin binding are produced by scripts/build-android.sh. The
// preBuild hook only regenerates them when they are missing, so incremental
// builds never rebuild the Rust core needlessly.
// ---------------------------------------------------------------------------
val coreLib = File(projectDir, "src/main/jniLibs/arm64-v8a/libchess_core.so")
val coreBinding = File(projectDir, "../../target/uniffi/android/uniffi/chess_core/chess_core.kt")

tasks.register<Exec>("ensureCore") {
    description = "Builds the Rust core artifacts when jniLibs or the Kotlin binding are missing."
    onlyIf {
        val missing = !coreLib.exists() || !coreBinding.exists()
        if (missing) {
            logger.lifecycle("[ensureCore] core artifacts missing - running scripts/build-android.sh")
        } else {
            logger.lifecycle("[ensureCore] jniLibs and Kotlin binding present - skipping build-android.sh")
        }
        missing
    }
    workingDir(File(rootDir, ".."))
    commandLine("./scripts/build-android.sh")
}

tasks.named("preBuild") {
    dependsOn("ensureCore")
}
