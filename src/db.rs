//! Tablas en memoria para las funciones DB* de LiveMaker.
//!
//! La firma exacta de cada función no está documentada en pylivemaker; se acepta
//! el nombre de tabla como primer argumento y, si no coincide con una tabla
//! existente, se usa la tabla activa (DBSetActive).

use crate::lsb::expr::Value;
use crate::lsb::names::Func;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Table {
    pub fields: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub recno: usize,
}

impl Table {
    fn col(&mut self, f: &str) -> usize {
        match self.fields.iter().position(|x| x.eq_ignore_ascii_case(f)) {
            Some(i) => i,
            None => {
                self.fields.push(f.to_string());
                for r in &mut self.rows {
                    r.push(Value::Empty);
                }
                self.fields.len() - 1
            }
        }
    }
    fn get(&self, row: usize, f: &str) -> Value {
        let Some(c) = self.fields.iter().position(|x| x.eq_ignore_ascii_case(f)) else { return Value::Empty };
        self.rows.get(row).and_then(|r| r.get(c)).cloned().unwrap_or_default()
    }
    fn set(&mut self, row: usize, f: &str, v: Value) {
        let c = self.col(f);
        let n = self.fields.len();
        if row >= self.rows.len() {
            self.rows.resize(row + 1, vec![Value::Empty; n]);
        }
        self.rows[row][c] = v;
    }
    fn find(&self, f: &str, v: &Value, rows: impl Iterator<Item = usize>) -> i64 {
        for r in rows {
            if self.get(r, f).as_str() == v.as_str() {
                return r as i64;
            }
        }
        -1
    }
}

#[derive(Debug, Default)]
pub struct Db {
    pub tables: BTreeMap<String, Table>,
    pub active: String,
}

fn typed(f: u8, v: Value) -> Value {
    match f {
        Func::DBGETINT | Func::DBDIRECTGETINT => Value::Int(v.as_int()),
        Func::DBGETFLOAT | Func::DBDIRECTGETFLOAT => Value::Float(v.as_f64()),
        Func::DBGETBOOL | Func::DBDIRECTGETBOOL => Value::Bool(v.truthy()),
        _ => Value::Str(v.as_str()),
    }
}

