//! The JSON API: state and target, settings, modes, candidates, calibration and trials.
use crate::common::*;
use prysm_ports::{AdapterSettings, Settings};
use prysm_runtime::fakes::{self, FakeOutputSettings, FakeSourceSettings};
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
        let state = portal.state().await;
        assert_eq!(state["target"], "screen_sync");
        assert_eq!(state["session"]["state"], "screen_sync");
        assert!(portal.log.wait_for("output.write:", 1).await);

        let (status, _) = portal.target("off").await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(portal.state().await["session"]["state"], "stopped");
        assert_eq!(portal.log.count("output.idle::Off"), 1);

        let (status, body) = portal.target("sideways").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["kind"], "bad_request");
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
        let (status, _) = portal
            .json(
                Method::POST,
                modes,
                json!({ "kind": "fake", "width": "wide" }),
            )
            .await;
        assert!(status.is_client_error(), "{status}");
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
        // The fake source finds nothing: the default.
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

        // Lighting must be on first.
        let (status, body) = put(gray.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["kind"], "needs_screen_sync");
        assert!(body.get("field").is_none());
        assert!(portal.state().await.get("calibration").is_none());

        portal.target("screen_sync").await;
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

        // Closed pattern set, exact fields.
        for bad in [
            json!({ "pattern": "gray-64", "uncorrected": false }),
            json!({ "pattern": "gray-192", "uncorrected": false }),
            json!({ "pattern": "response", "uncorrected": false }),
            json!({ "pattern": "red" }),
            json!({ "pattern": "red", "uncorrected": false, "token": "x" }),
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

        // A foreign page can neither set nor clear it.
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

        // Leaving screen sync clears it.
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

        // Accepted with lighting off or on; nothing saved, no new revision.
        assert_eq!(put(processing.clone()).await.0, StatusCode::NO_CONTENT);
        portal.target("screen_sync").await;
        assert_eq!(put(processing.clone()).await.0, StatusCode::NO_CONTENT);
        let state = portal.state().await;
        assert_eq!(state["revision"], before["revision"]);
        assert_eq!(state["settings"], before["settings"]);
        assert_eq!(portal.store.saved(), Some(fakes::settings()));

        // Validated like settings, exact fields.
        let mut out_of_range = processing.clone();
        out_of_range["brightness_percent"] = json!(101);
        let (status, body) = put(out_of_range).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["kind"], "invalid");
        assert_eq!(body["field"], "processing.brightness_percent");
        let mut unknown = processing.clone();
        unknown["token"] = json!("x");
        let (status, body) = put(unknown).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["kind"], "bad_request");

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
async fn adapter_failures_are_bad_gateway_and_missing_settings_are_invalid() {
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
        let (status, body) = portal.target("screen_sync").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body,
            json!({
                "kind": "adapter",
                "message": "fake source cannot open",
                "section": "capture",
            })
        );
        assert_eq!(
            portal.state().await["session"]["failure"]["kind"],
            "adapter"
        );
        // The state says the same side, for the portal's Video stop.
        assert_eq!(
            portal.state().await["session"]["failure"]["section"],
            "capture"
        );
        portal.shutdown().await.unwrap();

        // The output opens at the first frame: its failure ends the session
        // and is reported in the state, not in the reply.
        let failing = with_output(FakeOutputSettings {
            fail_open: true,
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
        let (status, body) = portal.target("screen_sync").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["field"], "capture");
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

        let mut undecodable = settings;
        undecodable["output"] = json!({ "kind": "fake", "fail_after": "soon" });
        let (status, body) = replace(&portal, initial, undecodable).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .starts_with("cannot decode `fake` settings:")
        );
        assert_eq!(portal.state().await["revision"], initial);
        assert_eq!(portal.store.saved(), Some(fakes::settings()));
        portal.shutdown().await.unwrap();
    })
    .await;
}

/// The format number is the file's, not the settings document's.
#[tokio::test]
async fn settings_with_a_format_number_are_a_bad_request() {
    within_async(LIMIT, async {
        let portal = start(fakes::settings(), Duration::from_secs(5)).await;
        let (initial, mut settings) = dimmed(&portal).await;
        settings["format"] = json!(1);
        let (status, body) = replace(&portal, initial, settings).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["kind"], "bad_request");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("unknown field `format`"),
            "{body}"
        );
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
