#![no_main]

use std::collections::HashMap;
use futures::Stream;
use prysm_capture::Frame;
use prysm_render::{Color, PrysmRenderer, Zone};

#[derive(Debug)]
pub struct DesktopRenderer {
    frame_rx: Option<tokio::sync::broadcast::Receiver<Frame>>,
}

impl DesktopRenderer {
    pub fn new() -> Self {
        Self { frame_rx: None }
    }

    pub fn with_frames(mut self, rx: tokio::sync::broadcast::Receiver<Frame>) -> Self {
        self.frame_rx = Some(rx);
        self
    }
}

impl Default for DesktopRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper function to convert prysm Color to egui Color32
fn color_to_egui(color: Color) -> egui::Color32 {
    egui::Color32::from_rgb(color.r, color.g, color.b)
}

/// Helper function to calculate the rectangle for a zone within the available UI space
fn zone_rect(zone: Zone, ui_rect: egui::Rect) -> egui::Rect {
    let width = ui_rect.width();
    let height = ui_rect.height();

    // Zone dimensions as proportions
    let left_right_width = width * 0.1875;  // 150/800 = 0.1875
    let center_width = width * 0.625;       // 500/800 = 0.625
    let top_bottom_height = height * 0.1667; // 100/600 = 0.1667
    let middle_height = height * 0.6667;    // 400/600 = 0.6667

    let left_x = ui_rect.left();
    let center_x = left_x + left_right_width;
    let right_x = center_x + center_width;

    let top_y = ui_rect.top();
    let middle_y = top_y + top_bottom_height;
    let bottom_y = middle_y + middle_height;

    match zone {
        Zone::TopLeft => egui::Rect::from_min_size(
            egui::pos2(left_x, top_y),
            egui::vec2(left_right_width, top_bottom_height),
        ),
        Zone::Top => egui::Rect::from_min_size(
            egui::pos2(center_x, top_y),
            egui::vec2(center_width, top_bottom_height),
        ),
        Zone::TopRight => egui::Rect::from_min_size(
            egui::pos2(right_x, top_y),
            egui::vec2(left_right_width, top_bottom_height),
        ),
        Zone::Left => egui::Rect::from_min_size(
            egui::pos2(left_x, middle_y),
            egui::vec2(left_right_width, middle_height),
        ),
        Zone::Right => egui::Rect::from_min_size(
            egui::pos2(right_x, middle_y),
            egui::vec2(left_right_width, middle_height),
        ),
        Zone::BottomLeft => egui::Rect::from_min_size(
            egui::pos2(left_x, bottom_y),
            egui::vec2(left_right_width, top_bottom_height),
        ),
        Zone::Bottom => egui::Rect::from_min_size(
            egui::pos2(center_x, bottom_y),
            egui::vec2(center_width, top_bottom_height),
        ),
        Zone::BottomRight => egui::Rect::from_min_size(
            egui::pos2(right_x, bottom_y),
            egui::vec2(left_right_width, top_bottom_height),
        ),
    }
}

/// Helper function to calculate the center rectangle for video display
fn center_video_rect(ui_rect: egui::Rect) -> egui::Rect {
    let width = ui_rect.width();
    let height = ui_rect.height();

    // Center rectangle dimensions (same proportions as the center area)
    let left_right_width = width * 0.1875;  // 150/800 = 0.1875
    let center_width = width * 0.625;       // 500/800 = 0.625
    let top_bottom_height = height * 0.1667; // 100/600 = 0.1667
    let middle_height = height * 0.6667;    // 400/600 = 0.6667

    let center_x = ui_rect.left() + left_right_width;
    let middle_y = ui_rect.top() + top_bottom_height;

    egui::Rect::from_min_size(
        egui::pos2(center_x, middle_y),
        egui::vec2(center_width, middle_height),
    )
}

/// The eframe application that displays zone colors
struct PrysmApp {
    colors: HashMap<Zone, Color>,
    rx: tokio::sync::mpsc::Receiver<HashMap<Zone, Color>>,
    frame_rx: Option<tokio::sync::mpsc::Receiver<Frame>>,
    texture_handle: Option<egui::TextureHandle>,
}

impl eframe::App for PrysmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for new colors (non-blocking)
        while let Ok(colors) = self.rx.try_recv() {
            self.colors = colors;
        }

        // Poll for new frames (non-blocking)
        if let Some(ref mut rx) = self.frame_rx {
            while let Ok(frame) = rx.try_recv() {
                // Convert Frame to ColorImage
                let color_image = egui::ColorImage::from_rgb(
                    [frame.width as usize, frame.height as usize],
                    &frame.data,
                );

                // Update or create texture
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

        // Draw the UI
        egui::CentralPanel::default().show(ctx, |ui| {
            let available = ui.available_rect_before_wrap();
            let default_color = Color::black();

            // Draw each zone as a colored rectangle
            for zone in Zone::all() {
                let color = self.colors.get(zone).unwrap_or(&default_color);
                let rect = zone_rect(*zone, available);
                ui.painter().rect_filled(rect, 0.0, color_to_egui(*color));
            }

            // Draw video feed in center rectangle
            if let Some(texture) = &self.texture_handle {
                let video_rect = center_video_rect(available);
                ui.painter().image(
                    texture.id(),
                    video_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        });

        // Request continuous repainting for smooth updates
        ctx.request_repaint();
    }
}

impl PrysmRenderer for DesktopRenderer {
    fn run(&mut self, input: impl Stream<Item = HashMap<Zone, Color>> + Send + 'static) {
        // 1. Create mpsc channel for async->sync bridge (colors)
        let (tx, rx) = tokio::sync::mpsc::channel(10);

        // 2. Spawn async task to consume color stream
        tokio::spawn(async move {
            futures::pin_mut!(input);
            use futures::StreamExt;
            while let Some(colors) = input.next().await {
                let _ = tx.send(colors).await;
            }
        });

        // 3. Handle frame receiver if present
        let frame_rx_mpsc = if let Some(mut broadcast_rx) = std::mem::take(&mut self.frame_rx) {
            let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(10);

            // Spawn async task to bridge broadcast receiver to mpsc sender
            tokio::spawn(async move {
                while let Ok(frame) = broadcast_rx.recv().await {
                    if frame_tx.send(frame).await.is_err() {
                        break; // Receiver dropped
                    }
                }
            });

            Some(frame_rx)
        } else {
            None
        };

        // 4. Initialize app state with black zones
        let mut colors = HashMap::new();
        for zone in Zone::all() {
            colors.insert(*zone, Color::black());
        }

        let app = PrysmApp {
            colors,
            rx,
            frame_rx: frame_rx_mpsc,
            texture_handle: None,
        };

        // 5. Configure and run the native window
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([800.0, 600.0])
                .with_title("Prysm - Ambient Lighting Zones"),
            ..Default::default()
        };

        // 6. Run the app (blocking, consumes thread)
        let _ = eframe::run_native(
            "Prysm",
            options,
            Box::new(|_cc| Ok(Box::new(app))),
        );
    }
}