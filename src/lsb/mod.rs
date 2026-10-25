//! Parser de scripts compilados LSB (port de pylivemaker `livemaker/lsb/`).

pub mod expr;
pub mod names;
pub mod tpword;

use crate::util::cp932;
pub use names::{CmdType, Prop, cmd_name, func_name, prop_name};
pub use tpword::{Glyph, TpWord};

pub const MIN_LSB_VERSION: u32 = 103;
pub const MAX_LSB_VERSION: u32 = 117;

#[derive(Debug)]
pub struct ParseError(pub String);
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ParseError {}
pub type Result<T> = std::result::Result<T, ParseError>;

pub struct Reader<'a> {
    pub d: &'a [u8],
    pub p: usize,
}
impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Self { d, p: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.p.checked_add(n).is_none_or(|e| e > self.d.len()) {
            return Err(ParseError(format!("unexpected EOF at {} (+{n})", self.p)));
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(cp932(self.take(n)?))
    }
    pub fn count(&mut self) -> Result<usize> {
        let n = self.u32()? as usize;
        if n > self.d.len() - self.p.min(self.d.len()) + 1 {
            // cada elemento ocupa al menos un byte: protege contra basura
            return Err(ParseError(format!("implausible count {n} at {}", self.p)));
        }
        Ok(n)
    }
}

/// Delphi `Extended` (x87 80 bits, little endian) -> f64.
pub fn extended_to_f64(b: &[u8]) -> f64 {
    let mant = u64::from_le_bytes(b[0..8].try_into().unwrap());
    let se = u16::from_le_bytes([b[8], b[9]]);
    let sign = if se & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = (se & 0x7FFF) as i32;
    if exp == 0 && mant == 0 {
        return 0.0 * sign;
    }
    if exp == 0x7FFF {
        return if mant << 1 == 0 {
            sign * f64::INFINITY
        } else {
            f64::NAN
        };
    }
    sign * (mant as f64 / (1u64 << 63) as f64) * 2f64.powi(exp - 16383)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Var(String),
    Int(i32),
    Float(f64),
    Flag(bool),
    Str(String),
}

impl Param {
    fn parse(r: &mut Reader) -> Result<Self> {
        let t = r.u8()?;
        Ok(match t {
            1 => Param::Int(r.i32()?),
            2 => Param::Float(extended_to_f64(r.take(10)?)),
            3 => Param::Flag(r.u8()? != 0),
            4 => Param::Str(r.string()?),
            _ => Param::Var(r.string()?),
        })
    }
}

#[derive(Debug, Clone)]
pub struct OpeData {
    pub op: u8,
    pub name: String,
    pub func: Option<u8>,
    pub operands: Vec<Param>,
}

pub mod op {
    pub const NONE: u8 = 0x00;
    pub const TO: u8 = 0x01;
    pub const PLUS: u8 = 0x02;
    pub const MINUS: u8 = 0x03;
    pub const MUL: u8 = 0x04;
    pub const DIV: u8 = 0x05;
    pub const MOD: u8 = 0x06;
    pub const OR: u8 = 0x07;
    pub const AND: u8 = 0x08;
    pub const XOR: u8 = 0x09;
    pub const DIMTO: u8 = 0x0A;
    pub const FUNC: u8 = 0x0B;
    pub const EQUAL: u8 = 0x0C;
    pub const BIG: u8 = 0x0D;
    pub const SMALL: u8 = 0x0E;
    pub const EBIG: u8 = 0x0F;
    pub const ESMALL: u8 = 0x10;
    pub const SHIFTL: u8 = 0x11;
    pub const SHIFTR: u8 = 0x12;
    pub const COMBOSTR: u8 = 0x13;
    pub const NEQUAL: u8 = 0x14;
}

/// Expresión compilada (`TLiveParser`): lista de operaciones de tres direcciones.
#[derive(Debug, Clone, Default)]
pub struct LiveParser {
    pub entries: Vec<OpeData>,
}

