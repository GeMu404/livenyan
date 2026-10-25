# livenyan

Reproductor nativo y libre (Rust + SDL3) para juegos hechos con LiveMaker.
Port de los formatos documentados por [pylivemaker](https://github.com/pmrowla/pylivemaker).
Prototipo v0: arranca, interpreta scripts, muestra imágenes y texto. Muchos juegos aún no serán jugables.

## Compilar

Arch Linux:

```sh
sudo pacman -S --needed rust sdl3 ffmpeg fontconfig noto-fonts-cjk
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
livenyan juego.exe --debug-layout # contornos y etiquetas; oculta sprites y animaciones
```

`--info`, `--dump-lsb` y `--trace` **no muestran el texto del juego**; usa `--show-text` para verlo.
`--debug-layout` abre una ventana de diagnóstico con cajas por tipo de objeto y límites individuales
de botones `.lpm`; oculta las imágenes y animaciones, pero conserva el texto del juego dentro de
sus cajas con el mismo formato que en el renderizado normal.
Los componentes `Movie` WMV se decodifican con `ffmpeg` (debe estar en `PATH`) y se muestran como
animaciones; la repetición, pausa y detención siguen las propiedades y comandos del script.
Otras opciones: `--res 1024x768`, `--font RUTA`, `--entry NOMBRE`, `--threads N`, `--fullscreen`, `--mute`, `--config RUTA`.
Al acercar el ratón al borde superior aparece una barra con Salir, Pantalla completa y Opciones.
Opciones permite apagar o activar el fundido de GAL animados de imagen cuyo objeto o grupo visual
ocupe al menos un tercio de la resolución lógica. Los grupos se deducen de un `TAG` explícito (incluido cero), imágenes
hermanas bajo el mismo `PARENT`, o de imágenes animadas superpuestas con la misma secuencia de
duraciones de cuadros. En todos los casos se mide el rectángulo unido sin codificar nombres del juego.
El fundido de dos cuadros opacos reemplaza primero el cuadro base y mezcla el
siguiente sobre él, evitando que se transparente el fondo. Los cuadros con canal alfa mantienen la
animación original para no producir artefactos; controles, menús, texto y vídeos también quedan
excluidos. El modo de movimiento compensado se retiró porque la síntesis por CPU ralentizaba la
presentación; no se activa aunque exista una preferencia guardada de una versión anterior.
Cuando el fundido está activo usa VSync y el reloj del monitor (incluidos 144 Hz); si el renderer no
lo permite, el limitador alternativo consulta la frecuencia actual de pantalla. La preferencia se guarda en
`$XDG_CONFIG_HOME/livenyan/ui-preferences.toml`.

Tiempos de arranque: `LIVENYAN_LOG=info livenyan juego.exe`.

- Configuración: `$XDG_CONFIG_HOME/livenyan/config.toml` (ver `config.example.toml`).
- Partidas: `$XDG_DATA_HOME/livenyan/<juego>/`.
- Nunca se modifican los archivos del juego.

## Ventana

Ventana normal redimensionable (va bien en Hyprland). Se dibuja a la resolución lógica del juego
y se presenta con letterbox (bandas negras, aspecto correcto); el ratón se mapea a coordenadas lógicas.
F11 o Alt+Enter: pantalla completa. SDL3 elige Wayland si está disponible; no se fuerza X11.
Si el renderer no acepta VSync, el bucle limita el render a 60 FPS para evitar consumir CPU sin límite.

## Estado

Implementado: archivo VF (exe/dat/partido, zlib, cifrado LM Pro), LSB completo (52+ comandos parseados),
expresiones, VM (Calc, If/Elseif/Else, Call/Exit, Jump/Label, While*, Break/Continue, Wait, VarNew/VarDel),
objetos (ImgNew/MesNew/BoxNew/TileNew/Button/EditNew, SetProp/GetProp/ObjDel, PropMotion lineal), TextIns con
efecto máquina de escribir, menús PrevMenu con miniaturas GAL animadas, botones GAL animados en bucle
por defecto, clips WMV como componentes animados, sonido wav/ogg, decodificación en hilos con el primer fotograma listo antes de mostrar el clip
y reproducción de pistas de imagen
LiveCinema112 (posición y opacidad animadas; solo clips de imagen sin rotación, escala ni recorte),
conservación del último frame completo mientras se decodifican recursos de la escena siguiente,
menús nativos básicos con posición, tipografía, colores y marcos definidos por las propiedades y
manejadores del script, altura tipográfica de menús normalizada a las métricas de la fuente,
escritura Unicode en EditNew y descarte de menús por RCLICKSCRAP/MenuClose.

Pendiente: audio de los vídeos, pistas LiveCinema de audio/color y transformaciones de imagen,
transiciones Flip, guardado/carga, historial, eventos de texto restantes, lógica completa de menús/botones
de LiveNovel, Timer, fuentes/estilos del juego.

## Pruebas

Los fixtures son sintéticos (sin datos de juegos reales):

```sh
pip install pylivemaker
python3 tools/gen_fixtures.py tests/fixtures
cargo test
```

## Licencia

GPL-3.0-or-later (derivado de pylivemaker, GPL-3.0). Ver `LICENSE`.
