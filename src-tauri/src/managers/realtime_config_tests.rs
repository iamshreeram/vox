//! Authoritative tests for `realtime_config` and the new settings fields (design R9).

use super::*;
use crate::settings::get_default_settings;

fn on() -> AppSettings {
    let mut s = get_default_settings();
    s.exp_rt_enabled = true;
    s
}

fn direct_ready() -> AppSettings {
    let mut s = on();
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = "https://gw.example.com/anthropic".into();
    s.exp_rt_model = "some-model".into();
    s.exp_rt_token = "tok-123".into();
    s
}

// ---- defaults and persistence ------------------------------------------------

#[test]
fn defaults_are_opt_in() {
    let s = get_default_settings();
    assert!(!s.exp_rt_enabled, "master must default OFF");
    assert!(!s.exp_rt_direct_endpoint);
    assert!(!s.exp_rt_ack);
    assert!(!s.exp_rt_proxy_enabled);
    assert!(!s.exp_rt_timing_log);
    assert!(s.exp_rt_stream_reply && s.exp_rt_early_speech && s.exp_rt_fast_voice);
    assert!(s.exp_rt_prewarm && s.exp_rt_auto_proxy);
    assert_eq!(s.exp_rt_endpoint_url, "");
    assert_eq!(s.exp_rt_model, "");
    assert!(s.exp_rt_token.is_empty());
    assert_eq!(s.exp_rt_proxy_url, "");
}

#[test]
fn old_stores_without_the_fields_load_with_defaults() {
    let mut v = serde_json::to_value(get_default_settings()).unwrap();
    let obj = v.as_object_mut().unwrap();
    let keys: Vec<String> = obj
        .keys()
        .filter(|k| k.starts_with("exp_rt_"))
        .cloned()
        .collect();
    assert_eq!(
        keys.len(),
        14,
        "all 14 experimental fields are serialized: {keys:?}"
    );
    for k in &keys {
        obj.remove(k);
    }
    let s: AppSettings = serde_json::from_value(v).unwrap();
    assert!(!s.exp_rt_enabled);
    assert!(s.exp_rt_stream_reply && s.exp_rt_prewarm);
}

#[test]
fn token_round_trips_but_debug_never_prints_it() {
    let mut s = get_default_settings();
    s.exp_rt_token = "super-secret-token-value".into();
    let v = serde_json::to_value(&s).unwrap();
    assert_eq!(
        v["exp_rt_token"], "super-secret-token-value",
        "must persist as a plain string"
    );
    let back: AppSettings = serde_json::from_value(v).unwrap();
    assert_eq!(back.exp_rt_token.as_str(), "super-secret-token-value");
    let dbg = format!("{s:?}");
    assert!(
        !dbg.contains("super-secret-token-value"),
        "Debug leaked the token"
    );
    assert!(dbg.contains("[REDACTED]"));
}

// ---- RealtimeConfig ------------------------------------------------------------

#[test]
fn master_off_forces_every_effective_flag_off() {
    let mut s = get_default_settings();
    s.exp_rt_ack = true;
    s.exp_rt_timing_log = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_proxy_enabled = true;
    s.exp_rt_proxy_url = "http://proxy.example:8080".into();
    let c = RealtimeConfig::from_settings(&s);
    assert_eq!(
        c,
        RealtimeConfig {
            enabled: false,
            direct_endpoint: false,
            stream_reply: false,
            early_speech: false,
            fast_voice: false,
            ack: false,
            prewarm: false,
            auto_proxy: false,
            manual_proxy: None,
            timing_log: false,
            agent_tools: false,
            agent_tools_guard_actions: true,
        }
    );
}

#[test]
fn master_on_reflects_each_toggle() {
    let mut s = on();
    s.exp_rt_ack = true;
    s.exp_rt_timing_log = true;
    let c = RealtimeConfig::from_settings(&s);
    assert!(
        c.enabled && c.stream_reply && c.early_speech && c.fast_voice && c.prewarm && c.auto_proxy
    );
    assert!(c.ack && c.timing_log);
    assert!(!c.direct_endpoint);
    s.exp_rt_prewarm = false;
    s.exp_rt_fast_voice = false;
    s.exp_rt_auto_proxy = false;
    s.exp_rt_direct_endpoint = true;
    let c = RealtimeConfig::from_settings(&s);
    assert!(!c.prewarm && !c.fast_voice && !c.auto_proxy && c.direct_endpoint);
}

