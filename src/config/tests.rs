use super::*;

const BASE: &str = r##"
show = ["main"]

[bar.main]
theme = "theme.toml"
left = ["[termway]"]
"##;

    #[test]
    fn parses_layout_and_settings() {
        let config = Config::parse(BASE).expect("valid config");
        assert_eq!(config.bar.left, ["[termway]"]);
    }

    #[test]
    fn allows_inline_literals_and_multiple_occurrences() {
        let toml = r##"
show = ["main"]

[bar.main]
theme = "theme.toml"
left = ["[termway]", "[ | ]", "[status]", "[  ]"]
right = ["[ | ]"]
"##;
        let config = Config::parse(toml).expect("inline literals and duplicates allowed");
        assert_eq!(config.bar.left, ["[termway]", "[ | ]", "[status]", "[  ]"]);
        assert_eq!(config.bar.right, ["[ | ]"]);
        assert!(config.has_literals());
    }

    #[test]
    fn rejects_undefined_display_id_but_allows_literals() {
        let toml = r##"
show = ["main"]

[bar.main]
theme = "theme.toml"
left = ["[termway]", "missing"]
"##;
        let error = Config::parse(toml).expect_err("undefined display ID must fail");
        assert!(error.to_string().contains("unknown provider \"missing\""));
    }

    #[test]
    fn parses_explicit_bar_height_and_validates() {
        let config = Config::parse(&BASE.replace("[bar.main]", "[bar.main]\nheight = 32"))
            .expect("explicit height must succeed");
        assert_eq!(config.bar_definitions["main"].height, Some(32));

        let error = Config::parse(&BASE.replace("[bar.main]", "[bar.main]\nheight = 0"))
            .expect_err("height = 0 must fail");
        assert!(error.to_string().contains("height must be greater than zero"));

        let error = Config::parse(&BASE.replace("[bar.main]", "[bar.main]\nheight = 600"))
            .expect_err("height > 512 must fail");
        assert!(error.to_string().contains("height must be at most 512"));
    }

    #[test]
    fn rejects_unknown_main_fields() {
        let error = Config::parse(&BASE.replace("[bar.main]", "[bar.main]\noutput = \"*\""))
            .expect_err("unknown output must fail");
        assert!(error.to_string().contains("unknown field `output`"));
        let error = Config::parse(&format!("unknown_scalar = 123\n{BASE}"))
            .expect_err("unknown top-level scalar must fail");
        assert!(error.to_string().contains("unknown field `unknown_scalar`"));
        let error = Config::parse(&format!("[component.clock]\n{BASE}"))
            .expect_err("component table in config must fail");
        assert!(error.to_string().contains("components are configured directly"));
        let error = Config::parse(&format!("[display.label]\n{BASE}"))
            .expect_err("display table in config must fail");
        assert!(error.to_string().contains("[display] is removed"));
        let error = Config::parse(&BASE.replace("[bar.main]", "[bar.main]\nbackground = \"#101820\""))
            .expect_err("visual fields must belong to the theme file");
        assert!(error.to_string().contains("unknown field `background`"));
        let error = Config::parse(&BASE.replace("[bar.main]", "[font]\n[bar.main]"))
            .expect_err("font must belong to the theme file");
        assert!(error.to_string().contains("unknown field `font`"));
    }

    #[test]
    fn substitutes_settings_and_context() {
        let provider = ProviderConfig {
            program: Some("script".into()),
            expression: None,
            args: vec!["${setting.icon}".into(), "${context}".into()],
            timeout: None,
            triggers: vec![],
            on_activate: true,
            every: None,
            debounce: DEFAULT_EVENT_DEBOUNCE,
            settings: BTreeMap::from([("icon".into(), toml::Value::String("x".into()))]),
            actions: BTreeMap::new(),
        };
        let command = provider
            .command_for(&toml::Table::new(), true)
            .expect("command");
        assert_eq!(command, ["script", "x", "${context}"]);
    }

    #[test]
    fn normalizes_source_specific_triggers() {
        let trigger = TriggerConfig {
            wayland: vec![WaylandTrigger {
                id: "active-window".into(),
                event: "toplevel".into(),
            }],
            ..Default::default()
        }
        .normalize()
        .pop()
        .expect("trigger");
        assert_eq!(trigger.kind, EventSourceKind::Wayland);
        assert_eq!(trigger.event, "toplevel");
    }

    #[test]
    fn loads_referenced_provider_and_source() {
        let root = env::temp_dir().join(format!("cellbar-config-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("components/example")).expect("component directory");
        fs::create_dir_all(root.join("sources/events")).expect("source directory");
        fs::write(
            root.join("config.toml"),
            r#"
[bar.main]
theme = "theme.toml"
left = ["example"]
"#,
        )
        .expect("main config");
        fs::write(
            root.join("theme.toml"),
            r##"
[font]
families = ["monospace"]
size = 12.0

[surface]
background = "#181a1f"

[text]
foreground = "#d8dee9"

"##,
        )
        .expect("theme config");
        fs::write(
            root.join("components/example/manifest.toml"),
            r#"
[provider]
expression = "context.event"
[triggers]
[[triggers.on_source]]
id = "changed"
source = "events"
event = "changed"
"#,
        )
        .expect("provider manifest");
        fs::write(
            root.join("sources/events/manifest.toml"),
            r#"
[source]
program = "events"
restart_after = "1s"
[[emits]]
event = "changed"
"#,
        )
        .expect("source manifest");

        let config = Config::load(root.join("config.toml")).expect("resolved configuration");
        assert!(config.providers.contains_key("example"));
        assert!(config.sources.contains_key("events"));
        assert_eq!(config.theme.background, "#181a1f".parse().unwrap());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unused_display_does_not_error_and_does_not_load_provider() {
        let root = env::temp_dir().join(format!("cellbar-unused-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("components/used")).expect("provider directory");
        fs::create_dir_all(root.join("components/nonexistent_provider")).expect("unused directory");
        fs::write(
            root.join("config.toml"),
            r#"
[bar.main]
theme = "theme.toml"
left = ["used"]
"#,
        )
        .expect("main config");
        fs::write(
            root.join("theme.toml"),
            r##"
[font]
families = ["monospace"]
size = 12.0

[surface]
background = "#181a1f"

[text]
foreground = "#d8dee9"
"##,
        )
        .expect("theme config");
        fs::write(
            root.join("components/used/manifest.toml"),
            r#"
[provider]
expression = "context.event"
[triggers]
on_activate = true
"#,
        )
        .expect("provider manifest");

        let config = Config::load(root.join("config.toml")).expect("configuration should succeed");
        assert!(config.providers.contains_key("used"));
        assert!(!config.providers.contains_key("nonexistent_provider"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn resolves_relative_action_paths_in_provider_manifest() {
        let root = env::temp_dir().join(format!("cellbar-action-path-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let provider_dir = root.join("components/tray");
        fs::create_dir_all(&provider_dir).expect("provider directory");
        fs::write(
            root.join("config.toml"),
            r#"
[bar.main]
theme = "theme.toml"
right = ["tray"]
"#,
        )
        .expect("main config");
        fs::write(
            root.join("theme.toml"),
            r##"
[font]
families = ["monospace"]
size = 12.0

[surface]
background = "#181a1f"

[text]
foreground = "#d8dee9"
"##,
        )
        .expect("theme config");
        fs::write(
            provider_dir.join("manifest.toml"),
            r#"
[provider]
program = "./cellbar-tray"

[triggers]
on_activate = true

[actions]
"click:tray:*" = { command = "./cellbar-tray", args = ["--action", "activate", "${target}"] }
"right_click:tray:*" = "./helper.sh context ${target}"
"middle_click:tray:*" = { command = "echo", args = ["${provider_dir}"] }
"#,
        )
        .expect("provider manifest");

        let config = Config::load(root.join("config.toml")).expect("configuration should succeed");
        let tray = config.providers.get("tray").expect("tray provider loaded");

        let click = tray.actions.get("click:tray:*").expect("click action");
        assert_eq!(
            click,
            &ActionSpec::Full(ActionConfig {
                command: provider_dir.join("./cellbar-tray").to_string_lossy().into_owned(),
                args: vec!["--action".into(), "activate".into(), "${target}".into()],
                refresh: None,
                debounce: None,
            })
        );

        let right_click = tray.actions.get("right_click:tray:*").expect("right_click action");
        assert_eq!(
            right_click,
            &ActionSpec::Command(format!(
                "{} context ${{target}}",
                provider_dir.join("./helper.sh").to_string_lossy()
            ))
        );

        let middle_click = tray.actions.get("middle_click:tray:*").expect("middle_click action");
        assert_eq!(
            middle_click,
            &ActionSpec::Full(ActionConfig {
                command: "echo".into(),
                args: vec![provider_dir.to_string_lossy().into_owned()],
                refresh: None,
                debounce: None,
            })
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn resolves_named_theme_styles() {
        let root = env::temp_dir().join(format!("cellbar-theme-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("config directory");
        fs::write(
            root.join("config.toml"),
            r#"
[bar.main]
theme = "theme.toml"
margin = { top = 8, right = 12, bottom = 6, left = 12 }
left = ["[termway](@accent)"]
"#,
        )
        .expect("main config");
        fs::write(
            root.join("theme.toml"),
            r##"
[font]
families = ["monospace"]
size = 13.0
cell_width_adjust = 2

[surface]
background = "base"
padding = { horizontal = 12, vertical = 4 }
radius = 6

[text]
foreground = "text"

[colors]
base = "#101820"
text = "#D8DEE9"
accent = "#88C0D0"


[styles.accent]
foreground = "accent"
background = "base"
bold = true
"##,
        )
        .expect("theme config");

        let config = Config::load(root.join("config.toml")).expect("resolved theme");
        let accent = config
            .theme
            .style_for(Some("accent"))
            .expect("accent style");
        let accent_color: Rgba = "#88C0D0".parse().expect("accent color");
        assert_eq!(accent.foreground, accent_color);
        assert_eq!(accent.background, Some("#101820".parse().unwrap()));
        assert!(accent.bold);
        assert_eq!(config.font_style_requirements(), (true, false));
        assert_eq!(
            config.theme.padding,
            SurfacePadding {
                horizontal: 12,
                vertical: 4
            }
        );
        assert_eq!(config.theme.bar_height(), 25);
        assert_eq!(config.theme.font.cell_width_adjust, 2);
        assert_eq!(config.theme.radius, 6);
        assert_eq!(
            config.bar.margin,
            BarMargins {
                top: 8,
                right: 12,
                bottom: 6,
                left: 12,
            }
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_old_surface_spacing_fields() {
        for field in ["padding = 8", "gap = 8"] {
            let theme = format!(
                "[font]\n[surface]\nbackground = \"#101820\"\n{field}\n[text]\nforeground = \"#ffffff\"\n"
            );
            assert!(toml::from_str::<ThemeConfig>(&theme).is_err(), "{field}");
        }
    }

    #[test]
    fn validates_derived_height_and_width_adjustment() {
        let mut theme = Theme::default();
        assert_eq!(theme.bar_height(), 31);
        theme.font.cell_width_adjust = -256;
        theme.padding.vertical = 128;
        assert_eq!(theme.bar_height(), 271);
        let mut errors = Vec::new();
        theme.validate(&mut errors);
        assert!(errors.is_empty(), "{errors:?}");
        theme.font.cell_width_adjust = 257;
        theme.font.size = 256.0;
        theme.validate(&mut errors);
        assert!(errors.iter().any(|e| e.contains("cell_width_adjust")));
        assert!(errors.iter().any(|e| e.contains("derived bar height")));
        theme.padding.vertical = 129;
        theme.validate(&mut errors);
        assert!(errors.iter().any(|e| e.contains("padding.vertical")));
    }

    #[test]
    fn line_height_scales_the_cell_and_the_bar() {
        assert_eq!(cell_height(12.0, DEFAULT_LINE_HEIGHT), 15);
        assert_eq!(cell_height(12.0, 1.5), 18);
        assert_eq!(cell_height(1.0, 0.5), 1);
        let mut theme = Theme::default();
        assert_eq!(theme.bar_height(), 31);
        theme.font.line_height = 1.5;
        assert_eq!(theme.bar_height(), 34);
    }

    #[test]
    fn parses_line_height_and_defaults_it() {
        let with = r##"
[font]
line_height = 1.4
[surface]
background = "#000000"
[text]
foreground = "#ffffff"
"##;
        let config: ThemeConfig = toml::from_str(with).expect("line_height parses");
        assert_eq!(config.resolve().expect("valid").font.line_height, 1.4);

        let without = r##"
[font]
[surface]
background = "#000000"
[text]
foreground = "#ffffff"
"##;
        let config: ThemeConfig = toml::from_str(without).expect("line_height defaults");
        assert_eq!(
            config.resolve().expect("valid").font.line_height,
            DEFAULT_LINE_HEIGHT
        );
    }

    #[test]
    fn rejects_out_of_range_line_height() {
        for value in ["0.4", "4.1", "nan"] {
            let theme = format!(
                "[font]\nline_height = {value}\n[surface]\nbackground = \"#000000\"\n[text]\nforeground = \"#ffffff\"\n"
            );
            let config: ThemeConfig = toml::from_str(&theme).expect("parses");
            let errors = config.resolve().expect_err("must reject");
            assert!(
                errors.iter().any(|error| error.contains("line_height")),
                "{value}: {errors:?}"
            );
        }
    }

    #[test]
    fn resolves_base_text_style() {
        let theme = r##"
[font]
[surface]
background = "#101820"
[colors]
text = "#D8DEE9"
highlight = "#313244"
[text]
foreground = "text"
background = "highlight"
bold = true
"##;
        let config: ThemeConfig = toml::from_str(theme).expect("parses");
        let theme = config.resolve().expect("valid base text style");
        assert_eq!(theme.default_style.foreground, "#D8DEE9".parse().unwrap());
        assert_eq!(
            theme.default_style.background,
            Some("#313244".parse().unwrap())
        );
        assert!(theme.default_style.bold);
    }

    #[test]
    fn rejects_reserved_default_style() {
        let theme = r##"
[font]
[surface]
background = "#000000"
[text]
foreground = "#ffffff"
[styles.default]
foreground = "#ffffff"
"##;
        let config: ThemeConfig = toml::from_str(theme).expect("parses");
        let errors = config.resolve().expect_err("default is reserved");
        assert!(
            errors.iter().any(|error| error.contains("reserved")),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_invalid_static_text_markup() {
        let root = env::temp_dir().join(format!("cellbar-unknown-style-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("config directory");
        fs::write(
            root.join("config.toml"),
            r#"
[bar.main]
theme = "theme.toml"
left = ["[unterminated"]
"#,
        )
        .expect("main config");
        fs::write(
            root.join("theme.toml"),
            r##"
[font]
[surface]
background = "#101820"
[text]
foreground = "#D8DEE9"
"##,
        )
        .expect("theme config");

        let error = Config::load(root.join("config.toml")).expect_err("invalid text markup");
        assert!(error.to_string().contains("valid markup") || error.to_string().contains("unknown provider"), "{error}");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rejects_missing_theme_file() {
        let root = env::temp_dir().join(format!("cellbar-missing-theme-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("config directory");
        fs::write(
            root.join("config.toml"),
            "[bar.main]\ntheme = \"missing.toml\"\n",
        )
        .expect("main config");

        let error = Config::load(root.join("config.toml")).expect_err("missing theme");
        assert!(error.to_string().contains("cannot read theme"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn validates_expression_provider_settings() {
        let provider = ProviderConfig {
            program: None,
            expression: Some("settings.icon + context.event".into()),
            args: vec![],
            timeout: None,
            triggers: vec![],
            on_activate: true,
            every: None,
            debounce: DEFAULT_EVENT_DEBOUNCE,
            settings: BTreeMap::from([("icon".into(), toml::Value::String("x".into()))]),
            actions: BTreeMap::new(),
        };
        let mut errors = Vec::new();
        provider.validate_definition(&mut errors, "example", &BTreeMap::new());
        assert!(
            errors.is_empty(),
            "unexpected validation errors: {errors:?}"
        );
        let settings = provider
            .settings_for(&toml::Table::new())
            .expect("settings");
        assert_eq!(settings["icon"].as_str(), Some("x"));
    }

    #[test]
    fn parses_font_families_with_inline_mappings() {
        let toml_str = r##"
[font]
families = [
    { "U+E000-U+F8FF" = "Symbols Nerd Font" },
    "JetBrains Mono",
    "Noto Sans CJK SC",
    { "󰀝󰂄" = "Custom Icon Font" }
]
size = 14.0
[surface]
background = "#1D1E1E"
[text]
foreground = "#C0C3C5"
"##;
        let config: ThemeConfig = toml::from_str(toml_str).expect("valid theme config");
        assert_eq!(config.font.families.len(), 4);
        match &config.font.families[0] {
            FontFamilyEntry::Mapping(map) => {
                assert_eq!(
                    map.get("U+E000-U+F8FF").map(String::as_str),
                    Some("Symbols Nerd Font")
                );
            }
            _ => panic!("expected mapping"),
        }
        match &config.font.families[1] {
            FontFamilyEntry::Family(name) => assert_eq!(name, "JetBrains Mono"),
            _ => panic!("expected family"),
        }
        let font_names = config.font.font_names();
        assert!(font_names.contains(&"Symbols Nerd Font".to_owned()));
        assert!(font_names.contains(&"JetBrains Mono".to_owned()));
        assert!(font_names.contains(&"Noto Sans CJK SC".to_owned()));
        assert!(font_names.contains(&"Custom Icon Font".to_owned()));
    }

    #[test]
    fn parses_multiple_bars_with_different_positions_and_layouts() {
        let text = r#"
show = ["top-main"]

[bar.top-main]
theme = "theme.toml"
outputs = ["DP-1"]
position = "top"
left = ["[cellbar]"]

[bar.bottom-dock]
theme = "dock-theme.toml"
outputs = ["DP-1", "HDMI-A-1"]
position = "bottom"
center = ["[cellbar]"]
"#;
        let config = Config::parse(text).expect("valid multi-bar config");
        assert_eq!(config.show, vec!["top-main"]);
        assert_eq!(config.bar_definitions.len(), 2);
        assert_eq!(config.bar_definitions["top-main"].position, BarPosition::Top);
        assert_eq!(config.bar_definitions["top-main"].outputs, vec!["DP-1"]);

        assert_eq!(config.bar_definitions["bottom-dock"].position, BarPosition::Bottom);
        assert_eq!(
            config.bar_definitions["bottom-dock"].outputs,
            vec!["DP-1", "HDMI-A-1"]
        );
    }

    #[test]
    fn parses_unspecified_show_defaulting_to_all_defined_bars() {
        let text = r#"
[bar.bar-a]
theme = "theme.toml"
position = "top"

[bar.bar-b]
theme = "theme.toml"
position = "bottom"
"#;
        let config = Config::parse(text).expect("valid config without show specified");
        assert_eq!(config.show, vec!["bar-a", "bar-b"]);
    }

    #[test]
    fn parses_explicit_empty_show_leaving_show_empty() {
        let text = r#"
show = []

[bar.bar-a]
theme = "theme.toml"
position = "top"

[bar.bar-b]
theme = "theme.toml"
position = "bottom"
"#;
        let config = Config::parse(text).expect("valid config with empty show list");
        assert!(config.show.is_empty());
        assert_eq!(config.bar_definitions.len(), 2);
    }

    #[test]
    fn rejects_bars_alias() {
        let text = r#"
bars = ["bar-b"]

[bar.bar-a]
theme = "theme.toml"

[bar.bar-b]
theme = "theme.toml"
"#;
        let error = Config::parse(text).expect_err("only show is supported");
        assert!(error.to_string().contains("unknown field `bars`"));
    }

    #[test]
    fn referenced_display_ids_includes_displays_from_unlisted_bars() {
        let text = r#"
show = ["active"]

[bar.active]
theme = "theme.toml"
left = ["active-display"]

[bar.drawer]
theme = "theme.toml"
left = ["drawer-display"]
"#;
        let config: Config = toml::from_str(text).expect("valid toml");
        let referenced = config.referenced_display_ids();
        assert!(referenced.contains("active-display"));
        assert!(referenced.contains("drawer-display"));
    }

    #[test]
    fn rejects_removed_bar_surface_options() {
        let text = r#"
[bar.hud]
theme = "theme.toml"
layer = "overlay"
exclusive = false
auto_hide = "3s"
"#;
        let err = Config::parse(text).expect_err("removed surface options must fail");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn parses_scope_container_styles_in_theme() {
        use crate::config::ThemeConfig;
        let toml = r##"
[font]
families = ["monospace"]
size = 12.0

[colors]
base = "#00000000"
pill = "#313244"

[surface]
background = "base"

[styles.pill]
background = "pill"
radius = 8

[text]
foreground = "#cdd6f4"
"##;
        let theme_config: ThemeConfig = toml::from_str(toml).expect("valid theme toml");
        let theme = theme_config.resolve().expect("resolved theme");
        let pill_style = theme.style_for(Some("pill")).unwrap();
        assert_eq!(pill_style.background, Some("#313244".parse().unwrap()));
        assert_eq!(pill_style.radius, Some(8));
    }

    #[test]
    fn test_examples_capsule_config_loads_successfully() {
        let path = std::path::Path::new("examples/capsules/config.toml");
        let config = Config::load(path).expect("examples/capsules/config.toml should load without errors");
        let pill_style = config.theme.style_for(Some("pill")).unwrap();
        assert_eq!(pill_style.radius, Some(16));
    }

    #[test]
    fn test_examples_flat_config_loads_successfully() {
        let path = std::path::Path::new("examples/flat/config.toml");
        assert!(Config::load(path).is_ok());
    }

    #[test]
    fn test_style_inset_y_and_radius_parsing() {
        let theme_toml = r##"
[font]
families = ["monospace"]
size = 12.0

[surface]
background = "#181a1f"

[styles.pill]
background = "#313244"
inset_y = 3
radius = 12

[styles.badge]
background = "#cba6f7"
inset_y = 2
radius = 4

[styles.plain]
foreground = "#ffffff"
"##;
        let theme_cfg: ThemeConfig = toml::from_str(theme_toml).expect("valid toml");
        let theme = theme_cfg.resolve().expect("valid theme");
        
        let pill = theme.style_for(Some("pill")).unwrap();
        assert_eq!(pill.inset_y, 3);
        assert_eq!(pill.radius, Some(12));

        let badge = theme.style_for(Some("badge")).unwrap();
        assert_eq!(badge.inset_y, 2);
        assert_eq!(badge.radius, Some(4));

        let plain = theme.style_for(Some("plain")).unwrap();
        assert_eq!(plain.inset_y, 0);
        assert_eq!(plain.radius, None);
    }

    #[test]
    fn test_pure_layout_syntax_and_component_rejection() {
        let toml = r##"
show = ["main"]

[bar.main]
theme = "theme.toml"
left = ["workspaces", "[  ]", "clock"]
"##;
        let config: Config = toml::from_str(toml).expect("pure layout syntax must parse successfully");
        assert!(config.components.contains_key("workspaces"));
        assert!(config.components.contains_key("clock"));
        assert_eq!(config.bar_definitions["main"].left, ["workspaces", "[  ]", "clock"]);

        // Component tables in config.toml must be strictly rejected under route 1
        let invalid_toml = r##"
show = ["main"]
[bar.main]
theme = "theme.toml"
left = ["clock"]

[component.clock]
interval = "30s"
"##;
        let err = toml::from_str::<Config>(invalid_toml).unwrap_err();
        assert!(err.to_string().contains("components are configured directly"));
    }

    #[test]
    fn test_manifest_flat_settings_parsing() {
        let manifest_toml = r##"
[provider]
expression = "settings.format"

[settings]
format = "%H:%M"
interval = "10s"
hide_empty = true
count = 42
opacity = 0.85
"##;
        let manifest: ProviderManifest = toml::from_str(manifest_toml).expect("valid manifest");
        let settings = &manifest.settings;
        assert_eq!(settings["format"].as_str(), Some("%H:%M"));
        assert_eq!(settings["hide_empty"].as_bool(), Some(true));
        assert_eq!(settings["count"].as_integer(), Some(42));
        assert_eq!(settings["opacity"].as_float(), Some(0.85));
    }
