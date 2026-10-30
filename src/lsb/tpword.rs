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
    Char {
        ch: String,
        decorator: i32,
        link: i32,
        speed: u32,
        condition: i32,
    },
    /// Cambio de alineación/div.
    Div {
        align: u8,
    },
    /// 0=línea 1=página 2=pausa 3=borrar
    Return {
        kind: u8,
    },
    Indent,
    Undent,
    Event {
        event: String,
    },
    Var {
        name: String,
        hist: bool,
    },
    Img {
        src: String,
        align: u8,
    },
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
        let mut tw = TpWord {
            version: v,
            ..Default::default()
        };
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
            tw.decorators.push(Decorator {
                ruby,
                raw: [a, b, c, u7],
            });
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

    pub fn translation_template(&self) -> String {
        let mut text = String::new();
        let mut token_index = 0;
        for glyph in &self.body {
            match glyph {
                Glyph::Char { ch, .. } => text.push_str(ch),
                Glyph::Return { .. } => text.push('\n'),
                glyph => {
                    text.push_str(&translation_token(glyph, token_index));
                    token_index += 1;
                }
            }
        }
        text
    }

    pub fn replace_translation_text(&mut self, translated: &str) -> bool {
        let mut tokens = std::collections::HashMap::new();
        let mut token_index = 0;
        for glyph in &self.body {
            if !matches!(glyph, Glyph::Char { .. } | Glyph::Return { .. }) {
                tokens.insert(translation_token(glyph, token_index), glyph.clone());
                token_index += 1;
            }
        }

        let style = self.body.iter().find_map(|glyph| match glyph {
            Glyph::Char {
                decorator,
                link,
                speed,
                condition,
                ..
            } => Some((*decorator, *link, *speed, *condition)),
            _ => None,
        });
        let mut seen = std::collections::HashSet::new();
        let mut breaks = self.body.iter().filter_map(|glyph| match glyph {
            Glyph::Return { kind } => Some(*kind),
            _ => None,
        });
        let mut body = Vec::new();
        let mut position = 0;
        while position < translated.len() {
            if translated[position..].starts_with("{{LV:") {
                let Some(end) = translated[position..].find("}}") else {
                    return false;
                };
                let end = position + end + 2;
                let token = &translated[position..end];
                let Some(glyph) = tokens.get(token) else {
                    return false;
                };
                if !seen.insert(token) {
                    return false;
                }
                body.push(glyph.clone());
                position = end;
                continue;
            }

            let Some(ch) = translated[position..].chars().next() else {
                break;
            };
            position += ch.len_utf8();
            if ch == '\n' {
                body.push(Glyph::Return {
                    kind: breaks.next().unwrap_or(brk::LINE),
                });
            } else {
                let (decorator, link, speed, condition) = style.unwrap_or((0, -1, 0, -1));
                body.push(Glyph::Char {
                    ch: ch.to_string(),
                    decorator,
                    link,
                    speed,
                    condition,
                });
            }
        }

        if seen.len() != tokens.len() {
            return false;
        }
        self.body = body;
        true
    }

    pub fn replace_plain_text(&mut self, translated: &str) -> bool {
        if self.body.is_empty()
            || self
                .body
                .iter()
                .any(|glyph| !matches!(glyph, Glyph::Char { .. } | Glyph::Return { .. }))
        {
            return false;
        }
        let Some((decorator, link, speed, condition)) =
            self.body.iter().find_map(|glyph| match glyph {
                Glyph::Char {
                    decorator,
                    link,
                    speed,
                    condition,
                    ..
                } => Some((*decorator, *link, *speed, *condition)),
                _ => None,
            })
        else {
            return false;
        };
        let mut breaks = self.body.iter().filter_map(|glyph| match glyph {
            Glyph::Return { kind } => Some(*kind),
            _ => None,
        });
        let mut body = Vec::new();
        for ch in translated.chars() {
            if ch == '\n' {
                body.push(Glyph::Return {
                    kind: breaks.next().unwrap_or(brk::LINE),
                });
            } else {
                body.push(Glyph::Char {
                    ch: ch.to_string(),
                    decorator,
                    link,
                    speed,
                    condition,
                });
            }
        }
        body.extend(breaks.map(|kind| Glyph::Return { kind }));
        self.body = body;
        true
    }

    pub fn char_count(&self) -> usize {
        self.body
            .iter()
            .filter(|g| matches!(g, Glyph::Char { .. }))
            .count()
    }

    pub fn images(&self) -> impl Iterator<Item = &str> {
        self.body.iter().filter_map(|g| match g {
            Glyph::Img { src, .. } => Some(src.as_str()),
            _ => None,
        })
    }
}

