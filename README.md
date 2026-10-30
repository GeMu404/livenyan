# livenyan

Reproductor nativo y libre (Rust + SDL3) para juegos hechos con LiveMaker.
Port de los formatos documentados por [pylivemaker](https://github.com/pmrowla/pylivemaker).
Prototipo v0: arranca, interpreta scripts, muestra imágenes y texto. Muchos juegos aún no serán jugables.
Es una reimplementación compatible del motor, no un reproductor exclusivo de un solo juego:
el soporte depende de los formatos y comandos LiveMaker implementados, así que la compatibilidad
se amplía y valida progresivamente con fixtures sintéticos y juegos de prueba.

Compatibilidad (comandos, tags, workarounds, pendiente): [`COMPATIBILITY.md`](./COMPATIBILITY.md).  
Detalle de interfaz (menú, LFT, media): [`LIVEMAKER-INTERFACE-REFERENCE.md`](./LIVEMAKER-INTERFACE-REFERENCE.md).

## Compilar

Arch Linux:

```sh
sudo pacman -S --needed rust sdl3 ffmpeg fontconfig noto-fonts-cjk
cargo build --release
```

Requiere **SDL ≥ 3.4** (el present usa `SDL_SetDefaultTextureScaleMode`, que no existe
en 3.2: con 3.2 el enlazado falla). Sin SDL 3.4 del sistema (o en Windows):
`cargo build --release --features static-sdl` (necesita `cmake` y compiladores de C y C++;
compila el SDL incluido en `sdl3-sys`).

El renderer SDL es automático por defecto (`window.renderer` / `--renderer` vacío).
Puedes forzar un driver (`opengl`, `vulkan`, `software`, `gpu`, …); si no está
disponible, cae al automático. El backend Vulkan/ash experimental y NVIDIA Optical
Flow quedaron archivados en el tag `archive/vulkan-optical-flow`.

Escalado del present (letterbox lógico→ventana): `scale_filter` / `--scale-filter` /
Opciones → **Linear** (suave), **Nearest** (píxel duro en assets y present) o
**Sharp** (assets nearest + present nearest). Sin upscale IA.

Binario: `target/release/livenyan`

## Uso

```sh
livenyan /ruta/juego.exe          # lee el archivo VF dentro del exe, sin extraer nada
livenyan /ruta/al/juego/          # carpeta del juego (.exe/.dat/.ext) o archivo ya extraído
livenyan --info juego.exe         # estadísticas, resolución, script de entrada
livenyan --dump-lsb juego.exe 00000001.lsb
livenyan --dump-texts ./traduccion juego.exe
livenyan --trace --steps 20000 juego.exe   # VM sin ventana; avanza y elige la 1ª opción
livenyan juego.exe --debug-layout # contornos y etiquetas; oculta sprites y animaciones
livenyan juego.exe --fps-meter    # muestra FPS presentados y refresco detectado
```

`--info`, `--dump-lsb` y `--trace` **no muestran el texto del juego**; usa `--show-text` para verlo.
`--dump-texts DIR` exporta un catálogo editable `texts.toml` con diálogos `TextIns`, opciones
estáticas de menú y pistas de subtítulos LCM, que son los textos de juego que el cargador aplica.
No incluye nombres, rutas, literales auxiliares de scripts, archivos de texto ni tablas. Cada entrada tiene un identificador estable
(`id`) y un texto (`text`): el catálogo exportado contiene el original y se traduce reemplazando
directamente el valor de `text`. Los marcadores `{{LV:TIPO:N}}` representan variables u otros
elementos dinámicos del diálogo: deben conservarse exactamente una vez, aunque pueden moverse para
que la frase traducida suene natural. Si faltan o se alteran, esa entrada no se aplica para evitar
perder contenido del juego. Si el juego inserta una opción elegida como variable en un texto,
también se muestra su traducción de menú sin cambiar el valor original que usan los scripts. Al
volver a exportar se conservan los textos editados con el mismo ID.
También se puede exportar desde Opciones dentro del juego,
eligiendo la carpeta de destino. Para cargar uno o varios idiomas, copia el catálogo traducido como
un archivo `.toml` dentro de la carpeta `translations` junto al juego (por ejemplo
`translations/es.toml`, `translations/en.toml`). Opcionalmente agrega `name = "Español"` junto a
`version = 1` para mostrar un nombre legible en el selector. Al iniciar aparecerá un selector con
esos idiomas y la opción original; si no encuentra catálogos con traducciones, el juego inicia
normalmente. El primer idioma disponible queda seleccionado inicialmente; usa las flechas para
elegir otro o la opción original. El cargador ignora IDs que no correspondan a esos tipos de texto.
`--debug-layout` abre una ventana de diagnóstico con cajas por tipo de objeto y límites individuales
de botones `.lpm`; oculta las imágenes y animaciones, pero conserva el texto del juego dentro de
sus cajas con el mismo formato que en el renderizado normal.
Los componentes `Movie` WMV se decodifican con `ffmpeg` (debe estar en `PATH`) y se muestran como
animaciones; la repetición, pausa y detención siguen las propiedades y comandos del script.
Se conservan los timestamps originales y los cuadros repetidos consecutivos se identifican sin
forzar la salida a 30 FPS; el VM conserva un búfer acotado alrededor del playhead.
Las texturas SDL de imágenes y todos los cuadros GAL comparten una caché GPU LRU de hasta 256 MiB;
los recursos recientes se liberan al cambiar de escena para limitar el crecimiento de memoria.
Las texturas ya usadas en el frame actual no se expulsan mientras se dibuja ese frame; si la escena
activa supera el presupuesto, la caché puede excederlo temporalmente para no hacer desaparecer
recursos visibles. Cada cinco segundos, el log `info` informa una estimación del uso de texturas por
categoría (GAL/imágenes, vídeo, UI y destino de escena). Se basa en dimensiones y formato RGBA
estimado; no es una lectura del driver. Para verlo: `LIVENYAN_LOG=info`.

Otras opciones: `--res 1024x768`, `--font RUTA`, `--entry NOMBRE`, `--threads N`, `--fullscreen`, `--mute`, `--config RUTA`, `--renderer DRIVER`.
Al acercar el ratón al borde superior aparece una barra con Salir, Pantalla completa y Opciones.
Opciones permite apagar o activar el fundido de GAL animados. El fundido cubre objetos o grupos
visuales de imagen que ocupen al menos un tercio de la resolución lógica.
Los grupos se deducen de un `TAG` explícito (incluido cero), imágenes
hermanas bajo el mismo `PARENT`, o de imágenes animadas superpuestas con la misma secuencia de
duraciones de cuadros. En todos los casos se mide el rectángulo unido sin codificar nombres del juego.
El fundido de dos cuadros opacos reemplaza primero el cuadro base y mezcla el
siguiente sobre él, evitando que se transparente el fondo. Los cuadros con canal alfa mantienen la
animación original para no producir artefactos; controles, menús, texto y vídeos también quedan
excluidos. La síntesis anterior por CPU se retiró porque ralentizaba la presentación.
Cuando el fundido está activo usa VSync y el reloj del monitor (incluidos 144 Hz); si el renderer no
lo permite, el limitador alternativo consulta la frecuencia actual de pantalla. La preferencia se guarda en
`$XDG_CONFIG_HOME/livenyan/ui-preferences.toml`.
El medidor de FPS se activa con `--fps-meter`, con F3 durante la partida o desde Opciones. Muestra
los cuadros presentados por segundo (promedio móvil de medio segundo) junto con la frecuencia de
refresco detectada; con VSync, ambos valores permiten comprobar si la presentación alcanza los 144 Hz.

Tiempos de arranque: `LIVENYAN_LOG=info livenyan juego.exe`.

### Depuración

| Opción | Qué hace |
|---|---|
| `--trace [--steps N]` | VM sin ventana durante N pasos (5000 por defecto) |
| `--pick N` | con `--trace`: opción elegida en los menús (0 = primera habilitada) |
| `--trace-conds` | con `--trace`: imprime las condiciones If/Elseif/While con sus valores |
| `--gal-stats RUTA` | decodifica una imagen GAL del archivo y muestra solo estadísticas |
| `--snapshot-frame N SALIDA.png` | renderiza oculto hasta el frame N (reloj simulado a 60 fps), guarda la captura y sale |

| Variable | Qué hace |
|---|---|
| `LIVENYAN_LOG` | nivel de log de `env_logger` (`warn` por defecto; `info`, `debug`, `livenyan::vm=debug`…) |
| `LIVENYAN_CONDS` | parte del nombre de un script: sus condiciones van al log `debug`, también con ventana |
| `LIVENYAN_TEST_GAME_DIR`, `LIVENYAN_TEST_FONT` | solo para los tests ignorados (ver [Pruebas](#pruebas)) |

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
efecto máquina de escribir e imágenes GAL inline (primer cuadro, proporción conservada y alineación
vertical), menús PrevMenu con miniaturas GAL animadas, botones GAL animados en bucle
por defecto, clips WMV como componentes animados, sonido wav/ogg, decodificación en hilos con el primer fotograma listo antes de mostrar el clip
y reproducción de pistas de imagen
LiveCinema 108-112 (posición y opacidad animadas; clips de imagen sin rotación, escala ni recorte),
conservación del último frame hasta que decodifica la imagen de la escena siguiente (menús y overlays pueden entrar después),
menús nativos básicos con posición, tipografía, colores y marcos definidos por las propiedades y
manejadores del script; las funciones de estado de ítems aceptan índices numéricos y nombres,
altura tipográfica de menús normalizada a las métricas de la fuente,
escritura Unicode en EditNew y descarte de menús por RCLICKSCRAP/MenuClose.
Historial de diálogo básico con `TextIns.Hist`, `GetHistoryCount`, `GetGroupIndex`, `CallHist` y `ClrHist`;
eventos de texto `PAUSE`, `MESOFF`, `PLAYMOVIE` y `STOPMOVIE`.
Los temporizadores disparan `ONPLAYEND` al vencer, una vez o repetidamente según `REPEAT`; al
pausarlos con `PAUSED`, conservan el tiempo restante hasta el siguiente disparo.
`GameSave` / `GameLoad` escriben y leen slots en `save.dat` (página, etiqueta, caption y variables globales).

Pendiente: audio de los vídeos, pistas LiveCinema de audio/color y transformaciones de imagen,
transiciones Flip distintas de NONE/FADE, eventos de texto restantes, lógica
completa de menús/botones de LiveNovel y fuentes/estilos del juego.
La reproducción WAV/OGG usa el dispositivo de salida predeterminado de SDL3; SDL convierte los
formatos y sigue los cambios de salida predeterminada del sistema. El log de inicio muestra el
dispositivo seleccionado.

## Pruebas

Los fixtures son sintéticos (sin datos de juegos reales):

```sh
pip install pylivemaker
python3 tools/gen_fixtures.py tests/fixtures
cargo test
```

Algunos tests necesitan recursos que no se versionan y están marcados `#[ignore]`;
se ejecutan con `cargo test -- --ignored` definiendo:

| Variable | Contenido |
|---|---|
| `LIVENYAN_TEST_GAME_DIR` | carpeta `data` de un juego LiveNovel (lee `ノベルシステム/画像/btn.gal`) |
| `LIVENYAN_TEST_FONT` | ruta a `ume-tgo4.ttf` (Ume Gothic) |

## Licencia

GPL-3.0-or-later (derivado de pylivemaker, GPL-3.0). Ver `LICENSE`.
