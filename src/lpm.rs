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
    if !d.starts_with(b"LivePrevMenu") {
        return Err("not an LPM file".into());
    }
    let v: u32 = std::str::from_utf8(d.get(12..15).ok_or("short")?).ok().and_then(|s| s.parse().ok()).ok_or("bad version")?;
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
        r.s().ok_or_else(e)?;
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
        if v > 103 {
            for _ in 0..5 {
                r.s().ok_or_else(e)?;
            }
            r.u32().ok_or_else(e)?;
        }
        if v > 104 {
            r.s().ok_or_else(e)?;
        }
        if v > 105 {
            r.s().ok_or_else(e)?;
        }
        out.push(Button { a, b, src, name, src_selected });
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
        assert_eq!((b[1].a, b[1].b, b[1].src_selected.as_str()), ((101, 200), (7, 8), "a2.gal"));
    }
}
