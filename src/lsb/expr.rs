//! Evaluador de expresiones `TLiveParser` y tipo de valor dinámico.

use super::names::Func;
use super::{op, LiveParser, Param};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Array(Vec<Value>),
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Empty => false,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Bool(b) => *b,
            Value::Str(s) => !s.is_empty() && s != "0",
            Value::Array(a) => !a.is_empty(),
        }
    }
    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i64,
            Value::Bool(b) => *b as i64,
            Value::Str(s) => s.trim().parse().unwrap_or(0),
            _ => 0,
        }
    }
    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            Value::Bool(b) => *b as i64 as f64,
            Value::Str(s) => s.trim().parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }
    pub fn as_str(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => if *b { "1".into() } else { "0".into() },
            Value::Str(s) => s.clone(),
            Value::Array(a) => a.iter().map(|v| v.as_str()).collect::<Vec<_>>().join("\r\n"),
        }
    }
    pub fn from_param(p: &Param) -> Value {
        match p {
            Param::Int(i) => Value::Int(*i as i64),
            Param::Float(f) => Value::Float(*f),
            Param::Flag(b) => Value::Bool(*b),
            Param::Str(s) => Value::Str(s.clone()),
            Param::Var(_) => Value::Empty,
        }
    }
    /// Valor por defecto según ParamType (0=Var 1=Int 2=Float 3=Flag 4=Str).
    pub fn default_for(t: u8) -> Value {
        match t {
            1 => Value::Int(0),
            2 => Value::Float(0.0),
            3 => Value::Bool(false),
            4 => Value::Str(String::new()),
            _ => Value::Empty,
        }
    }
    fn is_num(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Empty)
    }
}

/// Entorno que el VM provee al evaluador.
pub trait Env {
    fn get_var(&mut self, name: &str) -> Value;
    fn set_var(&mut self, name: &str, v: Value);
    /// Funciones dependientes del motor (GetProp, Exists, …). `None` => no soportada.
    fn call_engine(&mut self, func: u8, args: &[Value], raw: &[Param]) -> Option<Value>;
    fn random(&mut self) -> f64;
    fn warn_once(&mut self, key: &str, msg: &str);
}

fn cmp(a: &Value, b: &Value) -> std::cmp::Ordering {
    if a.is_num() && b.is_num() {
        a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(std::cmp::Ordering::Equal)
    } else {
        a.as_str().cmp(&b.as_str())
    }
}

fn arith(a: &Value, b: &Value, fi: fn(i64, i64) -> Option<i64>, ff: fn(f64, f64) -> f64) -> Value {
    match (a, b) {
        (Value::Float(_), _) | (_, Value::Float(_)) => Value::Float(ff(a.as_f64(), b.as_f64())),
        _ => match fi(a.as_int(), b.as_int()) {
            Some(v) => Value::Int(v),
            None => Value::Int(0),
        },
    }
}

