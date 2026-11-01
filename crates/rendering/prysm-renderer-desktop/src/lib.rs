//! Native video output with sampled edge colors and a detected-crop overlay.
mod builder;
mod image;
mod layout;

pub use builder::DesktopRendererBuilder;
pub use eframe::egui;
pub use layout::LayoutConfig;

use image::frame_to_image;
use layout::{LayoutDimensions, calculate_led_distribution};
use prysm_core::{ColorStrip, EdgeColors, Frame, Viewport};
use tokio_util::sync::CancellationToken;

/// Keep the detected crop paired with the frame it describes.
#[derive(Debug, Clone)]
pub struct DisplayFrame {
    pub frame: Frame,
    pub viewport: Option<Viewport>,
}

type Toolbar = Box<dyn FnMut(&mut egui::Ui) + Send>;

/// The eframe application that displays edge color gradients
pub struct DesktopRenderer {
    toolbar: Option<Toolbar>,
    edge_colors_rx: tokio::sync::watch::Receiver<EdgeColors>,
    frame_rx: Option<tokio::sync::watch::Receiver<DisplayFrame>>,
    shutdown_token: Option<CancellationToken>,
    texture_handle: Option<egui::TextureHandle>,
    crop_rect: Option<egui::Rect>,
    layout_config: LayoutConfig,
    /// Scratch buffer reused across frames to avoid a large per-frame allocation
    rgb_scratch: Vec<u8>,
}

impl eframe::App for DesktopRenderer {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self
            .shutdown_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            tracing::info!("Shutdown signal received, closing window");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let edge_colors = self.edge_colors_rx.borrow_and_update().clone();

