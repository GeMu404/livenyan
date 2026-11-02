# Contribuir a livenyan

## Cómo colaborar (lo importante)

1. Jugá un juego hecho con LiveMaker en el runtime original y en livenyan.
2. Sacá capturas de **la misma escena** en ambos: original y livenyan.
3. Indicá qué falla (texto, menú, imagen, salto, audio, etc.).
4. Abrí un [issue](../../issues) con esas capturas y una descripción corta.

No hace falta un informe de ingeniería ni un prompt para la IA. Eso alcanza.

Nunca subas al repo el juego, assets extraídos ni dumps con nombres/textos de un título concreto.
Las capturas del issue sirven para mostrar el fallo; no las metas en commits ni en fixtures.

## Herramientas de IA

Cursor y GitHub Copilot se usaron **de forma extensiva** en el desarrollo.
No hace falta (ni conviene) poner `Co-authored-by: Cursor` / `Copilot` en cada commit:
el log se llena de ruido. Bastá con esta nota. En el código: cero menciones a la herramienta.

## Commits (si mandás código)

- Español, imperativo, asunto ≤72 caracteres.
- Prefijo de ámbito en minúsculas: `vm:`, `gal:`, `texto:`, `render:`, `video:`, `docs:`, …
- Un asunto por commit. El cuerpo dice **por qué** y cita `docs/livenovel/…` o una entrada
  H\*/I\* de [`COMPATIBILITY.md`](COMPATIBILITY.md).
- Plantilla opcional: `git config commit.template .gitmessage`.

```
gal: leer la duración del frame de la cabecera Gale

Los bytes 4..8 tras la máscara dan ms plausibles (H23).
Sin ellos las animaciones iban a ritmo fijo.
```

## Comentarios en código

- Solo lo que el código no dice: porqué, fuente o duda.
- Suposición sin fuente → etiqueta (`no confirmado`, `inventado`, `experimento`…) y H\* si mueve comportamiento.
- `unsafe` con `// SAFETY:`; `#[allow(…)]` con motivo en la misma línea.
- Sin banners, sin “antes pasaba X” (eso va en el commit).

## Antes de abrir un PR

```
cargo fmt --check
cargo clippy --all-targets
cargo test --lib
```
