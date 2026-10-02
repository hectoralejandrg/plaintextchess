// Root build file for the PlainTextChess Android app.
// Pinned toolchain: Gradle 8.7 (wrapper), AGP 8.5.2, Kotlin 1.9.24.
// The Compose compiler (1.5.14, targets Kotlin 1.9.24) is enabled per
// module via android.composeOptions - 1.x releases are not Gradle plugins.
plugins {
    id("com.android.application") version "8.5.2" apply false
    id("org.jetbrains.kotlin.android") version "1.9.24" apply false
}
