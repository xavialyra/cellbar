use std::{collections::BTreeMap, path::Path};

use chrono::{Local, TimeZone};
use serde_json::Value;

use super::ast::{Ast, BinaryOp, Expr, Stmt, TemplateSegment};
use super::value::DynValue;
use crate::images::{self, ImageAlign, ImageFit, ImageShape, ImageSpec, Part};

struct EvalContext<'a> {
    variables: BTreeMap<String, DynValue>,
    base: &'a Path,
}

impl<'a> EvalContext<'a> {
    fn eval_expr(&mut self, expr: &Expr) -> Result<DynValue, String> {
        match expr {
            Expr::Literal(v) => Ok(v.clone()),
            Expr::Template(segments) => {
                let mut parts = Vec::new();
                let mut str_buf = String::new();
                for seg in segments {
                    match seg {
                        TemplateSegment::Literal(lit) => {
                            str_buf.push_str(lit);
                        }
                        TemplateSegment::Expr(sub_expr) => {
                            let val = self.eval_expr(sub_expr)?;
                            match val {
                                DynValue::Rich(p) => {
                                    if !str_buf.is_empty() {
                                        parts.push(Part::Text(std::mem::take(&mut str_buf)));
                                    }
                                    parts.extend(p);
                                }
                                other => {
                                    str_buf.push_str(&escape_markup_text(&other.as_string()));
                                }
                            }
                        }
                    }
                }
                if parts.is_empty() {
                    Ok(DynValue::String(str_buf))
                } else {
                    if !str_buf.is_empty() {
                        parts.push(Part::Text(str_buf));
                    }
                    Ok(DynValue::Rich(parts))
                }
            }
            Expr::Var(name) => self
                .variables
                .get(name)
                .cloned()
                .ok_or_else(|| format!("undefined variable {name}")),
            Expr::List(items) => {
                let mut evaluated = Vec::with_capacity(items.len());
                for item in items {
                    evaluated.push(self.eval_expr(item)?);
                }
                Ok(DynValue::List(evaluated))
            }
            Expr::Block(statements) => {
                let mut last = DynValue::Null;
                for stmt in statements {
                    match stmt {
                        Stmt::Let(name, expr) => {
                            let val = self.eval_expr(expr)?;
                            self.variables.insert(name.clone(), val.clone());
                            last = val;
                        }
                        Stmt::Expr(expr) => {
                            last = self.eval_expr(expr)?;
                        }
                    }
                }
                Ok(last)
            }
            Expr::FieldAccess {
                target,
                field,
                optional,
            } => {
                let val = self.eval_expr(target)?;
                if *optional && matches!(val, DynValue::Null) {
                    return Ok(DynValue::Null);
                }
                match val {
                    DynValue::Map(map) => Ok(map.get(field).cloned().unwrap_or(DynValue::Null)),
                    _ => Ok(DynValue::Null),
                }
            }
            Expr::IndexAccess {
                target,
                index,
                optional,
            } => {
                let target_val = self.eval_expr(target)?;
                if *optional && matches!(target_val, DynValue::Null) {
                    return Ok(DynValue::Null);
                }
                let index_val = self.eval_expr(index)?;
                match (target_val, index_val) {
                    (DynValue::List(items), DynValue::Number(n)) => {
                        let idx = n as isize;
                        if idx >= 0 && (idx as usize) < items.len() {
                            Ok(items[idx as usize].clone())
                        } else if idx < 0 && (idx.unsigned_abs()) <= items.len() {
                            Ok(items[items.len() - idx.unsigned_abs()].clone())
                        } else {
                            Ok(DynValue::Null)
                        }
                    }
                    (DynValue::Map(map), DynValue::String(k)) => {
                        Ok(map.get(&k).cloned().unwrap_or(DynValue::Null))
                    }
                    _ => Ok(DynValue::Null),
                }
            }
            Expr::UnaryNot(inner) => {
                let val = self.eval_expr(inner)?;
                Ok(DynValue::Bool(!val.is_truthy()))
            }
            Expr::UnaryNeg(inner) => {
                let val = self.eval_expr(inner)?;
                match val {
                    DynValue::Number(n) => Ok(DynValue::Number(-n)),
                    _ => Ok(DynValue::Null),
                }
            }
            Expr::Binary { op, left, right } => match op {
                BinaryOp::Coalesce => {
                    let l = self.eval_expr(left)?;
                    if matches!(l, DynValue::Null) {
                        self.eval_expr(right)
                    } else {
                        Ok(l)
                    }
                }
                BinaryOp::And => {
                    let l = self.eval_expr(left)?;
                    if !l.is_truthy() {
                        Ok(l)
                    } else {
                        self.eval_expr(right)
                    }
                }
                BinaryOp::Or => {
                    let l = self.eval_expr(left)?;
                    if l.is_truthy() {
                        Ok(l)
                    } else {
                        self.eval_expr(right)
                    }
                }
                BinaryOp::In => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    let contained = match (&l, &r) {
                        (DynValue::String(needle), DynValue::String(haystack)) => {
                            haystack.contains(needle)
                        }
                        (_, DynValue::List(items)) => items.iter().any(|item| item.equals(&l)),
                        (_, DynValue::Map(map)) => map.contains_key(&l.as_string()),
                        _ => false,
                    };
                    Ok(DynValue::Bool(contained))
                }
                BinaryOp::NotIn => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    let contained = match (&l, &r) {
                        (DynValue::String(needle), DynValue::String(haystack)) => {
                            haystack.contains(needle)
                        }
                        (_, DynValue::List(items)) => items.iter().any(|item| item.equals(&l)),
                        (_, DynValue::Map(map)) => map.contains_key(&l.as_string()),
                        _ => false,
                    };
                    Ok(DynValue::Bool(!contained))
                }
                BinaryOp::Equal => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    Ok(DynValue::Bool(l.equals(&r)))
                }
                BinaryOp::NotEqual => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    Ok(DynValue::Bool(!l.equals(&r)))
                }
                BinaryOp::Less => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    match (l, r) {
                        (DynValue::Number(a), DynValue::Number(b)) => Ok(DynValue::Bool(a < b)),
                        (DynValue::String(a), DynValue::String(b)) => Ok(DynValue::Bool(a < b)),
                        _ => Ok(DynValue::Bool(false)),
                    }
                }
                BinaryOp::LessEqual => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    match (l, r) {
                        (DynValue::Number(a), DynValue::Number(b)) => Ok(DynValue::Bool(a <= b)),
                        (DynValue::String(a), DynValue::String(b)) => Ok(DynValue::Bool(a <= b)),
                        _ => Ok(DynValue::Bool(false)),
                    }
                }
                BinaryOp::Greater => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    match (l, r) {
                        (DynValue::Number(a), DynValue::Number(b)) => Ok(DynValue::Bool(a > b)),
                        (DynValue::String(a), DynValue::String(b)) => Ok(DynValue::Bool(a > b)),
                        _ => Ok(DynValue::Bool(false)),
                    }
                }
                BinaryOp::GreaterEqual => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    match (l, r) {
                        (DynValue::Number(a), DynValue::Number(b)) => Ok(DynValue::Bool(a >= b)),
                        (DynValue::String(a), DynValue::String(b)) => Ok(DynValue::Bool(a >= b)),
                        _ => Ok(DynValue::Bool(false)),
                    }
                }
                BinaryOp::Sub => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    match (l, r) {
                        (DynValue::Number(a), DynValue::Number(b)) => Ok(DynValue::Number(a - b)),
                        _ => Ok(DynValue::Null),
                    }
                }
                BinaryOp::Add => {
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    match (l, r) {
                        (DynValue::Number(a), DynValue::Number(b)) => Ok(DynValue::Number(a + b)),
                        (DynValue::Rich(mut l_parts), DynValue::Rich(r_parts)) => {
                            l_parts.extend(r_parts);
                            Ok(DynValue::Rich(l_parts))
                        }
                        (DynValue::Rich(mut l_parts), other) => {
                            l_parts.push(Part::Text(other.as_string()));
                            Ok(DynValue::Rich(l_parts))
                        }
                        (other, DynValue::Rich(mut r_parts)) => {
                            let mut res = vec![Part::Text(other.as_string())];
                            res.append(&mut r_parts);
                            Ok(DynValue::Rich(res))
                        }
                        (DynValue::List(mut l_items), DynValue::List(r_items)) => {
                            l_items.extend(r_items);
                            Ok(DynValue::List(l_items))
                        }
                        (l, r) => Ok(DynValue::String(format!(
                            "{}{}",
                            l.as_string(),
                            r.as_string()
                        ))),
                    }
                }
            },
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.eval_expr(condition)?;
                if cond_val.is_truthy() {
                    self.eval_expr(then_branch)
                } else if let Some(else_branch) = else_branch {
                    self.eval_expr(else_branch)
                } else {
                    Ok(DynValue::Null)
                }
            }
            Expr::Lambda { .. } => {
                Err("lambda expressions cannot be evaluated outside method calls".into())
            }
            Expr::Map(entries) => {
                let mut map = BTreeMap::new();
                for (k, v_expr) in entries {
                    map.insert(k.clone(), self.eval_expr(v_expr)?);
                }
                Ok(DynValue::Map(map))
            }
            Expr::Call { func, args } => {
                let mut evaluated_args = Vec::with_capacity(args.len());
                for a in args {
                    evaluated_args.push(self.eval_expr(a)?);
                }
                self.eval_func(func, evaluated_args)
            }
            Expr::MethodCall {
                target,
                method,
                args,
                optional,
            } => {
                let target_val = self.eval_expr(target)?;
                if *optional && matches!(target_val, DynValue::Null) {
                    return Ok(DynValue::Null);
                }
                self.eval_method(method, target, target_val, args)
            }
        }
    }

    fn call_lambda(
        &mut self,
        lambda: &Expr,
        arg: &DynValue,
        index: usize,
    ) -> Result<DynValue, String> {
        let (params, body) = match lambda {
            Expr::Lambda { params, body } => (params, body),
            other => return Err(format!("expected closure, got {other:?}")),
        };
        let prev_vars = self.variables.clone();
        if let Some(param0) = params.first() {
            self.variables.insert(param0.clone(), arg.clone());
        }
        if let Some(param1) = params.get(1) {
            self.variables
                .insert(param1.clone(), DynValue::Number(index as f64));
        }
        let result = self.eval_expr(body);
        self.variables = prev_vars;
        result
    }

    fn eval_method(
        &mut self,
        method: &str,
        target_expr: &Expr,
        target_val: DynValue,
        args: &[Expr],
    ) -> Result<DynValue, String> {
        match (target_val, method) {
            // Higher-order list methods
            (DynValue::List(items), "map") => {
                if args.is_empty() {
                    return Err("map requires a closure or template string".into());
                }
                match &args[0] {
                    Expr::Lambda { .. } => {
                        let mut mapped = Vec::with_capacity(items.len());
                        for (i, item) in items.iter().enumerate() {
                            mapped.push(self.call_lambda(&args[0], item, i)?);
                        }
                        Ok(DynValue::List(mapped))
                    }
                    other => {
                        let template_val = self.eval_expr(other)?;
                        let template = template_val.as_string();
                        let mut mapped = Vec::with_capacity(items.len());
                        for item in &items {
                            match item {
                                DynValue::Map(m) => {
                                    mapped.push(DynValue::String(render_template_map(
                                        template.clone(),
                                        m,
                                    )));
                                }
                                _ => mapped.push(item.clone()),
                            }
                        }
                        Ok(DynValue::List(mapped))
                    }
                }
            }
            (DynValue::List(items), "filter") => {
                if args.is_empty() {
                    return Err("filter requires a predicate closure".into());
                }
                let mut filtered = Vec::new();
                for (i, item) in items.iter().enumerate() {
                    if self.call_lambda(&args[0], item, i)?.is_truthy() {
                        filtered.push(item.clone());
                    }
                }
                Ok(DynValue::List(filtered))
            }
            (DynValue::List(items), "find") => {
                if args.is_empty() {
                    return Err("find requires a predicate closure".into());
                }
                for (i, item) in items.iter().enumerate() {
                    if self.call_lambda(&args[0], item, i)?.is_truthy() {
                        return Ok(item.clone());
                    }
                }
                Ok(DynValue::Null)
            }
            (DynValue::List(items), "any") => {
                if args.is_empty() {
                    return Ok(DynValue::Bool(!items.is_empty()));
                }
                for (i, item) in items.iter().enumerate() {
                    if self.call_lambda(&args[0], item, i)?.is_truthy() {
                        return Ok(DynValue::Bool(true));
                    }
                }
                Ok(DynValue::Bool(false))
            }
            (DynValue::List(items), "all") => {
                if args.is_empty() {
                    return Ok(DynValue::Bool(true));
                }
                for (i, item) in items.iter().enumerate() {
                    if !self.call_lambda(&args[0], item, i)?.is_truthy() {
                        return Ok(DynValue::Bool(false));
                    }
                }
                Ok(DynValue::Bool(true))
            }
            (DynValue::List(items), "join") => {
                let sep = if let Some(arg) = args.first() {
                    self.eval_expr(arg)?
                } else {
                    DynValue::String(String::new())
                };
                let has_rich = items.iter().any(|it| matches!(it, DynValue::Rich(_)))
                    || matches!(sep, DynValue::Rich(_));
                if has_rich {
                    let mut res = Vec::new();
                    for (i, item) in items.iter().enumerate() {
                        if i > 0 {
                            res.extend(sep.clone().into_parts());
                        }
                        res.extend(item.clone().into_parts());
                    }
                    Ok(DynValue::Rich(res))
                } else {
                    let sep_str = sep.as_string();
                    let joined = items
                        .iter()
                        .map(|it| it.as_string())
                        .collect::<Vec<_>>()
                        .join(&sep_str);
                    Ok(DynValue::String(joined))
                }
            }
            (DynValue::List(items), "contains") => {
                if args.is_empty() {
                    return Err("contains requires a value".into());
                }
                let val = self.eval_expr(&args[0])?;
                Ok(DynValue::Bool(items.iter().any(|it| it.equals(&val))))
            }
            (DynValue::List(items), "len" | "count") => Ok(DynValue::Number(items.len() as f64)),
            (DynValue::List(items), "is_empty") => Ok(DynValue::Bool(items.is_empty())),
            (DynValue::List(items), "first") => {
                Ok(items.first().cloned().unwrap_or(DynValue::Null))
            }
            (DynValue::List(items), "last") => Ok(items.last().cloned().unwrap_or(DynValue::Null)),
            (DynValue::List(mut items), "reverse") => {
                items.reverse();
                Ok(DynValue::List(items))
            }
            (DynValue::List(items), "take") => {
                let count = match args.first().map(|a| self.eval_expr(a)).transpose()? {
                    Some(DynValue::Number(n)) => n as usize,
                    _ => 0,
                };
                Ok(DynValue::List(items.into_iter().take(count).collect()))
            }
            (DynValue::List(items), "skip") => {
                let count = match args.first().map(|a| self.eval_expr(a)).transpose()? {
                    Some(DynValue::Number(n)) => n as usize,
                    _ => 0,
                };
                Ok(DynValue::List(items.into_iter().skip(count).collect()))
            }

            // String methods
            (DynValue::String(s), "trim") => Ok(DynValue::String(s.trim().to_owned())),
            (DynValue::String(s), "to_uppercase") => Ok(DynValue::String(s.to_uppercase())),
            (DynValue::String(s), "to_lowercase") => Ok(DynValue::String(s.to_lowercase())),
            (DynValue::String(s), "len") => Ok(DynValue::Number(s.chars().count() as f64)),
            (DynValue::String(s), "is_empty") => Ok(DynValue::Bool(s.is_empty())),
            (DynValue::String(s), "contains") => {
                let sub = if let Some(arg) = args.first() {
                    self.eval_expr(arg)?.as_string()
                } else {
                    String::new()
                };
                Ok(DynValue::Bool(s.contains(&sub)))
            }
            (DynValue::String(s), "starts_with") => {
                let prefix = if let Some(arg) = args.first() {
                    self.eval_expr(arg)?.as_string()
                } else {
                    String::new()
                };
                Ok(DynValue::Bool(s.starts_with(&prefix)))
            }
            (DynValue::String(s), "ends_with") => {
                let suffix = if let Some(arg) = args.first() {
                    self.eval_expr(arg)?.as_string()
                } else {
                    String::new()
                };
                Ok(DynValue::Bool(s.ends_with(&suffix)))
            }
            (DynValue::String(s), "split") => {
                let sep = if let Some(arg) = args.first() {
                    self.eval_expr(arg)?.as_string()
                } else {
                    " ".into()
                };
                let parts: Vec<DynValue> = s
                    .split(&sep)
                    .map(|p| DynValue::String(p.to_owned()))
                    .collect();
                Ok(DynValue::List(parts))
            }
            (DynValue::String(s), "replace") => {
                if args.len() < 2 {
                    return Err("replace requires from and to".into());
                }
                let from = self.eval_expr(&args[0])?.as_string();
                let to = self.eval_expr(&args[1])?.as_string();
                let replaced = s.replace(&from, &to);
                let res = DynValue::String(replaced);
                if let Expr::Var(var_name) = target_expr {
                    self.variables.insert(var_name.clone(), res.clone());
                }
                Ok(res)
            }
            (DynValue::String(template), "render" | "render_template") => {
                if args.is_empty() {
                    return Err("render method requires data map".into());
                }
                let data = self.eval_expr(&args[0])?;
                match data {
                    DynValue::Map(map) => Ok(DynValue::String(render_template_map(template, &map))),
                    _ => Err("argument to render must be a map".into()),
                }
            }

            // Map methods
            (DynValue::Map(map), "contains" | "has") => {
                let key = if let Some(arg) = args.first() {
                    self.eval_expr(arg)?.as_string()
                } else {
                    String::new()
                };
                Ok(DynValue::Bool(map.contains_key(&key)))
            }
            (DynValue::Map(map), "get") => {
                let key = if let Some(arg) = args.first() {
                    self.eval_expr(arg)?.as_string()
                } else {
                    String::new()
                };
                let default = if let Some(arg) = args.get(1) {
                    self.eval_expr(arg)?
                } else {
                    DynValue::Null
                };
                Ok(map.get(&key).cloned().unwrap_or(default))
            }
            (DynValue::Map(map), "keys") => {
                let keys = map.keys().map(|k| DynValue::String(k.clone())).collect();
                Ok(DynValue::List(keys))
            }
            (DynValue::Map(map), "values") => {
                let vals = map.values().cloned().collect();
                Ok(DynValue::List(vals))
            }
            (DynValue::Map(map), "len" | "count") => Ok(DynValue::Number(map.len() as f64)),
            (DynValue::Map(map), "is_empty") => Ok(DynValue::Bool(map.is_empty())),

            // Universal methods
            (val, "to_string") => Ok(DynValue::String(val.as_string())),

            (other, method) => Err(format!("cannot call method {method:?} on {other:?}")),
        }
    }

    fn eval_func(&mut self, func: &str, args: Vec<DynValue>) -> Result<DynValue, String> {
        match func {
            "print" | "debug" | "now" => Err(format!("restricted function {func}")),
            "render" | "render_template" => {
                if args.len() < 2 {
                    return Err("render requires template and data map".into());
                }
                let template = args[0].as_string();
                match &args[1] {
                    DynValue::Map(map) => Ok(DynValue::String(render_template_map(template, map))),
                    _ => Err("second argument to render must be a map".into()),
                }
            }
            "format_percent" => {
                if args.is_empty() {
                    return Ok(DynValue::String("0%".into()));
                }
                let num = match &args[0] {
                    DynValue::Number(n) => *n,
                    _ => 0.0,
                };
                if num.fract() == 0.0 {
                    Ok(DynValue::String(format!("{num:.0}%")))
                } else {
                    Ok(DynValue::String(format!("{num:.1}%")))
                }
            }
            "format_size" => {
                let kib = match args.first() {
                    Some(DynValue::Number(n)) => *n as i64,
                    _ => 0,
                };
                Ok(DynValue::String(format_memory_size(kib)))
            }
            "format_memory" => {
                if args.len() < 5 {
                    return Err("format_memory requires template and 4 numbers".into());
                }
                let template = args[0].as_string();
                let used = match &args[1] {
                    DynValue::Number(n) => *n as i64,
                    _ => 0,
                };
                let total = match &args[2] {
                    DynValue::Number(n) => *n as i64,
                    _ => 0,
                };
                let available = match &args[3] {
                    DynValue::Number(n) => *n as i64,
                    _ => 0,
                };
                let percent = match &args[4] {
                    DynValue::Number(n) => *n as i64,
                    _ => 0,
                };
                Ok(DynValue::String(format_memory(
                    template, used, total, available, percent,
                )))
            }
            "strftime" => {
                if args.len() < 2 {
                    return Err("strftime requires timestamp and format".into());
                }
                let ts = match &args[0] {
                    DynValue::Number(n) => *n as i64,
                    _ => 0,
                };
                let fmt = args[1].as_string();
                Ok(DynValue::String(format_timestamp(ts, fmt)))
            }
            "image" => {
                if args.is_empty() {
                    return Err("image requires at least src argument".into());
                }
                let src = args[0].as_string();
                let mut width = 2usize;
                let mut fit = ImageFit::TextMatch;
                let mut shape = ImageShape::Rect;
                let mut align = ImageAlign::Center;
                let mut fallback = String::new();

                if args.len() > 1 {
                    match &args[1] {
                        DynValue::Map(options) => {
                            for k in options.keys() {
                                if !["width", "fit", "shape", "align", "fallback"]
                                    .contains(&k.as_str())
                                {
                                    return Err(format!("unknown image option {k}"));
                                }
                            }
                            if let Some(w) = options.get("width") {
                                match w {
                                    DynValue::Number(n) => width = *n as usize,
                                    _ => return Err("image width must be an integer".into()),
                                }
                            }
                            if let Some(f) = options.get("fit") {
                                match f.as_string().as_str() {
                                    "contain" => fit = ImageFit::Contain,
                                    "cover" => fit = ImageFit::Cover,
                                    "stretch" => fit = ImageFit::Stretch,
                                    "scale_down" => fit = ImageFit::ScaleDown,
                                    "text_match" => fit = ImageFit::TextMatch,
                                    other => return Err(format!("unknown image fit {other}")),
                                }
                            }
                            if let Some(s) = options.get("shape") {
                                match s.as_string().as_str() {
                                    "rect" => shape = ImageShape::Rect,
                                    "circle" => shape = ImageShape::Circle,
                                    other => return Err(format!("unknown image shape {other}")),
                                }
                            }
                            if let Some(a) = options.get("align") {
                                match a.as_string().as_str() {
                                    "left" => align = ImageAlign::Left,
                                    "center" => align = ImageAlign::Center,
                                    "right" => align = ImageAlign::Right,
                                    other => return Err(format!("unknown image align {other}")),
                                }
                            }
                            if let Some(fb) = options.get("fallback") {
                                fallback = fb.as_string();
                            }
                        }
                        _ => return Err("second argument to image must be an options map".into()),
                    }
                }

                let mut spec = ImageSpec {
                    src: Path::new(&src).to_path_buf(),
                    width,
                    fit,
                    shape,
                    align,
                    fallback,
                };
                spec.resolve(self.base)?;
                Ok(DynValue::Rich(vec![Part::Image(spec)]))
            }
            other => Err(format!("unknown function {other}")),
        }
    }
}

