# Contribuir a livenyan

## Commits

- Español, imperativo, asunto de 72 caracteres como máximo.
- Prefijo de ámbito en minúsculas: `vm:`, `gal:`, `lpm:`, `lcm:`, `texto:`, `render:`,
  `audio:`, `video:`, `docs:`, `tests:`, `build:`.
- Un asunto por commit. El cuerpo explica **por qué** y cita el manual
  (`docs/livenovel/…`) o la entrada H\*/I\* de [`COMPATIBILITY.md`](COMPATIBILITY.md).
- Sin trailers de coautoría automáticos.
- Plantilla: `git config commit.template .gitmessage`.

Ejemplo:

```
gal: leer la duración del frame de la cabecera Gale

Los bytes 4..8 tras la máscara dan valores de ms plausibles (H23).
Sin ellos las animaciones iban a ritmo fijo.
```

## Comentarios

- Un comentario dice lo que el código no puede decir: el porqué, la fuente o la duda.
- Toda suposición sin fuente lleva su etiqueta (`no confirmado`, `inventado`, `experimento`…)
  y, si puede mover comportamiento, su H\* en `COMPATIBILITY.md`.
- Nada de banners de sección, pasos numerados que repiten el código ni historias de
  «antes pasaba X» (eso va en el mensaje del commit).
- `unsafe` siempre con `// SAFETY:` y el invariante que lo hace correcto.
- `#[allow(clippy::…)]` siempre con el motivo en la misma línea.

## Contenido del juego

Nunca se sube contenido de un juego: textos, imágenes, capturas, assets extraídos ni nombres
de scripts u objetos de un juego concreto. Tests con fixtures sintéticos; lo que necesite un
juego real va con `#[ignore]` y una variable de entorno (ver README → Pruebas).

## Antes de subir

```
cargo fmt --check
cargo clippy --all-targets
cargo test --lib
```