/// Evalúa una expresión. Las temporales `____*` viven solo durante la evaluación.
pub fn eval(lp: &LiveParser, env: &mut dyn Env) -> Value {
    let mut temps: HashMap<&str, Value> = HashMap::new();
    let mut last = Value::Empty;
    for e in &lp.entries {
        let mut args = Vec::with_capacity(e.operands.len());
        for p in &e.operands {
            args.push(match p {
                Param::Var(n) if n.starts_with("____") => temps.get(n.as_str()).cloned().unwrap_or_default(),
                Param::Var(n) => env.get_var(n),
                other => Value::from_param(other),
            });
        }
        let a = args.first().cloned().unwrap_or_default();
        let b = args.get(1).cloned().unwrap_or_default();
        let v = match e.op {
            op::TO => args.last().cloned().unwrap_or_default(),
            op::PLUS => {
                if matches!(a, Value::Str(_)) || matches!(b, Value::Str(_)) {
                    Value::Str(a.as_str() + &b.as_str())
                } else {
                    arith(&a, &b, |x, y| Some(x.wrapping_add(y)), |x, y| x + y)
                }
            }
            op::MINUS => arith(&a, &b, |x, y| Some(x.wrapping_sub(y)), |x, y| x - y),
            op::MUL => arith(&a, &b, |x, y| Some(x.wrapping_mul(y)), |x, y| x * y),
            // Nota: la semántica exacta de división entera en LiveMaker no está verificada.
            op::DIV => arith(&a, &b, |x, y| x.checked_div(y), |x, y| x / y),
            op::MOD => arith(&a, &b, |x, y| x.checked_rem(y), |x, y| x % y),
            op::OR | op::AND | op::XOR => {
                if matches!(a, Value::Bool(_)) || matches!(b, Value::Bool(_)) {
                    let (x, y) = (a.truthy(), b.truthy());
                    Value::Bool(match e.op {
                        op::OR => x || y,
                        op::AND => x && y,
                        _ => x ^ y,
                    })
                } else {
                    let (x, y) = (a.as_int(), b.as_int());
                    Value::Int(match e.op {
                        op::OR => x | y,
                        op::AND => x & y,
                        _ => x ^ y,
                    })
                }
            }
            op::SHIFTL => Value::Int(a.as_int().wrapping_shl(b.as_int() as u32)),
            op::SHIFTR => Value::Int(a.as_int().wrapping_shr(b.as_int() as u32)),
            op::EQUAL => Value::Bool(cmp(&a, &b).is_eq()),
            op::NEQUAL => Value::Bool(!cmp(&a, &b).is_eq()),
            op::BIG => Value::Bool(cmp(&a, &b).is_gt()),
            op::SMALL => Value::Bool(cmp(&a, &b).is_lt()),
            op::EBIG => Value::Bool(cmp(&a, &b).is_ge()),
            op::ESMALL => Value::Bool(cmp(&a, &b).is_le()),
            op::COMBOSTR => Value::Str(a.as_str() + &b.as_str()),
            op::DIMTO => {
                let mut cur = a;
                for idx in &args[1.min(args.len())..] {
                    cur = match cur {
                        Value::Array(v) => v.get(idx.as_int().max(0) as usize).cloned().unwrap_or_default(),
                        _ => Value::Empty,
                    };
                }
                cur
            }
            op::FUNC => call_func(e.func.unwrap_or(255), &args, &e.operands, env),
            op::NONE => Value::Empty,
            other => {
                env.warn_once(&format!("op{other}"), &format!("unknown operator {other}"));
                Value::Empty
            }
        };
        if e.name.starts_with("____") {
            temps.insert(e.name.as_str(), v.clone());
        } else if !e.name.is_empty() {
            env.set_var(&e.name, v.clone());
        }
        last = v;
    }
    last
}

