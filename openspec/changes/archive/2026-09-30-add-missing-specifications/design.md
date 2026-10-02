# Design

## Context

El proyecto `plaintextchess-monorepo` actualmente utiliza OpenSpec para desarrollo specification-driven pero carece de 9 especificaciones críticas que están documentadas en el plan.md. Estas especificaciones faltantes cubren áreas esenciales:

- Validación de movimientos de ajedrez en Rust
- Sistema Glicko-2 de rating de jugadores
- Contratos FFI entre Rust y aplicaciones móviles
- Manejo de gestos táctiles para iOS y Android
- Optimización de rendimiento para iOS
- Componentes Material Design para Android
- Requisitos cross-platform
- Especificaciones de localización multi-idioma

El diseño actual incluye:
- Arquitectura multi-crate con `chess-core` crate Rust
- Aplicación iOS SwiftUI con tablero 8x8 y vistas MVVM
- Aplicación Android Jetpack Compose con drag-and-drop nativo
- Estructura existente de especificaciones en `openspec/specs/`

El sistema FFI actual incluye `get_board_state()`, `get_valid_moves()`, `play_move()`, y funciones relacionadas de rating. Las especificaciones UI actuales cubren interfaces de tablero, piezas y controles de juego básicos, pero no incluyen gestos avanzados o detalles de componentes Material Design.

## Goals / Non-Goals

### Goals
- Completar el ecosistema de especificaciones del proyecto para asegurar coordinación completa entre Rust y aplicaciones móviles
- Establecer contratos FFI bien-documentados para mantener compatibilidad cross-platform
- Establecer especificaciones de manejo de gestos para experiencia de usuario consistente
- Documentar optimizaciones de rendimiento y componentes Material Design
- Proporcionar especificaciones cross-platform y de localización que sirvan como fuente única de verdad para equipo multi-equipo

### Non-Goals
- Reimplementar las funcionalidades existentes del núcleo de ajedrez Rust
- Re-escribir la arquitectura de aplicaciones móviles SwiftUI o Jetpack Compose
- Agregar nuevas características más allá de la documentación specification-driven
- Modificar las especificaciones UI existentes (solo agregar nuevas)
- Crear herramientas o utilidades para la implementación