impl LiveParser {
    pub fn parse(r: &mut Reader) -> Result<Self> {
        let n = r.count()?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let op = r.u8()?;
            let name = r.string()?;
            let count = r.count()?;
            let func = if op == op::FUNC { Some(r.u8()?) } else { None };
            let mut operands = Vec::with_capacity(count);
            for _ in 0..count {
                operands.push(Param::parse(r)?);
            }
            entries.push(OpeData {
                op,
                name,
                func,
                operands,
            });
        }
        Ok(Self { entries })
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Si la expresión es un literal simple, lo devuelve.
    pub fn literal(&self) -> Option<&Param> {
        match self.entries.as_slice() {
            [e] if e.op == op::TO => e.operands.last().filter(|p| !matches!(p, Param::Var(_))),
            _ => None,
        }
    }
    /// Todos los literales de cadena (para prefetch de recursos).
    pub fn strings(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .flat_map(|e| e.operands.iter())
            .filter_map(|p| match p {
                Param::Str(s) => Some(s.as_str()),
                _ => None,
            })
    }
    /// Representación legible (sin literales de texto si `redact`).
    pub fn render(&self, redact: bool) -> String {
        use std::collections::HashMap;
        let mut m: HashMap<&str, String> = HashMap::new();
        let p = |m: &HashMap<&str, String>, x: &Param| -> String {
            match x {
                Param::Var(v) if v.starts_with("____") => {
                    m.get(v.as_str()).cloned().unwrap_or_else(|| v.clone())
                }
                Param::Var(v) => v.clone(),
                Param::Int(i) => i.to_string(),
                Param::Float(f) => f.to_string(),
                Param::Flag(b) => (*b as i32).to_string(),
                Param::Str(s) if redact => format!("\"<{} chars>\"", s.chars().count()),
                Param::Str(s) => format!("{s:?}"),
            }
        };
        let mut last = String::new();
        for e in &self.entries {
            let a: Vec<String> = e.operands.iter().map(|x| p(&m, x)).collect();
            let bin = |s: &str| {
                format!(
                    "({} {s} {})",
                    a.first().cloned().unwrap_or_default(),
                    a.get(1).cloned().unwrap_or_default()
                )
            };
            let s = match e.op {
                op::TO => a.last().cloned().unwrap_or_default(),
                op::PLUS => bin("+"),
                op::MINUS => bin("-"),
                op::MUL => bin("*"),
                op::DIV => bin("/"),
                op::MOD => bin("%"),
                op::OR => bin("|"),
                op::AND => bin("&"),
                op::XOR => bin("^"),
                op::EQUAL => bin("=="),
                op::BIG => bin(">"),
                op::SMALL => bin("<"),
                op::EBIG => bin(">="),
                op::ESMALL => bin("<="),
                op::SHIFTL => bin("<<"),
                op::SHIFTR => bin(">>"),
                op::COMBOSTR => bin("++"),
                op::NEQUAL => bin("!="),
                op::DIMTO => {
                    let mut s = a.first().cloned().unwrap_or_default();
                    for i in a.iter().skip(1) {
                        s += &format!("[{i}]");
                    }
                    s
                }
                op::FUNC => format!("{}({})", func_name(e.func.unwrap_or(255)), a.join(", ")),
                _ => format!("?op{}", e.op),
            };
            last = if e.name.starts_with("____") || e.op != op::TO {
                if e.name == "____arg" || e.name.starts_with("____") {
                    s.clone()
                } else {
                    format!("{} = {s}", e.name)
                }
            } else {
                format!("{} = {s}", e.name)
            };
            m.insert(e.name.as_str(), s);
        }
        last
    }
}

#[derive(Debug, Clone, Default)]
pub struct LabelRef {
    pub page: String,
    pub label: u32,
}

