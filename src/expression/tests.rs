use std::{collections::BTreeMap, path::Path};

use serde_json::Value;

use super::eval::{evaluate, evaluate_parts};
use super::parser::compile;
use crate::images::{ImageFit, ImageShape, Part};

#[test]
fn evaluates_mixed_images_and_text() {
    let ast = compile(
        "image(settings.icon, #{ width: 3, fit: \"cover\", shape: \"circle\" }) + \" CPU \" + format_percent(context.data.percent)",
    )
    .unwrap();
    let parts = evaluate_parts(
        &ast,
        &serde_json::json!({"data": {"percent": 42}}),
        &BTreeMap::new(),
        &serde_json::json!({"icon": "icon.png"}),
        Path::new("/tmp/provider"),
    )
    .unwrap();
    assert!(
        matches!(&parts[0], Part::Image(spec) if spec.width == 3 && spec.fit == ImageFit::Cover
        && spec.shape == ImageShape::Circle && spec.src == Path::new("/tmp/provider/icon.png"))
    );
    assert_eq!(parts[1], Part::Text(" CPU ".into()));
    assert_eq!(parts[2], Part::Text("42%".into()));

    let default = compile("image(\"x.png\")").unwrap();
    let plain = evaluate_parts(
        &default,
        &Value::Null,
        &BTreeMap::new(),
        &Value::Null,
        Path::new("."),
    )
    .unwrap();
    assert!(matches!(&plain[0], Part::Image(spec) if spec.shape == ImageShape::Rect));

    let bad_shape = compile("image(\"x.png\", #{ shape: \"oval\" })").unwrap();
    assert!(
        evaluate_parts(
            &bad_shape,
            &Value::Null,
            &BTreeMap::new(),
            &Value::Null,
            Path::new(".")
        )
        .is_err()
    );

    let bad = compile("image(\"x.png\", #{ width: 0 })").unwrap();
    assert!(
        evaluate_parts(
            &bad,
            &Value::Null,
            &BTreeMap::new(),
            &Value::Null,
            Path::new(".")
        )
        .is_err()
    );
}

#[test]
fn evaluates_context_and_settings() {
    let ast =
        compile("settings.icon + format_percent(context.data.percent)").expect("valid expression");
    let context = serde_json::json!({"data": {"percent": 42}});
    let settings = serde_json::json!({"icon": "CPU "});
    assert_eq!(
        evaluate(&ast, &context, &BTreeMap::new(), &settings),
        Ok("CPU 42%".into())
    );
}

#[test]
fn evaluates_cpu_template_with_mutating_replace() {
    let ast = compile(
        "let text = settings.template; text.replace(\"{percent}\", format_percent(context.data.percent)); text",
    )
    .expect("valid CPU expression");
    let context = serde_json::json!({"data": {"percent": 42}});
    let settings = serde_json::json!({"template": "CPU {percent}"});

    assert_eq!(
        evaluate(&ast, &context, &BTreeMap::new(), &settings),
        Ok("CPU 42%".into())
    );
}

#[test]
fn rejects_io_and_time_functions() {
    let context = Value::Null;
    let settings = Value::Null;
    for source in ["print(\"not allowed\")", "debug(\"not allowed\")", "now()"] {
        let ast = compile(source).expect("valid expression syntax");
        assert!(
            evaluate(&ast, &context, &BTreeMap::new(), &settings).is_err(),
            "unexpectedly enabled restricted function {source:?}"
        );
    }
}

#[test]
fn formats_builtin_values() {
    let ast = compile(
        "strftime(context.data.timestamp, settings.format) + \" \" + format_memory(settings.template, context.data.used_kib, context.data.total_kib, context.data.available_kib, context.data.percent)",
    )
    .expect("valid expression");
    let context = serde_json::json!({
        "data": {
            "timestamp": 0,
            "used_kib": 8192000,
            "total_kib": 16384000,
            "available_kib": 8192000,
            "percent": 50
        }
    });
    let settings = serde_json::json!({
        "format": "%Y",
        "template": "MEM {used} / {total} ({percent})"
    });
    assert_eq!(
        evaluate(&ast, &context, &BTreeMap::new(), &settings),
        Ok("1970 MEM 7.8G / 15.6G (50%)".into())
    );
}

