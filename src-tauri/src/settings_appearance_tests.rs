//! Appearance settings (UI enterprise S1, DESIGN Revision 3, contract 1): the enums `Accent`,
//! `ChatTheme` and `VisualStyle`, their field-local lenient deserializers (checked through the
//! raw AppSettings JSON, like `chat_opacity_pct`), defaults, and the stale-write guard of
//! `preserve_launch_fields`.

use crate::settings::{
    get_default_settings, preserve_launch_fields, Accent, AppSettings, ChatTheme,
    CustomThemeColors, Theme, VisualStyle,
};
use serde_json::{json, Value};

fn stored_json() -> Value {
    let mut value = serde_json::to_value(get_default_settings()).expect("defaults serialize");
    let obj = value.as_object_mut().expect("settings is an object");
    // An unrelated non-default value: it must survive every appearance field edge case.
    obj.insert("orb_size_px".to_string(), json!(140));
    value
}

fn parse_with(field: &str, raw: Value) -> AppSettings {
    let mut value = stored_json();
    value
        .as_object_mut()
        .expect("object")
        .insert(field.to_string(), raw);
    serde_json::from_value(value).expect("a lenient field never fails the whole object")
}

fn parse_without(field: &str) -> AppSettings {
    let mut value = stored_json();
    value.as_object_mut().expect("object").remove(field);
    serde_json::from_value(value).expect("a missing field parses")
}

#[test]
fn defaults_are_amber_dark_and_vibrant() {
    let s = get_default_settings();
    assert_eq!(s.accent, Accent::Amber);
    assert_eq!(s.chat_theme, ChatTheme::Dark);
    assert_eq!(s.visual_style, VisualStyle::Vibrant);
}

#[test]
fn enum_wire_names_are_snake_case() {
    let accents = [
        (Accent::Amber, "amber"),
        (Accent::Blue, "blue"),
        (Accent::Violet, "violet"),
        (Accent::Emerald, "emerald"),
        (Accent::Rose, "rose"),
        (Accent::Slate, "slate"),
    ];
    for (value, wire) in accents {
        assert_eq!(serde_json::to_value(value).expect("ser"), json!(wire));
    }
    assert_eq!(
        serde_json::to_value(ChatTheme::FollowApp).expect("ser"),
        json!("follow_app")
    );
    assert_eq!(
        serde_json::to_value(ChatTheme::Dark).expect("ser"),
        json!("dark")
    );
    assert_eq!(
        serde_json::to_value(ChatTheme::Light).expect("ser"),
        json!("light")
    );
    assert_eq!(
        serde_json::to_value(VisualStyle::Vibrant).expect("ser"),
        json!("vibrant")
    );
    assert_eq!(
        serde_json::to_value(VisualStyle::Restrained).expect("ser"),
        json!("restrained")
    );
}

#[test]
fn accent_lenient_table_valid_values_parse_everything_else_is_amber() {
    let valid = [
        ("amber", Accent::Amber),
        ("blue", Accent::Blue),
        ("violet", Accent::Violet),
        ("emerald", Accent::Emerald),
        ("rose", Accent::Rose),
        ("slate", Accent::Slate),
    ];
    for (wire, expected) in valid {
        assert_eq!(parse_with("accent", json!(wire)).accent, expected, "{wire}");
    }
    let garbage = [
        json!("teal"),
        json!("BLUE"),
        json!(" blue"),
        json!(""),
        json!(7),
        json!(-1.5),
        json!(true),
        json!(null),
        json!([]),
        json!({}),
        json!({"accent": "blue"}),
    ];
    for raw in garbage {
        assert_eq!(
            parse_with("accent", raw.clone()).accent,
            Accent::Amber,
            "{raw}"
        );
    }
    assert_eq!(parse_without("accent").accent, Accent::Amber, "missing key");
}

