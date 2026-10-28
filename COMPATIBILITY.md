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
| 8–10 | BoxNew / ImgNew / MesNew | `ok` | Crear componentes | `step_obj_new` | |
| 11 | Timer | `ok` | Temporizador | Timers + eventos | |
| 12 | Movie | `ok` / `partial` | Vídeo | ffmpeg; decode **nativo** (I1) | Manual size; **interpretación** latencia |
| 13 | Flip | `partial` | Wipe `WP_*` + Act FL_* | `NONE`/`FADE` OK; resto **fade approx**; Act como fade in/out | `graphic.html` / `wipe.rs`; Act capas **no confirmado** |
| 14 | Calc | `ok` | Evaluar expresión | | |
| 15–16 | VarNew / VarDel | `ok` | Vars + Scope | Scope 0\|3 → globals; 1\|2 locals | Reset conserva scope 3 |
| 17–18 | GetProp / SetProp | `ok` | Props objeto | Missing SetProp = no-op LM | |
| 19 | ObjDel | `ok` | Borrar + cascada | `scrap_component` | |
| 20 | TextIns | `ok` | Insertar texto TpWord | Plan Pending + plays | `<PG>` coalesce: `script.html` |
| 21 | MovieStop | `ok` | Parar película | | |
| 22 | ClrHist | `ok` | Limpiar historial | | |
| 23 | Cinema | `placeholder` | LiveCinema | Timed placeholder si no hay LCM usable | `unimpl:cmd Cinema…` |
| 24 | Caption | `ok` | Caption | | |
| 25–26 | Menu / MenuClose | `ok` / `partial` | Menús | Ver interface reference | |
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
| 59–60 | SaveCabinet / LoadCabinet | `ok` | Guardar/restaurar objs | | |
| 61–63 | IFDEF / IFNDEF / ENDIF | `noop` | Preprocesador chart | No-op si aparecen en LSB | Runtime **no confirmado** |

---

## Tags / sysevents de texto (`\x01NAME`…)

Dispatch: `system_text_event` en `src/vm/text_hist.rs`. Manual: `docs/livenovel/script.html`.

| Tag | Estado | Esperado | livenyan | Fuente / notas |
|-----|--------|----------|----------|----------------|
| PAUSE | `ok` | Clic / TEXTPAUSED + ONNOTIFY | Igual | `script.html` / novel system |
| TEXTCLEAR | `ok` | Vaciar caja | Igual | |
| MESON / MESOFF | `ok` | Visibilidad vía novel system | Solo ONNOTIFY | No hardcodear VISIBLE |
| WAITPLAY | `ok` | Esperar efecto/obj | `wait_for` + click skip | |
| PLAYMOVIE / STOPMOVIE | `ok` / `partial` | Vídeo inline | Objeto `ムービー` + ffmpeg | |
| CREATECG / CHANGECG / DELETECG | `partial` | CG / color / alín | Subset de args | Resto **sin verificar** |
| PLAYSND / STOPSND | `ok` | SE/BGM canales | | |
| TEXTSPD | `partial` | TIME ms/char; ABSOLUTE | Override `text_char_ms` | Orden args **no confirmado**; relativo baseline 60 **inventado** |
| TXSPF / TXSPN / TXSPS | `partial` | Fast / normal / slow | 0 ms / clear+256 / ×3 baseline | TXSPS ratio **inventado** |
| SYSMENUON/OFF SAVELOADON/OFF | `partial` | Permisos menú/save | ONNOTIFY only | Puede no bastar |
| OPENURL | `ok` | Abrir URL | `xdg-open` / `open` / `cmd start` | `script.html` + Func OPENURL |
| WALLPAPER | `unimpl` | Wallpaper SO | Aviso | Sin API |
| WAITFOR | `ok` | Esperar componente | Como WAITPLAY + CLICKABORT | `script.html` |
| QUAKE / STOPQUAKE | `partial` | Temblor Q/W/B | Offset dibujo | Orden args **no confirmado**; Q hash **inventado** |
| *(otros)* | `unimpl` | — | `unimpl:text sysevent` | Inventariar con `--trace` |

`<PG>` / `<PS>`: glifos Return, no sysevent. Coalesce clic: **fuente primaria** `script.html` + fusión **inventada** para no doble-clic en el mismo TextIns.

---

## Interpretaciones livenyan (divergencia deliberada)

Diseño estable: **no** son bugs ni “hasta que haya 1:1”.

