# livenyan

Reproductor nativo (Rust + SDL3) de juegos [LiveMaker](https://www.livemaker.net/).
Reimplementa formatos y comandos documentados (base [pylivemaker](https://github.com/pmrowla/pylivemaker));
no es un clon bit a bit del runtime Win32 ni un cliente de un solo juego.

**Compatibilidad** (qué hay, huecos, interpretaciones): [`COMPATIBILITY.md`](./COMPATIBILITY.md).  
**Cómo reportar fallos / contribuir:** [`CONTRIBUTING.md`](./CONTRIBUTING.md).  
Interfaz (menú, LFT, media): [`LIVEMAKER-INTERFACE-REFERENCE.md`](./LIVEMAKER-INTERFACE-REFERENCE.md).

Cursor y Copilot se usaron de forma extensiva en el desarrollo; ver CONTRIBUTING.

## Compilar

Arch Linux (hace falta **SDL ≥ 3.4**):

```sh
sudo pacman -S --needed rust sdl3 ffmpeg fontconfig noto-fonts-cjk
cargo build --release
```

Sin SDL 3.4 del sistema (u otros SO): `cargo build --release --features static-sdl`
(necesita `cmake` y toolchain C/C++).

Binario: `target/release/livenyan`.

## Uso

```sh
livenyan /ruta/juego.exe
livenyan /ruta/al/juego/
livenyan --info juego.exe
livenyan juego.exe --debug-layout   # wireframe; no muestra gráficos del juego
```

Opciones útiles: `--fullscreen`, `--mute`, `--font RUTA`, `--res WxH`,
`--scale-filter linear|nearest|sharp|native`, `--fps-meter`, `--config RUTA`.

Config: `$XDG_CONFIG_HOME/livenyan/config.toml` (ver `config.example.toml`).  
Saves: `$XDG_DATA_HOME/livenyan/<juego>/`. Los archivos del juego no se modifican.

Traducciones: carpeta `translations/*.toml` junto al juego (exportar con
`--dump-texts DIR` o desde Opciones). Detalle de marcadores `{{LV:…}}` en el
comentario del catálogo exportado. El selector incluye banderas PNG de 96×64
en `assets/flags/96x64/`: `es.png` para `es.toml` y `ja.png` para `ja.toml`.

### Depuración

| Opción / variable | Qué hace |
|---|---|
| `--trace [--steps N]` | VM sin ventana |
| `--dump-lsb`, `--gal-stats` | inspección de script / GAL (stats, no pixels) |
| `LIVENYAN_LOG` | `warn` por defecto; `info` / `debug` para tiempos y texturas |

## Pruebas

Fixtures sintéticos (sin datos de juegos reales):

```sh
pip install pylivemaker
python3 tools/gen_fixtures.py tests/fixtures
cargo test
```

Tests `#[ignore]`: `LIVENYAN_TEST_GAME_DIR` y `LIVENYAN_TEST_FONT` (ver CONTRIBUTING / issues).

## Licencia

GPL-3.0-or-later (derivado de pylivemaker). Ver `LICENSE`.
