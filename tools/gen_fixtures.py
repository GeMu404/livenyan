#!/usr/bin/env python3
"""Genera fixtures sintéticos (LSB, GAL, archivo VF, exe falso) usando pylivemaker.
Uso: python3 tools/gen_fixtures.py tests/fixtures
Requiere: pip install pylivemaker
No contiene datos de ningún juego real."""
import os, struct, sys, zlib, tempfile
from livemaker.archive import LMArchive, LMCompressType
from livemaker.lsb.lmscript import LMScript
from livemaker.lsb import command as C
from livemaker.lsb.core import LiveParser, OpeData, OpeDataType as T, OpeFuncType as F, Param, ParamType as P, LiveParserArray
from livemaker.lsb.novel import TpWord, TWdChar, TWdOpeReturn

out = sys.argv[1] if len(sys.argv) > 1 else "tests/fixtures"
os.makedirs(out, exist_ok=True)

def V(n): return Param(n, P.Var)
def I(n): return Param(n, P.Int)
def S(s): return Param(s, P.Str)
def B(b): return Param(int(b), P.Flag)
def expr(*entries): return LiveParser(list(entries))
def to(name, val): return OpeData(T.To, name, operands=[val])
def lit(v): return expr(to("____arg", v))
def op(t, name, a, b): return OpeData(t, name, operands=[a, b])

# command_params: one bool list per command type; component commands use bit j -> PropertyType(j+1)
NPROPS = 0xAD
cmd_params = [[False] * NPROPS for _ in range(0x40)]
def enable(ctype, *props):
    for p in props: cmd_params[ctype][p - 1] = True
from livemaker.lsb.core import PropertyType as PT
for ct in (C.CommandType.ImgNew, C.CommandType.MesNew, C.CommandType.BoxNew):
    enable(ct, PT.PR_NAME, PT.PR_SOURCE, PT.PR_LEFT, PT.PR_TOP, PT.PR_WIDTH, PT.PR_HEIGHT, PT.PR_PRIORITY)
enable(C.CommandType.Sound, PT.PR_NAME, PT.PR_SOURCE, PT.PR_VOLUME, PT.PR_REPEAT)
enable(C.CommandType.Menu, PT.PR_NAME, PT.PR_TEXT)

def comps(ctype, vals):
    # vals: dict PropertyType -> LiveParser, in enabled order
    order = [i + 1 for i, f in enumerate(cmd_params[ctype]) if f]
    return [vals.get(PT(p), expr()) for p in order]

def tpword(text):
    body = []
    for ch in text:
        if ch == "\n": body.append(TWdOpeReturn(break_type=0, condition=-1))
        else: body.append(TWdChar(ch=ch, decorator=0, condition=-1, link=-1, text_speed=0))
    body.append(TWdOpeReturn(break_type=1, condition=-1))
    return TpWord(version=105, decorators=[], conditions=[], links=[], body=body)

def mk(cls, line, indent=0, **kw):
    return cls(Indent=indent, LineNo=line, **kw)

cp = lambda t: cmd_params[t]
cmds = []
n = 0
def add(cls, indent=0, **kw):
    global n
    n += 1
    if issubclass(cls, C.BaseComponentCommand):
        kw["command_params"] = cp(cls.type)
    cmds.append(mk(cls, n, indent, **kw))
    return len(cmds) - 1