impl Db {
    /// Separa (tabla, resto de args).
    fn split<'a>(&self, a: &'a [Value]) -> (String, &'a [Value]) {
        if let Some(first) = a.first() {
            let n = first.as_str();
            if self.tables.contains_key(&n) {
                return (n, &a[1..]);
            }
        }
        (self.active.clone(), a)
    }

    /// Carga TSV: primera línea = nombres de campo.
    pub fn load_tsv(&mut self, table: &str, text: &str) -> usize {
        let t = self.tables.entry(table.to_string()).or_default();
        let mut lines = text.lines().filter(|l| !l.is_empty());
        if t.fields.is_empty() {
            if let Some(h) = lines.next() {
                t.fields = h.split('\t').map(|s| s.to_string()).collect();
            }
        }
        for l in lines {
            let mut row: Vec<Value> = l.split('\t').map(|s| Value::Str(s.to_string())).collect();
            row.resize(t.fields.len(), Value::Empty);
            t.rows.push(row);
        }
        t.rows.len()
    }

    /// Ejecuta una función DB*. `load` lee un fichero del juego (para DBLoadTsvFile).
    pub fn call(&mut self, f: u8, a: &[Value], load: &dyn Fn(&str) -> Option<String>) -> Option<Value> {
        let s = |v: &[Value], i: usize| v.get(i).map(|x| x.as_str()).unwrap_or_default();
        let n = |v: &[Value], i: usize| v.get(i).map(|x| x.as_int()).unwrap_or(0);
        Some(match f {
            Func::DBCREATETABLE => {
                let name = s(a, 0);
                self.tables.insert(name.clone(), Table::default());
                if self.active.is_empty() {
                    self.active = name;
                }
                Value::Bool(true)
            }
            Func::DBDELETETABLE => Value::Bool(self.tables.remove(&s(a, 0)).is_some()),
            Func::DBSETACTIVE => {
                self.active = s(a, 0);
                Value::Bool(self.tables.contains_key(&self.active))
            }
            Func::DBGETACTIVE => Value::Str(self.active.clone()),
            Func::DBGETTABLECOUNT => Value::Int(self.tables.len() as i64),
            Func::DBGETTABLE => Value::Str(self.tables.keys().nth(n(a, 0).max(0) as usize).cloned().unwrap_or_default()),
            Func::DBCOPYTABLE => {
                let src = self.tables.get(&s(a, 0)).cloned().unwrap_or_default();
                self.tables.insert(s(a, 1), src);
                Value::Bool(true)
            }
            Func::DBLOADTSVFILE => {
                let (t, r) = self.split(a);
                let (t, file) = if r.len() >= 2 { (s(r, 0), s(r, 1)) } else { (t, s(r, 0)) };
                match load(&file) {
                    Some(text) => Value::Int(self.load_tsv(&t, &text) as i64),
                    None => Value::Int(0),
                }
            }
            _ => {
                let (t, r) = self.split(a);
                let tb = self.tables.entry(t).or_default();
                match f {
                    Func::DBADDFIELD => {
                        tb.col(&s(r, 0));
                        Value::Bool(true)
                    }
                    Func::DBSETRECNO => {
                        tb.recno = n(r, 0).max(0) as usize;
                        Value::Bool(tb.recno < tb.rows.len())
                    }
                    Func::DBGETRECNO => Value::Int(tb.recno as i64),
                    Func::DBRECORDCOUNT => Value::Int(tb.rows.len() as i64),
                    Func::DBINSERT => {
                        tb.rows.push(vec![Value::Empty; tb.fields.len()]);
                        tb.recno = tb.rows.len() - 1;
                        Value::Int(tb.recno as i64)
                    }
                    Func::DBDELETE => {
                        if tb.recno < tb.rows.len() {
                            tb.rows.remove(tb.recno);
                        }
                        Value::Bool(true)
                    }
                    Func::DBCLEARRECORD => {
                        if let Some(row) = tb.rows.get_mut(tb.recno) {
                            row.iter_mut().for_each(|v| *v = Value::Empty);
                        }
                        Value::Bool(true)
                    }
                    Func::DBCLEARTABLE => {
                        tb.rows.clear();
                        tb.recno = 0;
                        Value::Bool(true)
                    }
                    Func::DBGETINT | Func::DBGETFLOAT | Func::DBGETBOOL | Func::DBGETSTR => typed(f, tb.get(tb.recno, &s(r, 0))),
                    Func::DBSETINT | Func::DBSETFLOAT | Func::DBSETBOOL | Func::DBSETSTR => {
                        let rn = tb.recno;
                        tb.set(rn, &s(r, 0), r.get(1).cloned().unwrap_or_default());
                        Value::Bool(true)
                    }
                    // DBDirectGet*(tabla, campo, registro)
                    Func::DBDIRECTGETINT | Func::DBDIRECTGETFLOAT | Func::DBDIRECTGETBOOL | Func::DBDIRECTGETSTR => {
                        typed(f, tb.get(n(r, 1).max(0) as usize, &s(r, 0)))
                    }
                    Func::DBDIRECTSETINT | Func::DBDIRECTSETFLOAT | Func::DBDIRECTSETBOOL | Func::DBDIRECTSETSTR => {
                        tb.set(n(r, 1).max(0) as usize, &s(r, 0), r.get(2).cloned().unwrap_or_default());
                        Value::Bool(true)
                    }
                    Func::DBFINDFIRST | Func::DBLOCATE => {
                        let i = tb.find(&s(r, 0), r.get(1).unwrap_or(&Value::Empty), 0..tb.rows.len());
                        if i >= 0 {
                            tb.recno = i as usize;
                        }
                        Value::Int(i)
                    }
                    Func::DBFINDLAST => {
                        let i = tb.find(&s(r, 0), r.get(1).unwrap_or(&Value::Empty), (0..tb.rows.len()).rev());
                        if i >= 0 {
                            tb.recno = i as usize;
                        }
                        Value::Int(i)
                    }
                    Func::DBFINDNEXT => {
                        let i = tb.find(&s(r, 0), r.get(1).unwrap_or(&Value::Empty), tb.recno + 1..tb.rows.len());
                        if i >= 0 {
                            tb.recno = i as usize;
                        }
                        Value::Int(i)
                    }
                    Func::DBFINDPRIOR => {
                        let i = tb.find(&s(r, 0), r.get(1).unwrap_or(&Value::Empty), (0..tb.recno).rev());
                        if i >= 0 {
                            tb.recno = i as usize;
                        }
                        Value::Int(i)
                    }
                    Func::DBSORT => {
                        let c = tb.col(&s(r, 0));
                        let desc = r.get(1).is_some_and(|v| !v.truthy());
                        tb.rows.sort_by(|x, y| {
                            let (a, b) = (&x[c], &y[c]);
                            let o = if matches!(a, Value::Int(_) | Value::Float(_)) || a.as_str().parse::<f64>().is_ok() {
                                a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(std::cmp::Ordering::Equal)
                            } else {
                                a.as_str().cmp(&b.as_str())
                            };
                            if desc { o.reverse() } else { o }
                        });
                        Value::Bool(true)
                    }
                    Func::DBINSERTTABLE | Func::DBCOPY => {
                        return None;
                    }
                    _ => return None,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tsv_and_find() {
        let mut db = Db::default();
        let load = |_: &str| Some("id\tname\n1\ta\n2\tb\n".to_string());
        db.call(Func::DBCREATETABLE, &[Value::Str("t".into())], &load);
        assert_eq!(db.call(Func::DBLOADTSVFILE, &[Value::Str("t".into()), Value::Str("x.tsv".into())], &load), Some(Value::Int(2)));
        assert_eq!(db.call(Func::DBFINDFIRST, &[Value::Str("t".into()), Value::Str("name".into()), Value::Str("b".into())], &load), Some(Value::Int(1)));
        assert_eq!(db.call(Func::DBGETINT, &[Value::Str("t".into()), Value::Str("id".into())], &load), Some(Value::Int(2)));
        assert_eq!(db.call(Func::DBDIRECTGETSTR, &[Value::Str("t".into()), Value::Str("name".into()), Value::Int(0)], &load), Some(Value::Str("a".into())));
    }
}
