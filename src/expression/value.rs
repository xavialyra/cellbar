use std::collections::BTreeMap;

use serde_json::Value;

use crate::images::Part;

#[derive(Clone, Debug)]
pub enum DynValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    List(Vec<DynValue>),
    Map(BTreeMap<String, DynValue>),
    Rich(Vec<Part>),
}

impl DynValue {
    pub fn from_json(val: &Value) -> Self {
        match val {
            Value::Null => DynValue::Null,
            Value::Bool(b) => DynValue::Bool(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    DynValue::Number(i as f64)
                } else if let Some(f) = n.as_f64() {
                    DynValue::Number(f)
                } else {
                    DynValue::Null
                }
            }
            Value::String(s) => DynValue::String(s.clone()),
            Value::Array(arr) => DynValue::List(arr.iter().map(DynValue::from_json).collect()),
            Value::Object(obj) => {
                let mut map = BTreeMap::new();
                for (k, v) in obj {
                    map.insert(k.clone(), DynValue::from_json(v));
                }
                DynValue::Map(map)
            }
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            DynValue::Null => String::new(),
            DynValue::Bool(b) => b.to_string(),
            DynValue::Number(n) => {
                if n.fract() == 0.0 {
                    format!("{:.0}", n)
                } else {
                    format!("{n}")
                }
            }
            DynValue::String(s) => s.clone(),
            DynValue::List(items) => items
                .iter()
                .map(|item| item.as_string())
                .collect::<Vec<_>>()
                .join(""),
            DynValue::Map(_) => String::new(),
            DynValue::Rich(parts) => {
                let mut s = String::new();
                for p in parts {
                    if let Part::Text(t) = p {
                        s.push_str(t);
                    }
                }
                s
            }
        }
    }

    pub fn into_parts(self) -> Vec<Part> {
        match self {
            DynValue::Rich(parts) => parts,
            _ => vec![Part::Text(self.as_string())],
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            DynValue::Null => false,
            DynValue::Bool(b) => *b,
            DynValue::Number(n) => *n != 0.0,
            DynValue::String(s) => !s.is_empty(),
            DynValue::List(l) => !l.is_empty(),
            DynValue::Map(m) => !m.is_empty(),
            DynValue::Rich(r) => !r.is_empty(),
        }
    }

    pub fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (DynValue::Null, DynValue::Null) => true,
            (DynValue::Bool(a), DynValue::Bool(b)) => a == b,
            (DynValue::Number(a), DynValue::Number(b)) => (a - b).abs() < f64::EPSILON,
            (DynValue::String(a), DynValue::String(b)) => a == b,
            (DynValue::List(a), DynValue::List(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.equals(y))
            }
            (DynValue::Map(a), DynValue::Map(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(k, v)| b.get(k).is_some_and(|bv| v.equals(bv)))
            }
            _ => false,
        }
    }
}
