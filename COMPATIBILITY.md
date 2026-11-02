# Compatibilidad LiveMaker → livenyan

Base de conocimiento del motor: **qué hay**, **cómo**, **comportamiento esperado**, **fuente**, **huecos**.

Antes de implementar o “arreglar” semántica: leer esto + [`LIVEMAKER-INTERFACE-REFERENCE.md`](LIVEMAKER-INTERFACE-REFERENCE.md) + la página citada bajo `docs/livenovel/`. No subir el repo entero al contexto: este archivo es el índice.

## Postura del proyecto (no es 1:1)

livenyan es una **interpretación moderna y óptima** del *repertorio* LiveMaker (comandos, props, novel system), no un emulador bit-a-bit del runtime de Windows.

- **Compatibilidad de contenido:** los scripts/LSB deben poder expresar la misma historia y lógica.
- **Libertad de implementación:** timing, decode, escalado, tipografía del host, I/O, etc. pueden **divergir a propósito** si mejora latencia, claridad o mantenimiento.
- **No confundir** “mejor que el original” con “no sabemos qué hacía el original”.

### Cómo etiquetar un cambio

| Etiqueta | ¿Qué es? | ¿Objetivo 1:1? | Ejemplo |
|----------|----------|----------------|---------|
| **fuente primaria** | Comportamiento tomado del manual / enum documentado | Seguir el manual | `PCReset` AllClear |
| **experimento** | Medido vs motor original | Acercarse o documentar delta | Comparar un wipe |
| **no confirmado** | Inferencia; manual calla | No venderlo como LM | Orden args `TEXTSPD` |
| **interpretación livenyan** | Divergencia **deliberada** (UX/perf/portabilidad). Diseño estable. | **No** — es la forma livenyan | Vídeo a tamaño nativo (menos latencia que forzar escala al lienzo en ffmpeg) |
| **inventado (hueco)** | Relleno para no romper flujo mientras falta diseño/datos | Sustituir o confirmar | Wipe desconocido → fade; `SAVESCREEN` → `true` |
| **temporal** | Apaño con fecha de caducidad explícita | Reemplazar | Placeholder Cinema ms fijos *hasta* LCM usable |

**No uses “parche” ni “fix temporal”** para una interpretación deliberada: eso ensucia el mapa.  
**Sí usa `temporal`** solo si el comentario/commit dice qué lo reemplaza.

Al documentar una **interpretación**: anota *qué hacía / haría LM (si se sabe)*, *qué hace livenyan*, *por qué* (latencia, Linux, SDL…).

## Cómo usar (humanos y agentes)

