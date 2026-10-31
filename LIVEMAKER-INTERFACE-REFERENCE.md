# Referencia de interfaz LiveMaker

Referencia de trabajo del comportamiento de **interfaz** de LiveMaker (menú, LFT, media) en livenyan.

El inventario completo de comandos, tags y apaños (qué está hecho, cómo, esperado frente a real,
pendiente) está en **[`COMPATIBILITY.md`](COMPATIBILITY.md)**. Lo que aún puede mover comportamiento
son las entradas **H15–H24** de su tabla de hallazgos; cita el ID antes de copiar una propiedad a
otro objeto.

**No inventar semántica.** Cada afirmación lleva su nivel de fuente; los niveles y cómo etiquetar
están en [`COMPATIBILITY.md` → Cómo etiquetar un cambio](COMPATIBILITY.md#cómo-etiquetar-un-cambio).

## Fuentes primarias (rutas)

| Fuente | Ruta / URL | Notas |
|---|---|---|
| Manual LiveNovel (local) | [`docs/livenovel/`](docs/livenovel/README.md) | Copia de `_static/LiveNovel/` de pylivemaker; empezar por `topics.html` / `interface.html`. |
| Manual LiveNovel (upstream) | [pylivemaker `docs/_static/LiveNovel`](https://github.com/pmrowla/pylivemaker/tree/master/docs/_static/LiveNovel) | El mismo árbol; ReadTheDocs lo publica en `_static/LiveNovel/`. |
| Enums de propiedades / comandos | [pylivemaker `lsb/core.py` `PropertyType`](https://github.com/pmrowla/pylivemaker/blob/ceeb12911c571fe6da77107bb0eaf7eb29594612/src/livemaker/lsb/core.py) | Ids numéricos LSB (`PR_ROWCOUNT`, …), no semántica. |
| Exports / cadenas de `live.dll` | [`docs/live-dll-strings.txt`](docs/live-dll-strings.txt) | Solo API de media. Esta DLL **no** tiene GetProp/SetProp / `ROWCOUNT` / etc. |

## Menú — fuente primaria

De `docs/livenovel/interface.html` (secciones Menu / Scrollbar):

- `ROWCOUNT`: filas visibles a la vez. **fuente primaria**
- `TOPINDEX`: fila desde la que se empieza a mostrar. **fuente primaria** (origen del índice y fuera de rango: **no confirmado**, **H15**)
- `SCROLLSOURCE`: imagen de la barra del menú; el manual lista las partes en orden: arriba, arriba pulsado, abajo, abajo pulsado, fondo, fondo pulsado, grip. **fuente primaria**
- `X` / `Y` del menú: relativos al padre, no coordenadas absolutas de pantalla. **fuente primaria**
- Alineación horizontal (`HA_CENTER`): alineación del texto dentro del menú, no «centrar el menú en pantalla». **fuente primaria**
- `FONTHOVERCOLOR` y `HOVERCOLOR`: color del texto y del fondo solo cuando el cursor está sobre el ítem (interface.html, 「項目の上にマウスカーソルがある時」). **fuente primaria**
- `FONTBORDER`: ancho del contorno del texto (interface.html, 「フォントの縁の幅」). En el menú se dibuja igual que en el mensaje: 1 px si el valor no es 0. **fuente primaria** el ancho; el raster de 1 px es el mismo que el del mensaje.
- `CHECKSOURCE`: tira de dos estados, check normal y check con el cursor encima, a la izquierda del ítem (interface.html). El hueco reservado es el ancho de un estado (**interpretación**: ancho del archivo / 2).
- `IndexOfMenu`: índice del texto entre los ítems, o -1 (calc.html). **fuente primaria**

## Menú — no confirmado (el manual calla o se queda corto)

- Ancho automático / significado de `WIDTH` ausente o cero. **H16**
- Centrado automático cuando falta la posición. **H17**
- Colocación, orientación y escala de las partes de `SCROLLSOURCE` (frente al componente Scrollbar). **H18**
- Origen de `TOPINDEX` (0 o 1) y comportamiento fuera de rango. **H15**
- Que las reglas `OT_HORIZONTAL` / `OT_VERTICAL` del Scrollbar manden en el `SCROLLSOURCE` del menú. **H18**

No subir a estas casillas el script o el layout de un solo juego.

## Menú — inventado en livenyan

- Índices de opción base 0; `TOPINDEX` acotado a las opciones disponibles (con tests). **H15**
- Si el `WIDTH` del script no es positivo: medir el ancho intrínseco **antes** de `ONOPENING` con las etiquetas y la fuente elegida (márgenes, borde, barra del menú paginado) para que `GetProp(Sender, WIDTH)` en la apertura vea un valor. **H16**
- Si la fuente no carga: ancho de reserva `native_menu_size` (~60 % del ancho lógico).
- Sin `LEFT`: centrar el menú con el ancho medido o por defecto; un `LEFT` explícito es el borde izquierdo (relativo al padre). **H17**
- Imagen de lista Save/Load de 140×20 vista en los logs de un juego → siete celdas de 20×20 solo para ese asset; no es universal. **H19**
- Sustituto de fuente: Noto Sans Mono CJK JP cuando falta `ＭＳ ゴシック` (métricas casi monoespaciadas); sigue siendo una aproximación.
- Barra de scroll: tira vertical a la derecha; tamaño del grip según la proporción de filas visibles; teclado y rueda hechos; clic en flecha, página y grip sin hacer. **H18**
- Comparación del usuario: en el original la lista Save/Load sale centrada con la barra a la derecha; cuadra con lo anterior, no es una regla nueva del manual.

## Componente Scrollbar (distinto del SCROLLSOURCE del menú)

- Manual: `OT_HORIZONTAL` / `OT_VERTICAL`; `X` / `Y` relativos al padre. **fuente primaria**
- Equiparar esas reglas con el layout de `SCROLLSOURCE` del menú: **no confirmado**; no suponerlo. **H18**

## Tipografía embebida LivePixelFont (`.lft`)

- Manual LiveNovel / pylivemaker: **sin especificación** del contenedor `.lft`.
- Un juego LiveNovel: entrada VF `*.lft`, magic `LivePixelFont\0`, nombre interno CP932. `StatusFontName` del `.lpb` sigue siendo `ＭＳ ゴシック` (sistema); el `.lft` es recurso aparte y los scripts ponen `FONTNAME` = su nombre interno. **experimento** (bytes + LSB).
- Layout inferido (un solo juego; **no confirmado** fuera de version `100`, **H20**):
  - cabecera: magic + `u32` versión + tamaños zlib-dir / bitmaps + nombre[64] + métricas[232]
  - directorio zlib: registros de 18 B (`u16` código CP932 BE, `w/h`, `i16` bearings, `u16` advance, `u16` 0, `u32` offset)
  - bitmaps crudos 8 bpp, stride = `align4(w)`
- Espacio U+0020 ausente en ese `.lft` → livenyan sintetiza avance con el campo de métricas interpretado como ancho medio (**inventado en livenyan**, **H20**).
- Sustituto TTF/Noto cuando no hay `.lft` o el `FONTNAME` no coincide: sigue siendo **inventado en livenyan**.

## Media / `live.dll`

- Símbolos exportados `liveOgg*` / `liveVideo*` / `liveWmp*` / `liveScreen*`: **fuente primaria** solo de la *superficie nativa de la DLL de media* (ver `docs/live-dll-strings.txt`).
- Mapear esos exports 1:1 a comandos Media del LSB o a nombres de propiedad: **no confirmado** salvo cita de `docs/livenovel/media.html` o experimento. **H21**
- Dos juegos distintos traen el mismo `live.dll` (mismo `sha256`).

## Mantenimiento

Antes de cambiar geometría de menú, troceado de la barra, scroll o valores por defecto de propiedades:

1. Mirar `docs/livenovel/` (el HTML que toque) y este archivo.
2. Etiquetar el cambio (primaria / no confirmado / inventado). Si aún puede mover comportamiento, añadir o citar una **H\*** en `COMPATIBILITY.md`.
3. Preferir el texto del manual o una comparación controlada con el motor original; anotar la duda en vez de inventar una especificación.