#[test]
fn early_speech_needs_streaming() {
    let mut s = on();
    s.exp_rt_stream_reply = false;
    s.exp_rt_early_speech = true;
    let c = RealtimeConfig::from_settings(&s);
    assert!(!c.stream_reply);
    assert!(!c.early_speech);
}

#[test]
fn manual_proxy_needs_its_toggle_a_valid_url_and_the_master() {
    let mut s = on();
    s.exp_rt_proxy_url = "http://proxy.example:8080".into();
    assert_eq!(
        RealtimeConfig::from_settings(&s).manual_proxy,
        None,
        "toggle off"
    );
    s.exp_rt_proxy_enabled = true;
    assert_eq!(
        RealtimeConfig::from_settings(&s).manual_proxy.as_deref(),
        Some("http://proxy.example:8080")
    );
    s.exp_rt_proxy_url = "  http://proxy.example:8080  ".into();
    assert_eq!(
        RealtimeConfig::from_settings(&s).manual_proxy.as_deref(),
        Some("http://proxy.example:8080"),
        "trimmed"
    );
    s.exp_rt_proxy_url = "garbage".into();
    assert_eq!(
        RealtimeConfig::from_settings(&s).manual_proxy,
        None,
        "invalid"
    );
    s.exp_rt_proxy_url = "".into();
    assert_eq!(
        RealtimeConfig::from_settings(&s).manual_proxy,
        None,
        "empty"
    );
}

// ---- preflight_realtime ---------------------------------------------------------

#[test]
fn preflight_is_silent_when_off_or_not_direct() {
    assert_eq!(preflight_realtime(&get_default_settings()), Ok(()));
    let mut s = get_default_settings();
    s.exp_rt_direct_endpoint = true; // master off => nothing checked
    assert_eq!(preflight_realtime(&s), Ok(()));
    assert_eq!(
        preflight_realtime(&on()),
        Ok(()),
        "master on but direct endpoint off"
    );
}

#[test]
fn preflight_direct_endpoint_messages_and_order() {
    let mut s = on();
    s.exp_rt_direct_endpoint = true;
    assert_eq!(preflight_realtime(&s), Err(MSG_ENDPOINT_MISSING));
    s.exp_rt_endpoint_url = "https://gw.example.com".into();
    assert_eq!(preflight_realtime(&s), Err(MSG_MODEL_MISSING));
    s.exp_rt_model = "m".into();
    assert_eq!(preflight_realtime(&s), Err(MSG_TOKEN_MISSING));
    s.exp_rt_token = "t".into();
    assert_eq!(preflight_realtime(&s), Ok(()));

    s.exp_rt_endpoint_url = "ftp://gw.example.com".into();
    assert_eq!(preflight_realtime(&s), Err(MSG_ENDPOINT_INVALID));
    s.exp_rt_endpoint_url = "http://gw.example.com".into();
    assert_eq!(preflight_realtime(&s), Err(MSG_ENDPOINT_NEEDS_HTTPS));
    s.exp_rt_endpoint_url = "http://127.0.0.1:9999".into();
    assert_eq!(preflight_realtime(&s), Ok(()), "loopback http is fine");
}

#[test]
fn preflight_checks_an_enabled_manual_proxy_only() {
    let mut s = direct_ready();
    s.exp_rt_proxy_url = "garbage".into();
    assert_eq!(
        preflight_realtime(&s),
        Ok(()),
        "proxy toggle off => not checked"
    );
    s.exp_rt_proxy_enabled = true;
    assert_eq!(preflight_realtime(&s), Err(MSG_PROXY_INVALID));
    s.exp_rt_proxy_url = "http://proxy.example:3128".into();
    assert_eq!(preflight_realtime(&s), Ok(()));
    // a bad proxy does not matter while the master is off
    s.exp_rt_enabled = false;
    s.exp_rt_proxy_url = "garbage".into();
    assert_eq!(preflight_realtime(&s), Ok(()));
}