#[derive(Debug, Clone)]
pub enum Field {
    Expr(LiveParser),
    Exprs(Vec<LiveParser>),
    Str(String),
    U32(u32),
    Byte(u8),
    Page(LabelRef),
    Text(Box<TpWord>),
    /// (id de propiedad PropertyType, expresión)
    Props(Vec<(u8, LiveParser)>),
}

#[derive(Debug, Clone)]
pub struct Command {
    pub kind: u8,
    pub indent: u32,
    pub mute: bool,
    pub not_update: bool,
    pub line_no: u32,
    pub fields: Vec<(&'static str, Field)>,
}

impl Command {
    pub fn name(&self) -> &'static str {
        cmd_name(self.kind)
    }
    pub fn field(&self, k: &str) -> Option<&Field> {
        self.fields.iter().find(|(n, _)| *n == k).map(|(_, f)| f)
    }
    pub fn expr(&self, k: &str) -> Option<&LiveParser> {
        match self.field(k) {
            Some(Field::Expr(e)) => Some(e),
            _ => None,
        }
    }
    pub fn exprs(&self, k: &str) -> &[LiveParser] {
        match self.field(k) {
            Some(Field::Exprs(e)) => e,
            _ => &[],
        }
    }
    pub fn str(&self, k: &str) -> Option<&str> {
        match self.field(k) {
            Some(Field::Str(s)) => Some(s),
            _ => None,
        }
    }
    pub fn u32(&self, k: &str) -> Option<u32> {
        match self.field(k) {
            Some(Field::U32(v)) => Some(*v),
            Some(Field::Byte(v)) => Some(*v as u32),
            _ => None,
        }
    }
    pub fn page(&self) -> Option<&LabelRef> {
        match self.field("Page") {
            Some(Field::Page(p)) => Some(p),
            _ => None,
        }
    }
    pub fn text(&self) -> Option<&TpWord> {
        match self.field("Text") {
            Some(Field::Text(t)) => Some(t),
            _ => None,
        }
    }
    pub fn props(&self) -> &[(u8, LiveParser)] {
        match self.field("components") {
            Some(Field::Props(p)) => p,
            _ => &[],
        }
    }
    pub fn prop(&self, id: u8) -> Option<&LiveParser> {
        self.props().iter().find(|(p, _)| *p == id).map(|(_, e)| e)
    }
    /// Todas las cadenas literales referenciadas (para prefetch).
    pub fn strings(&self) -> Vec<&str> {
        let mut v = Vec::new();
        for (_, f) in &self.fields {
            match f {
                Field::Expr(e) => v.extend(e.strings()),
                Field::Exprs(es) => es.iter().for_each(|e| v.extend(e.strings())),
                Field::Props(ps) => ps.iter().for_each(|(_, e)| v.extend(e.strings())),
                Field::Text(t) => v.extend(t.images()),
                _ => {}
            }
        }
        v
    }
}

#[derive(Clone, Copy)]
enum F {
    E,
    EIf(u32),
    EA,
    EA2,
    S,
    U,
    UIf(u32),
    B,
    P,
    T,
    C,
}