1. Buscar el comando / tag / decisión por nombre.
2. Mirar **Estado** y **Fuente** (incl. interpretación vs inventado).
3. Cambio ambiguo de *semántica de script*: cita manual/strings/experimento; actualizar la fila.
4. Cambio de *pipeline/UX*: marcar **interpretación livenyan** + motivo; no fingir que el manual lo pide.
5. No promover inventado/temporal a “así es LiveMaker” ni a interpretación sin escribir el porqué.
6. Si un script repite un patrón ya visto, citar el **H\*** de [Hallazgos](#hallazgos-comportamiento-aún-movible) antes de copiar la propiedad a otro objeto.

### Estados (cobertura)

| Estado | Significado |
|--------|-------------|
| `ok` | Suficiente para scripts reales; puede incluir interpretación documentada. |
| `partial` | Subset útil; falta o approx en otra parte. |
| `placeholder` | Mantiene el flujo; efecto real incompleto (**suele ser temporal/inventado**). |
| `noop` | Aceptado sin efecto (documentado). |
| `unimpl` | `unimpl:*` / ignorado. |

### Tiers de fuente / decisión

`fuente primaria` · `experimento` · `no confirmado` · **`interpretación livenyan`** · `inventado (hueco)` · `temporal`

Citas: `docs/livenovel/…`, `docs/live-dll-strings.txt`, fixture/`--trace`, o párrafo “por qué divergimos” en esta tabla.

### Código de anclaje

| Área | Dónde mirar |
|------|-------------|
| Dispatch LSB | `src/vm/mod.rs` (`step`) |
| Flujo / Reset | `src/vm/step_flow.rs` |
| Flip / texto cmd | `src/vm/step_text.rs`, `src/vm/wipe.rs` |
| Objetos nuevos | `src/vm/step_obj.rs` |
| Tags en texto | `src/vm/text_hist.rs` → `system_text_event` |
| Velocidad texto | `src/vm/media.rs` (`cps_of`) |
| Props / menú | `LIVEMAKER-INTERFACE-REFERENCE.md`, `src/vm/menu.rs` |

---

## Comandos LSB (`CmdType`)

Ids: `src/lsb/names.rs`. Parse: `src/lsb/mod.rs`.

| Id | Cmd | Estado | Esperado (LiveMaker / manual) | livenyan hoy | Fuente / notas |
|----|-----|--------|-------------------------------|--------------|----------------|
| 0–2 | If / Elseif / Else | `ok` | Bifurcación por Calc | Igual | flujo estándar |
| 3 | Label | `ok` | Marca | Trace / destino Jump | |
| 4 | Jump | `ok` | Salto a Page[:Label] | `goto_script` | |
| 5 | Call | `ok` | Llamada con stack | Stack + Params | Params → `Param1..N` **provisional** |
| 6 | Exit | `ok` | Return si Calc | `do_return` | |
| 7 | Wait | `ok` / `no confirmado` | Espera tiempo / cond | Implementado; Wait 0 edge | Comentarios en `step_wait` |
| 8–10 | BoxNew / ImgNew / MesNew | `ok` | Crear componentes | `step_obj_new`. Imagen y tile: `PR_BRIGHTNESS` 0 igual, 255 blanco, -255 negro | `graphic.html` 明るさ |
| 11 | Timer | `ok` | Temporizador | Timers + eventos | |
| 12 | Movie | `ok` / `partial` | Vídeo | GaleX anim si magic/stem `.gal` (piso); si no ffmpeg CLI; decode **nativo** (I1, I15) | Manual size; **interpretación** latencia/port |
| 13 | Flip | `partial` | Placa + wipe | Con targets: fundido y `Delete` solo de los nombres planificados; los objetos nuevos no presentados forman la superficie entrante (**interpretación livenyan**), mientras que los ya presentados pasan a ser salientes en el siguiente Flip. El árbol del cuadro de mensaje queda fuera de ese seguimiento; si un Flip anterior lo archivó, un cambio no dirigido al cuadro lo restaura y no lo elimina. El borrado no alcanza descendientes creados después del plan. FL_OUT sin Delete → `back`; **experimento** (H3) MESOFF/FACE→再生中: `SetProp VISIBLE=1` desarchiva el objeto y su descendencia (sin Flip de entrada). Targets vacío: **interpretación** placa (I12), una pasada, sin framebuffer extra. NONE/FADE; WHITE/BLACK/FLASH overlay (I7); resto fade (I6) | `graphic.html` / novel MESOFF |
| 14 | Calc | `ok` | Evaluar expresión | | |
| 15–16 | VarNew / VarDel | `ok` | Vars + Scope | Scope 0\|3 → globals; 1\|2 locals | Reset conserva scope 3 |
| 17–18 | GetProp / SetProp | `ok` | Props objeto | Missing SetProp = no-op LM | |
| 19 | ObjDel | `ok` | Borrar + cascada | `scrap_component` | |
| 20 | TextIns | `ok` | Insertar texto TpWord | Plan Pending + plays | `<PG>` coalesce: `script.html` |
| 21 | MovieStop | `ok` | Parar película | | |
| 22 | ClrHist | `ok` | Limpiar historial | | |
| 23 | Cinema | `partial` | LiveCinema | LCM de imágenes: cada clip dura lo que marca el archivo (60 fps). `CHANGECG … REPEAT` no rebobina al primer clip: los anteriores se ven una vez y el último GAL sigue con las duraciones del propio archivo. Un LCM de texto sí repite la línea entera. `AUTOSCRAP` elimina el objeto al terminar una reproducción no repetida, después de su `ONPLAYEND`. El ancho del raster de texto se mide por su contenido, independiente de la posición animada del clip, para que la caché no recorte la marquesina al entrar. Sin LCM usable, placeholder en ms | `graphic.html` PR_REPEAT/PR_AUTOSCRAP; **experimento** test2, clip = un ciclo del GAL |
| 24 | Caption | `ok` | Caption | | |
| 25–26 | Menu / MenuClose | `ok` / `partial` | Menús | El script sigue tras el comando (SetCheck, Flip). El menú queda modal hasta cerrarlo; el clic dispara ONSELECT en el acto. Handlers de botón derecho en su rama visual (incluido el fondo padre) pueden ejecutarse durante la modalidad; eventos ajenos quedan retenidos. `PR_IGNOREMOUSE` deja pasar el clic al panel. livenyan interpreta 0–4 como los modos `IM_NONE`, `IM_ALL`, `IM_CLICK`, `IM_LCLICK`, `IM_RCLICK` (enumeración, no máscara); por eso 4 recibe clic izquierdo y omite el derecho. El texto se dibuja en el orden de la escena: un hermano con más prioridad (el deslizador) queda encima | interface.html; `graphic.html` マウス透過; el handler del fondo de sistema usa ONRBTNDOWN para cerrar |
| 27 | Comment | `ok` | Ignorar | | |
| 28 | TextClr | `ok` | Vaciar texto caja | | |
| 29 | CallHist | `ok` | Volcar historial | | |
| 30 | Button | `ok` | Botón | | |
| 31–35 | While* / Break / Continue | `ok` | Bucles | Yield por frame | |
| 36–37 | ParticleNew / FireNew | `placeholder` | FX | Solo objeto | |
| 38–39 | GameSave / GameLoad | `ok` / `partial` | Save slots | Globals + page/label | No snapshot escena completo |
| 40 | PCReset | `ok` | PC → Page; AllClear = stack | `step_pc_reset` | **fuente primaria** `sequence.html`「PCリセット」 |
| 41 | Reset | `ok` | Wipe escena + vars (≠ status) + stack → Page | `step_reset`; status = scope 3 | **fuente primaria** `sequence.html`「リセット」 |
| 42 | Sound | `ok` | Audio canal | SDL3 | |
| 43 | EditNew | `ok` | Edit | | |
| 44 | MemoNew | `placeholder` | Memo | Objeto vacío | |
| 45 | Terminate | `ok` | Fin programa | `State::Ended` | |
| 46 | DoEvent | `ok` | Despachar cola / yield | | |
| 47 | ClrRead | `ok` | Olvidar TextIns leídos | | |
| 48 | MapImgNew | `ok` | Imagen mapa | | |
| 49 | WaveNew | `placeholder` | Wave | Objeto vacío | |
| 50–54 | Tile / Slider / Scrollbar / Gauge / CGCaption | `ok` / `partial` | UI | Scrollbar/menú: interface ref | |
| 55 | MediaPlay | `ok` / `partial` | Play media | | |
| 56 | PrevMenuNew | `ok` | PrevMenu .lpm | | |
| 57 | PropMotion | `ok` | Animar prop numérica | | |
| 58 | FormatHist | `ok` | Formato historial | | |
| 59–60 | SaveCabinet / LoadCabinet | `ok` | Guardar/restaurar objs | SaveCabinet mueve solo componentes de la superficie trasera al cabinet; LoadCabinet los devuelve a la superficie trasera | `object.html` キャビネット保存 / キャビネット復帰 |
| 61–63 | IFDEF / IFNDEF / ENDIF | `noop` | Preprocesador chart | No-op si aparecen en LSB | Runtime **no confirmado** |

---

## Tags / sysevents de texto (`\x01NAME`…)

Dispatch: `system_text_event` en `src/vm/text_hist.rs`. Manual: `docs/livenovel/script.html`.

| Tag | Estado | Esperado | livenyan | Fuente / notas |
|-----|--------|----------|----------|----------------|
| PAUSE | `ok` | Clic / TEXTPAUSED + ONNOTIFY | Igual | `script.html` / novel system |
| TEXTCLEAR | `ok` | Vaciar caja | Igual | |
| MESON / MESOFF | `ok` | Visibilidad vía novel system | Solo ONNOTIFY | No hardcodear VISIBLE |
| WAITPLAY | `ok` | Esperar efecto/obj | `wait_for`; `CLICK` permite clic; Ctrl no corta un Movie activo | Borderline: `PLAYMOVIE` → `WAITPLAY Movie CLICK` → `STOPMOVIE`; comportamiento original reportado por usuario |
| PLAYMOVIE / STOPMOVIE | `ok` / `partial` | Vídeo inline | Objeto `ムービー` + ffmpeg | |
| CREATECG / CHANGECG / DELETECG | `partial` | CG / color / tamaño / posición / capa | `CREATECG`: tamaño intrínseco, coords numéricas o alineadas, `#n` como prioridad | Resto **sin verificar** |
| PLAYSND / STOPSND | `ok` | SE/BGM canales | | |
| TEXTSPD | `ok` / `partial` | TIME ms/char; ABSOLUTE | Override `text_char_ms` | Orden args **no confirmado**; relativo = **I8** |
| TXSPF / TXSPN / TXSPS | `ok` | Fast / normal / slow | 0 ms / clear+256 / ×3 baseline | **I8** |
| SYSMENUON/OFF SAVELOADON/OFF | `partial` | Permisos menú/save | ONNOTIFY only | Puede no bastar |
| OPENURL | `ok` | Abrir URL | `xdg-open` / `open` / `cmd start` | `script.html` + Func OPENURL |
| WALLPAPER | `noop` | Wallpaper SO | Ignore + warn | Como SETWALLPAPER false |
| WAITFOR | `ok` | Esperar componente | Como WAITPLAY + CLICKABORT | `script.html` |
| QUAKE / STOPQUAKE | `ok` / `partial` | Temblor Q/W/B | Offset dibujo | Orden args **no confirmado**; Q = **I9** |
| *(otros)* | `unimpl` | — | `unimpl:text sysevent` | Inventariar con `--trace` |

`<PG>` / `<PS>`: glifos Return, no sysevent. Con diálogo (`Glyph::Char`): clic según `script.html`. Coalesce `<PS>`/`{PAUSE}`+`<PG>`: fusión **inventada**. `<PG>` con `cur` vacío (p. ej. tras `{PAUSE}`/`TEXTCLEAR` o STOPSND): solo Clear, sin clic — **experimento** (H10). Sin caracteres en todo el TextIns (logo): `<PG>` solo Clear — **experimento** (si no, negro + clic muerto tras logo/Iniciar).

---

## Interpretaciones livenyan (divergencia deliberada)

Diseño estable: **no** son bugs ni “hasta que haya 1:1”.

| ID | Área | LM / ingenuo | livenyan | Por qué |
|----|------|--------------|----------|---------|
| I1 | Decode Movie/Cinema frames | Escalar en ffmpeg al tamaño lógico/pantalla (latencia alta) | Decode a **tamaño nativo**; escala en present/SDL | Menos CPU al abrir; letterbox/sharp ya escalan |
| I15 | Movie backends | Solo WMV + proceso `ffmpeg` | Dispatcher: GaleX multi-frame+delays (sin `Command`) o ffmpeg CLI si el host lo tiene; al resolver asset se prefiere `{stem}.gal` | Ports (Vita/etc.) y desktop sin PATH; scripts siguen apuntando a `.wmv` |
| I2 | Present | Ventana GDI LiveMaker | SDL3 + letterbox + `scale_filter` | Portabilidad / nitidez moderna |
| I2b | Present nativo | Ventana a tamaño de monitor / GDI | `scale_filter=native`: ventana = lógico, sin HiDPI ni letterbox; el escalado lo hace LS/Magpie/Gamescope | Un solo escalado externo (sin doble upscale) |
| I3 | Tipografía host | ＭＳ ゴシック | Noto CJK u otra si falta | Linux/Windows sin fuentes JP |
| I4 | `TOSCREEN*` / coords | Espacio ventana Windows | Identidad en espacio lógico | El letterbox lo resuelve el present |
| I5 | Renderer | DirectDraw/GDI/etc. | SDL auto (sin Vulkan/OF propio) | Mantenibilidad; OF archivado |
| I6 | Flip wipe ≠ NONE/FADE/BLINDH/BLINDV/WHITE/BLACK/FLASH | Wipe geométrico LM | `WP_BLINDH` abre con franjas verticales y `WP_BLINDV` con franjas horizontales, también en cajas semitransparentes; el wipe recorta complementariamente las superficies entrante y saliente. Los demás wipes no implementados usan fundido de superficies (`WP_FADE`) | `graphic.html` (orientación, anchura y demora de franja); el resto queda como interpretación |
| I7 | WP_WHITE/BLACK/FLASH | 表→色→裏 / 白→裏 | Overlay RGBA, sin tocar `ALPHA` | `graphic.html` + present SDL |
| I8 | TEXTSPD relativo / TXSPS | ? | Baseline 60 cps; slow ×3 | Números estables livenyan |
| I9 | QUAKE tipo Q | ? | Hash ruido acotado | Suficiente para “temblor” |
| I10 | GETDISPLAYMODE | Modos Win32 | Siempre `0` (ventana lógica) | Sin exclusive FS LM |
| I11 | Clic / hover | Rectángulo WIDTH×HEIGHT | Si la prop es ≤ 0, el tamaño del archivo (imagen, botón, tile) y luego zoom | El novel system no guarda esas props; si no, la miniatura se ve y no recibe el clic |
| I12 | Flip sin targets | `graphic.html`: `FL_OUT` mueve solo los targets al fondo; `FL_STAY` conserva solo los targets delante; `Delete` borra los componentes que sigan detrás tras el cambio | **Interpretación livenyan:** la cohorte agrupa objetos desde el Flip completado anterior; los objetos ya presentados pasan a ser salientes en el siguiente cambio. Esta marca se reinicia al completar cada Flip y al restaurar objetos de un gabinete, así cada transición A→B→A clasifica solo la escena vigente. La cohorte avanza al completar el Flip, no por cada frame. Mensajes y sus padres no se archivan; el fundido es de las dos superficies durante `Time`, sin escribir `ALPHA`. La superficie archivada que entra se compone detrás de la superficie frontal saliente, conservando el orden jerárquico | `graphic.html` (Flip: 表画面/裏画面, 処理); un `While` que cede frames partía la placa de un menú |
| I13 | Present al cambiar de escena | El cuadro anterior se queda hasta que decodifica cada imagen nueva (mapa, countdown, overlays) | Se presenta cuando está lista la banda de la imagen de fondo: prioridad mínima + 32. Menús y overlays entran en los frames siguientes. Si un Flip activo no puede presentarse por recursos pendientes, se pausa su reloj y su espera | Un clic del mapa de escenas dispara la carga del clip y de toda la UI; esperar a todo deja la escena vieja en pantalla |
| I14 | Texto de contorno al agrandar | Raster a `FONTHEIGHT` y estirar el framebuffer | El target de escena es el letterbox en píxeles. El texto de contorno se rasteriza a ese tamaño y se copia en coords lógicas (el estirado queda 1:1). `.lft` no se agranda. `scale_filter=native` se queda a 1×. SaveScreen hace readback fresco del último target presentado, conserva su resolución salvo resample explícito y mantiene layout lógico en la placa | `text.html` (`フォントの高さ`) y `calc.html` (`SaveScreen`: 実行画面を保存). Capturar antes del redibujado evita incluir el menú que el script acaba de abrir |

## Huecos / temporales (ejecución posible, deuda o duda)

| ID | Área | Qué hace | Clase | Cómo seguir |
|----|------|----------|-------|-------------|
| W1 | *(cerrado → I6/I7)* | — | — | — |
| W2 | *(cerrado → I12)* | — | — | — |
| W3 | Cinema sin LCM | Placeholder ms | **temporal** | LCM + duración real |
| W4 | Particle/Fire/Wave/Memo | Empty obj | **interpretación** no-op FX | Opcional: FX real |
| W5 | SAVESCREEN | GaleX on-demand | `ok` / **interpretación** | Readback **solo** si hay pedido; TRUE diferido hasta render; encode zlib en CPU |
| W5b | COPYFILE | mem:/save:/host | `ok` | calc.html; VFS `mem_files` + blob en `save.dat` |
| W6 | SETWALLPAPER / tag | `false` / ignore + warn | interpretación “no wallpaper host” | Opcional API DE |
| W7 | IFDEF* | no-op | no confirmado | ¿Aparecen en LSB? |
| W8 | TextIns caja ausente | Stand-in | inventado | OK si novel system lo asume |
| W9 | Call Params | `Param1..N` | no confirmado | Experimento |
| W10 | Props WIDTH/HEIGHT Movie | Ignorados | **I1** | Mantener |
| W11 | LCM angle/clip | Omitir | inventado | Parse o → interpretación |
| W12 | LFT U+0020 / TTF | Síntesis / Noto | inventado + I3 | |
| W13–W15 | *(cerrado → I8/I9)* | — | — | — |
| W16 | SYSMENU* | Solo ONNOTIFY | inventado | Trace novel system |
| W17 | WP_* ids | Orden graphic.html | no confirmado | Dump ids |

---

## Funciones Calc (`Func::*`) — stubs conocidos

| Func | Estado | Notas |
|------|--------|-------|
| OPENURL | `ok` | Abre URL del SO |
| SETWALLPAPER | `noop` | `false` + warn (sin API escritorio) |
| SAVESCREEN | `ok` | GaleX200 desde framebuffer; mem:/save:/host; readback on-demand |
| INPRIMARY | `ok` | Comprueba si el componente pertenece a la superficie delantera (`calc.html`) |
| DELETESTACK | `ok` | Elimina el frame indicado por índice base cero; `@StackCount` informa cuántos frames hay (`calc.html`) |
| COPYFILE | `ok` | mem:/save:/host (calc.html) |
| FIXSTR | `ok` | Quita lead SJIS huérfano / `U+FFFD` tras Copy por bytes |
| COPY (bytes) | `ok` | Índices CP932; JCopy = caracteres (**fuente primaria** calc.html) |
| ADDDELIMITER | `ok` | Si la cadena no está vacía, añade el separador al final (**fuente primaria** calc.html) |
| INDEXOFMENU | `ok` | Índice del texto en el menú, o -1 (**fuente primaria** calc.html) |
| GETDISPLAYMODE | `ok` | Siempre 0 (**I10**) |
| DB* | `ok` / `partial` | Subset en `src/db.rs`; `DBCreateTable` activa la tabla creada y `DBLoadTsvFile` carga en la activa (`calc.html`). La fila TSV inicial declara `nombre tipo [ancho]`; getters tipados convierten sus valores y `DBGetBool` reconoce `TRUE`/`FALSE` y `1`/`0` |
| Resto unimpl | `unimpl` | `unimpl:func …` en `--trace` |

---

## Pendiente priorizado (sin juego vs con juego)

### Se puede avanzar con manual + fixtures

- Wipes geométricos reales (blinds/scroll) o dejar I6.
- VFS `mem:` / `save:` + encoder GAL (SaveScreen).
- Fixtures sintéticos extra; ampliar tabla con nuevos `unimpl:`.

### Necesita juego buildado o original (`--trace` / ojos)

1. Inventario real `unimpl:cmd` / `unimpl:text` / `unimpl:func` (p. ej. GTE·SP **exportado**).
2. Confirmar W16 (SYSMENU). W1, W2 y W13–W15 cerrados.
3. Cinema/LCM en un título que lo use.
4. Paridad visual menú/save (wireframe + usuario mira assets).

---

## Hallazgos (comportamiento aún movible)

Cruce de este índice y de [`LIVEMAKER-INTERFACE-REFERENCE.md`](LIVEMAKER-INTERFACE-REFERENCE.md). Un **H\*** es una observación que hoy sostiene un comportamiento. Si otro script la repite, se cita el H y se dice qué propiedad más entra. Si no se sostiene, el comportamiento de la columna «Qué mueve» cambia.

**No entran** I1–I13 ni W4, W5, W6: son interpretación deliberada. Un hallazgo nuevo no las reescribe.

| ID | Hallazgo | Qué mueve hoy | Si no se sostiene, o si aparece en otro objeto |
|----|----------|---------------|--------------------------------------------------|
| H1 | `Call` publica `Param1..N` (**provisional**, W9) | Nombres que lee la página llamada | Otros nombres, o solo la pila: esos `ParamN` dejan de valer |
| H2 | `Wait` 0 tiene un edge distinto (**no confirmado**) | `step_wait` | Cambia el frame de espera o el salto de la condición |
| H3 | `SetProp VISIBLE=1` desarchiva el objeto y su descendencia, sin Flip de entrada. **Experimento** en un juego LiveNovel, MESOFF/FACE → 再生中 | Archivo en ese Flip | No copiarlo a otro objeto que escriba `VISIBLE` hasta repetir el caso |
| H4 | Cinema sin LCM usa milisegundos fijos (**temporal**, W3) | Duración del placeholder | Con LCM usable, el clip y el tiempo sustituyen el placeholder |
| H5 | `GameSave` guarda globals + page/label, no la escena | Qué restaura `GameLoad` | Un snapshot devolvería los objetos |
| H6 | `IFDEF` / `IFNDEF` / `ENDIF` en el LSB son no-op (**no confirmado**, W7) | Esas ramas se ejecutan | Si el runtime los honra, la rama cortada deja de correr |
| H7 | `CREATECG` / `CHANGECG` / `DELETECG`: subset de args; para `CREATECG`, los slots 4/5 reconocen tokens de alineación (`C`/`R`, `C`/`B`); los valores numéricos no son coordenadas. Interpretarlos así desplazaba sprites y menús | Posición de CG | **Confirmado por el usuario en test2/testgame:** al ignorar esos valores numéricos, los menús vuelven a funcionar. Prueba sintética cubre `(500, 428)`. No usar offsets por juego |
| H8 | Orden de args de `TEXTSPD` **no confirmado** | Qué número es TIME y cuál ABSOLUTE | El ritmo cambia si el orden está cruzado. La curva 60 cps / ×3 es I8 y se queda |
| H9 | Orden de args de `QUAKE` **no confirmado** | Qué valor es Q, W o B | El tipo de temblor se asigna a otro eje. El hash de Q es I9 y se queda |
| H10 | `<PS>` / `{PAUSE}` + `<PG>` se funden (**inventado**). `<PG>` con `cur` vacío solo hace Clear (**experimento**, tras STOPSND). TextIns sin caracteres: `<PG>` solo Clear (**experimento**, logo y título) | Clic de página frente a Clear | Si el original pide clic, el logo o el mensaje esperan o avanzan solos |
| H11 | `SYSMENU*` / `SAVELOAD*` solo lanzan ONNOTIFY (W16) | Permiso de menú y de save | Si además baja un flag, el menú seguiría abriéndose con solo el notify |
| H12 | Orden de ids `WP_*` tomado de `graphic.html` (**no confirmado**, W17) | Qué wipe es WHITE, BLACK o FLASH | Un id mal leído cambia el overlay. El resto sigue en el fundido de I6 |
| H13 | `TextIns` sin caja usa un stand-in (**inventado**, W8) | Dónde cae el texto | Si el novel system no crea caja, ese texto no debería pintarse |
| H14 | LCM angle/clip se omiten (**inventado**, W11) | Transform del cinema | Parsearlos rotaría o recortaría el frame |
| H15 | Origen de `TOPINDEX` y el fuera de rango: **no confirmado**. Choices en base 0 y clamp: **inventado** (interface ref) | Primera fila visible y el índice que el script lee | Origen 1, o un fuera de rango sin clamp, mueve la lista y el valor devuelto |
| H16 | `WIDTH` ausente o ≤ 0: ancho intrínseco medido **antes** de `ONOPENING` (**inventado**). El manual calla | `GetProp(Sender, WIDTH)` en la apertura, y el ancho dibujado | Si LiveMaker deja 0, el script de apertura lee 0 |
| H17 | `LEFT` ausente centra el menú (**inventado**). Posición omitida: **no confirmado** | Dónde aparece el menú | Si la omisión es el origen del padre, el menú pasa a esa esquina |
| H18 | Troceado, orientación y escala de `SCROLLSOURCE`: **no confirmado**. Hit-test de flecha, página y grip: sin hacer. `OT_HORIZONTAL` / `OT_VERTICAL` del Scrollbar no mandan el `SCROLLSOURCE` del menú | Clic en la barra y el eje del menú | Confirmar el troceado mueve el grip. Confirmar el eje tumba la barra de un menú horizontal |
| H19 | Una lista Save/Load midió 140×20 y se partió en siete celdas de 20×20. Un solo asset | Solo esa imagen | Reusarlo en otro menú parte mal la tira |
| H20 | Layout `.lft` leído en versión `100` de un solo juego (**no confirmado** fuera). U+0020 ausente: el avance del espacio se sintetiza (**inventado**, W12) | Glifos y el ancho del espacio | Otra versión, o un espacio real en el `.lft`, cambian el avance |
| H21 | Los exports de `live.dll` son la superficie nativa de media. Mapearlos 1:1 a comandos LSB o a nombres de prop: **no confirmado** | Qué símbolo atendería un comando de media | Ese mapeo cambiaría el dispatch. Hoy no se hace |
| H22 | La semántica geométrica de los dos pares binarios `.lpm` sigue sin confirmarse. Se conserva la heurística previa: usar el segundo par como posición solo cuando el primero coincide con el tamaño leído desde un prefijo GAL sin descompresión. La hipótesis de que esto causara el menú desplazado quedó descartada por la corrección H7, confirmada por el usuario en test2/testgame. XML guarda `Left`/`Top` en el primer par y `PrevLeft`/`PrevTop` en el segundo; la semántica visual sigue sujeta a la misma heurística. La captura existente de testgame en `--debug-layout` muestra `背景` 800×600 en x=500 y `選択メニュー` en x=564; esto solo mide lo dibujado, no demuestra la semántica `.lpm`. Log del usuario: `縦メニュー表示ボタン` en (480,428), clic derecho `no-handler`, clic izquierdo `ONNOTIFY` a `00000086.lsc:00000180`; la ruta de clic izquierdo sí dispara | Confirmar la semántica de `.lpm` con evidencia independiente | Menú desplazado resuelto mediante H7; no compensar con offsets por juego. No ejecutar juegos ni generar capturas |
| H23 | Gale binario: bytes 4..8 de la cabecera de frame = duración en ms (**experimento**, valores plausibles en un juego) | Ritmo de animaciones `.gal` | Si el campo es otra cosa, la animación corre a otro ritmo |
| H24 | Índice numérico de `ObjProp` en base 0 (PropertyType = índice + 1) (**experimento**: PARENT, LEFT y TOP cuadran) | Qué prop lee un `GetProp` numérico | Con otra base se leería la prop vecina |
| H25 | La captura ya existente de testgame reporta resolución `fallback 800×600`; contar GAL comprimidos para inferir la resolución lógica no es fiable porque el conjunto puede incluir CG y sprites grandes, no solo fondos | Resolución lógica que reciben `@ScrWidth/@ScrHeight`, que puede afectar posiciones calculadas por scripts | No inferirla de GAL comprimidos; mantener la ruta previa hasta que haya una fuente de resolución autoritativa. Esto no confirma el fallo de test2, cuyo LPB declara 800×600. No ejecutar juegos ni generar capturas |
| H26 | En testgame, el menú de personalidad codifica la opción previamente elegida con `●` en el propio texto y usa U+3000 en la otra. Las cuatro traducciones correspondientes de `translations/es.toml` comienzan con `●`, incluso para las variantes fuente que comienzan con U+3000. `translate_menu_choices` sustituía la etiqueta completa y convertía ambas en marcadas. El menú dinámico usa `HOVERCOLOR=65280`, pero no `CHECKSOURCE`; la documentación define `HOVERCOLOR` como fondo de la fila bajo el cursor y `CHECKSOURCE` como una tira de check normal/hover | Conservar el marcador que el script establece al traducir la etiqueta; separar ese estado del hover visual | El script alterna el marcador entre A y B. La traducción debe traducir el cuerpo y preservar el prefijo fuente (`●`/U+3000); `CHECKSOURCE` no es el mecanismo para este menú |

## Mantenimiento

Al tocar compatibilidad:

1. Actualizar la fila aquí (estado + “livenyan hoy” + fuente).
2. Si es interface/menú/LFT fino → también `LIVEMAKER-INTERFACE-REFERENCE.md`.
3. Marcar workarounds nuevos en la tabla W*.
4. Si el comportamiento puede moverse con una observación nueva, añadir o citar un H*.
5. Tests: preferir fixture/`--lib` sin assets gráficos reales.

Última pasada: 2026-10-29 — tabla de hallazgos H1–H21 (comportamiento aún movible).
