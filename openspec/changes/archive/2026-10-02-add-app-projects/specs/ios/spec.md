# Spec Delta

## ADDED Requirements

### Requirement: iOS App Project
The repository MUST contain an iOS application project that compiles against `AppCore.xcframework` and, at launch, shows a placeholder screen that creates a game session through the generated Swift bindings and displays the initial board state and the player's current rating.

#### Scenario: App build for the simulator
- **WHEN** the app project is built for the iOS simulator
- **THEN** the build completes successfully with the chess core framework linked

#### Scenario: Placeholder screen renders session data
- **WHEN** the app launches on a simulator
- **THEN** the placeholder screen displays the initial board state (standard start position) and the starting player rating without crashing or FFI errors

### Requirement: iOS XCFramework Simulator Support
The iOS build MUST produce `AppCore.xcframework` containing both the device slice and the Apple-simulator slice, so the framework can be linked by app builds targeting either destination.

#### Scenario: Build produces both slices
- **WHEN** the iOS build script runs
- **THEN** the resulting XCFramework contains one device slice and one simulator slice, each with the chess core static library and a valid framework Info.plist

#### Scenario: Simulator app uses the simulator slice
- **WHEN** an app build targets the iOS simulator
- **THEN** the correct simulator slice of the framework is selected and the app runs on the simulator