| ID | Área | LM / ingenuo | livenyan | Por qué |
|----|------|--------------|----------|---------|
| I1 | Decode Movie/Cinema frames | Escalar en ffmpeg al tamaño lógico/pantalla (latencia alta) | Decode a **tamaño nativo**; escala en present/SDL | Menos CPU al abrir; letterbox/sharp ya escalan |
| I2 | Present | Ventana GDI LiveMaker | SDL3 + letterbox + `scale_filter` | Portabilidad / nitidez moderna |
| I3 | Tipografía host | ＭＳ ゴシック | Noto CJK u otra si falta | Linux/Windows sin fuentes JP |
| I4 | `TOSCREEN*` / coords | Espacio ventana Windows | Identidad en espacio lógico | El letterbox lo resuelve el present |
| I5 | Renderer | DirectDraw/GDI/etc. | SDL auto (sin Vulkan/OF propio) | Mantenibilidad; OF archivado |

## Huecos / temporales (ejecución posible, deuda o duda)

| ID | Área | Qué hace | Clase | Cómo seguir |
|----|------|----------|-------|-------------|
| W1 | Flip wipes ≠ 0/1 | Fade `ALPHA` | inventado | Wipe real o aceptar fade como interpretación |
| W2 | Flip Act | Fade in/out | no confirmado | Experimento vs capas FL_* |
| W3 | Cinema sin LCM | Placeholder ms | **temporal** | LCM + duración real |
| W4 | Particle/Fire/Wave/Memo | Empty obj | inventado / temporal | FX o interpretación “no-op documentado” |
| W5 | SAVESCREEN / COPYFILE | `true` | inventado | Encoder / copy, o unimpl |
| W6 | SETWALLPAPER | `true` | inventado | API host o unimpl |
| W7 | IFDEF* | no-op | no confirmado | ¿Aparecen en LSB? |
| W8 | TextIns caja ausente | Stand-in | inventado | OK si novel system lo asume |
| W9 | Call Params | `Param1..N` | no confirmado | Experimento |
| W10 | Props WIDTH/HEIGHT en Movie LSB | Ignorados en decode | **interpretación** (ver I1) + manual: Movie no documenta esas props | Mantener |
| W11 | LCM angle/clip | Omitir | inventado | Parse completo o documentar como interpretación |
| W12 | LFT U+0020 / TTF | Síntesis / Noto | inventado + I3 | |
| W13 | TEXTSPD relativo | baseline 60 | inventado | Medir LM u fijar como interpretación |
| W14 | TXSPS | ×3 | inventado | Idem |
| W15 | QUAKE Q | Hash | inventado | Idem |
| W16 | SYSMENU* | Solo ONNOTIFY | inventado | Trace novel system |
| W17 | WP_* ids | Orden graphic.html | no confirmado | Dump ids |

---

## Funciones Calc (`Func::*`) — stubs conocidos

| Func | Estado | Notas |
|------|--------|-------|
| OPENURL | `ok` | Abre URL del SO |
| SETWALLPAPER | `inventado` | Siempre true |
| SAVESCREEN / COPYFILE | `inventado` | Siempre true |
| GETDISPLAYMODE | `partial` | Siempre 0 |
| DB* | `ok` / `partial` | Subset en `src/db.rs` |
| Resto unimpl | `unimpl` | `unimpl:func …` en `--trace` |

---

## Pendiente priorizado (sin juego vs con juego)

### Se puede avanzar con manual + fixtures

- Documentar/ajustar edge cases Wait / Prop faltantes con cita.
- Fixtures LSB sintéticos para PCReset/Reset/TEXTSPD (orden args cuando se confirme).
- Ampliar esta tabla cuando aparezca un `unimpl:` nuevo en logs de fixture.

### Necesita juego buildado o original (`--trace` / ojos)

1. Inventario real `unimpl:cmd` / `unimpl:text` / `unimpl:func` (p. ej. GTE·SP **exportado**).
2. Confirmar W2 (Act), W1 (un wipe), W13–W15 (TEXTSPD/QUAKE), W16 (SYSMENU).
3. Cinema/LCM en un título que lo use.
4. Paridad visual menú/save (wireframe + usuario mira assets).

---

## Mantenimiento

Al tocar compatibilidad:

1. Actualizar la fila aquí (estado + “livenyan hoy” + fuente).
2. Si es interface/menú/LFT fino → también `LIVEMAKER-INTERFACE-REFERENCE.md`.
3. Marcar workarounds nuevos en la tabla W*.
4. Tests: preferir fixture/`--lib` sin assets gráficos reales.

Última pasada de inventario comandos/tags: 2026-10-28 (post `PCReset`/`Reset` + tags TEXTSPD/QUAKE/OPENURL).