#[test]
fn preflight_messages_never_echo_values() {
    let mut s = direct_ready();
    s.exp_rt_endpoint_url = "https://user:pw@gw.example.com".into();
    let e = preflight_realtime(&s).unwrap_err();
    assert!(!e.contains("pw") && !e.contains("gw.example") && !e.contains("tok-123"));
}

// ---- apply_exp_rt_bool -----------------------------------------------------------

#[test]
fn every_bool_key_maps_to_its_own_field() {
    use ExpRtBool::*;
    let mut s = get_default_settings();
    let table: Vec<(ExpRtBool, fn(&AppSettings) -> bool)> = vec![
        (Enabled, |s| s.exp_rt_enabled),
        (DirectEndpoint, |s| s.exp_rt_direct_endpoint),
        (StreamReply, |s| s.exp_rt_stream_reply),
        (EarlySpeech, |s| s.exp_rt_early_speech),
        (FastVoice, |s| s.exp_rt_fast_voice),
        (Ack, |s| s.exp_rt_ack),
        (Prewarm, |s| s.exp_rt_prewarm),
        (AutoProxy, |s| s.exp_rt_auto_proxy),
        (ProxyEnabled, |s| s.exp_rt_proxy_enabled),
        (TimingLog, |s| s.exp_rt_timing_log),
    ];
    for (key, get) in &table {
        let before: Vec<bool> = table.iter().map(|(_, g)| g(&s)).collect();
        apply_exp_rt_bool(&mut s, *key, true);
        assert!(get(&s), "{key:?} on");
        apply_exp_rt_bool(&mut s, *key, false);
        assert!(!get(&s), "{key:?} off");
        // restore and make sure nothing else moved
        apply_exp_rt_bool(
            &mut s,
            *key,
            before[table.iter().position(|(k, _)| k == key).unwrap()],
        );
        let after: Vec<bool> = table.iter().map(|(_, g)| g(&s)).collect();
        assert_eq!(before, after, "{key:?} must touch only its own field");
    }
}

// ---- apply_exp_rt_text -----------------------------------------------------------

#[test]
fn text_setters_trim_and_store() {
    let mut s = get_default_settings();
    apply_exp_rt_text(&mut s, ExpRtText::Model, "  some-model  ").unwrap();
    assert_eq!(s.exp_rt_model, "some-model");
    apply_exp_rt_text(
        &mut s,
        ExpRtText::EndpointUrl,
        " https://gw.example.com/anthropic/ ",
    )
    .unwrap();
    assert_eq!(s.exp_rt_endpoint_url, "https://gw.example.com/anthropic");
    apply_exp_rt_text(&mut s, ExpRtText::Token, "  abc.def-123  ").unwrap();
    assert_eq!(s.exp_rt_token.as_str(), "abc.def-123");
    apply_exp_rt_text(&mut s, ExpRtText::ProxyUrl, "http://proxy.example:8080").unwrap();
    assert_eq!(s.exp_rt_proxy_url, "http://proxy.example:8080");
}

#[test]
fn empty_clears_each_field() {
    let mut s = direct_ready();
    s.exp_rt_proxy_url = "http://p.example:1".into();
    for k in [
        ExpRtText::EndpointUrl,
        ExpRtText::Model,
        ExpRtText::Token,
        ExpRtText::ProxyUrl,
    ] {
        apply_exp_rt_text(&mut s, k, "   ").unwrap();
    }
    assert_eq!(s.exp_rt_endpoint_url, "");
    assert_eq!(s.exp_rt_model, "");
    assert!(s.exp_rt_token.is_empty());
    assert_eq!(s.exp_rt_proxy_url, "");
}