fn spec(kind: u8) -> &'static [(&'static str, F)] {
    use CmdType as T;
    use F::*;
    const STOP: (&str, F) = ("StopEvent", EIf(0x6A));
    match CmdType::from(kind) {
        T::If | T::Elseif | T::Exit | T::Calc | T::WhileInit => &[("Calc", E)],
        T::Label | T::Comment => &[("Name", S)],
        T::Jump => &[("Page", P), ("Calc", E)],
        T::Call => &[("Page", P), ("Result", S), ("Calc", E), ("Params", EA)],
        T::Wait => &[("Calc", E), ("Time", E), STOP],
        T::Flip => &[
            ("Wipe", E),
            ("Time", E),
            ("Reverse", E),
            ("Act", E),
            ("Targets", EA),
            ("Delete", E),
            ("Param", EA2),
            ("Source", EIf(0x64)),
            STOP,
            ("DifferenceOnly", EIf(0x74)),
        ],
        T::VarNew => &[("Name", S), ("Type", B), ("InitVal", E), ("Scope", B)],
        T::VarDel => &[("Name", S)],
        T::GetProp => &[("ObjName", E), ("ObjProp", E), ("VarName", S)],
        T::SetProp => &[("ObjName", E), ("ObjProp", E), ("Value", E)],
        T::ObjDel => &[("Name", E)],
        T::TextIns => &[("Text", T), ("Target", E), ("Hist", E), ("Wait", E), STOP],
        T::MovieStop => &[("Target", E), ("Time", E), ("Wait", E), STOP],
        T::MenuClose | T::TextClr | T::MediaPlay => &[("Target", E)],
        T::CallHist => &[
            ("Target", E),
            ("Index", E),
            ("Count", E),
            ("CutBreak", E),
            ("FormatName", EIf(0x6E)),
        ],
        T::While | T::Break => &[("Calc", E), ("End", U)],
        T::WhileLoop | T::Continue => &[("Calc", E), ("Start", U)],
        T::GameSave => &[("No", E), ("Page", S), ("Label", UIf(0x68)), ("Caption", E)],
        T::GameLoad => &[("No", E)],
        T::PCReset | T::Reset => &[("Page", P), ("AllClear", B)],
        T::PropMotion => &[
            ("Name", E),
            ("ObjName", E),
            ("ObjProp", E),
            ("Value", E),
            ("Time", E),
            ("MoveType", E),
            ("Paused", EIf(0x6B)),
        ],
        T::FormatHist => &[("Name", E), ("Target", EIf(0x6E))],
        T::SaveCabinet | T::LoadCabinet => &[("components", C), ("Act", E), ("Targets", EA)],
        T::Else
        | T::ClrHist
        | T::Terminate
        | T::DoEvent
        | T::ClrRead
        | T::IFDEF
        | T::IFNDEF
        | T::ENDIF => &[],
        // resto: comandos de componentes
        _ => &[("components", C)],
    }
}

/// Script LSB parseado.
#[derive(Debug, Clone)]
pub struct Lsb {
    pub name: String,
    pub version: u32,
    pub flags: u8,
    /// Por tipo de comando: lista de PropertyType habilitados (orden de componentes).
    pub command_params: Vec<Vec<u8>>,
    pub commands: Vec<Command>,
}

impl Lsb {
    pub fn parse(name: &str, data: &[u8]) -> Result<Self> {
        if data.starts_with(b"LiveMaker") || data.starts_with(b"<?xml") {
            return Err(ParseError(
                "LSC/XML text scripts are not supported (expected binary LSB)".into(),
            ));
        }
        let mut r = Reader::new(data);
        let version = r.u32()?;
        if !(MIN_LSB_VERSION..=MAX_LSB_VERSION).contains(&version) {
            return Err(ParseError(format!("unsupported LSB version {version}")));
        }
        let flags = r.u8()?;
        let ccount = r.count()?;
        let psize = r.u32()? as usize;
        let mut command_params = Vec::with_capacity(ccount);
        for _ in 0..ccount {
            let bytes = r.take(psize)?;
            let mut v = Vec::new();
            for (i, b) in bytes.iter().enumerate() {
                for bit in 0..8 {
                    if b & (1 << bit) != 0 {
                        v.push((i * 8 + bit + 1) as u8); // bit j -> PropertyType(j+1)
                    }
                }
            }
            command_params.push(v);
        }
        let n = r.count()?;
        let mut commands = Vec::with_capacity(n);
        for i in 0..n {
            let c = parse_command(&mut r, version, &command_params)
                .map_err(|e| ParseError(format!("{name}: command #{i}: {}", e.0)))?;
            commands.push(c);
        }
        Ok(Self {
            name: name.to_string(),
            version,
            flags,
            command_params,
            commands,
        })
    }

