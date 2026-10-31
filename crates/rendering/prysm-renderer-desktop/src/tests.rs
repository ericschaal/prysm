use super::*;
use eframe::App;
use prysm_capture::PixelFormat;
use std::num::NonZeroU32;

#[test]
fn draws_application_toolbar_above_video() {
    let (_colors, colors_rx) = tokio::sync::watch::channel(EdgeColors::default());
    let mut app = DesktopRendererBuilder::new(colors_rx)
        .with_toolbar(|ui| {
            ui.label("Start capture");
        })
        .build();
    let context = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let output = context.run(Default::default(), |ctx| app.update(ctx, &mut frame));
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
        egui::Shape::Text(text) if text.galley.text() == "Start capture")));
}

#[test]
fn output_draws_each_edge_color_in_its_strip() {
    use prysm_core::LinearColor;

    let colors = EdgeColors::new(
        ColorStrip::fill(LinearColor::new(1.0, 0.0, 0.0), 1),
        ColorStrip::fill(LinearColor::new(0.0, 1.0, 0.0), 1),
        ColorStrip::fill(LinearColor::new(0.0, 0.0, 1.0), 1),
        ColorStrip::fill(LinearColor::new(1.0, 1.0, 1.0), 1),
    );
    for enable_border in [false, true] {
        let (_, receiver) = tokio::sync::watch::channel(colors.clone());
        let app = DesktopRendererBuilder::new(receiver)
            .with_layout(LayoutConfig {
                total_led_count: None,
                enable_border,
                ..LayoutConfig::default()
            })
            .build();
        let context = egui::Context::default();
        let mut expected = Vec::new();
        let output = context.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let layout = LayoutDimensions::calculate(
                    ui.available_rect_before_wrap(),
                    &app.layout_config,
                );
                expected = vec![
                    (egui::Color32::RED, layout.top_strip),
                    (egui::Color32::BLUE, layout.bottom_strip),
                    (egui::Color32::WHITE, layout.left_strip),
                    (egui::Color32::GREEN, layout.right_strip),
                ];
                app.draw_output(ui, &colors);
            });
        });
        let leds: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.rect.size() == egui::vec2(10.0, 10.0) => Some(rect),
                _ => None,
            })
            .collect();
        assert_eq!(leds.len(), 4);
        for (led, (color, strip)) in leds.iter().zip(expected) {
            assert_eq!(led.fill, color);
            assert!(strip.contains_rect(led.rect));
        }
    }
}

#[test]
fn displays_final_frame_after_sender_closes() {
    let (_colors, colors_rx) = tokio::sync::watch::channel(EdgeColors::default());
    let (frames, frames_rx) = tokio::sync::watch::channel(DisplayFrame {
        frame: Frame::dummy(4, 4),
        viewport: None,
    });
    let mut app = DesktopRendererBuilder::new(colors_rx)
        .with_frame_rx(frames_rx)
        .build();
    let context = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let _ = context.run(Default::default(), |ctx| app.update(ctx, &mut frame));
    frames.send_replace(DisplayFrame {
        frame: Frame::fill(255, 4, 4, PixelFormat::RGB24),
        viewport: Some(Viewport::full_frame(4, 4)),
    });
    drop(frames);

    let output = context.run(Default::default(), |ctx| app.update(ctx, &mut frame));
    let texture = app.texture_handle.as_ref().unwrap().id();
    let (_, update) = output
        .textures_delta
        .set
        .iter()
        .find(|(id, _)| *id == texture)
        .unwrap();
    let egui::ImageData::Color(image) = &update.image;
    assert!(
        image
            .pixels
            .iter()
            .all(|pixel| *pixel == egui::Color32::WHITE)
    );
    assert_eq!(
        app.crop_rect,
        Some(egui::Rect::from_min_max(
            egui::Pos2::ZERO,
            egui::pos2(1.0, 1.0)
        ))
    );
    let output = context.run(Default::default(), |ctx| app.update(ctx, &mut frame));
    assert!(
        output
            .textures_delta
            .set
            .iter()
            .all(|(id, _)| *id != texture)
    );
}

