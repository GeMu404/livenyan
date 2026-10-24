# livenyan

Reproductor nativo y libre (Rust + SDL3) para juegos hechos con LiveMaker.
Port de los formatos documentados por [pylivemaker](https://github.com/pmrowla/pylivemaker).
Prototipo v0: arranca, interpreta scripts, muestra imágenes y texto. Muchos juegos aún no serán jugables.

## Compilar

Arch Linux:

```sh
sudo pacman -S --needed rust sdl3 fontconfig noto-fonts-cjk
cargo build --release
```

Sin SDL3 del sistema (o en Windows): `cargo build --release --features static-sdl`
(necesita `cmake` y un compilador de C).

Binario: `target/release/livenyan`

## Uso

```sh
livenyan /ruta/juego.exe          # lee el archivo VF dentro del exe, sin extraer nada
livenyan /ruta/al/juego/          # carpeta del juego (.exe/.dat/.ext) o archivo ya extraído
livenyan --info juego.exe         # estadísticas, resolución, script de entrada
livenyan --dump-lsb juego.exe 00000001.lsb
livenyan --trace --steps 20000 juego.exe   # VM sin ventana; avanza y elige la 1ª opción
```

`--info`, `--dump-lsb` y `--trace` **no muestran el texto del juego**; usa `--show-text` para verlo.
Otras opciones: `--res 1024x768`, `--font RUTA`, `--entry NOMBRE`, `--threads N`, `--fullscreen`, `--mute`, `--config RUTA`.

Tiempos de arranque: `LIVENYAN_LOG=info livenyan juego.exe`.

- Configuración: `$XDG_CONFIG_HOME/livenyan/config.toml` (ver `config.example.toml`).
- Partidas: `$XDG_DATA_HOME/livenyan/<juego>/`.
- Nunca se modifican los archivos del juego.

## Ventana

Ventana normal redimensionable (va bien en Hyprland). Se dibuja a la resolución lógica del juego
y se presenta con letterbox (bandas negras, aspecto correcto); el ratón se mapea a coordenadas lógicas.
F11 o Alt+Enter: pantalla completa. SDL3 elige Wayland si está disponible; no se fuerza X11.

## Estado

Implementado: archivo VF (exe/dat/partido, zlib, cifrado LM Pro), LSB completo (52+ comandos parseados),
expresiones, VM (Calc, If/Elseif/Else, Call/Exit, Jump/Label, While*, Break/Continue, Wait, VarNew/VarDel),
objetos (ImgNew/MesNew/BoxNew/TileNew/Button, SetProp/GetProp/ObjDel, PropMotion lineal), TextIns con
efecto máquina de escribir, menús básicos, sonido wav/ogg, decodificación en hilos con prefetch.

Pendiente: vídeo (.wmv se salta), transiciones Flip, guardado/carga, historial, eventos de texto,
lógica completa de menús/botones de LiveNovel, Timer, fuentes/estilos del juego.

## Pruebas

Los fixtures son sintéticos (sin datos de juegos reales):

```sh
pip install pylivemaker
python3 tools/gen_fixtures.py tests/fixtures
cargo test
```

## Licencia

GPL-3.0-or-later (derivado de pylivemaker, GPL-3.0). Ver `LICENSE`.