fn translation_token(glyph: &Glyph, index: usize) -> String {
    let kind = match glyph {
        Glyph::Div { .. } => "DIV",
        Glyph::Indent => "INDENT",
        Glyph::Undent => "UNDENT",
        Glyph::Event { .. } => "EVENT",
        Glyph::Var { .. } => "VAR",
        Glyph::Img { .. } => "IMG",
        Glyph::Unknown(_) => "UNKNOWN",
        Glyph::Char { .. } | Glyph::Return { .. } => unreachable!(),
    };
    format!("{{{{LV:{kind}:{index}}}}}")
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
            let ch = if raw >> 8 == 0 {
                cp932(&[raw as u8])
            } else {
                cp932(&[(raw >> 8) as u8, raw as u8])
            };
            let decorator = r.i32()?;
            Glyph::Char {
                ch,
                decorator,
                link,
                speed,
                condition,
            }
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
            let name = if v < 102 {
                LiveParser::parse(r)?.render(false)
            } else {
                r.string()?
            };
            Glyph::Var {
                name,
                hist: t == 10,
            }
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

#[cfg(test)]
mod tests {
    use super::{Glyph, TpWord, brk};

    #[test]
    fn replaces_static_text_and_preserves_non_text_markup() {
        let mut text = TpWord {
            body: vec![
                Glyph::Char {
                    ch: "A".into(),
                    decorator: 2,
                    link: -1,
                    speed: 30,
                    condition: -1,
                },
                Glyph::Return { kind: brk::LINE },
                Glyph::Char {
                    ch: "B".into(),
                    decorator: 2,
                    link: -1,
                    speed: 30,
                    condition: -1,
                },
            ],
            ..TpWord::default()
        };

        assert!(text.replace_plain_text("Hola\nmundo"));
        assert_eq!(text.plain_text(), "Hola\nmundo");
        assert!(matches!(
            text.body.first(),
            Some(Glyph::Char {
                decorator: 2,
                speed: 30,
                ..
            })
        ));
    }

    #[test]
    fn does_not_replace_text_containing_runtime_variables() {
        let mut text = TpWord {
            body: vec![
                Glyph::Char {
                    ch: "A".into(),
                    decorator: 0,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Var {
                    name: "player".into(),
                    hist: false,
                },
            ],
            ..TpWord::default()
        };

        assert!(!text.replace_plain_text("translated"));
        assert_eq!(text.plain_text(), "A");
    }

    #[test]
    fn translation_template_preserves_runtime_variables_and_events() {
        let mut text = TpWord {
            body: vec![
                Glyph::Char {
                    ch: "A".into(),
                    decorator: 0,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Var {
                    name: "player".into(),
                    hist: false,
                },
                Glyph::Char {
                    ch: "B".into(),
                    decorator: 0,
                    link: -1,
                    speed: 0,
                    condition: -1,
                },
                Glyph::Event {
                    event: "notify".into(),
                },
            ],
            ..TpWord::default()
        };
        let template = text.translation_template();
        assert_eq!(template, "A{{LV:VAR:0}}B{{LV:EVENT:1}}");
        assert!(text.replace_translation_text("Hola {{LV:VAR:0}}{{LV:EVENT:1}}"));
        assert_eq!(text.plain_text(), "Hola ");
        assert!(matches!(text.body[5], Glyph::Var { ref name, .. } if name == "player"));
        assert!(matches!(text.body[6], Glyph::Event { ref event } if event == "notify"));
    }

    #[test]
    fn rejects_translations_with_missing_or_unknown_runtime_tokens() {
        let mut text = TpWord {
            body: vec![Glyph::Var {
                name: "player".into(),
                hist: false,
            }],
            ..TpWord::default()
        };
        assert!(!text.replace_translation_text("Hola"));
        assert!(!text.replace_translation_text("Hola {{LV:VAR:9}}"));
        assert!(matches!(text.body[0], Glyph::Var { .. }));
    }
}
