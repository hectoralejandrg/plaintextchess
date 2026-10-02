# Spec Delta

## Purpose
Define los requisitos y especificaciones para el núcleo de Rust que soporta el motor de ajedrez multi-plataforma.

## ADDED Requirements

### Requirement: Validación de movimiento legal
El sistema DEBE validar que todos los movimientos de ajedrez sean legales según las reglas estándar del ajedrez.

#### Scenario: Validación de movimiento legal para todas las piezas
- **WHEN** usuario intenta jugar cualquier movimiento de pieza
- **THEN** el sistema DEBE rechazar movimientos ilegales

#### Scenario: Validación de movimiento en jaque
- **WHEN** rey está en jaque
- **THEN** el sistema DEBE restringir movimientos

#### Scenario: Validación de movimiento para cada pieza
- **WHEN** usuario mueve rey, reina, torre, alfil, caballo, peón
- **THEN** el sistema DEBE aplicar movimiento específico de pieza

### Requirement: Sistema de rating Glicko-2
El sistema DEBE calcular y actualizar ratings de jugadores usando algoritmo Glicko-2.

#### Scenario: Cálculo de rating inicial
- **WHEN** nuevo jugador se une al juego
- **THEN** el sistema DEBE establecer rating inicial con desviación/volatilidad

#### Scenario: Actualización de rating después de partida
- **WHEN** partida termina contra oponente
- **THEN** el sistema DEBE calcular nuevo rating

#### Scenario: Persistencia de rating
- **WHEN** juego necesita guardarse o cargarse
- **THEN** el sistema DEBE serializar/deserializar rating