#[test]
fn evaluates_generic_template_render() {
    let ast = compile(
        r#"render(settings.template, #{ icon: "", percent: format_percent(context.data.percent) })"#,
    )
    .expect("valid render expression");
    let context = serde_json::json!({"data": {"percent": 42}});
    let settings = serde_json::json!({"template": "#cpu{ [{icon} {percent}](@cpu) }"});

    assert_eq!(
        evaluate(&ast, &context, &BTreeMap::new(), &settings),
        Ok("#cpu{ [ 42%](@cpu) }".into())
    );

    let ast_method =
        compile(r#"settings.template.render(#{ used: format_size(context.data.used_kib) })"#)
            .expect("valid render method expression");
    let mem_context = serde_json::json!({"data": {"used_kib": 8192000}});
    let mem_settings = serde_json::json!({"template": "#mem{ [ {used}] }"});
    assert_eq!(
        evaluate(&ast_method, &mem_context, &BTreeMap::new(), &mem_settings),
        Ok("#mem{ [ 7.8G] }".into())
    );

    let escaped =
        compile(r#"render("[{value}]", #{ value: "] [" })"#).expect("valid escaping expression");
    assert_eq!(
        evaluate(&escaped, &Value::Null, &BTreeMap::new(), &Value::Null),
        Ok("[\\] \\[]".into())
    );
}

#[test]
fn evaluates_functional_chaining_and_workspaces() {
    let source = r#"
        context.message.workspaces
            .filter(|w| !w.hidden)
            .map(|w| if w.active {
                `#ws:{w.id}{ [[{w.name}]](@accent) }`
            } else {
                `#ws:{w.id}{ [{w.name}] }`
            })
            .join("[ ]")
    "#;
    let ast = compile(source).expect("valid functional workspace expression");
    let context = serde_json::json!({
        "message": {
            "workspaces": [
                { "id": "1", "name": "1", "active": true, "hidden": false },
                { "id": "2", "name": "2", "active": false, "hidden": false },
                { "id": "3", "name": "3", "active": false, "hidden": true }
            ]
        }
    });
    let result = evaluate(&ast, &context, &BTreeMap::new(), &Value::Null).unwrap();
    assert_eq!(result, "#ws:1{ [[1]](@accent) }[ ]#ws:2{ [2] }");
}

#[test]
fn evaluates_ternary_and_template_literals() {
    let source = r#"
        let title = context.message.title;
        title != "" ? `[{title}](@window)` : "[Desktop]"
    "#;
    let ast = compile(source).expect("valid ternary expression");
    let context = serde_json::json!({
        "message": {
            "title": "Rust [1.95]"
        }
    });
    let result = evaluate(&ast, &context, &BTreeMap::new(), &Value::Null).unwrap();
    assert_eq!(result, "[Rust \\[1.95\\]](@window)");

    let empty_ctx = serde_json::json!({
        "message": {
            "title": ""
        }
    });
    let empty_res = evaluate(&ast, &empty_ctx, &BTreeMap::new(), &Value::Null).unwrap();
    assert_eq!(empty_res, "[Desktop]");
}

#[test]
fn test_window_provider_expression() {
    let source = r#"
        let title = context.message && context.message.title != "" ? context.message.title : "";
        title != ""
            ? (settings.template != ""
                ? settings.template.replace("{title}", title)
                : (settings.scope_style != ""
                    ? `#window(@${settings.scope_style}){ [${title}](@${settings.title_style}) }`
                    : `[${title}](@${settings.title_style})`))
            : settings.empty
    "#;
    let ast = compile(source).expect("valid window expression");
    let context = serde_json::json!({
        "message": { "title": "Firefox" }
    });
    let settings = serde_json::json!({
        "template": "",
        "scope_style": "pill",
        "title_style": "window",
        "empty": "#window(@pill){ [Desktop] }"
    });

    let result = evaluate(&ast, &context, &BTreeMap::new(), &settings).unwrap();
    assert_eq!(result, "#window(@pill){ [Firefox](@window) }");

    // Custom template
    let settings2 = serde_json::json!({
        "template": "#window(@pill){ [ ](@blue)[{title}](@subtext) }",
        "scope_style": "",
        "title_style": "window",
        "empty": "#window(@pill){ [Desktop] }"
    });
    let result2 = evaluate(&ast, &context, &BTreeMap::new(), &settings2).unwrap();
    assert_eq!(result2, "#window(@pill){ [ ](@blue)[Firefox](@subtext) }");
}

#[test]
fn evaluates_optional_chaining_and_nullish_coalescing() {
    // 1. Safe navigation on null/missing objects
    let ast1 =
        compile("context?.message?.title ?? \"fallback\"").expect("valid coalesce expression");
    let null_ctx = serde_json::json!({"message": null});
    assert_eq!(
        evaluate(&ast1, &null_ctx, &BTreeMap::new(), &serde_json::Value::Null).unwrap(),
        "fallback"
    );

    let empty_ctx = serde_json::json!({});
    assert_eq!(
        evaluate(
            &ast1,
            &empty_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "fallback"
    );

    // 2. Truthy/Falsy distinction: empty string, 0, and false must NOT be coalesced
    let ast_str = compile("context?.title ?? \"fallback\"").unwrap();
    let str_ctx = serde_json::json!({"title": ""});
    assert_eq!(
        evaluate(
            &ast_str,
            &str_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        ""
    );

    let ast_num = compile("context?.count ?? 100").unwrap();
    let num_ctx = serde_json::json!({"count": 0});
    assert_eq!(
        evaluate(
            &ast_num,
            &num_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "0"
    );

    let ast_bool = compile("context?.enabled ?? true").unwrap();
    let bool_ctx = serde_json::json!({"enabled": false});
    assert_eq!(
        evaluate(
            &ast_bool,
            &bool_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "false"
    );

    // 3. Present value is retained
    let valid_ctx = serde_json::json!({"message": {"title": "Terminal"}});
    assert_eq!(
        evaluate(
            &ast1,
            &valid_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "Terminal"
    );

    // 4. Optional indexing and method chaining
    let ast_idx = compile("context?.items?.[0] ?? \"none\"").unwrap();
    let list_ctx = serde_json::json!({"items": ["first", "second"]});
    assert_eq!(
        evaluate(
            &ast_idx,
            &list_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "first"
    );

    let no_list_ctx = serde_json::json!({"items": null});
    assert_eq!(
        evaluate(
            &ast_idx,
            &no_list_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "none"
    );

    // 5. Optional method call
    let ast_method = compile("context?.title?.replace(\"a\", \"o\") ?? \"default\"").unwrap();
    let meth_ctx = serde_json::json!({"title": "cat"});
    assert_eq!(
        evaluate(
            &ast_method,
            &meth_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "cot"
    );

    let no_meth_ctx = serde_json::json!({"title": null});
    assert_eq!(
        evaluate(
            &ast_method,
            &no_meth_ctx,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "default"
    );
}

#[test]
fn evaluates_modern_map_literals_and_in_operator() {
    // 1. Modern map literal with standard keys and shorthand
    let ast_map = compile(
        r#"
        let title = "Firefox";
        let count = 42;
        let m = { title, count, "extra-key": 100 };
        m.title + " " + m.count + " " + m["extra-key"]
        "#,
    )
    .unwrap();
    assert_eq!(
        evaluate(
            &ast_map,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "Firefox 42 100"
    );

    // 2. Map rendering with shorthand
    let ast_render = compile(
        r#"
        let time = "12:30";
        render("[{time}]", { time })
        "#,
    )
    .unwrap();
    assert_eq!(
        evaluate(
            &ast_render,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "[12:30]"
    );

    // 3. 'in' operator on lists
    let ast_in_list = compile(r#"1 in [1, 2, 3]"#).unwrap();
    assert_eq!(
        evaluate(
            &ast_in_list,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "true"
    );
    let ast_not_in_list = compile(r#"4 in [1, 2, 3]"#).unwrap();
    assert_eq!(
        evaluate(
            &ast_not_in_list,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "false"
    );
    let ast_explicit_not_in = compile(r#"4 not in [1, 2, 3]"#).unwrap();
    assert_eq!(
        evaluate(
            &ast_explicit_not_in,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "true"
    );

    // 4. 'in' operator on strings (substring check)
    let ast_in_str = compile(r#""bar" in "foobarbaz""#).unwrap();
    assert_eq!(
        evaluate(
            &ast_in_str,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "true"
    );
    let ast_not_in_str = compile(r#""xyz" not in "foobarbaz""#).unwrap();
    assert_eq!(
        evaluate(
            &ast_not_in_str,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "true"
    );

    // 5. 'in' operator on maps (key membership)
    let ast_in_map = compile(r#""title" in { title: "Cellbar", count: 1 }"#).unwrap();
    assert_eq!(
        evaluate(
            &ast_in_map,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "true"
    );
    let ast_not_in_map = compile(r#""unknown" not in { title: "Cellbar" }"#).unwrap();
    assert_eq!(
        evaluate(
            &ast_not_in_map,
            &serde_json::Value::Null,
            &BTreeMap::new(),
            &serde_json::Value::Null
        )
        .unwrap(),
        "true"
    );
}

#[test]
fn evaluates_list_and_string_methods() {
    let ast = compile(
        r#"
        let list = [1, 2, 3, 4, 5];
        let evens = list.filter(|x| x == 2 || x == 4);
        evens.join("-")
    "#,
    )
    .unwrap();
    let res = evaluate(&ast, &Value::Null, &BTreeMap::new(), &Value::Null).unwrap();
    assert_eq!(res, "2-4");

    let ast_str = compile(
        r#"
        let s = "  hello world  ";
        s.trim().to_uppercase()
    "#,
    )
    .unwrap();
    let res_str = evaluate(&ast_str, &Value::Null, &BTreeMap::new(), &Value::Null).unwrap();
    assert_eq!(res_str, "HELLO WORLD");

    let ast_idx = compile(
        r#"
        let list = ["a", "b", "c"];
        list[0] + list[-1]
    "#,
    )
    .unwrap();
    let res_idx = evaluate(&ast_idx, &Value::Null, &BTreeMap::new(), &Value::Null).unwrap();
    assert_eq!(res_idx, "ac");
}
