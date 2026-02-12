#![no_main]

use futures::Stream;
use prysm_capture::Frame;
use prysm_render::{Color, EdgeSpectrums, PrysmRenderer};

#[derive(Debug)]
pub struct DesktopRenderer {
    frame_rx: Option<tokio::sync::broadcast::Receiver<Frame>>,
    layout_config: LayoutConfig,
}

impl DesktopRenderer {
    pub fn new() -> Self {
        Self {
            frame_rx: None,
            layout_config: LayoutConfig::default(),
        }
    }

    pub fn with_frames(mut self, rx: tokio::sync::broadcast::Receiver<Frame>) -> Self {
        self.frame_rx = Some(rx);
        self
    }

    /// Configure layout dimensions
    pub fn with_layout(mut self, config: LayoutConfig) -> Self {
        self.layout_config = config;
        self
    }

    /// Set LED strip width
    pub fn with_led_strip_width(mut self, width_px: f32) -> Self {
        self.layout_config.led_strip_width_px = width_px;
        self
    }

    /// Configure border
    pub fn with_border(mut self, width_px: f32, enabled: bool) -> Self {
        self.layout_config.border_width_px = width_px;
        self.layout_config.enable_border = enabled;
        self
    }

    /// Configure LED appearance
    pub fn with_led_size(mut self, size_px: f32) -> Self {
        self.layout_config.led_size_px = size_px;
        self
    }

    /// Configure LED spacing
    pub fn with_led_spacing(mut self, spacing_ratio: f32) -> Self {
        self.layout_config.led_spacing_ratio = spacing_ratio;
        self
    }
}

impl Default for DesktopRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for desktop renderer layout
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    /// Width of LED strip representation in pixels (uniform for all edges)
    pub led_strip_width_px: f32,

    /// Width of border between TV and LED strips in pixels
    pub border_width_px: f32,

    /// Whether to render the border
    pub enable_border: bool,

    /// Size of individual LED squares in pixels
    pub led_size_px: f32,

    /// Spacing between LEDs as a fraction of LED size (default: 0.3 = 30% spacing)
    pub led_spacing_ratio: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            led_strip_width_px: 60.0,
            border_width_px: 10.0,
            enable_border: true,
            led_size_px: 8.0,
            led_spacing_ratio: 0.3,
        }
    }
}

/// Calculated layout dimensions for all UI elements
struct LayoutDimensions {
    top_strip: egui::Rect,
    bottom_strip: egui::Rect,
    left_strip: egui::Rect,
    right_strip: egui::Rect,
    top_border: Option<egui::Rect>,
    bottom_border: Option<egui::Rect>,
    left_border: Option<egui::Rect>,
    right_border: Option<egui::Rect>,
    video_rect: egui::Rect,
}

impl LayoutDimensions {
    /// Calculate layout from available space and configuration
    fn calculate(available: egui::Rect, config: &LayoutConfig) -> Self {
        let led_width = config.led_strip_width_px;
        let border_width = if config.enable_border {
            config.border_width_px
        } else {
            0.0
        };

        // Calculate LED strip rectangles (excluding corners)
        let top_strip = egui::Rect::from_min_size(
            egui::pos2(available.left() + led_width, available.top()),
            egui::vec2(available.width() - 2.0 * led_width, led_width),
        );

        let bottom_strip = egui::Rect::from_min_size(
            egui::pos2(available.left() + led_width, available.bottom() - led_width),
            egui::vec2(available.width() - 2.0 * led_width, led_width),
        );

        let left_strip = egui::Rect::from_min_size(
            egui::pos2(available.left(), available.top() + led_width),
            egui::vec2(led_width, available.height() - 2.0 * led_width),
        );

        let right_strip = egui::Rect::from_min_size(
            egui::pos2(available.right() - led_width, available.top() + led_width),
            egui::vec2(led_width, available.height() - 2.0 * led_width),
        );

        // Calculate border rectangles (if enabled)
        let (top_border, bottom_border, left_border, right_border) = if config.enable_border {
            let inner_top = available.top() + led_width;
            let inner_bottom = available.bottom() - led_width;
            let inner_left = available.left() + led_width;
            let inner_right = available.right() - led_width;
            let inner_width = inner_right - inner_left;
            let inner_height = inner_bottom - inner_top;

            (
                Some(egui::Rect::from_min_size(
                    egui::pos2(inner_left, inner_top),
                    egui::vec2(inner_width, border_width),
                )),
                Some(egui::Rect::from_min_size(
                    egui::pos2(inner_left, inner_bottom - border_width),
                    egui::vec2(inner_width, border_width),
                )),
                Some(egui::Rect::from_min_size(
                    egui::pos2(inner_left, inner_top + border_width),
                    egui::vec2(border_width, inner_height - 2.0 * border_width),
                )),
                Some(egui::Rect::from_min_size(
                    egui::pos2(inner_right - border_width, inner_top + border_width),
                    egui::vec2(border_width, inner_height - 2.0 * border_width),
                )),
            )
        } else {
            (None, None, None, None)
        };

        // Calculate video rectangle (center area after LED strips and borders)
        let video_left = available.left() + led_width + border_width;
        let video_top = available.top() + led_width + border_width;
        let video_width = available.width() - 2.0 * (led_width + border_width);
        let video_height = available.height() - 2.0 * (led_width + border_width);

        let video_rect = egui::Rect::from_min_size(
            egui::pos2(video_left, video_top),
            egui::vec2(video_width, video_height),
        );

        Self {
            top_strip,
            bottom_strip,
            left_strip,
            right_strip,
            top_border,
            bottom_border,
            left_border,
            right_border,
            video_rect,
        }
    }
}