    /// Índice del comando con un LineNo dado.
    pub fn index_of_line(&self, line: u32) -> Option<usize> {
        self.commands.iter().position(|c| c.line_no == line)
    }
}

fn parse_command(r: &mut Reader, version: u32, cparams: &[Vec<u8>]) -> Result<Command> {
    let kind = r.u8()?;
    if kind > 0x3F {
        return Err(ParseError(format!("unknown command type 0x{kind:02x}")));
    }
    let indent = r.u32()?;
    let mute = r.u8()? != 0;
    let not_update = r.u8()? != 0;
    let line_no = r.u32()?;
    let mut fields = Vec::new();
    for &(name, f) in spec(kind) {
        let v = match f {
            F::E => Field::Expr(LiveParser::parse(r)?),
            F::EIf(min) => {
                if version > min {
                    Field::Expr(LiveParser::parse(r)?)
                } else {
                    continue;
                }
            }
            F::EA => {
                let n = r.count()?;
                Field::Exprs(
                    (0..n)
                        .map(|_| LiveParser::parse(r))
                        .collect::<Result<_>>()?,
                )
            }
            F::EA2 => Field::Exprs(
                (0..2)
                    .map(|_| LiveParser::parse(r))
                    .collect::<Result<_>>()?,
            ),
            F::S => Field::Str(r.string()?),
            F::U => Field::U32(r.u32()?),
            F::UIf(min) => {
                if version > min {
                    Field::U32(r.u32()?)
                } else {
                    continue;
                }
            }
            F::B => Field::Byte(r.u8()?),
            F::P => {
                let page = r.string()?;
                let label = r.u32()?;
                Field::Page(LabelRef { page, label })
            }
            F::T => {
                let len = r.u32()? as usize;
                let sub = r.take(len)?;
                Field::Text(Box::new(TpWord::parse(sub)?))
            }
            F::C => {
                let enabled = cparams
                    .get(kind as usize)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let mut v = Vec::with_capacity(enabled.len());
                for &p in enabled {
                    v.push((p, LiveParser::parse(r)?));
                }
                Field::Props(v)
            }
        };
        fields.push((name, v));
    }
    Ok(Command {
        kind,
        indent,
        mute,
        not_update,
        line_no,
        fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fx(n: &str) -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(n),
        )
        .unwrap()
    }
    #[test]
    fn parse_main() {
        let l = Lsb::parse("00000001.lsb", &fx("00000001.lsb")).unwrap();
        assert_eq!(l.version, 117);
        let names: Vec<_> = l.commands.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            [
                "VarNew",
                "VarNew",
                "Label",
                "Calc",
                "If",
                "Jump",
                "Else",
                "Calc",
                "ImgNew",
                "MesNew",
                "Sound",
                "Call",
                "TextIns",
                "Wait",
                "WhileInit",
                "While",
                "Calc",
                "WhileLoop",
                "ObjDel",
                "Exit"
            ]
        );
        let img = &l.commands[8];
        assert_eq!(
            img.prop(Prop::SOURCE).and_then(|e| e.literal()),
            Some(&Param::Str("bg.gal".into()))
        );
        assert_eq!(
            l.commands[3].expr("Calc").unwrap().render(false),
            "count = (count + 1)"
        );
        let t = l.commands[12].text().unwrap();
        assert_eq!(t.version, 105);
        assert_eq!(t.plain_text(), "こんにちは、世界\nテスト\n");
        assert_eq!(l.commands[15].u32("End"), Some(17));
        assert_eq!(l.commands[5].page().unwrap().label, 2);
        assert_eq!(l.commands[5].indent, 1);
    }
    #[test]
    fn extended() {
        // 1.5 en x87 extended
        let mut b = [0u8; 10];
        b[0..8].copy_from_slice(&0xC000_0000_0000_0000u64.to_le_bytes());
        b[8..10].copy_from_slice(&16383u16.to_le_bytes());
        assert_eq!(extended_to_f64(&b), 1.5);
    }
}
