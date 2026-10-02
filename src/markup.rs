use std::path::{Path, PathBuf};

use crate::config::DisplayAlign;
use crate::images::{ImageAlign, ImageFit, ImageShape, ImageSpec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkupPart {
    Text(String),
    Image(ImageSpec),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkupSpan {
    pub part: MarkupPart,
    pub style: Option<String>,
    pub max_width: Option<usize>,
    pub min_width: Option<usize>,
    pub align: Option<DisplayAlign>,
    pub scope_style: Option<String>,
    pub root_scope_id: Option<usize>,
    pub root_scope_style: Option<String>,
    pub is_sub_scope: bool,
    pub target: Option<String>,
}

/// Parses a Cellbar Markup stream into structured spans.
///
/// Syntax rules:
/// - Interaction & style scopes: `#scope_id{ ... }`, `#scope_id(@style){ ... }`, or `#(@style){ ... }`
///   (supports nesting, inner scope takes precedence, inner nodes inherit outer style if unspecified)
/// - Text nodes: `[content]` or `[content](@style_name)`
/// - Image nodes: `![path](width [fit] [shape] [align] [fallback])`
/// - Escaping: `\[`, `\]`, `\#`, `\@`, `\\` are treated as literal characters.
/// - Whitespace outside nodes is formatting and is discarded.
/// - Any non-whitespace text outside a node is invalid.
pub fn parse_markup(input: &str, base_dir: &Path) -> Result<Vec<MarkupSpan>, String> {
    #[derive(Clone, Debug)]
    struct ScopeContext {
        id: usize,
        target: Option<String>,
        style: Option<String>,
    }

    let mut next_scope_id = 1usize;

    let mut spans = Vec::new();
    let mut scope_stack: Vec<ScopeContext> = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut literal_buf = String::new();

    let flush_literal = |buf: &mut String| -> Result<(), String> {
        if buf.trim().is_empty() {
            buf.clear();
            Ok(())
        } else {
            Err(format!("markup contains text outside a node: {:?}", buf))
        }
    };

    while i < len {
        // 1. Escaped character
        if chars[i] == '\\' && i + 1 < len {
            let next = chars[i + 1];
            if next == '['
                || next == ']'
                || next == '('
                || next == ')'
                || next == '#'
                || next == '@'
                || next == '{'
                || next == '}'
                || next == '\\'
            {
                literal_buf.push(next);
                i += 2;
                continue;
            }
        }

        // 2. Image node: `![path](options)`
        if chars[i] == '!' && i + 1 < len && chars[i + 1] == '[' {
            flush_literal(&mut literal_buf)?;
            i += 2; // skip `![`
            let mut path_str = String::new();
            let mut closed = false;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    path_str.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if chars[i] == ']' {
                    i += 1;
                    closed = true;
                    break;
                }
                path_str.push(chars[i]);
                i += 1;
            }
            if !closed {
                return Err("markup contains an unclosed image node".into());
            }

            // Check for options `(...)`
            let mut width = 2usize;
            let mut fit = ImageFit::default();
            let mut shape = ImageShape::default();
            let mut align = ImageAlign::default();
            let mut fallback = "*".to_owned();

            if i < len && chars[i] == '(' {
                i += 1; // skip `(`
                let mut opts_str = String::new();
                while i < len && chars[i] != ')' {
                    opts_str.push(chars[i]);
                    i += 1;
                }
                if i < len && chars[i] == ')' {
                    i += 1; // skip `)`
                }
                for token in opts_str.split_whitespace() {
                    if let Ok(w) = token.parse::<usize>() {
                        width = w;
                    } else {
                        match token {
                            "cover" => fit = ImageFit::Cover,
                            "contain" => fit = ImageFit::Contain,
                            "stretch" | "fill" => fit = ImageFit::Stretch,
                            "textmatch" => fit = ImageFit::TextMatch,
                            "scaledown" => fit = ImageFit::ScaleDown,
                            "circle" => shape = ImageShape::Circle,
                            "rect" => shape = ImageShape::Rect,
                            "left" => align = ImageAlign::Left,
                            "center" => align = ImageAlign::Center,
                            "right" => align = ImageAlign::Right,
                            fb if fb.starts_with("fallback=") => {
                                fallback = fb.trim_start_matches("fallback=").to_owned();
                            }
                            _ => {}
                        }
                    }
                }
            }

            let mut spec = ImageSpec {
                src: PathBuf::from(path_str),
                width,
                fit,
                shape,
                align,
                fallback,
            };
            let _ = spec.resolve(base_dir);

            let active_target = scope_stack.iter().rev().find_map(|s| s.target.clone());
            let active_style = scope_stack.iter().rev().find_map(|s| s.style.clone());
            let outer_style = scope_stack.iter().find_map(|s| s.style.clone());
            let root_scope_id = scope_stack.first().map(|s| s.id);
            let root_scope_style = scope_stack.first().and_then(|s| s.style.clone());
            let is_sub_scope = !scope_stack.is_empty() && scope_stack.len() >= 2;

            spans.push(MarkupSpan {
                part: MarkupPart::Image(spec),
                style: active_style,
                max_width: None,
                min_width: None,
                align: None,
                scope_style: outer_style,
                root_scope_id,
                root_scope_style,
                is_sub_scope,
                target: active_target,
            });
            continue;
        }

        // 3. Text node: `[content]` or `[content](@style options...)`
        if chars[i] == '[' {
            flush_literal(&mut literal_buf)?;
            i += 1; // skip `[`
            let mut text_content = String::new();
            let mut bracket_depth = 0;
            let mut closed = false;
            while i < len {
                if chars[i] == '\\' && i + 1 < len {
                    text_content.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                if chars[i] == '[' {
                    bracket_depth += 1;
                    text_content.push('[');
                    i += 1;
                    continue;
                }
                if chars[i] == ']' {
                    if bracket_depth > 0 {
                        bracket_depth -= 1;
                        text_content.push(']');
                        i += 1;
                        continue;
                    } else {
                        i += 1; // end of text node
                        closed = true;
                        break;
                    }
                }
                text_content.push(chars[i]);
                i += 1;
            }

            if !closed {
                return Err("markup contains an unclosed text node".into());
            }

            // Check for options `(...)`
            let mut style_name = None;
            let mut max_width = None;
            let mut min_width = None;
            let mut align = None;

            if i < len && chars[i] == '(' {
                let start_paren = i;
                i += 1; // skip `(`
                let mut opts_str = String::new();
                let mut paren_closed = false;
                while i < len {
                    if chars[i] == ')' {
                        i += 1;
                        paren_closed = true;
                        break;
                    }
                    opts_str.push(chars[i]);
                    i += 1;
                }
                if paren_closed {
                    let mut valid = true;
                    let mut has_any = false;
                    for token in opts_str.split_whitespace() {
                        if let Some(st) = token.strip_prefix('@') {
                            if !st.is_empty() {
                                style_name = Some(st.to_string());
                                has_any = true;
                            } else {
                                valid = false;
                                break;
                            }
                        } else if let Some(mw) = token
                            .strip_prefix("max_width=")
                            .or_else(|| token.strip_prefix("max="))
                        {
                            if let Ok(w) = mw.parse::<usize>() {
                                max_width = Some(w);
                                has_any = true;
                            } else {
                                valid = false;
                                break;
                            }
                        } else if let Some(mw) = token
                            .strip_prefix("min_width=")
                            .or_else(|| token.strip_prefix("min="))
                        {
                            if let Ok(w) = mw.parse::<usize>() {
                                min_width = Some(w);
                                has_any = true;
                            } else {
                                valid = false;
                                break;
                            }
                        } else if let Some(al) = token.strip_prefix("align=") {
                            match al {
                                "left" => {
                                    align = Some(DisplayAlign::Left);
                                    has_any = true;
                                }
                                "center" => {
                                    align = Some(DisplayAlign::Center);
                                    has_any = true;
                                }
                                "right" => {
                                    align = Some(DisplayAlign::Right);
                                    has_any = true;
                                }
                                _ => {
                                    valid = false;
                                    break;
                                }
                            }
                        } else {
                            match token {
                                "left" => {
                                    align = Some(DisplayAlign::Left);
                                    has_any = true;
                                }
                                "center" => {
                                    align = Some(DisplayAlign::Center);
                                    has_any = true;
                                }
                                "right" => {
                                    align = Some(DisplayAlign::Right);
                                    has_any = true;
                                }
                                _ => {
                                    valid = false;
                                    break;
                                }
                            }
                        }
                    }
                    if !valid || !has_any {
                        style_name = None;
                        max_width = None;
                        min_width = None;
                        align = None;
                        i = start_paren;
                    }
                } else {
                    i = start_paren;
                }
            }

            let active_target = scope_stack.iter().rev().find_map(|s| s.target.clone());
            let active_style = scope_stack.iter().rev().find_map(|s| s.style.clone());
            let outer_style = scope_stack.iter().find_map(|s| s.style.clone());
            let root_scope_id = scope_stack.first().map(|s| s.id);
            let root_scope_style = scope_stack.first().and_then(|s| s.style.clone());
            let is_sub_scope =
                !scope_stack.is_empty() && (style_name.is_some() || scope_stack.len() >= 2);

            spans.push(MarkupSpan {
                part: MarkupPart::Text(text_content),
                style: style_name.or(active_style),
                max_width,
                min_width,
                align,
                scope_style: outer_style,
                root_scope_id,
                root_scope_style,
                is_sub_scope,
                target: active_target,
            });
            continue;
        }

        // 4. Scope start: `#identifier{` or `#identifier(@style){` or `#(@style){`
        if chars[i] == '#' {
            let start_pos = i;
            let mut j = i + 1;
            let mut id = String::new();
            while j < len
                && (chars[j].is_ascii_alphanumeric()
                    || chars[j] == '_'
                    || chars[j] == '-'
                    || chars[j] == ':'
                    || chars[j] == '*')
            {
                id.push(chars[j]);
                j += 1;
            }

            let mut style_name = None;
            // Check for optional `(@style_name)`
            let mut k = j;
            if k < len && chars[k] == '(' {
                k += 1;
                while k < len && chars[k].is_whitespace() {
                    k += 1;
                }
                if k < len && chars[k] == '@' {
                    k += 1;
                    let mut s = String::new();
                    while k < len
                        && (chars[k].is_ascii_alphanumeric() || chars[k] == '_' || chars[k] == '-')
                    {
                        s.push(chars[k]);
                        k += 1;
                    }
                    while k < len && chars[k].is_whitespace() {
                        k += 1;
                    }
                    if k < len && chars[k] == ')' {
                        k += 1;
                        if !s.is_empty() {
                            style_name = Some(s);
                            j = k;
                        }
                    }
                }
            }

            while j < len && chars[j].is_whitespace() {
                j += 1;
            }

            if (!id.is_empty() || style_name.is_some()) && j < len && chars[j] == '{' {
                flush_literal(&mut literal_buf)?;
                let target = if !id.is_empty() { Some(id) } else { None };
                let scope_id = next_scope_id;
                next_scope_id += 1;
                scope_stack.push(ScopeContext {
                    id: scope_id,
                    target,
                    style: style_name,
                });
                i = j + 1; // skip through `{`
                continue;
            } else {
                literal_buf.push('#');
                i = start_pos + 1;
                continue;
            }
        }

        // 5. Scope end: `}`
        if chars[i] == '}' {
            if !scope_stack.is_empty() {
                flush_literal(&mut literal_buf)?;
                scope_stack.pop();
                i += 1;
                continue;
            } else {
                literal_buf.push('}');
                i += 1;
                continue;
            }
        }

        // 6. Normal character
        literal_buf.push(chars[i]);
        i += 1;
    }

    flush_literal(&mut literal_buf)?;
    if !scope_stack.is_empty() {
        return Err("markup contains an unclosed interaction scope".into());
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_node() {
        let spans = parse_markup("[hello world]", Path::new(".")).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].part, MarkupPart::Text("hello world".into()));
        assert_eq!(spans[0].style, None);
        assert_eq!(spans[0].scope_style, None);
        assert_eq!(spans[0].target, None);
    }

    #[test]
    fn test_parse_styled_node() {
        let spans = parse_markup("[ 2 ](@accent)", Path::new(".")).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].part, MarkupPart::Text(" 2 ".into()));
        assert_eq!(spans[0].style, Some("accent".into()));
        assert_eq!(spans[0].scope_style, None);
        assert_eq!(spans[0].target, None);
    }

    #[test]
    fn test_parse_scope_block() {
        let spans = parse_markup("#btn{ [Click Me](@accent) }", Path::new(".")).unwrap();
        // `[Click Me](@accent)` is under `#btn`, space around it also shares target
        assert_eq!(spans[0].target, Some("btn".into()));
        assert_eq!(spans[0].style, Some("accent".into()));
        assert_eq!(spans[0].scope_style, None);
        assert_eq!(spans[0].part, MarkupPart::Text("Click Me".into()));
    }

    #[test]
    fn test_parse_scope_with_style_and_inheritance() {
        let input = "#player(@pill){ [⏮] [ ⏸ ](@active) [⏭] }";
        let spans = parse_markup(input, Path::new(".")).unwrap();
        assert_eq!(spans.len(), 3);

        // [⏮] inherits scope style "pill"
        assert_eq!(spans[0].part, MarkupPart::Text("⏮".into()));
        assert_eq!(spans[0].target, Some("player".into()));
        assert_eq!(spans[0].style, Some("pill".into()));
        assert_eq!(spans[0].scope_style, Some("pill".into()));

        // [ ⏸ ] has explicit inner style "active", while outer scope_style is "pill"
        assert_eq!(spans[1].part, MarkupPart::Text(" ⏸ ".into()));
        assert_eq!(spans[1].target, Some("player".into()));
        assert_eq!(spans[1].style, Some("active".into()));
        assert_eq!(spans[1].scope_style, Some("pill".into()));

        // [⏭] inherits scope style "pill"
        assert_eq!(spans[2].part, MarkupPart::Text("⏭".into()));
        assert_eq!(spans[2].target, Some("player".into()));
        assert_eq!(spans[2].style, Some("pill".into()));
        assert_eq!(spans[2].scope_style, Some("pill".into()));
    }

    #[test]
    fn test_parse_anonymous_style_scope_and_nesting() {
        let input = "#(@capsule){ [Outer] #btn(@inner){ [Inner] } }";
        let spans = parse_markup(input, Path::new(".")).unwrap();
        assert_eq!(spans.len(), 2);

        // [Outer] has no target, style "capsule", scope_style "capsule"
        assert_eq!(spans[0].part, MarkupPart::Text("Outer".into()));
        assert_eq!(spans[0].target, None);
        assert_eq!(spans[0].style, Some("capsule".into()));
        assert_eq!(spans[0].scope_style, Some("capsule".into()));

        // [Inner] has target "btn", style "inner", root scope_style "capsule"
        assert_eq!(spans[1].part, MarkupPart::Text("Inner".into()));
        assert_eq!(spans[1].target, Some("btn".into()));
        assert_eq!(spans[1].style, Some("inner".into()));
        assert_eq!(spans[1].scope_style, Some("capsule".into()));
    }

    #[test]
    fn test_parse_nested_scopes() {
        let input = "#player{ #prev{ [⏮] }  #play{ [ ⏸ ](@accent) }  [Queen - Song](@muted) }";
        let spans = parse_markup(input, Path::new(".")).unwrap();
        // Whitespace outside nodes is discarded; only 3 explicit nodes are produced
        assert_eq!(spans.len(), 3);

        // prev: inner scope takes precedence
        assert_eq!(spans[0].part, MarkupPart::Text("⏮".into()));
        assert_eq!(spans[0].target, Some("prev".into()));

        // play
        assert_eq!(spans[1].part, MarkupPart::Text(" ⏸ ".into()));
        assert_eq!(spans[1].target, Some("play".into()));
        assert_eq!(spans[1].style, Some("accent".into()));

        // song name: inherits outer scope
        assert_eq!(spans[2].part, MarkupPart::Text("Queen - Song".into()));
        assert_eq!(spans[2].target, Some("player".into()));
        assert_eq!(spans[2].style, Some("muted".into()));
    }

    #[test]
    fn test_parse_workspaces_example() {
        // Explicit [ ] nodes define visible spacing; formatting whitespace outside is discarded
        let input = "#ws:1{ [ 1 ] } [ ] #ws:2{ [ 2 ](@accent) } [ ] #ws:3{ [ 3 ](@warning) }";
        let spans = parse_markup(input, Path::new(".")).unwrap();
        assert_eq!(spans.len(), 5);

        assert_eq!(spans[0].target, Some("ws:1".into()));
        assert_eq!(spans[0].part, MarkupPart::Text(" 1 ".into()));
        assert_eq!(spans[0].style, None);

        // Explicit [ ] spacer node: target is None, physically isolated without interaction target
        assert_eq!(spans[1].part, MarkupPart::Text(" ".into()));
        assert_eq!(spans[1].target, None);

        assert_eq!(spans[2].target, Some("ws:2".into()));
        assert_eq!(spans[2].part, MarkupPart::Text(" 2 ".into()));
        assert_eq!(spans[2].style, Some("accent".into()));

        assert_eq!(spans[3].part, MarkupPart::Text(" ".into()));
        assert_eq!(spans[3].target, None);

        assert_eq!(spans[4].target, Some("ws:3".into()));
        assert_eq!(spans[4].part, MarkupPart::Text(" 3 ".into()));
        assert_eq!(spans[4].style, Some("warning".into()));

        let active = parse_markup("#ws:2{ [[2]](@accent) }", Path::new(".")).unwrap();
        assert_eq!(active[0].part, MarkupPart::Text("[2]".into()));
        assert_eq!(active[0].target, Some("ws:2".into()));
        assert_eq!(active[0].style, Some("accent".into()));
    }

    #[test]
    fn test_parse_image_node() {
        let input = "#cover{ ![cover.png](3 circle contain) }";
        let spans = parse_markup(input, Path::new("/tmp")).unwrap();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].target, Some("cover".into()));
        match &spans[0].part {
            MarkupPart::Image(spec) => {
                assert_eq!(spec.src, PathBuf::from("/tmp/cover.png"));
                assert_eq!(spec.width, 3);
                assert_eq!(spec.shape, ImageShape::Circle);
                assert_eq!(spec.fit, ImageFit::Contain);
            }
            _ => panic!("expected image"),
        }
    }

    #[test]
    fn test_parse_balanced_brackets_in_content() {
        let input = "[Track [Remix] (Live)](#player)";
        // Non-style trailing syntax is invalid outside an explicit node.
        assert!(parse_markup(input, Path::new(".")).is_err());
    }

    #[test]
    fn test_parse_text_node_attributes() {
        let input = "[title](@subtext max_width=30 min_width=10 align=center) [short](max=15 right) [plain](@bold)";
        let spans = parse_markup(input, Path::new(".")).unwrap();
        assert_eq!(spans.len(), 3);

        assert_eq!(spans[0].part, MarkupPart::Text("title".into()));
        assert_eq!(spans[0].style, Some("subtext".into()));
        assert_eq!(spans[0].max_width, Some(30));
        assert_eq!(spans[0].min_width, Some(10));
        assert_eq!(spans[0].align, Some(DisplayAlign::Center));

        assert_eq!(spans[1].part, MarkupPart::Text("short".into()));
        assert_eq!(spans[1].style, None);
        assert_eq!(spans[1].max_width, Some(15));
        assert_eq!(spans[1].min_width, None);
        assert_eq!(spans[1].align, Some(DisplayAlign::Right));

        assert_eq!(spans[2].part, MarkupPart::Text("plain".into()));
        assert_eq!(spans[2].style, Some("bold".into()));
        assert_eq!(spans[2].max_width, None);
        assert_eq!(spans[2].min_width, None);
        assert_eq!(spans[2].align, None);
    }

    #[test]
    fn test_escaped_brackets() {
        let input = r#"\[literal\]"#;
        assert!(parse_markup(input, Path::new(".")).is_err());
    }
}
