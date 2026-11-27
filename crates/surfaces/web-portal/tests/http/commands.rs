//! The JSON API: state and target, settings, modes, candidates, calibration and trials.
use crate::common::*;
use prysm_ports::{AdapterSettings, Settings};
use prysm_runtime::fakes::{self, FakeOpen, FakeOutputSettings, FakeSourceSettings};
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn state_read_and_set_target() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let state = portal.state().await;
        assert_eq!(state["target"], "off");
        assert_eq!(state["session"], json!({ "state": "stopped" }));
        assert!(state["revision"].is_u64());
        assert_eq!(state["settings"]["capture"]["kind"], "fake");
        assert!(state.get("failure").is_none(), "{state}");

        let (status, _) = portal.target("screen_sync").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(portal.log.wait_for("output.write:", 1).await);
        let state = portal.state().await;
        assert_eq!(state["target"], "screen_sync");
        assert_eq!(state["session"]["state"], "screen_sync");

        let (status, _) = portal.target("off").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(portal.log.wait_for("output.idle::Off", 1).await);
        assert_eq!(portal.state().await["session"]["state"], "stopped");

        let (status, body) = portal.target("sideways").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["kind"], "bad_request");
        let (status, body) = portal
            .json(Method::PUT, "/api/v1/target", json!(["screen_sync"]))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["kind"], "bad_request");
        assert_eq!(portal.state().await["target"], "off");
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// Capture modes are an on-request lookup of the posted capture section:
/// no revision or state change, unknown kinds and undecodable sections are
/// refused, and the origin policy applies.
#[tokio::test]
async fn capture_modes_answer_for_the_posted_section() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let revision = portal.state().await["revision"].clone();
        let modes = "/api/v1/capture/modes";

        let (status, body) = portal
            .json(
                Method::POST,
                modes,
                json!({ "kind": "fake", "width": 99999 }),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({ "status": "known", "modes": [{
                "width": { "min": 16, "max": 16, "step": 1 },
                "height": { "min": 9, "max": 9, "step": 1 },
                "fps": { "min": 1, "max": 60, "step": 1 },
            }] })
        );
        assert_eq!(portal.log.count("source.modes"), 1);

        let (status, body) = portal
            .json(Method::POST, modes, json!({ "kind": "hue" }))
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "capture.kind");
        let (status, body) = portal
            .json(
                Method::POST,
                modes,
                json!({ "kind": "fake", "width": "wide" }),
            )
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "capture.width");
        let (status, body) = portal
            .json(Method::POST, modes, json!({ "path": "x" }))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["kind"], "bad_request");
        let (status, _) = portal.get(modes).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        let foreign = portal
            .request(Method::POST, modes)
            .header(header::ORIGIN, "http://evil.example")
            .json(&json!({ "kind": "fake" }))
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        assert_eq!(portal.log.count("source.modes"), 1);
        assert_eq!(portal.state().await["revision"], revision);
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// Candidates are an on-request lookup: no revision or state change, unknown
/// kinds name the field, other paths are unknown routes, and the origin
/// policy applies as to every API call.
#[tokio::test]
async fn candidates_list_what_an_adapter_finds() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let revision = portal.state().await["revision"].clone();

        let (status, body) = portal.get("/api/v1/candidates/output/fake").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!([{ "label": "Fake lights", "detail": "fake", "fields": { "name": "found" } }])
        );
        assert_eq!(portal.log.count("output.candidates"), 1);
        let (status, body) = portal.get("/api/v1/candidates/capture/fake").await;
        assert_eq!((status, body), (StatusCode::OK, json!([])));

        let (status, body) = portal.get("/api/v1/candidates/output/hue").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "output.kind");
        for path in [
            "/api/v1/candidates/processing/fake",
            "/api/v1/candidates/output/",
            "/api/v1/candidates/output",
            "/api/v1/candidates/output/fake/more",
        ] {
            let (status, body) = portal.get(path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(body["kind"], "not_found");
        }
        let (status, _) = portal
            .json(Method::PUT, "/api/v1/candidates/output/fake", json!({}))
            .await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        let foreign = portal
            .request(Method::GET, "/api/v1/candidates/output/fake")
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        assert_eq!(portal.log.count("output.candidates"), 1);
        assert_eq!(portal.state().await["revision"], revision);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn calibration_shows_switches_and_clears_a_pattern_without_saving() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let put = |body: Value| portal.json(Method::PUT, "/api/v1/calibration", body);
        let gray = json!({ "pattern": "gray_192", "uncorrected": false });

        let (status, body) = put(gray.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["kind"], "needs_screen_sync");
        assert!(body.get("field").is_none());
        assert!(portal.state().await.get("calibration").is_none());

        portal.target("screen_sync").await;
        portal
            .engine
            .snapshot()
            .wait_for(|snapshot| matches!(snapshot.session, prysm_ports::SessionState::ScreenSync))
            .await
            .unwrap();
        let before = portal.state().await;
        let (status, _) = put(gray.clone()).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let state = portal.state().await;
        assert_eq!(state["calibration"], gray);
        assert_eq!(state["revision"], before["revision"]);
        assert_eq!(state["settings"], before["settings"]);

        let grid = json!({ "pattern": "review_grid", "uncorrected": true });
        assert_eq!(put(grid.clone()).await.0, StatusCode::NO_CONTENT);
        assert_eq!(portal.state().await["calibration"], grid);

        for bad in [
            json!({ "pattern": "gray-64", "uncorrected": false }),
            json!({ "pattern": "gray-192", "uncorrected": false }),
            json!({ "pattern": "response", "uncorrected": false }),
            json!({ "pattern": "red" }),
            json!({ "pattern": "red", "uncorrected": false, "token": "x" }),
            json!(["red", false]),
            json!(null),
        ] {
            let (status, body) = put(bad.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
            assert_eq!(body["kind"], "bad_request");
        }
        let untyped = portal
            .request(Method::PUT, "/api/v1/calibration")
            .body(gray.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(untyped.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(portal.state().await["calibration"], grid);

        let get = portal
            .request(Method::GET, "/api/v1/calibration")
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);

        for method in [Method::PUT, Method::DELETE] {
            let refused = portal
                .request(method.clone(), "/api/v1/calibration")
                .header(header::ORIGIN, "http://evil.example")
                .json(&gray)
                .send()
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::FORBIDDEN, "{method}");
        }
        assert_eq!(portal.state().await["calibration"], grid);

        let cleared = portal
            .request(Method::DELETE, "/api/v1/calibration")
            .send()
            .await
            .unwrap();
        assert_eq!(cleared.status(), StatusCode::NO_CONTENT);
        assert!(portal.state().await.get("calibration").is_none());

        assert_eq!(put(gray.clone()).await.0, StatusCode::NO_CONTENT);
        portal.target("off").await;
        let state = portal.state().await;
        assert!(state.get("calibration").is_none());
        assert_eq!(state["revision"], before["revision"]);
        assert_eq!(portal.store.saved(), Some(fakes::settings()));
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_processing_trial_applies_without_saving() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let before = portal.state().await;
        let mut processing = before["settings"]["processing"].clone();
        processing["brightness_percent"] = json!(40);
        let put = |body: Value| portal.json(Method::PUT, "/api/v1/processing/trial", body);

        assert_eq!(put(processing.clone()).await.0, StatusCode::NO_CONTENT);
        portal.target("screen_sync").await;
        assert_eq!(put(processing.clone()).await.0, StatusCode::NO_CONTENT);
        let state = portal.state().await;
        assert_eq!(state["revision"], before["revision"]);
        assert_eq!(state["settings"], before["settings"]);
        assert_eq!(portal.store.saved(), Some(fakes::settings()));

        let mut out_of_range = processing.clone();
        out_of_range["brightness_percent"] = json!(101);
        let (status, body) = put(out_of_range).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["kind"], "invalid");
        assert_eq!(body["field"], "processing.brightness_percent");
        // The body is the processing section: a problem inside it names
        // the field; a body that is not one is malformed.
        let mut unknown = processing.clone();
        unknown["token"] = json!("x");
        let (status, body) = put(unknown).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "processing.token");
        let mut style = processing.clone();
        style["style"] = json!({ "kind": "loud" });
        let (status, body) = put(style).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "processing.style.kind");
        for whole in [json!(3), json!([])] {
            let (status, body) = put(whole.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{whole}");
            assert_eq!(body["kind"], "bad_request");
        }

        let get = portal
            .request(Method::GET, "/api/v1/processing/trial")
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);
        let refused = portal
            .request(Method::PUT, "/api/v1/processing/trial")
            .header(header::ORIGIN, "http://evil.example")
            .json(&processing)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        assert_eq!(portal.store.saved(), Some(fakes::settings()));
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn accepted_target_reports_typed_start_failures_in_state() {
    within_async(LIMIT, async {
        let failing = Settings {
            capture: Some(AdapterSettings::new(
                "fake",
                &FakeSourceSettings {
                    fail_open: true,
                    ..FakeSourceSettings::default()
                },
            )),
            ..fakes::settings()
        };
        let portal = start(failing, Duration::from_secs(5)).await;
        assert_eq!(portal.target("screen_sync").await.0, StatusCode::NO_CONTENT);
        portal
            .engine
            .snapshot()
            .wait_for(|snapshot| snapshot.session.failure().is_some())
            .await
            .unwrap();
        assert_eq!(
            portal.state().await["session"]["failure"],
            json!({
                "kind": "adapter",
                "message": "fake source cannot open",
                "section": "capture",
            })
        );
        portal.shutdown().await.unwrap();

        let failing = with_output(FakeOutputSettings {
            open: FakeOpen::Fails,
            ..FakeOutputSettings::default()
        });
        let portal = start(failing, Duration::from_secs(5)).await;
        let (status, _) = portal.target("screen_sync").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        portal
            .engine
            .snapshot()
            .wait_for(|snapshot| snapshot.session.failure().is_some())
            .await
            .unwrap();
        let state = portal.state().await;
        assert_eq!(state["session"]["failure"]["kind"], "adapter");
        assert_eq!(
            state["session"]["failure"]["message"],
            "fake output cannot open"
        );
        assert_eq!(state["session"]["failure"]["section"], "output");
        portal.shutdown().await.unwrap();

        let portal = start(Settings::default(), Duration::from_secs(5)).await;
        assert_eq!(portal.target("screen_sync").await.0, StatusCode::NO_CONTENT);
        portal
            .engine
            .snapshot()
            .wait_for(|snapshot| snapshot.session.failure().is_some())
            .await
            .unwrap();
        let failed = portal.state().await;
        assert_eq!(failed["session"]["failure"]["kind"], "invalid");
        assert_eq!(failed["session"]["failure"]["field"], "capture");
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// `PUT /api/v1/settings` of `settings` expecting `expected`.
async fn replace(portal: &Portal, expected: u64, settings: Value) -> (StatusCode, Value) {
    portal
        .json(
            Method::PUT,
            "/api/v1/settings",
            json!({ "expected": expected, "settings": settings }),
        )
        .await
}

/// The current revision and the saved settings with 40 % brightness.
async fn dimmed(portal: &Portal) -> (u64, Value) {
    let state = portal.state().await;
    let mut settings = state["settings"].clone();
    settings["processing"]["brightness_percent"] = json!(40);
    (state["revision"].as_u64().unwrap(), settings)
}

#[tokio::test]
async fn a_settings_replace_saves_and_moves_to_the_next_revision() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let (initial, settings) = dimmed(&portal).await;
        let (status, body) = replace(&portal, initial, settings).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!({ "revision": initial + 1 }));
        assert_eq!(
            portal.store.saved().unwrap().processing.brightness_percent,
            40
        );
        let state = portal.state().await;
        assert_eq!(state["revision"], initial + 1);
        assert_eq!(state["settings"]["processing"]["brightness_percent"], 40);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_settings_replace_of_an_old_revision_conflicts() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let (initial, settings) = dimmed(&portal).await;
        assert_eq!(
            replace(&portal, initial, settings.clone()).await.0,
            StatusCode::OK
        );
        let (status, body) = replace(&portal, initial, settings).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["kind"], "stale_revision");
        assert_eq!(body["current"], initial + 1);
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// Each refused with its field and cause, and nothing saved.
#[tokio::test]
async fn invalid_settings_are_unprocessable_and_not_saved() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let (initial, settings) = dimmed(&portal).await;

        let mut invalid = settings.clone();
        invalid["capture"] = serde_json::to_value(AdapterSettings::new(
            "fake",
            &FakeSourceSettings {
                invalid: true,
                ..FakeSourceSettings::default()
            },
        ))
        .unwrap();
        let (status, body) = replace(&portal, initial, invalid).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body,
            json!({ "kind": "invalid", "message": "fake rejects it", "field": "capture.invalid" })
        );

        let mut range = settings.clone();
        range["processing"]["style"] = json!({ "kind": "custom" });
        range["processing"]["custom_style"] = json!({
            "smoothing_seconds": 0.3,
            "cut_response_percent": 100,
            "brightness_ramp_seconds": 0.0,
            "spatial_reach_percent": 0,
            "color_focus_percent": 0,
            "saturation_boost_percent": 0,
            "dark_floor_percent": 0,
        });
        let (status, body) = replace(&portal, initial, range).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body["field"],
            "processing.custom_style.spatial_reach_percent"
        );

        let mut unknown = settings.clone();
        unknown["output"] = json!({ "kind": "laser" });
        let (status, body) = replace(&portal, initial, unknown).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "output.kind");

        let mut undecodable = settings.clone();
        undecodable["output"] = json!({ "kind": "fake", "fail_after": "soon" });
        let (status, body) = replace(&portal, initial, undecodable).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "output.fail_after");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .starts_with("cannot decode `fake` settings:"),
            "{body}"
        );

        // Problems inside `settings` that its types refuse while decoding.
        let calibrations = json!([{ "id": "a", "name": "A" }, { "id": 2, "name": "B" }]);
        for (section, name, value, field) in [
            (None, "format", json!(1), "format"),
            (
                Some("processing"),
                "style",
                json!({ "kind": "loud" }),
                "processing.style.kind",
            ),
            (
                Some("processing"),
                "brightnes_percent",
                json!(80),
                "processing.brightnes_percent",
            ),
            (
                Some("processing"),
                "brightness_percent",
                json!("bright"),
                "processing.brightness_percent",
            ),
            (
                Some("processing"),
                "calibrations",
                calibrations,
                "processing.calibrations[1].id",
            ),
            (None, "processing", json!([]), "processing"),
            (
                Some("processing"),
                "calibrations",
                json!([{ "id": "a", "name": "A", "correction": [] }]),
                "processing.calibrations[0].correction",
            ),
            (
                Some("processing"),
                "calibrations",
                json!([["a", "A"]]),
                "processing.calibrations[0]",
            ),
            (
                Some("processing"),
                "custom_style",
                json!([0.5, 50, 0.3, 50, 50, 50, 5]),
                "processing.custom_style",
            ),
            (
                Some("processing"),
                "style",
                json!(["natural"]),
                "processing.style",
            ),
            (None, "capture", json!({ "path": "x" }), "capture"),
            (None, "output", json!(3), "output"),
        ] {
            let mut refused = settings.clone();
            match section {
                Some(section) => refused[section][name] = value,
                None => refused[name] = value,
            }
            let (status, body) = replace(&portal, initial, refused).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{field}: {body}");
            assert_eq!(body["kind"], "invalid", "{field}");
            assert_eq!(body["field"], field, "{body}");
        }
        assert_eq!(portal.state().await["revision"], initial);
        assert_eq!(portal.store.saved(), Some(fakes::settings()));
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// A body that is not JSON, and the envelope around `settings` (a top-level
/// field unknown, missing or of the wrong type), are malformed requests: no
/// setting is at fault. The format number is the file's: inside `settings`
/// it is an unknown setting (above).
#[tokio::test]
async fn a_malformed_settings_request_is_a_bad_request() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let (initial, settings) = dimmed(&portal).await;
        let valid = json!({ "expected": initial, "settings": settings });
        let bodies = [
            json!({ "expected": "x", "settings": settings }).to_string(),
            json!({ "expected": initial, "settings": settings, "force": true }).to_string(),
            json!({ "settings": settings }).to_string(),
            json!({ "expected": initial, "settings": 3 }).to_string(),
            json!({ "expected": initial, "settings": [] }).to_string(),
            json!([]).to_string(),
            json!([initial, settings]).to_string(),
            "{".to_owned(),
            format!("{valid} {{}}"),
            r#"{ "expected": 1, "settings": { "processing": { "brightness_percent": 80, } } }"#
                .to_owned(),
        ];
        for text in bodies {
            let response = portal
                .request(Method::PUT, "/api/v1/settings")
                .header(header::CONTENT_TYPE, "application/json")
                .body(text.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{text}");
            let body: Value = response.json().await.unwrap();
            assert_eq!(body["kind"], "bad_request", "{text}");
            assert!(body.get("field").is_none(), "{text}");
        }
        assert_eq!(portal.state().await["revision"], initial);
        portal.shutdown().await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn a_settings_body_must_be_json_of_at_most_one_mebibyte() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let plain = portal
            .request(Method::PUT, "/api/v1/settings")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(plain.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let large = portal
            .request(Method::PUT, "/api/v1/settings")
            .header(header::CONTENT_TYPE, "application/json")
            .body(vec![b' '; 1024 * 1024 + 1])
            .send()
            .await
            .unwrap();
        assert_eq!(large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        portal.shutdown().await.unwrap();
    })
    .await;
}
