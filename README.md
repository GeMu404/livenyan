# livenyan

Reproductor nativo y libre (Rust + SDL3) para juegos hechos con LiveMaker.
Port de los formatos documentados por [pylivemaker](https://github.com/pmrowla/pylivemaker).
Prototipo v0: arranca, interpreta scripts, muestra imágenes y texto. Muchos juegos aún no serán jugables.
Es una reimplementación compatible del motor, no un reproductor exclusivo de un solo juego:
el soporte depende de los formatos y comandos LiveMaker implementados, así que la compatibilidad
se amplía y valida progresivamente con fixtures sintéticos y juegos de prueba.

## Compilar

Arch Linux:

```sh
sudo pacman -S --needed rust sdl3 ffmpeg fontconfig noto-fonts-cjk
cargo build --release
```

Sin SDL3 del sistema (o en Windows): `cargo build --release --features static-sdl`
(necesita `cmake` y un compilador de C).

La interpolación experimental con NVIDIA Optical Flow se excluye del build por defecto.
Para habilitar el modo de prueba, compila con `--features nvidia-optical-flow` y ejecuta
con `--optical-flow` (también se puede seleccionar desde Opciones). Requiere una GPU/driver
que exponga `VK_NV_optical_flow`; el renderer Vulkan normal no activa ni requiere esa extensión.

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
normalmente. El cargador ignora IDs que no correspondan a esos tipos de texto.
`--debug-layout` abre una ventana de diagnóstico con cajas por tipo de objeto y límites individuales
de botones `.lpm`; oculta las imágenes y animaciones, pero conserva el texto del juego dentro de
sus cajas con el mismo formato que en el renderizado normal.
Los componentes `Movie` WMV se decodifican con `ffmpeg` (debe estar en `PATH`) y se muestran como
animaciones; la repetición, pausa y detención siguen las propiedades y comandos del script.
Se conservan los timestamps originales y los cuadros repetidos consecutivos se identifican sin
forzar la salida a 30 FPS; el VM conserva un búfer acotado alrededor del playhead para sincronizar
la presentación y permitir la interpolación GPU opcional.
El renderer predeterminado es `vulkan`: reutiliza el renderer 2D Vulkan de SDL3 para dibujar la escena
existente, manteniendo su comportamiento para sprites, texto, controles y vídeo sin volver a
implementar cada operación. No requiere hardware NVIDIA. Si el renderer Vulkan compartido no está
disponible, se intenta el renderer Vulkan normal de SDL y finalmente el fallback automático; se puede
elegir otro driver con `--renderer` o `window.renderer` en la configuración. SDL conserva la ventana,
entrada, audio y la API 2D de alto nivel, mientras la presentación usa Vulkan cuando el driver está
disponible.

Además, existe un backend Vulkan directo experimental con un contrato de comandos independiente de
SDL. Sus pruebas sintéticas cubren la subida, actualización y liberación diferida de texturas RGBA,
composición offscreen con copia, recorte, relleno y alpha, reutilización de recursos y presentación
directa a swapchain. Esa ruta aún no sustituye el renderer SDL en la aplicación. El pipeline conserva
recursos entre frames y las subidas son asíncronas; las actualizaciones de igual tamaño reutilizan la
misma imagen Vulkan (útil para cuadros de vídeo). No se inspeccionan ni procesan imágenes del juego en
estas pruebas.
Con `nvidia-optical-flow`, el primer prototipo para el juego procesa únicamente pares de cuadros GAL
opacos de imagen que ocupan exactamente la resolución lógica completa. Inserta el cuadro medio
compensado durante la segunda mitad del intervalo; el resto de objetos y los cuadros con alfa siguen
la reproducción normal. Si la inicialización o síntesis falla, avisa en el log y vuelve al fundido
convencional. El trabajo ocurre al cambiar de par y puede causar tirones; no está activado por defecto
y aún no es una interpolación global de la escena. El campo de movimiento se interpola entre celdas
de 4×4; cuando detecta desacuerdo fuerte entre vectores vecinos, reduce la compensación y se acerca
a una mezcla simple para atenuar bloques y desgarros. No elimina los artefactos propios del algoritmo.
El mismo modo puede procesar vídeos WMV solo cuando son opacos y ocupan exactamente la pantalla
lógica; usa sus timestamps para situar el punto medio y omite el Optical Flow en cuadros duplicados.
Los demás vídeos mantienen su reproducción normal. Esto agrega trabajo síncrono al cambiar de cuadro
y puede bajar los FPS; si ocurre, desactívalo desde Opciones.

La prueba sintética procesa dos cuadros de 800×600 con una máscara de 48×48 desplazada 12 px sobre
un fondo fijo: en una RTX 4060 el flujo medio local devuelve (12.00, 0.03) px y el shader reconstruye
el píxel del punto medio con luminancia 240 (referencia 240; la mezcla directa sin compensación da
156). Mide por separado la primera ejecución y la repetición con recursos reutilizados; incluye lectura
de vuelta y no representa necesariamente el coste estable de la integración en pantalla.
Otras opciones: `--res 1024x768`, `--font RUTA`, `--entry NOMBRE`, `--threads N`, `--fullscreen`, `--mute`, `--config RUTA`.
Al acercar el ratón al borde superior aparece una barra con Salir, Pantalla completa y Opciones.
Opciones permite apagar, activar el fundido de GAL animados o, en builds con Optical Flow,
activar el modo experimental. El fundido cubre objetos o grupos visuales de imagen que ocupen al
menos un tercio de la resolución lógica; Optical Flow se limita a GAL opacos a pantalla completa.
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
LiveCinema 108-112 (posición y opacidad animadas; clips de imagen sin rotación, escala ni recorte),
conservación del último frame completo mientras se decodifican recursos de la escena siguiente,
menús nativos básicos con posición, tipografía, colores y marcos definidos por las propiedades y
manejadores del script, altura tipográfica de menús normalizada a las métricas de la fuente,
escritura Unicode en EditNew y descarte de menús por RCLICKSCRAP/MenuClose.

Pendiente: audio de los vídeos, pistas LiveCinema de audio/color y transformaciones de imagen,
transiciones Flip, guardado/carga, historial, eventos de texto restantes, lógica completa de menús/botones
de LiveNovel, Timer, fuentes/estilos del juego.
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

## Licencia

GPL-3.0-or-later (derivado de pylivemaker, GPL-3.0). Ver `LICENSE`.
