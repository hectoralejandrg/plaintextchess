# Spec Delta

## ADDED Requirements

### Requirement: Android App Project
The repository MUST contain an Android application project that bundles the chess core native libraries for the supported ABIs and, at launch, shows a placeholder screen that creates a game session through the generated Kotlin bindings and displays the initial board state and the player's current rating.

#### Scenario: Debug APK contains the native libraries
- **WHEN** the app project is built for debug
- **THEN** the resulting APK contains the chess core shared library for both the aarch64 and x86_64 ABIs

#### Scenario: Placeholder screen renders session data
- **WHEN** the app launches on a device or emulator matching a supported ABI
- **THEN** the placeholder screen displays the initial board state (standard start position) and the starting player rating without crashing, UnsatisfiedLinkError, or FFI errors

### Requirement: Self-Sufficient Android Build via Gradle Wrapper
The Android app project MUST be buildable with a committed Gradle wrapper so that no global Gradle installation is required; a machine with only a compatible JDK and the Android SDK MUST be able to build the app.

#### Scenario: Wrapper build on a clean machine
- **WHEN** the wrapper build entry point is run on a machine without a global Gradle installation
- **THEN** the build completes and produces the app package using the wrapper's pinned Gradle version