#[test]
fn crop_outline_tracks_the_displayed_frame_and_window_size() {
    let (_colors, colors_rx) = tokio::sync::watch::channel(EdgeColors::default());
    let (frames, frames_rx) = tokio::sync::watch::channel(DisplayFrame {
        frame: Frame::dummy(640, 360),
        viewport: None,
    });
    let mut app = DesktopRendererBuilder::new(colors_rx)
        .with_frame_rx(frames_rx)
        .build();
    let context = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    for (size, viewport) in [
        (
            [960.0, 540.0],
            Viewport {
                x: 64,
                y: 36,
                width: 512,
                height: 288,
            },
        ),
        ([1280.0, 720.0], Viewport::full_frame(640, 360)),
    ] {
        frames
            .send(DisplayFrame {
                frame: Frame::dummy(640, 360),
                viewport: Some(viewport),
            })
            .unwrap();
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                ..Default::default()
            },
            |ctx| app.update(ctx, &mut frame),
        );
        let video = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.calc_bounds())
                }
                _ => None,
            })
            .expect("video texture");
        let outline = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.color == egui::Color32::from_rgb(57, 255, 20) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("neon crop outline");
        let expected_min = video.min
            + video.size() * egui::vec2(viewport.x as f32 / 640.0, viewport.y as f32 / 360.0);
        let expected_size = video.size()
            * egui::vec2(
                viewport.width as f32 / 640.0,
                viewport.height as f32 / 360.0,
            );
        assert!(outline.min.distance(expected_min) < 0.01);
        assert!((outline.size() - expected_size).length() < 0.01);
    }
}

#[test]
fn schedules_output_refreshes_at_default_and_configured_rates() {
    for (rate, nanos) in [
        (None, 33_333_334),
        (Some(2), 500_000_000),
        (Some(60), 16_666_667),
        (Some(u32::MAX), 1),
    ] {
        let (_sender, receiver) = tokio::sync::watch::channel(EdgeColors::default());
        let mut builder = DesktopRendererBuilder::new(receiver);
        if let Some(rate) = rate {
            builder = builder.with_refresh_rate_hz(NonZeroU32::new(rate).unwrap());
        }
        let mut app = builder.build();
        let context = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let mut output = egui::FullOutput::default();
        // Let egui's initial layout repaints settle before checking the timer.
        for _ in 0..3 {
            output = context.run(
                egui::RawInput {
                    predicted_dt: 0.0,
                    ..Default::default()
                },
                |ctx| app.update(ctx, &mut frame),
            );
        }
        assert_eq!(
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay,
            Duration::from_nanos(nanos)
        );
    }
}

#[test]
fn shutdown_requests_an_immediate_repaint() {
    let (_sender, receiver) = tokio::sync::watch::channel(EdgeColors::default());
    let shutdown = CancellationToken::new();
    let app = DesktopRendererBuilder::new(receiver)
        .with_shutdown_token(&shutdown)
        .build();
    let context = egui::Context::default();
    let (sender, receiver) = std::sync::mpsc::channel();
    context.set_request_repaint_callback(move |info| {
        sender.send(info.delay).unwrap();
    });
    spawn_shutdown_notifier(&app, context);
    shutdown.cancel();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
        Duration::ZERO
    );
}

#[test]
fn led_distribution_preserves_total_including_small_and_odd_counts() {
    assert_eq!(calculate_led_distribution(1920, 1080, 5), [2, 1, 1, 1]);
    assert_eq!(
        calculate_led_distribution(1920, 1080, 300),
        [96, 54, 96, 54]
    );
    for (width, height) in [(1920, 1080), (1080, 1920), (1, 1), (0, 0), (1, 1000)] {
        for total in 0..=301 {
            let [top, right, bottom, left] = calculate_led_distribution(width, height, total);
            assert_eq!(top + right + bottom + left, total);
            assert!(top.abs_diff(bottom) <= 1);
            assert!(right.abs_diff(left) <= 1);
        }
    }
}

#[test]
fn zero_leds_draw_no_shapes() {
    let (_, receiver) = tokio::sync::watch::channel(EdgeColors::default());
    let app = DesktopRendererBuilder::new(receiver).build();
    let context = egui::Context::default();
    let _ = context.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let before = ctx.graphics_mut(|g| g.entry(ui.layer_id()).all_entries().count());
            app.render_discrete_leds(
                ui,
                &ColorStrip::default(),
                ui.max_rect(),
                EdgePosition::Top,
                0,
            );
            let after = ctx.graphics_mut(|g| g.entry(ui.layer_id()).all_entries().count());
            assert_eq!(before, after);
        });
    });
}

#[test]
fn limited_range_output_displays_black_and_white() {
    let mut frame = Frame::new(vec![16, 128, 235, 128], 2, 1, PixelFormat::YUYV);
    frame.yuv_range = prysm_capture::YuvRange::Limited;
    frame.yuv_matrix = prysm_capture::YuvStandardMatrix::Bt709;
    let image = frame_to_image(&frame, &mut Vec::new()).unwrap();
    assert_eq!(image.pixels, [egui::Color32::BLACK, egui::Color32::WHITE]);
}

#[test]
fn bgr_frames_display_in_rgb_order() {
    let frame = Frame::new(vec![10, 20, 200, 255, 0, 0], 2, 1, PixelFormat::BGR24);
    let image = frame_to_image(&frame, &mut Vec::new()).unwrap();
    assert_eq!(image.size, [2, 1]);
    assert_eq!(
        image.pixels,
        [egui::Color32::from_rgb(200, 20, 10), egui::Color32::BLUE]
    );
}