pub fn render_template_map(template: String, data: &BTreeMap<String, DynValue>) -> String {
    let mut output = template;
    for (key, value) in data {
        let placeholder = format!("{{{key}}}");
        let val_str = match value {
            DynValue::String(s) => escape_markup_text(s),
            DynValue::Number(n) => {
                if n.fract() == 0.0 {
                    format!("{:.0}", n)
                } else {
                    format!("{n:.1}")
                }
            }
            DynValue::Bool(b) => b.to_string(),
            _ => escape_markup_text(&value.as_string()),
        };
        output = output.replace(&placeholder, &val_str);
    }
    output
}

pub fn escape_markup_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '[' | ']' | '(' | ')' | '#' | '@' | '{' | '}'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn format_memory(
    template: String,
    used_kib: i64,
    total_kib: i64,
    available_kib: i64,
    percent: i64,
) -> String {
    let replacements = [
        ("{used}", format_memory_size(used_kib)),
        ("{total}", format_memory_size(total_kib)),
        ("{available}", format_memory_size(available_kib)),
        ("{percent}", format!("{percent}%")),
    ];
    replacements
        .into_iter()
        .fold(template, |output, (placeholder, value)| {
            output.replace(placeholder, &value)
        })
}

fn format_memory_size(kib: i64) -> String {
    const MIB: i64 = 1024;
    const GIB: i64 = 1024 * 1024;
    let kib = kib.max(0);
    if kib >= GIB {
        format!("{:.1}G", kib as f64 / GIB as f64)
    } else if kib >= MIB {
        format!("{}M", kib / MIB)
    } else {
        format!("{kib}K")
    }
}

