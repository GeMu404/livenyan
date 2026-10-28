# LiveNovel manual mirror

Local copy of the LiveMaker **LiveNovel** HTML manual, mirrored from pylivemaker’s
`docs/_static/LiveNovel/` (same tree as on ReadTheDocs `_static/LiveNovel/`).

| | |
|---|---|
| Upstream | https://github.com/pmrowla/pylivemaker/tree/master/docs/_static/LiveNovel |
| Mirrored commit | `ceeb12911c571fe6da77107bb0eaf7eb29594612` (2025-03-30) |
| Language | Japanese (Shift_JIS HTML) |
| Role in livenyan | **Primary source** for script/UI semantics. Prefer this over guesses. |

Open `index.html` or `topics.html` in a browser. Do not treat English paraphrases
elsewhere in the repo as stronger than these pages.

## Quick index (high-value pages)

| Topic | File |
|---|---|
| Table of contents | `topics.html` |
| Interface (Menu, Scrollbar, Slider, …) | `interface.html` |
| Object GetProp / SetProp | `object.html` |
| Properties overview | `prop.html` |
| Scenario script | `script.html` |
| Calc / operators / **functions** | `calc.html` (`#func`, `#sysvar`, `#calc`) |
| Text / message / history | `text.html` |
| Graphics / surfaces | `graphic.html` |
| Sound / movie | `media.html` |
| Control flow (IF, loop, call, …) | `sequence.html` |
| Save / load | `file.html` |
| Constants | `const.html` |

Full TOC (tutorials, menus, FAQ): see `topics.html`.

## Related local extracts

- `../live-dll-strings.txt` — PE exports/strings from `live.dll` (media API only; not LSB Prop/Cmd tables).
- `../../LIVEMAKER-INTERFACE-REFERENCE.md` — working notes with source tiers.
