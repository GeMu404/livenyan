//! Evaluador de expresiones `TLiveParser` y tipo de valor dinámico.

use super::names::Func;
use super::{LiveParser, Param, op};
use crate::util::{cp932, to_cp932};
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
            Value::Bool(b) => {
                if *b {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Value::Str(s) => s.clone(),
            Value::Array(a) => a
                .iter()
                .map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("\r\n"),
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
        matches!(
            self,
            Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Empty
        )
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
    let number = |value: &Value| match value {
        Value::Str(s) => {
            let s = s.trim();
            if s.is_empty() {
                Some(0.0)
            } else {
                s.parse::<f64>().ok()
            }
        }
        value if value.is_num() => Some(value.as_f64()),
        _ => None,
    };
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.as_str().cmp(&b.as_str()),
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

fn numeric_value(value: &Value) -> Value {
    let Value::Str(s) = value else {
        return value.clone();
    };
    let s = s.trim();
    s.parse::<i64>()
        .map(Value::Int)
        .or_else(|_| s.parse::<f64>().map(Value::Float))
        .unwrap_or(Value::Int(0))
}

/// Evalúa una expresión. Las temporales `____*` viven solo durante la evaluación.
pub fn eval(lp: &LiveParser, env: &mut dyn Env) -> Value {
    let mut temps: HashMap<&str, Value> = HashMap::new();
    let mut array_lvalues: HashMap<&str, (String, usize)> = HashMap::new();
    let mut last = Value::Empty;
    for e in &lp.entries {
        let mut args = Vec::with_capacity(e.operands.len());
        for p in &e.operands {
            args.push(match p {
                Param::Var(n) if n.starts_with("____") => {
                    temps.get(n.as_str()).cloned().unwrap_or_default()
                }
                Param::Var(n) => env.get_var(n),
                other => Value::from_param(other),
            });
        }
        let a = args.first().cloned().unwrap_or_default();
        let b = args.get(1).cloned().unwrap_or_default();
        let v = match e.op {
            op::TO => {
                let val = args.last().cloned().unwrap_or_default();
                if let Some((arr_name, idx)) = array_lvalues.get(e.name.as_str()) {
                    let mut arr = match env.get_var(arr_name) {
                        Value::Array(v) => v,
                        Value::Empty => vec![],
                        other => vec![other],
                    };
                    if *idx >= arr.len() {
                        arr.resize(*idx + 1, Value::Empty);
                    }
                    arr[*idx] = val.clone();
                    env.set_var(arr_name, Value::Array(arr));
                }
                val
            }
            op::PLUS => {
                let (a, b) = (numeric_value(&a), numeric_value(&b));
                arith(&a, &b, |x, y| Some(x.wrapping_add(y)), |x, y| x + y)
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
                if let (Some(Param::Var(arr_var)), Some(idx_val)) =
                    (e.operands.first(), args.get(1))
                    && !arr_var.starts_with("____")
                {
                    array_lvalues.insert(
                        e.name.as_str(),
                        (arr_var.clone(), idx_val.as_int().max(0) as usize),
                    );
                }
                let mut cur = a;
                for idx in &args[1.min(args.len())..] {
                    cur = match cur {
                        Value::Array(v) => v
                            .get(idx.as_int().max(0) as usize)
                            .cloned()
                            .unwrap_or_default(),
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

/// Lead byte CP932 / Shift_JIS (pares de 2 bytes). **fuente primaria** calc.html FixStr/Copy.
fn is_sjis_lead(b: u8) -> bool {
    (0x81..=0x9f).contains(&b) || (0xe0..=0xfc).contains(&b)
}

/// Quita un lead byte final incompleto tras un Copy por bytes (calc.html FixStr).
/// En livenyan las cadenas son Unicode: un corte SJIS a mitad de carácter suele
/// decodificar como `U+FFFD` al final — se elimina (**interpretación** del mismo caso).
fn fix_str_sjis(s: &str) -> String {
    // Primero el síntoma Unicode del corte a mitad de carácter.
    let mut trimmed = s.to_string();
    while trimmed.ends_with('\u{FFFD}') {
        trimmed.pop();
    }
    // Luego lead SJIS huérfano si aún quedara tras re-encode.
    let mut bytes = to_cp932(&trimmed);
    let mut i = 0usize;
    while i < bytes.len() {
        if is_sjis_lead(bytes[i]) {
            if i + 1 >= bytes.len() {
                bytes.pop();
                break;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    cp932(&bytes)
}

/// `Copy` por bytes CP932, índice 1-based; `count == None` → hasta el final.
fn sjis_copy(s: &str, start_1based: i64, count: Option<i64>) -> String {
    let bytes = to_cp932(s);
    let start = (start_1based - 1).max(0) as usize;
    if start >= bytes.len() {
        return String::new();
    }
    let end = match count {
        Some(n) => (start + n.max(0) as usize).min(bytes.len()),
        None => bytes.len(),
    };
    cp932(&bytes[start..end])
}

fn sjis_delete(s: &str, start_1based: i64, count: Option<i64>) -> String {
    let mut bytes = to_cp932(s);
    let start = ((start_1based - 1).max(0) as usize).min(bytes.len());
    let end = match count {
        Some(n) => (start + n.max(0) as usize).min(bytes.len()),
        None => bytes.len(),
    };
    bytes.drain(start..end);
    cp932(&bytes)
}

fn eval_calc_assignment(source: &str, env: &mut dyn Env) -> Option<Value> {
    let (target, expression) = source.split_once('=')?;
    let target = target.trim();
    if target.is_empty() || target.chars().any(char::is_whitespace) {
        return None;
    }

    let expression = expression.trim();
    let value = if let Some(call) = expression
        .strip_prefix("GetProp(")
        .and_then(|expression| expression.strip_suffix(')'))
    {
        let (object_expression, property) = call.rsplit_once(',')?;
        let (prefix, index_var) = object_expression.split_once("++")?;
        let prefix = prefix.trim().strip_prefix('"')?.strip_suffix('"')?;
        let index_var = index_var.trim();
        if index_var.is_empty() || index_var.chars().any(char::is_whitespace) {
            return None;
        }
        let property = property.trim().parse::<i32>().ok()?;
        let object = Value::Str(format!("{prefix}{}", env.get_var(index_var).as_str()));
        let arguments = [object, Value::Int(i64::from(property))];
        let raw = [Param::Str(arguments[0].as_str()), Param::Int(property)];
        env.call_engine(Func::GETPROP, &arguments, &raw)?
    } else if !expression.is_empty()
        && !expression.chars().any(char::is_whitespace)
        && !expression
            .chars()
            .any(|c| matches!(c, '"' | '(' | ')' | '=' | ','))
    {
        env.get_var(expression)
    } else {
        return None;
    };
    env.set_var(target, value.clone());
    Some(value)
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
            v => Value::Int(if f == Func::LENGTH {
                encoding_rs::SHIFT_JIS.encode(&v.as_str()).0.len()
            } else {
                v.as_str().chars().count()
            } as i64),
        },
        Func::GETARRAYSIZE => Value::Int(match g(0) {
            Value::Array(v) => v.len() as i64,
            _ => 0,
        }),
        // Copy: índices en **bytes** CP932 (calc.html). JCopy: caracteres Unicode.
        Func::COPY => Value::Str(sjis_copy(&s(0), n(1), a.get(2).map(|v| v.as_int()))),
        Func::JCOPY => {
            let c = chars(&s(0));
            let st = (n(1) - 1).max(0) as usize;
            let cnt = match a.get(2) {
                Some(v) => v.as_int().max(0) as usize,
                None => c.len().saturating_sub(st),
            };
            Value::Str(c.iter().skip(st).take(cnt).collect())
        }
        // Delete: bytes CP932; muta la variable (calc.html「なし」). JDelete: chars.
        Func::DELETE => {
            let out = Value::Str(sjis_delete(&s(0), n(1), a.get(2).map(|v| v.as_int())));
            if let Some(name) = var_name(raw, 0) {
                env.set_var(name, out.clone());
            }
            out
        }
        Func::JDELETE => {
            let mut c = chars(&s(0));
            let st = ((n(1) - 1).max(0) as usize).min(c.len());
            let end = match a.get(2) {
                Some(v) => (st + v.as_int().max(0) as usize).min(c.len()),
                None => c.len(),
            };
            c.drain(st..end);
            let out = Value::Str(c.into_iter().collect());
            if let Some(name) = var_name(raw, 0) {
                env.set_var(name, out.clone());
            }
            out
        }
        // FixStr: si el final es lead byte SJIS huérfano, lo quita (calc.html).
        Func::FIXSTR => Value::Str(fix_str_sjis(&s(0))),
        Func::INSERT | Func::JINSERT => {
            let mut c = chars(&s(1));
            let at = ((n(2) - 1).max(0) as usize).min(c.len());
            for (k, ch) in s(0).chars().enumerate() {
                c.insert(at + k, ch);
            }
            Value::Str(c.into_iter().collect())
        }
        Func::POS | Func::JPOS => Value::Int(
            s(1).find(&s(0))
                .map(|b| s(1)[..b].chars().count() as i64 + 1)
                .unwrap_or(0),
        ),
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
        Func::CHR => Value::Str(
            char::from_u32(n(0) as u32)
                .map(String::from)
                .unwrap_or_default(),
        ),
        // Delphi IsDelimiter(Delimiters, S, Index): ¿S[Index] (base 1) está en Delimiters?
        Func::ISDELIMITER => {
            let c = chars(&s(1));
            let i = n(2);
            Value::Bool(i >= 1 && (i as usize) <= c.len() && s(0).contains(c[i as usize - 1]))
        }
        Func::ORD => Value::Int(s(0).chars().next().map(|c| c as i64).unwrap_or(0)),
        Func::EXTRACTFILENAME => {
            Value::Str(s(0).rsplit(['\\', '/']).next().unwrap_or("").to_string())
        }
        Func::EXTRACTFILEEXT => {
            let f = s(0);
            let base = f.rsplit(['\\', '/']).next().unwrap_or("");
            Value::Str(
                base.rfind('.')
                    .map(|i| base[i..].to_string())
                    .unwrap_or_default(),
            )
        }
        Func::EXTRACTFILEPATH => {
            let f = s(0);
            Value::Str(
                f.rfind(['\\', '/'])
                    .map(|i| f[..=i].to_string())
                    .unwrap_or_default(),
            )
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
            Value::Array(v) => Value::Int(
                v.iter()
                    .position(|e| e.as_str() == s(1))
                    .map(|i| i as i64)
                    .unwrap_or(-1),
            ),
            _ => Value::Int(-1),
        },
        Func::ARRAYTOSTRING => Value::Str(match g(0) {
            Value::Array(v) => v
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join(&if a.len() > 1 { s(1) } else { "\r\n".into() }),
            v => v.as_str(),
        }),
        Func::STRINGTOARRAY => {
            let sep = if a.len() > 2 {
                s(2)
            } else {
                "\r\n".to_string()
            };
            let arr: Vec<Value> = s(0)
                .split(sep.as_str())
                .map(|p| Value::Str(p.to_string()))
                .collect();
            if let Some(name) = var_name(raw, 1) {
                env.set_var(name, Value::Array(arr.clone()));
            }
            Value::Int(arr.len() as i64)
        }
        Func::ADDARRAY
        | Func::INSERTARRAY
        | Func::DELETEARRAY
        | Func::SETARRAY
        | Func::TRIMARRAY
        | Func::UNIQUEARRAY
        | Func::SORTSTR => {
            let Some(name) = var_name(raw, 0) else {
                return Value::Empty;
            };
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
            let source = s(0);
            match eval_calc_assignment(&source, env) {
                Some(value) => value,
                None => {
                    env.warn_once(
                        "Calc()",
                        "Calc() only supports assignments from indexed GetProp expressions",
                    );
                    Value::Int(0)
                }
            }
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
        fn call_engine(&mut self, func: u8, args: &[Value], _: &[Param]) -> Option<Value> {
            if func == Func::GETPROP {
                self.0.get(&args.first()?.as_str()).cloned()
            } else {
                None
            }
        }
        fn random(&mut self) -> f64 {
            0.5
        }
        fn warn_once(&mut self, _: &str, _: &str) {}
    }
    fn od(op: u8, name: &str, func: Option<u8>, ops: Vec<Param>) -> OpeData {
        OpeData {
            op,
            name: name.into(),
            func,
            operands: ops,
        }
    }
    #[test]
    fn basic_ops() {
        let mut e = E(HashMap::new());
        e.0.insert("a".into(), Value::Int(5));
        let lp = LiveParser {
            entries: vec![
                od(
                    op::MUL,
                    "____1",
                    None,
                    vec![Param::Var("a".into()), Param::Int(3)],
                ),
                od(
                    op::PLUS,
                    "____2",
                    None,
                    vec![Param::Var("____1".into()), Param::Int(1)],
                ),
                od(op::TO, "b", None, vec![Param::Var("____2".into())]),
            ],
        };
        assert_eq!(eval(&lp, &mut e), Value::Int(16));
        assert_eq!(e.0["b"], Value::Int(16));
        let lp = LiveParser {
            entries: vec![
                od(
                    op::FUNC,
                    "____1",
                    Some(Func::ADDARRAY),
                    vec![Param::Var("_tmp".into()), Param::Str("x".into())],
                ),
                od(
                    op::FUNC,
                    "____2",
                    Some(Func::ADDARRAY),
                    vec![Param::Var("_tmp".into()), Param::Str("y".into())],
                ),
                od(
                    op::FUNC,
                    "____arg",
                    Some(Func::GETARRAYSIZE),
                    vec![Param::Var("_tmp".into())],
                ),
            ],
        };
        assert_eq!(eval(&lp, &mut e), Value::Int(2));
        // calc.html: Copy es por bytes CP932 — Copy("あいうえお", 3, 2) == "い"
        let lp = LiveParser {
            entries: vec![od(
                op::FUNC,
                "____arg",
                Some(Func::COPY),
                vec![
                    Param::Str("あいうえお".into()),
                    Param::Int(3),
                    Param::Int(2),
                ],
            )],
        };
        assert_eq!(eval(&lp, &mut e), Value::Str("い".into()));

        // JCopy: por caracteres — JCopy("あいうえお", 3, 2) == "うえ"
        let lp = LiveParser {
            entries: vec![od(
                op::FUNC,
                "____arg",
                Some(Func::JCOPY),
                vec![
                    Param::Str("あいうえお".into()),
                    Param::Int(3),
                    Param::Int(2),
                ],
            )],
        };
        assert_eq!(eval(&lp, &mut e), Value::Str("うえ".into()));

        // FixStr(Copy("あい", 1, 3)) == "あ" (calc.html)
        let lp = LiveParser {
            entries: vec![
                od(
                    op::FUNC,
                    "____c",
                    Some(Func::COPY),
                    vec![Param::Str("あい".into()), Param::Int(1), Param::Int(3)],
                ),
                od(
                    op::FUNC,
                    "____arg",
                    Some(Func::FIXSTR),
                    vec![Param::Var("____c".into())],
                ),
            ],
        };
        assert_eq!(eval(&lp, &mut e), Value::Str("あ".into()));

        // Test array element assignment through DIMTO and TO (e.g. @Result[i] = S)
        e.0.insert("@Result".into(), Value::Array(vec![Value::Empty; 3]));
        e.0.insert("i".into(), Value::Int(1));
        e.0.insert("S".into(), Value::Str("Ranura 1".into()));
        let lp_arr = LiveParser {
            entries: vec![
                od(
                    op::DIMTO,
                    "____d_0",
                    None,
                    vec![Param::Var("@Result".into()), Param::Var("i".into())],
                ),
                od(op::TO, "____d_0", None, vec![Param::Var("S".into())]),
            ],
        };
        eval(&lp_arr, &mut e);
        assert_eq!(
            e.0["@Result"],
            Value::Array(vec![
                Value::Empty,
                Value::Str("Ranura 1".into()),
                Value::Empty
            ])
        );
    }

    #[test]
    fn plus_adds_numeric_strings_while_concat_operator_joins_strings() {
        let mut env = E(HashMap::new());
        let add = LiveParser {
            entries: vec![od(
                op::PLUS,
                "result",
                None,
                vec![Param::Str("500".into()), Param::Int(306)],
            )],
        };
        assert_eq!(eval(&add, &mut env), Value::Int(806));

        let add_float = LiveParser {
            entries: vec![od(
                op::PLUS,
                "result",
                None,
                vec![Param::Str("1.5".into()), Param::Float(0.25)],
            )],
        };
        assert_eq!(eval(&add_float, &mut env), Value::Float(1.75));

        let concat = LiveParser {
            entries: vec![od(
                op::COMBOSTR,
                "result",
                None,
                vec![Param::Str("500".into()), Param::Int(306)],
            )],
        };
        assert_eq!(eval(&concat, &mut env), Value::Str("500306".into()));
    }

    #[test]
    fn calc_assigns_existing_and_indexed_getprop_values() {
        let mut env = E(HashMap::new());
        env.0.insert("i".into(), Value::Int(0));
        env.0.insert("エディット0".into(), Value::Str("12".into()));
        env.0
            .insert("職業（姉）".into(), Value::Str("初期値".into()));
        let initial_value = LiveParser {
            entries: vec![od(
                op::FUNC,
                "____result",
                Some(Func::CALC),
                vec![Param::Str("__s = 職業（姉）".into())],
            )],
        };
        assert_eq!(eval(&initial_value, &mut env), Value::Str("初期値".into()));
        assert_eq!(env.0["__s"], Value::Str("初期値".into()));

        let calc = LiveParser {
            entries: vec![od(
                op::FUNC,
                "____result",
                Some(Func::CALC),
                vec![Param::Str(
                    "職業（姉） = GetProp(\"エディット\"++i,50)".into(),
                )],
            )],
        };

        assert_eq!(eval(&calc, &mut env), Value::Str("12".into()));
        assert_eq!(env.0["職業（姉）"], Value::Str("12".into()));
    }

    #[test]
    fn comparisons_coerce_numeric_strings_but_keep_text_order_for_non_numbers() {
        let mut env = E(HashMap::new());
        let less_than_limit = LiveParser {
            entries: vec![od(
                op::SMALL,
                "result",
                None,
                vec![Param::Int(3), Param::Str("15".into())],
            )],
        };
        assert_eq!(eval(&less_than_limit, &mut env), Value::Bool(true));

        let text_order = LiveParser {
            entries: vec![od(
                op::SMALL,
                "result",
                None,
                vec![Param::Str("alpha".into()), Param::Str("beta".into())],
            )],
        };
        assert_eq!(eval(&text_order, &mut env), Value::Bool(true));

        let zero_is_not_different_from_empty_string = LiveParser {
            entries: vec![od(
                op::NEQUAL,
                "result",
                None,
                vec![Param::Int(0), Param::Str(String::new())],
            )],
        };
        assert_eq!(
            eval(&zero_is_not_different_from_empty_string, &mut env),
            Value::Bool(false)
        );
    }

    #[test]
    fn fix_str_strips_orphan_after_byte_copy() {
        let broken = sjis_copy("あい", 1, Some(3));
        let codes: Vec<u32> = broken.chars().map(|c| c as u32).collect();
        assert!(
            codes.contains(&0xfffd) || codes == [0x3042],
            "unexpected copy codes {codes:?} for {broken:?}"
        );
        let fixed = fix_str_sjis(&broken);
        assert_eq!(fixed, "あ", "broken={broken:?} codes={codes:?}");
    }
}