fn format_timestamp(timestamp: i64, format: String) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Local::now)
        .format(&format)
        .to_string()
}

#[allow(dead_code)]
pub fn evaluate(
    ast: &Ast,
    context: &Value,
    state: &BTreeMap<String, Value>,
    settings: &Value,
) -> Result<String, String> {
    let parts = evaluate_parts(ast, context, state, settings, Path::new("."))?;
    let mut s = String::new();
    for p in parts {
        if let Part::Text(t) = p {
            s.push_str(&t);
        }
    }
    Ok(s)
}

pub fn evaluate_parts(
    ast: &Ast,
    context: &Value,
    state: &BTreeMap<String, Value>,
    settings: &Value,
    base: &Path,
) -> Result<Vec<Part>, String> {
    let mut variables = BTreeMap::new();
    variables.insert("context".into(), DynValue::from_json(context));
    let mut state_map = BTreeMap::new();
    for (k, v) in state {
        state_map.insert(k.clone(), DynValue::from_json(v));
    }
    variables.insert("state".into(), DynValue::Map(state_map));
    variables.insert("settings".into(), DynValue::from_json(settings));

    let mut ctx = EvalContext { variables, base };
    let mut last_val = DynValue::Null;
    for stmt in ast.statements() {
        match stmt {
            Stmt::Let(name, expr) => {
                let v = ctx.eval_expr(expr)?;
                ctx.variables.insert(name.clone(), v.clone());
                last_val = v;
            }
            Stmt::Expr(expr) => {
                last_val = ctx.eval_expr(expr)?;
            }
        }
    }

    let mut parts = last_val.into_parts();
    images::validate_parts(&mut parts, base)?;
    Ok(parts)
}