#[test]
fn chat_theme_lenient_table_valid_values_parse_everything_else_is_dark() {
    let valid = [
        ("dark", ChatTheme::Dark),
        ("follow_app", ChatTheme::FollowApp),
        ("light", ChatTheme::Light),
    ];
    for (wire, expected) in valid {
        assert_eq!(
            parse_with("chat_theme", json!(wire)).chat_theme,
            expected,
            "{wire}"
        );
    }
    let garbage = [
        json!("followapp"),
        json!("FollowApp"),
        json!("system"),
        json!("aurora"),
        json!(3),
        json!(null),
        json!(false),
        json!(["light"]),
    ];
    for raw in garbage {
        assert_eq!(
            parse_with("chat_theme", raw.clone()).chat_theme,
            ChatTheme::Dark,
            "{raw}"
        );
    }
    assert_eq!(
        parse_without("chat_theme").chat_theme,
        ChatTheme::Dark,
        "missing key"
    );
}

#[test]
fn visual_style_lenient_table_valid_values_parse_everything_else_is_vibrant() {
    assert_eq!(
        parse_with("visual_style", json!("restrained")).visual_style,
        VisualStyle::Restrained
    );
    assert_eq!(
        parse_with("visual_style", json!("vibrant")).visual_style,
        VisualStyle::Vibrant
    );
    let garbage = [
        json!("Restrained"),
        json!("minimal"),
        json!(1),
        json!(null),
        json!({}),
        json!(true),
    ];
    for raw in garbage {
        assert_eq!(
            parse_with("visual_style", raw.clone()).visual_style,
            VisualStyle::Vibrant,
            "{raw}"
        );
    }
    assert_eq!(
        parse_without("visual_style").visual_style,
        VisualStyle::Vibrant,
        "missing key"
    );
}

#[test]
fn garbage_appearance_values_leave_unrelated_fields_intact() {
    let s = parse_with("accent", json!(42));
    assert_eq!(s.orb_size_px, Some(140), "unrelated field survives");
    let s = parse_with("chat_theme", json!("nope"));
    assert_eq!(s.orb_size_px, Some(140));
    assert_eq!(
        s.accent,
        Accent::Amber,
        "other appearance fields keep defaults"
    );
    let s = parse_with("visual_style", json!(null));
    assert_eq!(s.orb_size_px, Some(140));
    assert_eq!(s.theme, get_default_settings().theme);
}

#[test]
fn a_stale_write_cannot_change_the_appearance_fields() {
    let mut committed = get_default_settings();
    committed.theme = Theme::Aurora;
    committed.custom_theme_colors = CustomThemeColors {
        accent: "#123456".to_string(),
        ..CustomThemeColors::default()
    };
    committed.accent = Accent::Rose;
    committed.chat_theme = ChatTheme::FollowApp;
    committed.visual_style = VisualStyle::Restrained;

    // A whole-object write read before the change, at the defaults.
    let mut stale = get_default_settings();
    stale.selected_model = "incoming-model".to_string();
    let merged = preserve_launch_fields(stale, &committed);

    assert_eq!(merged.theme, Theme::Aurora, "theme kept");
    assert_eq!(
        merged.custom_theme_colors, committed.custom_theme_colors,
        "colours kept"
    );
    assert_eq!(merged.accent, Accent::Rose, "accent kept");
    assert_eq!(merged.chat_theme, ChatTheme::FollowApp, "chat theme kept");
    assert_eq!(
        merged.visual_style,
        VisualStyle::Restrained,
        "visual style kept"
    );
    assert_eq!(
        merged.selected_model, "incoming-model",
        "unrelated field still incoming"
    );
}

#[test]
fn a_stale_write_of_an_older_appearance_cannot_win_either_way() {
    let mut committed = get_default_settings();
    committed.accent = Accent::Slate;
    let mut stale_other = get_default_settings();
    stale_other.accent = Accent::Blue;
    let merged = preserve_launch_fields(stale_other, &committed);
    assert_eq!(
        merged.accent,
        Accent::Slate,
        "a stale non-default cannot win"
    );
}