#[test]
fn invalid_text_is_rejected_and_leaves_settings_unchanged() {
    let mut s = direct_ready();
    s.exp_rt_proxy_url = "http://p.example:1".into();
    let snapshot = (
        s.exp_rt_endpoint_url.clone(),
        s.exp_rt_model.clone(),
        s.exp_rt_token.as_str().to_string(),
        s.exp_rt_proxy_url.clone(),
    );
    let too_long_url = format!("https://x.example.com/{}", "a".repeat(MAX_URL_CHARS));
    let cases: Vec<(ExpRtText, String)> = vec![
        (ExpRtText::EndpointUrl, "ftp://x.example.com".into()),
        (ExpRtText::EndpointUrl, "https://u:p@x.example.com".into()),
        (ExpRtText::EndpointUrl, "https://x.example.com/?k=v".into()),
        (ExpRtText::EndpointUrl, "http://x.example.com".into()), // plain http, key semantics: must be https
        (ExpRtText::EndpointUrl, too_long_url),
        (ExpRtText::EndpointUrl, "https://x.example.com/\u{7}".into()),
        (ExpRtText::Model, "bad\nmodel".into()),
        (ExpRtText::Model, "m".repeat(MAX_MODEL_CHARS + 1)),
        (ExpRtText::Token, "has space".into()),
        (ExpRtText::Token, "tab\there".into()),
        (ExpRtText::Token, "t".repeat(MAX_TOKEN_CHARS + 1)),
        (ExpRtText::Token, "ctl\u{1b}[31m".into()),
        (ExpRtText::ProxyUrl, "garbage".into()),
        (ExpRtText::ProxyUrl, "ftp://p.example:1".into()),
        (ExpRtText::ProxyUrl, "http://p.example:1/?q=1".into()),
    ];
    for (k, v) in cases {
        let r = apply_exp_rt_text(&mut s, k, &v);
        assert!(r.is_err(), "{k:?} {v:?} must be rejected");
        let msg = r.unwrap_err();
        assert!(!msg.contains(v.trim()), "message must not echo input");
        assert_eq!(
            snapshot,
            (
                s.exp_rt_endpoint_url.clone(),
                s.exp_rt_model.clone(),
                s.exp_rt_token.as_str().to_string(),
                s.exp_rt_proxy_url.clone(),
            ),
            "{k:?} {v:?} changed settings"
        );
    }
}

#[test]
fn loopback_http_endpoint_is_accepted() {
    let mut s = get_default_settings();
    apply_exp_rt_text(
        &mut s,
        ExpRtText::EndpointUrl,
        "http://127.0.0.1:8080/anthropic",
    )
    .unwrap();
    assert_eq!(s.exp_rt_endpoint_url, "http://127.0.0.1:8080/anthropic");
}

// ---- M2-0: agent tools flags (design R4-1) ----------------------------------

#[test]
fn agent_tools_flags_follow_master_and_tools_toggle() {
    for master in [false, true] {
        for tools in [false, true] {
            let mut s = get_default_settings();
            s.exp_rt_enabled = master;
            s.agent_tools_enabled = tools;
            let c = RealtimeConfig::from_settings(&s);
            assert_eq!(
                c.agent_tools,
                master && tools,
                "agent_tools must be master && agent_tools_enabled (master={master}, tools={tools})"
            );
        }
    }
}

#[test]
fn agent_tools_guard_actions_mirrors_the_setting_in_every_case() {
    for master in [false, true] {
        for tools in [false, true] {
            for guard in [false, true] {
                let mut s = get_default_settings();
                s.exp_rt_enabled = master;
                s.agent_tools_enabled = tools;
                s.agent_tools_guard_actions = guard;
                let c = RealtimeConfig::from_settings(&s);
                assert_eq!(
                    c.agent_tools_guard_actions, guard,
                    "guard must copy the setting (master={master}, tools={tools}, guard={guard})"
                );
            }
        }
    }
}

#[test]
fn agent_tools_guard_actions_defaults_true_in_realtime_config() {
    let c = RealtimeConfig::from_settings(&get_default_settings());
    assert!(c.agent_tools_guard_actions);
    assert!(!c.agent_tools);
}