        self.update_video_texture(ui.ctx());
        if let Some(toolbar) = self.toolbar.as_mut() {
            egui::Panel::top("capture_controls").show(ui, |ui| toolbar(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| self.draw_output(ui, &edge_colors));
    }
}

impl DesktopRenderer {
    #[allow(
        clippy::cast_precision_loss,
        reason = "pixel coordinates are converted to approximate normalized GUI coordinates"
    )]
    fn update_video_texture(&mut self, ctx: &egui::Context) {
        if let Some(frame_rx) = self.frame_rx.as_mut() {
            let has_changed = frame_rx.borrow().has_changed();
            let needs_initial_texture = self.texture_handle.is_none();

            if has_changed || needs_initial_texture {
                // Clone is cheap (Arc'd data) and releases the watch lock
                // before the conversion below, so the sender is never blocked.
                let display = frame_rx.borrow_and_update().clone();
                let frame = display.frame;
                if let Some(color_image) = frame_to_image(&frame, &mut self.rgb_scratch) {
                    self.crop_rect = display.viewport.map(|crop| {
                        egui::Rect::from_min_max(
                            egui::pos2(
                                crop.x as f32 / frame.width() as f32,
                                crop.y as f32 / frame.height() as f32,
                            ),
                            egui::pos2(
                                (crop.x + crop.width) as f32 / frame.width() as f32,
                                (crop.y + crop.height) as f32 / frame.height() as f32,
                            ),
                        )
                    });
                    if let Some(texture) = &mut self.texture_handle {
                        texture.set(color_image, egui::TextureOptions::LINEAR);
                    } else {
                        self.texture_handle = Some(ctx.load_texture(
                            "video_feed",
                            color_image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
            }
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "available GUI dimensions are quantized to nonnegative pixel counts"
    )]
    fn draw_output(&self, ui: &mut egui::Ui, edge_colors: &EdgeColors) {
        let available = ui.available_rect_before_wrap();

        let layout = LayoutDimensions::calculate(available, &self.layout_config);

        let [top_leds, right_leds, bottom_leds, left_leds] =
            if let Some(total_leds) = self.layout_config.total_led_count {
                let render_width = available.width() as usize;
                let render_height = available.height() as usize;
                calculate_led_distribution(render_width, render_height, total_leds)
            } else {
                [
                    edge_colors.top.len(),
                    edge_colors.right.len(),
                    edge_colors.bottom.len(),
                    edge_colors.left.len(),
                ]
            };

        for (strip, rect, edge, count) in [
            (
                &edge_colors.top,
                layout.top_strip,
                EdgePosition::Top,
                top_leds,
            ),
            (
                &edge_colors.bottom,
                layout.bottom_strip,
                EdgePosition::Bottom,
                bottom_leds,
            ),
            (
                &edge_colors.left,
                layout.left_strip,
                EdgePosition::Left,
                left_leds,
            ),
            (
                &edge_colors.right,
                layout.right_strip,
                EdgePosition::Right,
                right_leds,
            ),
        ] {
            self.render_discrete_leds(ui, strip, rect, edge, count);
        }

        for border in [
            layout.top_border,
            layout.bottom_border,
            layout.left_border,
            layout.right_border,
        ]
        .into_iter()
        .flatten()
        {
            ui.painter().rect_filled(border, 0.0, egui::Color32::BLACK);
        }

        if let Some(texture) = &self.texture_handle {
            ui.painter().image(
                texture.id(),
                layout.video_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            if let Some(crop) = self.crop_rect {
                let transform = egui::emath::RectTransform::from_to(
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    layout.video_rect,
                );
                let rect = transform.transform_rect(crop);
                for (width, color) in [
                    (5.0, egui::Color32::BLACK),
                    (3.0, egui::Color32::from_rgb(57, 255, 20)),
                ] {
                    ui.painter()
                        .rect_stroke(rect, 0.0, (width, color), egui::StrokeKind::Inside);
                }
            }
        }
    }

    /// Render discrete individual LEDs along an edge
    #[allow(
        clippy::cast_precision_loss,
        reason = "LED indices are converted to approximate GUI coordinates"
    )]
    fn render_discrete_leds(
        &self,
        ui: &mut egui::Ui,
        strip: &ColorStrip,
        rect: egui::Rect,
        edge: EdgePosition,
        display_led_count: usize,
    ) {
        if display_led_count == 0 {
            return;
        }
        let led_size = self.layout_config.led_size_px;

        let spacing = match edge {
            EdgePosition::Top | EdgePosition::Bottom => rect.width() / display_led_count as f32,
            EdgePosition::Left | EdgePosition::Right => rect.height() / display_led_count as f32,
        };

        for i in 0..display_led_count {
            let Some(color) = strip.color_at(i, display_led_count) else {
                continue;
            };
            let color = color.to_srgb();
            let egui_color = egui::Color32::from_rgb(color.r, color.g, color.b);

            // Calculate LED center position based on edge
            // Position LEDs close to the TV (inner edge of LED strip)
            let (center_x, center_y) = match edge {
                EdgePosition::Top => {
                    let x = rect.left() + (i as f32 + 0.5) * spacing;
                    let y = rect.bottom() - led_size / 2.0; // Bottom of top strip (closer to video)
                    (x, y)
                }
                EdgePosition::Bottom => {
                    let x = rect.left() + (i as f32 + 0.5) * spacing;
                    let y = rect.top() + led_size / 2.0; // Top of bottom strip (closer to video)
                    (x, y)
                }
                EdgePosition::Left => {
                    let x = rect.right() - led_size / 2.0; // Right of left strip (closer to video)
                    let y = rect.top() + (i as f32 + 0.5) * spacing;
                    (x, y)
                }
                EdgePosition::Right => {
                    let x = rect.left() + led_size / 2.0; // Left of right strip (closer to video)
                    let y = rect.top() + (i as f32 + 0.5) * spacing;
                    (x, y)
                }
            };

            let led_rect = egui::Rect::from_center_size(
                egui::pos2(center_x, center_y),
                egui::vec2(led_size, led_size),
            );

            ui.painter().rect_filled(led_rect, 1.0, egui_color);
        }
    }
}

/// Edge position identifier for LED rendering
#[derive(Debug, Clone, Copy)]
enum EdgePosition {
    Top,
    Bottom,
    Left,
    Right,
}

/// Wake the GUI when processed colors arrive or shutdown is requested.
fn spawn_repaint_notifier(app: &DesktopRenderer, ctx: egui::Context) {
    let shutdown = app.shutdown_token.clone().unwrap_or_default();
    let mut colors = app.edge_colors_rx.clone();
    std::thread::spawn(move || {
        futures::executor::block_on(async move {
            loop {
                let changed = {
                    let changed = colors.changed();
                    let cancelled = shutdown.cancelled();
                    futures::pin_mut!(changed, cancelled);
                    match futures::future::select(changed, cancelled).await {
                        futures::future::Either::Left((result, _)) => result.is_ok(),
                        futures::future::Either::Right(_) => false,
                    }
                };
                ctx.request_repaint();
                if !changed {
                    break;
                }
            }
        });
    });
}

/// Run the desktop renderer on the main thread (blocking).
///
/// This function must be called from the main thread. It will:
/// 1. Call `eframe::run_native()` (blocking)
/// 2. Signal shutdown when the window closes
///
/// # Threading
/// This function blocks the calling thread until the window is closed.
/// Async work should be running on a separate runtime thread.
///
/// # Arguments
/// * `app` - The configured desktop renderer
/// * `shutdown_token` - Token to signal shutdown when window closes
pub fn run(mut app: DesktopRenderer, shutdown_token: &CancellationToken) -> eframe::Result {
    app.shutdown_token
        .get_or_insert_with(|| shutdown_token.clone());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 600.0])
            .with_title("Prysm - Ambient Lighting"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "Prysm",
        options,
        Box::new(move |cc| {
            spawn_repaint_notifier(&app, cc.egui_ctx.clone());
            Ok(Box::new(app))
        }),
    );

    // Signal shutdown when window closes
    tracing::info!("Window closed, signaling shutdown to runtime thread");
    shutdown_token.cancel();

    result
}

#[cfg(test)]
mod tests;
