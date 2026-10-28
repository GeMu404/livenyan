# LiveMaker interface reference

Working reference for LiveMaker interface behavior in LiveNyan.  
**Do not invent semantics.** Every claim below is tagged by source tier.

## Source tiers

| Tier | Meaning | Use when implementing |
|---|---|---|
| **fuente primaria** | Documented in the LiveNovel manual (local mirror) and/or named in pylivemaker enums that match the manual. | May drive behavior and tests. |
| **no confirmado** | Reasonable inference, single-game observation, or manual silence. | Must not become “how LiveMaker works” without a citation upgrade. |
| **inventado en livenyan** | Engine choice for gaps (fallback, clamp, Linux font substitute, etc.). | Keep labeled; cover with tests; do not back-port into the manual. |

Ambiguous fixes require an explicit citation: **manual** (`docs/livenovel/…`), **strings** (`docs/live-dll-strings.txt` or equivalent extract), or a **controlled experiment** vs the original engine. See `.cursor/rules/livenyan.mdc`.

## Primary sources (paths)

| Source | Path / URL | Notes |
|---|---|---|
| LiveNovel manual (local) | [`docs/livenovel/`](docs/livenovel/README.md) | Mirror of pylivemaker `_static/LiveNovel/`; start at `topics.html` / `interface.html`. |
| LiveNovel manual (upstream) | [pylivemaker `docs/_static/LiveNovel`](https://github.com/pmrowla/pylivemaker/tree/master/docs/_static/LiveNovel) | Same tree; ReadTheDocs `_static/LiveNovel/` is the hosted view. |
| Property / command enums | [pylivemaker `lsb/core.py` `PropertyType`](https://github.com/pmrowla/pylivemaker/blob/ceeb12911c571fe6da77107bb0eaf7eb29594612/src/livemaker/lsb/core.py) | Numeric LSB ids (`PR_ROWCOUNT`, …), not prose semantics. |
| `live.dll` exports/strings | [`docs/live-dll-strings.txt`](docs/live-dll-strings.txt) | Media helper API only. **No** GetProp/SetProp / `ROWCOUNT` / etc. in this DLL. |

## Menu — fuente primaria

From `docs/livenovel/interface.html` (Menu / Scrollbar sections):

- `ROWCOUNT` — number of rows visible at once. **fuente primaria**
- `TOPINDEX` — which row to start displaying from. **fuente primaria** (index origin / OOR: **no confirmado**)
- `SCROLLSOURCE` — image for the menu scrollbar; manual lists parts in order: up, pressed up, down, pressed down, background, pressed background, grip. **fuente primaria**
- Menu `X` / `Y` — offsets relative to the parent, not absolute screen coordinates. **fuente primaria**
- Horizontal alignment (`HA_CENTER`) — text alignment inside the menu; not “center the menu widget on screen”. **fuente primaria**

## Menu — no confirmado (manual silent or incomplete)

- Automatic menu width / meaning of missing or zero `WIDTH`.
- Automatic centering when position is omitted.
- Placement, orientation, and scaling of a Menu’s `SCROLLSOURCE` parts (vs the separate Scrollbar component).
- `TOPINDEX` origin (0 vs 1) and out-of-range behavior.
- That Scrollbar’s `OT_HORIZONTAL` / `OT_VERTICAL` rules control a Menu’s `SCROLLSOURCE`.

Do not promote a single game’s script or layout into these slots.

## Menu — inventado en livenyan

- Zero-based choice indices; clamp `TOPINDEX` to available choices (keep tested).
- If script `WIDTH` is not positive: measure intrinsic width **before** `ONOPENING` from labels + selected font (margins, border, paged-menu scrollbar) so `GetProp(Sender, WIDTH)` in opening scripts sees a value.
- Font load failure → fallback width `native_menu_size` (~60% of logical screen width).
- Missing `LEFT` → center the measured/default-width menu; explicit `LEFT` stays left edge (parent-relative).
- Save/Load list image observed 140×20 in one game’s debug logs → seven 20×20 cells for that asset only; not universal.
- Font substitute: prefer Noto Sans Mono CJK JP when `ＭＳ ゴシック` is missing (monospace-ish metrics); still an approximation.
- Scrollbar renderer: vertical strip on the right; grip size from visible-row ratio; keyboard + wheel implemented; arrow/page/grip mouse hit-testing not established.
- User comparison: original Save/Load list centered with scrollbar on the right — consistent with scripts/centering choice above, not a new manual rule.

## Scrollbar component (separate from Menu SCROLLSOURCE)

- Manual: `OT_HORIZONTAL` / `OT_VERTICAL`; parent-relative `X` / `Y`. **fuente primaria**
- Equating those rules with Menu `SCROLLSOURCE` layout: **no confirmado** / do not assume.

## Tipografía embebida LivePixelFont (`.lft`)

- Manual LiveNovel / pylivemaker: **sin especificación** del contenedor `.lft`.
- testgame: entrada VF `font0.lft`, magic `LivePixelFont\0`, nombre interno CP932 `JKゴシックM`. `StatusFontName` del `.lpb` sigue siendo `ＭＳ ゴシック` (sistema); el `.lft` es recurso aparte y los scripts ponen `FONTNAME` = `JKゴシックM`. **experimento** (bytes + LSB).
- Layout inferido (solo testgame; **no confirmado** fuera de version `100`):
  - cabecera: magic + `u32` versión + tamaños zlib-dir / bitmaps + nombre[64] + métricas[232]
  - directorio zlib: registros de 18 B (`u16` código CP932 BE, `w/h`, `i16` bearings, `u16` advance, `u16` 0, `u32` offset)
  - bitmaps crudos 8 bpp, stride = `align4(w)`
- Espacio U+0020 ausente en ese `.lft` → livenyan sintetiza avance con el campo de métricas interpretado como ancho medio (**inventado en livenyan**).
- Sustituto TTF/Noto cuando no hay `.lft` o el `FONTNAME` no coincide: sigue siendo **inventado en livenyan**.

## Media / `live.dll`

- Exported `liveOgg*` / `liveVideo*` / `liveWmp*` / `liveScreen*` symbols: **fuente primaria** for *native media DLL surface only* (see `docs/live-dll-strings.txt`).
- Mapping those exports 1:1 onto LSB Media commands or script property names: **no confirmado** unless cited from `docs/livenovel/media.html` or experiment.
- testgame and test2 `live.dll`: identical (`sha256` in the strings file).

## Maintenance

Before changing menu geometry, scrollbar slicing, scrolling, or property defaults:

1. Check `docs/livenovel/` (relevant HTML) and this file.
2. Tag the change as primaria / no confirmado / inventado.
3. Prefer manual text or a controlled original-engine comparison; record uncertainty instead of inventing a spec.