# main script: counter loop, if/else, call sub, image, text, wait, sound, jump to label
add(C.VarNew, Name="count", Type=P.Int, InitVal=lit(I(0)), Scope=0)
add(C.VarNew, Name="name", Type=P.Str, InitVal=lit(S("nyan")), Scope=0)
add(C.Label, Name="top")
add(C.Calc, Calc=expr(op(T.Plus, "____1", V("count"), I(1)), to("count", V("____1"))))
add(C.If, Calc=expr(op(T.Small, "____arg", V("count"), I(3))))
jmp = add(C.Jump, indent=1, Page=C.LabelReference("00000001.lsb", 2), Calc=lit(B(True)))
add(C.Else)
add(C.Calc, indent=1, Calc=expr(OpeData(T.Func, "____1", func=F.IntToStr, operands=[V("count")]), to("name", V("____1"))))
add(C.ImgNew, components=comps(C.CommandType.ImgNew, {PT.PR_NAME: lit(S("bg")), PT.PR_SOURCE: lit(S("bg.gal")), PT.PR_LEFT: lit(I(0)), PT.PR_TOP: lit(I(0))}))
add(C.MesNew, components=comps(C.CommandType.MesNew, {PT.PR_NAME: lit(S("msg")), PT.PR_LEFT: lit(I(20)), PT.PR_TOP: lit(I(400)), PT.PR_WIDTH: lit(I(760)), PT.PR_HEIGHT: lit(I(180))}))
add(C.Sound, components=comps(C.CommandType.Sound, {PT.PR_NAME: lit(S("se")), PT.PR_SOURCE: lit(S("se.wav"))}))
add(C.Call, Page=C.LabelReference("sub.lsb", 0), Result="ret", Calc=lit(B(True)), Params=LiveParserArray([], prefixed=True))
add(C.TextIns, Text=tpword("こんにちは、世界\nテスト"), Target=lit(S("msg")), Hist=lit(B(True)), Wait=lit(B(True)), StopEvent=expr())
add(C.Wait, Calc=lit(B(True)), Time=lit(I(100)), StopEvent=expr())
add(C.WhileInit, Calc=expr(to("count", I(0))))
w = add(C.While, Calc=expr(op(T.Small, "____arg", V("count"), I(2))), End=0)
add(C.Calc, indent=1, Calc=expr(op(T.Plus, "____1", V("count"), I(1)), to("count", V("____1"))))
wl = add(C.WhileLoop, Calc=expr(), Start=w)
cmds[w].args["End"] = wl
add(C.ObjDel, Calc=None) if False else add(C.ObjDel, Name=lit(S("bg")))
add(C.Exit, Calc=expr())
cmds[jmp].args["Page"] = C.LabelReference("00000001.lsb", 2)

cmd_params_list = cmd_params
main = LMScript(version=117, flags=0, command_params=cmd_params_list, commands=cmds)
open(os.path.join(out, "00000001.lsb"), "wb").write(main.to_lsb())

cmds = []; n = 0
add(C.Calc, Calc=expr(to("subran", I(1))))
add(C.Exit, Calc=lit(I(42)))
sub = LMScript(version=117, flags=0, command_params=cmd_params_list, commands=cmds)
open(os.path.join(out, "sub.lsb"), "wb").write(sub.to_lsb())

cmds = []; n = 0
add(C.Call, Page=C.LabelReference("00000001.lsb", 0), Result="", Calc=lit(B(True)), Params=LiveParserArray([], prefixed=True))
add(C.Exit, Calc=expr())
gm = LMScript(version=117, flags=0, command_params=cmd_params_list, commands=cmds)
open(os.path.join(out, "ゲームメイン.lsb"), "wb").write(gm.to_lsb())

