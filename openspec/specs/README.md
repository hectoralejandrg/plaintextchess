# OpenSpec Specs

This directory contains all specifications for the app-core-monorepo project.

## Overview

OpenSpec is a specification-driven development framework that helps teams and AI coding assistants agree on what to build before any code is written.

## Specs by Language

### Rust (Core)
See `rust-core/` for specifications related to the Rust chess engine.

### Swift (iOS)
See `ios/` for specifications related to the iOS UI.

### Kotlin (Android)
See `android/` for specifications related to the Android UI.

## Structure

Each spec directory contains:
- `.md` files with requirements and scenarios
- Implementation tasks and checklists
- Design decisions

## Navigation

- [Rust Core Specs](/openspec/specs/rust-core) - Core chess logic
- [iOS Specs](/openspec/specs/ios) - iOS UI and features  
- [Android Specs](/openspec/specs/android) - Android UI and features
- [Shared Specs](/openspec/specs/shared) - Cross-platform requirements

## How to Use

1. Read specs to understand requirements
2. Implement features following the spec
3. Use `/opsx:propose` to suggest changes
4. Archive completed changes with `/opsx:archive`