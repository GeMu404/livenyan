//! LivePrevMenu (.lpm): botones de una prevista de menú (formato según pylivemaker lpm.py).
//! Campos sin documentar se conservan como números para depurar (nunca se imprime texto).

#[derive(Debug, Clone, Default)]
pub struct Button {
    /// Dos primeros enteros ("width"/"height" en pylivemaker; pueden ser posición).
    pub a: (i32, i32),
    /// unk15/unk16.
    pub b: (i32, i32),
    pub src: String,
    pub name: String,
    pub src_selected: String,
    /// Imagen cuando el ítem está deshabilitado (manual: 無効時の画像).
    pub src_disabled: String,
    /// Grupo del botón. `-1` es un toggle independiente (manual: グループ番号 -1
    /// en オート / スキップ). `0` si el archivo no trae el campo.
    pub group: i32,
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn s(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        if n > 1 << 20 {
            return None;
        }
        let b = self.take(n)?;
        Some(encoding_rs::SHIFT_JIS.decode(b).0.into_owned())
    }
}

pub fn parse(d: &[u8]) -> Result<(u32, Vec<Button>), String> {
    if d.starts_with(b"<?xml") {
        return parse_xml(d);
    }
    if !d.starts_with(b"LivePrevMenu") {
        return Err("not an LPM file".into());
    }
    let v: u32 = std::str::from_utf8(d.get(12..15).ok_or("short")?)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or("bad version")?;
    let mut r = Rd { d, p: 15 + 8 };
    let n = r.u32().ok_or("short")? as usize;
    let mut out = Vec::new();
    for i in 0..n.min(1024) {
        let e = || format!("button {i}: truncated");
        let a = (r.u32().ok_or_else(e)? as i32, r.u32().ok_or_else(e)? as i32);
        let src = r.s().ok_or_else(e)?;
        r.take(1).ok_or_else(e)?;
        let name = r.s().ok_or_else(e)?;
        let src_selected = r.s().ok_or_else(e)?;
        // pylivemaker unk3, justo después de la imagen seleccionada: 無効時の画像.
        let src_disabled = r.s().ok_or_else(e)?;
        r.s().ok_or_else(e)?;
        if v > 100 {
            r.s().ok_or_else(e)?;
        }
        if v > 102 {
            r.s().ok_or_else(e)?;
            r.s().ok_or_else(e)?;
        }
        for _ in 0..3 {
            r.s().ok_or_else(e)?;
        }
        if v > 101 {
            r.s().ok_or_else(e)?;
            r.s().ok_or_else(e)?;
        }
        let b = (r.u32().ok_or_else(e)? as i32, r.u32().ok_or_else(e)? as i32);
        r.s().ok_or_else(e)?;
        let mut group = 0i32;
        if v > 103 {
            for _ in 0..5 {
                r.s().ok_or_else(e)?;
            }
            group = r.u32().ok_or_else(e)? as i32;
        }
        if v > 104 {
            r.s().ok_or_else(e)?;
        }
        if v > 105 {
            r.s().ok_or_else(e)?;
        }
        out.push(Button {
            a,
            b,
            src,
            name,
            src_selected,
            src_disabled,
            group,
        });
    }
    Ok((v, out))
}

fn xml_tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let i = s.find(&open)? + open.len();
    let j = s[i..].find(&close)? + i;
    Some(&s[i..j])
}

/// Variante XML de .lpm (`<PrevMenu>` con `<Item>`), texto en Shift-JIS.
pub fn parse_xml(d: &[u8]) -> Result<(u32, Vec<Button>), String> {
    let s = encoding_rs::SHIFT_JIS.decode(d).0;
    let v: u32 = xml_tag(&s, "Version")
        .and_then(|t| t.trim().parse().ok())
        .ok_or("xml: bad version")?;
    let num = |it: &str, n: &str| {
        xml_tag(it, n)
            .and_then(|t| t.trim().parse::<i32>().ok())
            .unwrap_or(0)
    };
    let txt = |it: &str, n: &str| {
        xml_tag(it, n)
            .map(|t| t.trim().to_string())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for chunk in s.split("<Item>").skip(1) {
        let it = chunk.split("</Item>").next().unwrap_or("");
        let settle = txt(it, "SettleImagePath");
        let src_selected = if settle.is_empty() {
            txt(it, "InImagePath")
        } else {
            settle
        };
        out.push(Button {
            a: (num(it, "Left"), num(it, "Top")),
            b: (num(it, "PrevLeft"), num(it, "PrevTop")),
            src: txt(it, "Path"),
            name: txt(it, "Name"),
            src_selected,
            src_disabled: txt(it, "InvalidImagePath"),
            group: num(it, "Group"),
        });
    }
    Ok((v, out))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_v106_buttons() {
        let s = |v: &mut Vec<u8>, t: &str| {
            v.extend((t.len() as u32).to_le_bytes());
            v.extend(t.as_bytes());
        };
        let mut d = b"LivePrevMenu106".to_vec();
        d.extend([0u8; 8]);
        d.extend(2u32.to_le_bytes());
        for i in 0..2u32 {
            d.extend((100 + i).to_le_bytes());
            d.extend(200u32.to_le_bytes());
            s(&mut d, "a.gal");
            d.push(0);
            s(&mut d, "b");
            s(&mut d, "a2.gal");
            for _ in 0..3 + 2 + 3 + 2 {
                s(&mut d, "");
            }
            d.extend(7u32.to_le_bytes());
            d.extend(8u32.to_le_bytes());
            s(&mut d, "");
            for _ in 0..5 {
                s(&mut d, "");
            }
            d.extend(0u32.to_le_bytes());
            s(&mut d, "");
            s(&mut d, "");
        }
        let (v, b) = super::parse(&d).unwrap();
        assert_eq!(v, 106);
        assert_eq!(b.len(), 2);
        assert_eq!(
            (b[1].a, b[1].b, b[1].src_selected.as_str()),
            ((101, 200), (7, 8), "a2.gal")
        );
    }
}