# --- evt/: regresión del bucle de espera de LiveNovel (終了待ち): TextIns a un objeto que aún no
# existe, luego MesNew con ONPLAYEND y un While que espera la variable que pone el manejador ---
evt = os.path.join(out, "evt"); os.makedirs(evt, exist_ok=True)
enable(C.CommandType.MesNew, PT.PR_ONPLAYEND)
cmds = []; n = 0
add(C.VarNew, Name="__done", Type=P.Flag, InitVal=lit(B(False)), Scope=0)
add(C.TextIns, Text=tpword("ab"), Target=lit(S("mbox")), Hist=lit(B(True)), Wait=lit(B(True)), StopEvent=expr())
add(C.Call, Page=C.LabelReference("終了待ち.lsb", 0), Result="", Calc=lit(B(True)), Params=LiveParserArray([], prefixed=True))
add(C.Exit, Calc=expr())
open(os.path.join(evt, "ゲームメイン.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.MesNew, components=comps(C.CommandType.MesNew, {PT.PR_NAME: lit(S("mbox")), PT.PR_WIDTH: lit(I(300)), PT.PR_HEIGHT: lit(I(100)), PT.PR_ONPLAYEND: lit(S("end.lsc"))}))
w = add(C.While, Calc=expr(op(T.Equal, "____arg", V("__done"), I(0))), End=0)
add(C.Calc, indent=1, Calc=expr(to("spin", I(1))))
wl = add(C.WhileLoop, Calc=expr(), Start=w)
cmds[w].args["End"] = wl
add(C.Exit, Calc=expr())
open(os.path.join(evt, "終了待ち.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.Calc, Calc=expr(to("__done", B(True))))
add(C.Exit, Calc=expr())
open(os.path.join(evt, "end.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())

# --- evt2/: Exit condicional (Calc falso no sale) + TextIns vacío que igual dispara ONPLAYEND ---
evt2 = os.path.join(out, "evt2"); os.makedirs(evt2, exist_ok=True)
cmds = []; n = 0
add(C.VarNew, Name="__done", Type=P.Flag, InitVal=lit(B(False)), Scope=0)
add(C.MesNew, components=comps(C.CommandType.MesNew, {PT.PR_NAME: lit(S("mbox")), PT.PR_WIDTH: lit(I(300)), PT.PR_HEIGHT: lit(I(100)), PT.PR_ONPLAYEND: lit(S("end.lsc"))}))
add(C.Call, Page=C.LabelReference("cond.lsb", 0), Result="", Calc=lit(B(True)), Params=LiveParserArray([], prefixed=True))
add(C.TextIns, Text=TpWord(version=105, decorators=[], conditions=[], links=[], body=[]), Target=lit(S("mbox")), Hist=lit(B(True)), Wait=lit(B(False)), StopEvent=expr())
w = add(C.While, Calc=expr(op(T.Equal, "____arg", V("__done"), I(0))), End=0)
add(C.Calc, indent=1, Calc=expr(to("spin", I(1))))
wl = add(C.WhileLoop, Calc=expr(), Start=w)
cmds[w].args["End"] = wl
add(C.Exit, Calc=expr())
open(os.path.join(evt2, "ゲームメイン.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.Exit, Calc=expr(op(T.Equal, "____arg", I(0), I(1))))
add(C.Calc, Calc=expr(to("reached", I(1))))
add(C.Exit, Calc=expr())
open(os.path.join(evt2, "cond.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.Calc, Calc=expr(to("__done", B(True))))
add(C.Exit, Calc=expr())
open(os.path.join(evt2, "end.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())

# --- evt3/: @ParamStr/@ParamCount por marco (Call anidado, se restauran al volver) ---
evt3 = os.path.join(out, "evt3"); os.makedirs(evt3, exist_ok=True)
def pick(i): return OpeData(T.DimTo, "____1", operands=[V("@ParamStr"), I(i)])
def call(page, label, *ps):
    return add(C.Call, Page=C.LabelReference(page, label), Result="", Calc=lit(B(True)), Params=LiveParserArray([lit(S(x)) for x in ps], prefixed=True))
cmds = []; n = 0
call("p.lsb", 0, "abcd", "zz")
add(C.Exit, Calc=expr())
open(os.path.join(evt3, "ゲームメイン.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.If, Calc=expr(pick(0), op(T.Equal, "____arg", V("____1"), S("abcd"))))
call("p.lsb", 4, "inner")
add(C.Else)
add(C.Calc, indent=1, Calc=expr(to("first_ok", I(0))))
add(C.If, Calc=expr(pick(0), op(T.Equal, "____arg", V("____1"), S("inner"))))
add(C.Exit, indent=1, Calc=expr())
add(C.If, Calc=expr(pick(0), op(T.Equal, "____2", V("____1"), S("abcd")), op(T.Equal, "____3", V("@ParamCount"), I(2)), op(T.And, "____arg", V("____2"), V("____3"))))
add(C.Calc, indent=1, Calc=expr(to("restored_ok", I(1))))
add(C.Exit, Calc=expr())
open(os.path.join(evt3, "p.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())

# --- evt4/: While con End = índice tras el WhileLoop (como en juegos reales) y cadena
# If/Elseif dentro; ListCompo; SetProp con índice de propiedad numérico (base 0);
# IsDelimiter; Timer que dispara su manejador ---
evt4 = os.path.join(out, "evt4"); os.makedirs(evt4, exist_ok=True)
enable(C.CommandType.ImgNew, PT.PR_PARENT)
enable(C.CommandType.Timer, PT.PR_NAME, PT.PR_DELAY, PT.PR_ONPLAYEND)
def fn(name, f, *ops): return OpeData(T.Func, name, func=f, operands=list(ops))
cmds = []; n = 0
add(C.VarNew, Name="__tick", Type=P.Flag, InitVal=lit(B(False)), Scope=0)
add(C.BoxNew, components=comps(C.CommandType.BoxNew, {PT.PR_NAME: lit(S("par"))}))
add(C.ImgNew, components=comps(C.CommandType.ImgNew, {PT.PR_NAME: lit(S("c1")), PT.PR_PARENT: lit(S("par"))}))
add(C.ImgNew, components=comps(C.CommandType.ImgNew, {PT.PR_NAME: lit(S("c2")), PT.PR_PARENT: lit(S("par"))}))
add(C.Calc, Calc=expr(fn("____arg", F.ListCompo, V("ss"), I(0), S("par"))))
add(C.WhileInit, Calc=expr(to("i", I(0))))
w = add(C.While, Calc=expr(fn("____1", F.GetArraySize, V("ss")), op(T.Small, "____arg", V("i"), V("____1"))), End=0)
add(C.If, indent=1, Calc=expr(op(T.Equal, "____arg", V("i"), I(0))))
add(C.Calc, indent=2, Calc=expr(to("tmpy", I(0))))
add(C.Elseif, indent=1, Calc=expr(op(T.Equal, "____arg", V("i"), I(1))))
add(C.Calc, indent=2, Calc=expr(to("tmpy", I(5))))
add(C.SetProp, indent=1, ObjName=expr(OpeData(T.DimTo, "____arg", operands=[V("ss"), V("i")])), ObjProp=lit(I(3)), Value=expr(op(T.Plus, "____arg", V("i"), I(10))))
add(C.SetProp, indent=1, ObjName=expr(OpeData(T.DimTo, "____arg", operands=[V("ss"), V("i")])), ObjProp=lit(I(4)), Value=expr(to("____arg", V("tmpy"))))
wl = add(C.WhileLoop, Calc=expr(op(T.Plus, "____1", V("i"), I(1)), to("i", V("____1"))), Start=w)
cmds[w].args["End"] = wl + 1
add(C.Calc, Calc=expr(to("after", I(1))))
add(C.If, Calc=expr(fn("____arg", F.IsDelimiter, S(",;"), S("a;b"), I(2))))
add(C.Calc, indent=1, Calc=expr(to("delim", I(1))))
add(C.Timer, components=comps(C.CommandType.Timer, {PT.PR_NAME: lit(S("tm")), PT.PR_DELAY: lit(I(100)), PT.PR_ONPLAYEND: lit(S("tick.lsc"))}))
w = add(C.While, Calc=expr(op(T.Equal, "____arg", V("__tick"), I(0))), End=0)
add(C.Calc, indent=1, Calc=expr(to("spin", I(1))))
wl = add(C.WhileLoop, Calc=expr(), Start=w)
cmds[w].args["End"] = wl + 1
add(C.Exit, Calc=expr())
open(os.path.join(evt4, "ゲームメイン.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.Calc, Calc=expr(to("__tick", B(True))))
add(C.Exit, Calc=expr())
open(os.path.join(evt4, "tick.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())

# --- evt5/: bucle infinito con efectos repetidos (SetProp): --trace debe declararlo estancado ---
evt5 = os.path.join(out, "evt5"); os.makedirs(evt5, exist_ok=True)
cmds = []; n = 0
add(C.BoxNew, components=comps(C.CommandType.BoxNew, {PT.PR_NAME: lit(S("b"))}))
w = add(C.While, Calc=expr(op(T.Equal, "____arg", I(1), I(1))), End=0)
add(C.SetProp, indent=1, ObjName=lit(S("b")), ObjProp=lit(I(3)), Value=lit(I(1)))
wl = add(C.WhileLoop, Calc=expr(), Start=w)
cmds[w].args["End"] = wl + 1
add(C.Exit, Calc=expr())
open(os.path.join(evt5, "ゲームメイン.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())

# --- evt6/: Cinema (placeholder con PR_DURATION -> ONPLAYEND), PrevMenu elegido por --trace
# (respetando SetEnabled), InPrimary ---
evt6 = os.path.join(out, "evt6"); os.makedirs(evt6, exist_ok=True)
enable(C.CommandType.Cinema, PT.PR_NAME, PT.PR_SOURCE, PT.PR_DURATION, PT.PR_ONPLAYEND)
enable(C.CommandType.PrevMenuNew, PT.PR_NAME, PT.PR_ONSELECT)
def waitflag(v):
    w = add(C.While, Calc=expr(op(T.Equal, "____arg", V(v), I(0))), End=0)
    add(C.Calc, indent=1, Calc=expr(to("spin", I(1))))
    wl = add(C.WhileLoop, Calc=expr(), Start=w)
    cmds[w].args["End"] = wl + 1
cmds = []; n = 0
add(C.VarNew, Name="__mv", Type=P.Flag, InitVal=lit(B(False)), Scope=0)
add(C.VarNew, Name="__sel", Type=P.Flag, InitVal=lit(B(False)), Scope=0)
add(C.Cinema, components=comps(C.CommandType.Cinema, {PT.PR_NAME: lit(S("mv")), PT.PR_SOURCE: lit(S("op.wmv")), PT.PR_DURATION: lit(I(200)), PT.PR_ONPLAYEND: lit(S("mvend.lsc"))}))
waitflag("__mv")
add(C.PrevMenuNew, components=comps(C.CommandType.PrevMenuNew, {PT.PR_NAME: lit(S("title")), PT.PR_ONSELECT: lit(S("sel.lsc"))}))
add(C.Calc, Calc=expr(fn("____arg", F.SetEnabled, S("title"), I(0), B(False))))
add(C.If, Calc=expr(fn("____arg", F.InPrimary)))
add(C.Calc, indent=1, Calc=expr(to("primary", I(1))))
waitflag("__sel")
add(C.Exit, Calc=expr())
open(os.path.join(evt6, "ゲームメイン.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.Calc, Calc=expr(to("__mv", B(True))))
add(C.Exit, Calc=expr())
open(os.path.join(evt6, "mvend.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
cmds = []; n = 0
add(C.Calc, Calc=expr(OpeData(T.DimTo, "____1", operands=[V("@ParamStr"), I(0)]), to("chosen", V("____1"))))
add(C.Calc, Calc=expr(to("__sel", B(True))))
add(C.Exit, Calc=expr())
open(os.path.join(evt6, "sel.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())

# --- evt7/: bucle de escenas sin fin (a > b > a > ...): --trace debe informar del ciclo ---
evt7 = os.path.join(out, "evt7"); os.makedirs(evt7, exist_ok=True)
cmds = []; n = 0
add(C.Label, Name="top")
call("a.lsb", 0)
call("b.lsb", 0)
add(C.Jump, Page=C.LabelReference("ゲームメイン.lsb", 0), Calc=lit(B(True)))
open(os.path.join(evt7, "ゲームメイン.lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())
for nm in ("a", "b"):
    cmds = []; n = 0
    add(C.Wait, Calc=lit(B(True)), Time=lit(I(10)), StopEvent=expr())
    add(C.Exit, Calc=expr())
    open(os.path.join(evt7, nm + ".lsb"), "wb").write(LMScript(version=117, flags=0, command_params=cmd_params, commands=cmds).to_lsb())

# --- GAL (Gale106, zlib, 24bpp, 4x3, alpha) ---
def gal(w, h, bpp, pixels, alpha=None, block=(0, 0)):
    stride = ((w * bpp + 7) // 8 + 3) & ~3
    astride = (w + 3) & ~3
    hdr = bytearray(0x24)
    struct.pack_into("<IIIii", hdr, 0, 0, w, h, bpp, 1)
    hdr[0x15] = 0; hdr[0x16] = 0
    struct.pack_into("<Iii", hdr, 0x18, 0, block[0], block[1])
    b = bytearray(b"Gale106") + struct.pack("<i", len(hdr)) + hdr
    b += struct.pack("<I", 0)  # frame name
    b += struct.pack("<I", 0) + b"\0" * 9
    b += struct.pack("<Iiii", 1, w, h, bpp)
    b += struct.pack("<ii", 0, 0) + bytes([1]) + struct.pack("<ii", -1, 255) + bytes([1 if alpha else 0]) + struct.pack("<I", 0)
    rows = b"".join(pixels[y].ljust(stride, b"\0") for y in range(h))
    z = zlib.compress(rows)
    b += struct.pack("<i", len(z)) + z
    if alpha:
        a = zlib.compress(b"".join(alpha[y].ljust(astride, b"\0") for y in range(h)))
        b += struct.pack("<i", len(a)) + a
    else:
        b += struct.pack("<i", 0)
    return bytes(b)
px = [bytes([0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255]) for _ in range(3)]  # BGR: red, green, blue, white
al = [bytes([255, 128, 0, 255]) for _ in range(3)]
open(os.path.join(out, "bg.gal"), "wb").write(gal(4, 3, 24, px, al))

# WAV: 0.1 s silence
import wave
with wave.open(os.path.join(out, "se.wav"), "wb") as wf:
    wf.setnchannels(1); wf.setsampwidth(2); wf.setframerate(22050); wf.writeframes(b"\0\0" * 2205)

# --- archives: standalone .dat and fake exe (MZ stub + archive + trailer) ---
files = ["00000001.lsb", "sub.lsb", "ゲームメイン.lsb", "bg.gal", "se.wav"]
fake_exe = os.path.join(out, "stub.bin")
open(fake_exe, "wb").write(b"MZ" + b"\0" * 62)
with LMArchive(os.path.join(out, "game.exe"), "w", exe=fake_exe) as ar:
    for i, f in enumerate(files):
        ar.write(os.path.join(out, f), arcname=f, compress_type=LMCompressType.ZLIB if i % 2 == 0 else LMCompressType.NONE)
with LMArchive(os.path.join(out, "game.dat"), "w") as ar:
    for f in files:
        ar.write(os.path.join(out, f), arcname=f)
os.remove(fake_exe)
print("ok")

# --- GaleX 8bpp con paleta y bloques 2x2 (incluye referencia -2), esperado vía pylivemaker/PIL ---
def galex_pal_blocks():
    w, h, bw, bh = 5, 3, 2, 2
    pal = [(i * 40 % 256, i * 80 % 256, i * 20 % 256) for i in range(256)]  # (b,g,r)
    stride = (w + 3) & ~3
    blocks_w = (w + bw - 1) // bw
    blocks_h = (h + bh - 1) // bh
    refs, payload = [], b""
    pix = [[(x + y * 3) % 7 for x in range(w)] for y in range(h)]
    k = 0
    for by in range(blocks_h):
        for bx in range(blocks_w):
            if k == 3:  # copia del bloque 0
                refs.append((-2, 0))
                for yy in range(by * bh, min(h, by * bh + bh)):
                    for xx in range(bx * bw, min(w, bx * bw + bw)):
                        pix[yy][xx] = pix[yy - by * bh][xx - bx * bw]
            else:
                refs.append((-1, 0))
                for yy in range(by * bh, min(h, by * bh + bh)):
                    payload += bytes(pix[yy][bx * bw:min(w, bx * bw + bw)])
            k += 1
    packed = b"".join(struct.pack("<ii", a, b) for a, b in refs) + payload
    hexpal = "".join("%02x%02x%02x" % p for p in pal)
    xml = (f'<?xml version="1.0" encoding="Shift_JIS"?><Frames Version="200" Width="{w}" Height="{h}" Bpp="8" Count="1" '
           f'CompType="0" CompLevel="9" Randomized="0" BGColor="0" BlockWidth="{bw}" BlockHeight="{bh}">'
           f'<Frame Name="f0"><Layers Width="{w}" Height="{h}" Bpp="8"><RGB>{hexpal}</RGB>'
           f'<Layer Left="0" Top="0" Visible="1" Alpha="255" AlphaOn="0"/></Layers></Frame></Frames>').encode("cp932")
    zx = zlib.compress(xml)
    z = zlib.compress(packed)
    return b"GaleX200" + struct.pack("<i", len(zx)) + zx + struct.pack("<i", len(z)) + z

from PIL import Image
from livemaker import GalImagePlugin  # noqa: registra el formato
d = galex_pal_blocks()
open(os.path.join(out, "pal_blocks.galx"), "wb").write(d)
im = Image.open(os.path.join(out, "pal_blocks.galx")); im.load()
open(os.path.join(out, "pal_blocks.rgba"), "wb").write(im.convert("RGBA").tobytes())
im = Image.open(os.path.join(out, "bg.gal")); im.load()
print("bg.gal via pylivemaker:", im.mode, im.size, list(im.convert("RGBA").getdata())[:4])

# --- datos cifrados (LiveMaker Pro scramble), esperado vía pylivemaker.scramble.decrypt ---
from livemaker.scramble import LMScramble, decrypt, LIVEMAKER3_SCRAMBLE_KEY
plain = bytes(range(256)) * 3
chunk, seed = 100, 0x1234
chunks = [plain[i:i + chunk] for i in range(0, len(plain), chunk)]
seq = LMScramble.randseq(len(chunks), seed ^ LIVEMAKER3_SCRAMBLE_KEY)
slots = [None] * len(chunks)
for k, i in enumerate(seq):
    slots[i] = chunks[k]
# el último trozo (corto) debe quedar al final del flujo para que los offsets cuadren
enc = struct.pack("<iI", chunk, seed) + b"".join(slots)
ok = decrypt(enc) == plain
if ok:
    open(os.path.join(out, "scrambled.bin"), "wb").write(enc)
    open(os.path.join(out, "scrambled.plain"), "wb").write(plain)
print("scramble fixture:", ok)