fn var_name(raw: &[Param], i: usize) -> Option<&str> {
    match raw.get(i) {
        Some(Param::Var(n)) if !n.starts_with("____") => Some(n),
        Some(Param::Str(s)) => Some(s),
        _ => None,
    }
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// Funciones puras + manipulación de arrays por nombre de variable.
fn call_func(f: u8, a: &[Value], raw: &[Param], env: &mut dyn Env) -> Value {
    let g = |i: usize| a.get(i).cloned().unwrap_or_default();
    let s = |i: usize| g(i).as_str();
    let n = |i: usize| g(i).as_int();
    let x = |i: usize| g(i).as_f64();
    match f {
        Func::INTTOSTR => Value::Str(n(0).to_string()),
        Func::INTTOHEX => Value::Str(format!("{:X}", n(0))),
        Func::LENGTH | Func::JLENGTH => match g(0) {
            Value::Array(v) => Value::Int(v.len() as i64),
            v => Value::Int(if f == Func::LENGTH { encoding_rs::SHIFT_JIS.encode(&v.as_str()).0.len() } else { v.as_str().chars().count() } as i64),
        },
        Func::GETARRAYSIZE => Value::Int(match g(0) {
            Value::Array(v) => v.len() as i64,
            _ => 0,
        }),
        // Copy(s, index(1-based), count)
        Func::COPY | Func::JCOPY => {
            let c = chars(&s(0));
            let st = (n(1) - 1).max(0) as usize;
            let cnt = n(2).max(0) as usize;
            Value::Str(c.iter().skip(st).take(cnt).collect())
        }
        Func::DELETE | Func::JDELETE => {
            let mut c = chars(&s(0));
            let st = ((n(1) - 1).max(0) as usize).min(c.len());
            let end = (st + n(2).max(0) as usize).min(c.len());
            c.drain(st..end);
            Value::Str(c.into_iter().collect())
        }
        Func::INSERT | Func::JINSERT => {
            let mut c = chars(&s(1));
            let at = ((n(2) - 1).max(0) as usize).min(c.len());
            for (k, ch) in s(0).chars().enumerate() {
                c.insert(at + k, ch);
            }
            Value::Str(c.into_iter().collect())
        }
        Func::POS | Func::JPOS => Value::Int(s(1).find(&s(0)).map(|b| s(1)[..b].chars().count() as i64 + 1).unwrap_or(0)),
        Func::COMPARESTR => Value::Int(s(0).cmp(&s(1)) as i64),
        Func::COMPARETEXT => Value::Int(s(0).to_lowercase().cmp(&s(1).to_lowercase()) as i64),
        Func::TRIM | Func::JTRIM => match g(0) {
            Value::Array(v) => Value::Array(v),
            v => Value::Str(v.as_str().trim().to_string()),
        },
        Func::NOT => Value::Bool(!g(0).truthy()),
        Func::RANDOM => {
            let m = n(0).max(1);
            Value::Int((env.random() * m as f64) as i64)
        }
        Func::INT => Value::Int(x(0) as i64),
        Func::FLOAT => Value::Float(x(0)),
        Func::SIN => Value::Float(x(0).sin()),
        Func::COS => Value::Float(x(0).cos()),
        Func::TAN => Value::Float(x(0).tan()),
        Func::ARCSIN => Value::Float(x(0).asin()),
        Func::ARCCOS => Value::Float(x(0).acos()),
        Func::ARCTAN => Value::Float(x(0).atan()),
        Func::ARCTAN2 => Value::Float(x(0).atan2(x(1))),
        Func::HYPOT => Value::Float(x(0).hypot(x(1))),
        Func::ABS => Value::Int(n(0).abs()),
        Func::FABS => Value::Float(x(0).abs()),
        Func::MIN => Value::Int(n(0).min(n(1))),
        Func::MAX => Value::Int(n(0).max(n(1))),
        Func::FMIN => Value::Float(x(0).min(x(1))),
        Func::FMAX => Value::Float(x(0).max(x(1))),
        Func::STRTOINTDEF => Value::Int(s(0).trim().parse().unwrap_or(n(1))),
        Func::STRTOFLOATDEF => Value::Float(s(0).trim().parse().unwrap_or(x(1))),
        Func::LOWERCASE => Value::Str(s(0).to_lowercase()),
        Func::UPPERCASE => Value::Str(s(0).to_uppercase()),
        Func::STRINGOFCHAR => Value::Str(s(0).repeat(n(1).max(0) as usize)),
        Func::STRINGREPLACE => Value::Str(s(0).replace(&s(1), &s(2))),
        Func::CHR => Value::Str(char::from_u32(n(0) as u32).map(String::from).unwrap_or_default()),
        // Delphi IsDelimiter(Delimiters, S, Index): ¿S[Index] (base 1) está en Delimiters?
        Func::ISDELIMITER => {
            let c = chars(&s(1));
            let i = n(2);
            Value::Bool(i >= 1 && (i as usize) <= c.len() && s(0).contains(c[i as usize - 1]))
        }
        Func::ORD => Value::Int(s(0).chars().next().map(|c| c as i64).unwrap_or(0)),
        Func::EXTRACTFILENAME => Value::Str(s(0).rsplit(['\\', '/']).next().unwrap_or("").to_string()),
        Func::EXTRACTFILEEXT => {
            let f = s(0);
            let base = f.rsplit(['\\', '/']).next().unwrap_or("");
            Value::Str(base.rfind('.').map(|i| base[i..].to_string()).unwrap_or_default())
        }
        Func::EXTRACTFILEPATH => {
            let f = s(0);
            Value::Str(f.rfind(['\\', '/']).map(|i| f[..=i].to_string()).unwrap_or_default())
        }
        Func::CHANGEFILEEXT => {
            let f = s(0);
            let stem = match f.rfind('.') {
                Some(i) if !f[i..].contains(['\\', '/']) => &f[..i],
                _ => &f[..],
            };
            Value::Str(format!("{stem}{}", s(1)))
        }
        Func::ADDBACKSLASH => {
            let f = s(0);
            Value::Str(if f.ends_with('\\') { f } else { f + "\\" })
        }
        Func::INDEXOFSTR => match g(0) {
            Value::Array(v) => Value::Int(v.iter().position(|e| e.as_str() == s(1)).map(|i| i as i64).unwrap_or(-1)),
            _ => Value::Int(-1),
        },
        Func::ARRAYTOSTRING => Value::Str(match g(0) {
            Value::Array(v) => v.iter().map(|e| e.as_str()).collect::<Vec<_>>().join(&if a.len() > 1 { s(1) } else { "\r\n".into() }),
            v => v.as_str(),
        }),
        Func::STRINGTOARRAY => {
            let sep = if a.len() > 2 { s(2) } else { "\r\n".to_string() };
            let arr: Vec<Value> = s(0).split(sep.as_str()).map(|p| Value::Str(p.to_string())).collect();
            if let Some(name) = var_name(raw, 1) {
                env.set_var(name, Value::Array(arr.clone()));
            }
            Value::Int(arr.len() as i64)
        }
        Func::ADDARRAY | Func::INSERTARRAY | Func::DELETEARRAY | Func::SETARRAY | Func::TRIMARRAY | Func::UNIQUEARRAY
        | Func::SORTSTR => {
            let Some(name) = var_name(raw, 0) else { return Value::Empty };
            let mut arr = match env.get_var(name) {
                Value::Array(v) => v,
                Value::Empty => vec![],
                other => vec![other],
            };
            match f {
                Func::ADDARRAY => arr.extend(a.iter().skip(1).cloned()),
                Func::INSERTARRAY => {
                    let i = (n(1).max(0) as usize).min(arr.len());
                    arr.insert(i, g(2));
                }
                Func::DELETEARRAY => {
                    let i = n(1).max(0) as usize;
                    if i < arr.len() {
                        arr.remove(i);
                    }
                }
                Func::SETARRAY => {
                    let i = n(1).max(0) as usize;
                    if i >= arr.len() {
                        arr.resize(i + 1, Value::Empty);
                    }
                    arr[i] = g(2);
                }
                Func::TRIMARRAY => arr.retain(|v| !v.as_str().trim().is_empty()),
                Func::UNIQUEARRAY => {
                    let mut seen = std::collections::HashSet::new();
                    arr.retain(|v| seen.insert(v.as_str()));
                }
                _ => arr.sort_by_key(|v| v.as_str()),
            }
            let len = arr.len();
            env.set_var(name, Value::Array(arr));
            Value::Int(len as i64)
        }
        Func::COPYARRAY => {
            if let (Some(dst), src) = (var_name(raw, 0), g(1)) {
                env.set_var(dst, src);
            }
            Value::Empty
        }
        Func::FILLMEM => {
            if let Some(name) = var_name(raw, 0) {
                let len = match env.get_var(name) {
                    Value::Array(v) => v.len(),
                    _ => 0,
                };
                env.set_var(name, Value::Array(vec![g(1); len]));
            }
            Value::Empty
        }
        Func::CALC => {
            env.warn_once("Calc()", "Calc() string evaluation not supported");
            Value::Int(0)
        }
        _ => match env.call_engine(f, a, raw) {
            Some(v) => v,
            None => {
                let nm = super::func_name(f);
                env.warn_once(nm, &format!("unimplemented function {nm}()"));
                Value::Int(0)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsb::OpeData;
    struct E(HashMap<String, Value>);
    impl Env for E {
        fn get_var(&mut self, n: &str) -> Value {
            self.0.get(n).cloned().unwrap_or_default()
        }
        fn set_var(&mut self, n: &str, v: Value) {
            self.0.insert(n.into(), v);
        }
        fn call_engine(&mut self, _: u8, _: &[Value], _: &[Param]) -> Option<Value> {
            None
        }
        fn random(&mut self) -> f64 {
            0.5
        }
        fn warn_once(&mut self, _: &str, _: &str) {}
    }
    fn od(op: u8, name: &str, func: Option<u8>, ops: Vec<Param>) -> OpeData {
        OpeData { op, name: name.into(), func, operands: ops }
    }
    #[test]
    fn basic_ops() {
        let mut e = E(HashMap::new());
        e.0.insert("a".into(), Value::Int(5));
        let lp = LiveParser {
            entries: vec![
                od(op::MUL, "____1", None, vec![Param::Var("a".into()), Param::Int(3)]),
                od(op::PLUS, "____2", None, vec![Param::Var("____1".into()), Param::Int(1)]),
                od(op::TO, "b", None, vec![Param::Var("____2".into())]),
            ],
        };
        assert_eq!(eval(&lp, &mut e), Value::Int(16));
        assert_eq!(e.0["b"], Value::Int(16));
        let lp = LiveParser {
            entries: vec![
                od(op::FUNC, "____1", Some(Func::ADDARRAY), vec![Param::Var("_tmp".into()), Param::Str("x".into())]),
                od(op::FUNC, "____2", Some(Func::ADDARRAY), vec![Param::Var("_tmp".into()), Param::Str("y".into())]),
                od(op::FUNC, "____arg", Some(Func::GETARRAYSIZE), vec![Param::Var("_tmp".into())]),
            ],
        };
        assert_eq!(eval(&lp, &mut e), Value::Int(2));
        let lp = LiveParser {
            entries: vec![od(op::FUNC, "____arg", Some(Func::COPY), vec![Param::Str("あいうえお".into()), Param::Int(2), Param::Int(2)])],
        };
        assert_eq!(eval(&lp, &mut e), Value::Str("いう".into()));
    }
}
