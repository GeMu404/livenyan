//! Bloques de texto enriquecido `TpWord` (port de pylivemaker `lsb/novel.py`).

use super::{LiveParser, ParseError, Reader, Result};
use crate::util::cp932;

#[derive(Debug, Clone)]
pub struct Decorator {
    pub ruby: String,
    pub raw: [u32; 4],
}

#[derive(Debug, Clone)]
pub enum Glyph {
    Char { ch: String, decorator: i32, link: i32, speed: u32, condition: i32 },
    /// Cambio de alineación/div.
    Div { align: u8 },
    /// 0=línea 1=página 2=pausa 3=borrar
    Return { kind: u8 },
    Indent,
    Undent,
    Event { event: String },
    Var { name: String, hist: bool },
    Img { src: String, align: u8 },
    Unknown(u8),
}

pub mod brk {
    pub const LINE: u8 = 0;
    pub const PAGE: u8 = 1;
    pub const PAUSE: u8 = 2;
    pub const CLEAR: u8 = 3;
}

#[derive(Debug, Clone, Default)]
pub struct TpWord {
    pub version: u32,
    pub decorators: Vec<Decorator>,
    pub conditions: Vec<String>,
    pub links: Vec<String>,
    pub body: Vec<Glyph>,
}

impl TpWord {
    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Reader::new(d);
        if r.take(6)? != b"TpWord" {
            return Err(ParseError("missing TpWord signature".into()));
        }
        let vs = r.take(3)?;
        let v: u32 = std::str::from_utf8(vs)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| ParseError("bad TpWord version".into()))?;
        let mut tw = TpWord { version: v, ..Default::default() };
        let n = r.count()?;
        for _ in 0..n {
            let _count = r.u32()?;
            let a = r.u32()?;
            let b = r.u32()?;
            let c = r.u32()?;
            let _u5 = r.u8()?;
            let _u6 = r.u8()?;
            let u7 = if v < 100 { r.u8()? as u32 } else { r.u32()? };
            let _u8 = r.string()?;
            let ruby = r.string()?;
            if v >= 100 {
                r.u32()?;
                r.u32()?;
            }
            tw.decorators.push(Decorator { ruby, raw: [a, b, c, u7] });
        }
        if v >= 104 {
            let n = r.count()?;
            for _ in 0..n {
                r.u32()?;
                tw.conditions.push(r.string()?);
            }
        }
        if v >= 105 {
            let n = r.count()?;
            for _ in 0..n {
                r.u32()?;
                tw.links.push(r.string()?);
                r.string()?;
            }
        }
        let n = r.count()?;
        for _ in 0..n {
            tw.body.push(parse_glyph(&mut r, v)?);
        }
        Ok(tw)
    }

    /// Texto plano (solo para --show-text / pruebas).
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for g in &self.body {
            match g {
                Glyph::Char { ch, .. } => s.push_str(ch),
                Glyph::Return { .. } => s.push('\n'),
                _ => {}
            }
        }
        s
    }

    pub fn char_count(&self) -> usize {
        self.body.iter().filter(|g| matches!(g, Glyph::Char { .. })).count()
    }

    pub fn images(&self) -> impl Iterator<Item = &str> {
        self.body.iter().filter_map(|g| match g {
            Glyph::Img { src, .. } => Some(src.as_str()),
            _ => None,
        })
    }
}

fn parse_glyph(r: &mut Reader, v: u32) -> Result<Glyph> {
    let t = r.u8()?;
    if !matches!(t, 1..=7 | 9 | 10) {
        // pylivemaker: Select(..., Byte) consume solo el byte de tipo
        return Ok(Glyph::Unknown(t));
    }
    let condition = if v >= 104 { r.i32()? } else { -1 };
    let real = |r: &mut Reader| -> Result<(i32, u32)> {
        let link = if v < 105 {
            r.string()?;
            -1
        } else {
            r.i32()?
        };
        Ok((link, r.u32()?))
    };
    Ok(match t {
        1 => {
            let (link, speed) = real(r)?;
            let raw = r.u16()?;
            let ch = if raw >> 8 == 0 { cp932(&[raw as u8]) } else { cp932(&[(raw >> 8) as u8, raw as u8]) };
            let decorator = r.i32()?;
            Glyph::Char { ch, decorator, link, speed, condition }
        }
        2 => {
            let align = r.u8()?;
            if v >= 105 {
                r.i32()?;
                r.i32()?;
                r.u8()?;
            }
            Glyph::Div { align }
        }
        3 => Glyph::Return { kind: r.u8()? },
        4 => Glyph::Indent,
        5 => Glyph::Undent,
        6 => Glyph::Event { event: r.string()? },
        7 | 10 => {
            r.i32()?;
            if v > 100 {
                r.u32()?;
            }
            if v > 100 && v < 105 {
                r.string()?;
            }
            if v >= 105 {
                r.i32()?;
            }
            let name = if v < 102 { LiveParser::parse(r)?.render(false) } else { r.string()? };
            Glyph::Var { name, hist: t == 10 }
        }
        9 => {
            real(r)?;
            let src = r.string()?;
            let align = r.u8()?;
            if v >= 103 {
                r.string()?;
            }
            if v >= 105 {
                for _ in 0..4 {
                    r.i32()?;
                }
                r.string()?;
            }
            Glyph::Img { src, align }
        }
        _ => unreachable!(),
    })
}