/// Helper function to convert prysm Color to egui Color32
fn color_to_egui(color: Color) -> egui::Color32 {
    egui::Color32::from_rgb(color.r, color.g, color.b)
}

/// The eframe application that displays edge color gradients
struct PrysmApp {
    spectrums: EdgeSpectrums,
    rx: tokio::sync::mpsc::Receiver<EdgeSpectrums>,
    frame_rx: Option<tokio::sync::mpsc::Receiver<Frame>>,
    texture_handle: Option<egui::TextureHandle>,
    layout_config: LayoutConfig,
}

impl eframe::App for PrysmApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll for new spectrums (non-blocking)
        while let Ok(spectrums) = self.rx.try_recv() {
            self.spectrums = spectrums;
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

            // Calculate all layout dimensions
            let layout = LayoutDimensions::calculate(available, &self.layout_config);

            // Render LED strips (excluding corners)
            self.render_discrete_leds(ui, &self.spectrums.top, layout.top_strip, EdgePosition::Top);
            self.render_discrete_leds(ui, &self.spectrums.bottom, layout.bottom_strip, EdgePosition::Bottom);
            self.render_discrete_leds(ui, &self.spectrums.left, layout.left_strip, EdgePosition::Left);
            self.render_discrete_leds(ui, &self.spectrums.right, layout.right_strip, EdgePosition::Right);

            // Corners use default background (no LEDs in corners)

            // Render borders (if enabled)
            if let Some(border) = layout.top_border {
                ui.painter().rect_filled(border, 0.0, egui::Color32::BLACK);
            }
            if let Some(border) = layout.bottom_border {
                ui.painter().rect_filled(border, 0.0, egui::Color32::BLACK);
            }
            if let Some(border) = layout.left_border {
                ui.painter().rect_filled(border, 0.0, egui::Color32::BLACK);
            }
            if let Some(border) = layout.right_border {
                ui.painter().rect_filled(border, 0.0, egui::Color32::BLACK);
            }

            // Render video in center rectangle
            if let Some(texture) = &self.texture_handle {
                ui.painter().image(
                    texture.id(),
                    layout.video_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        });

        // Request continuous repainting for smooth updates
        ctx.request_repaint();
    }
}

impl PrysmApp {
    /// Render discrete individual LEDs along an edge
    fn render_discrete_leds(
        &self,
        ui: &mut egui::Ui,
        spectrum: &prysm_render::ColorSpectrum,
        rect: egui::Rect,
        edge: EdgePosition,
    ) {
        let sample_count = spectrum.len();
        let led_size = self.layout_config.led_size_px;

        // Calculate spacing between LED centers along the edge
        let spacing = match edge {
            EdgePosition::Top | EdgePosition::Bottom => rect.width() / sample_count as f32,
            EdgePosition::Left | EdgePosition::Right => rect.height() / sample_count as f32,
        };

        for i in 0..sample_count {
            let color = spectrum.color_at(i, sample_count);
            let egui_color = color_to_egui(color);

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

            // Calculate LED rectangle (centered square with fixed size)
            let led_rect = egui::Rect::from_center_size(
                egui::pos2(center_x, center_y),
                egui::vec2(led_size, led_size),
            );

            // Render LED as filled square with slight corner rounding
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

impl PrysmRenderer for DesktopRenderer {
    fn run(&mut self, input: impl Stream<Item = EdgeSpectrums> + Send + 'static) {
        // 1. Create mpsc channel for async->sync bridge (edge spectrums)
        let (tx, rx) = tokio::sync::mpsc::channel(10);

        // 2. Spawn async task to consume spectrum stream
        tokio::spawn(async move {
            futures::pin_mut!(input);
            use futures::StreamExt;
            while let Some(spectrums) = input.next().await {
                let _ = tx.send(spectrums).await;
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

        // 4. Initialize app state with black spectrums
        // Using typical HD resolution for initial state (will adapt to actual frames)
        let spectrums = EdgeSpectrums::black(1920, 1080, 40);

        let app = PrysmApp {
            spectrums,
            rx,
            frame_rx: frame_rx_mpsc,
            texture_handle: None,
            layout_config: self.layout_config.clone(),
        };

        // 5. Configure and run the native window
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([800.0, 600.0])
                .with_title("Prysm - Ambient Lighting"),
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