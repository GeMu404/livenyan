# Compatibilidad LiveMaker → livenyan

Base de conocimiento del motor: **qué hay**, **cómo**, **comportamiento esperado**, **fuente**, **huecos**.

Antes de implementar o “arreglar” semántica: leer esto + [`LIVEMAKER-INTERFACE-REFERENCE.md`](LIVEMAKER-INTERFACE-REFERENCE.md) + la página citada bajo `docs/livenovel/`. No subir el repo entero al contexto: este archivo es el índice.

## Cómo usar (humanos y agentes)

1. Buscar el comando / tag / workaround por nombre.
2. Mirar **Estado** y **Fuente**.
3. Si hay que cambiar comportamiento ambiguo: actualizar la fila **y** el código; etiquetar el cambio (`fuente primaria` / `no confirmado` / `inventado en livenyan` / `experimento`).
4. No promover un workaround a “así es LiveMaker” sin cita nueva.

### Estados

| Estado | Significado |
|--------|-------------|
| `ok` | Comportamiento alineado a fuente citada (puede faltar edge cases). |
| `partial` | Parte útil; resto aproximado o sin paridad. |
| `placeholder` | Objeto/tiempo para no romper el flujo; sin efecto real completo. |
| `noop` | Se acepta y no hace (casi) nada; documentado. |
| `unimpl` | Aviso `unimpl:*` / ignorado; no hay camino feliz. |
| `inventado` | Decisión livenyan consciente para poder ejecutar. |

### Tiers de fuente (igual que la interface reference)

`fuente primaria` · `no confirmado` · `inventado en livenyan` · `experimento`

Citas típicas: `docs/livenovel/<page>.html`, `docs/live-dll-strings.txt`, fixture/`--trace` vs original.

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
| 12 | Movie | `ok` / `partial` | Vídeo | ffmpeg externo | Sin ffmpeg → fallo/aviso |
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

## Registro de workarounds (ejecución posible)

Cosas hechas a propósito para no tumbar el juego. **No** son semántica LiveMaker.

| ID | Área | Inventado | Por qué | Cómo quitar / confirmar |
|----|------|-----------|---------|-------------------------|
| W1 | Flip wipes ≠ 0/1 | Fade `ALPHA` | Sin shaders de wipe | Experimento wipe a wipe; o clip masks |
| W2 | Flip Act | Fade in/out | Manual = capas FL_OUT/STAY | Experimento vs original |
| W3 | Cinema | Placeholder ms | Sin decode LCM completo | LCM path + duración real |
| W4 | Particle/Fire/Wave/Memo | Empty obj | Scripts crean y siguen | Implementar FX o no-op silencioso documentado |
| W5 | SAVESCREEN / COPYFILE | `true` | UI save no rompe | Encoder GAL / copy real |
| W6 | SETWALLPAPER | `true` no-op | Sin API escritorio | Plataforma o unimpl honesto |
| W7 | IFDEF* | no-op | Directivas compile-time | Confirmar si LSB las emite |
| W8 | TextIns caja ausente | Stand-in / graveyard | Novel system borra/recrea | |
| W9 | Call Params | `Param1..N` | Convención no verificada | Experimento / dump |
| W10 | Movie size LSB | Pantalla lógica | inventado previo | Probar vs prop LSB |
| W11 | LCM angle/clip | Omitir | No tumbar LCM | Parse completo |
| W12 | LFT U+0020 / TTF | Síntesis / Noto | Fuentes missing | | Ver interface ref |
| W13 | TEXTSPD relativo | baseline 60 cps | Default config | Medir motor original |
| W14 | TXSPS | ×3 periodo | Manual sin ratio | Experimento |
| W15 | QUAKE Q | Hash ruido | Sin RNG documentado | Experimento |
| W16 | SYSMENU* | Solo ONNOTIFY | Sin flags internas conocidas | Trace novel system |
| W17 | WP_* ids 2..38 | Orden tabla graphic.html | const.html sin números | Dump runtime / strings |

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
