use prysm_capture::{Frame, PixelFormat};
use prysm_core::{Color, EdgeSpectra, Spectrum};
use tokio_util::sync::CancellationToken;

pub struct DesktopRendererBuilder {
    layout_config: LayoutConfig,
    shutdown_token: Option<CancellationToken>,
    spectrum_rx: tokio::sync::watch::Receiver<EdgeSpectra>,
    frame_rx: Option<tokio::sync::watch::Receiver<Frame>>,
}

impl std::fmt::Debug for DesktopRendererBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopRenderer")
            .field("shutdown_token", &"<CancellationToken>")
            .field("spectrum_rx", &self.spectrum_rx)
            .field("frame_rx", &self.frame_rx)
            .field("layout_config", &self.layout_config)
            .finish()
    }
}

impl DesktopRendererBuilder {
    #[must_use]
    pub fn new(spectrum_rx: tokio::sync::watch::Receiver<EdgeSpectra>) -> Self {
        Self {
            frame_rx: None,
            shutdown_token: None,
            spectrum_rx,
            layout_config: LayoutConfig::default(),
        }
    }

    #[must_use]
    pub fn with_frame_rx(mut self, frame_rx: tokio::sync::watch::Receiver<Frame>) -> Self {
        self.frame_rx = Some(frame_rx);
        self
    }

    #[must_use]
    pub fn with_shutdown_token(mut self, token: &CancellationToken) -> Self {
        self.shutdown_token = Some(token.clone());
        self
    }

    /// Configure layout dimensions
    #[must_use]
    pub fn with_layout(mut self, config: LayoutConfig) -> Self {
        self.layout_config = config;
        self
    }

    /// Set LED strip width
    #[must_use]
    pub fn with_led_strip_width(mut self, width_px: f32) -> Self {
        self.layout_config.led_strip_width_px = width_px;
        self
    }

    /// Configure border
    #[must_use]
    pub fn with_border(mut self, width_px: f32, enabled: bool) -> Self {
        self.layout_config.border_width_px = width_px;
        self.layout_config.enable_border = enabled;
        self
    }

    /// Configure LED appearance
    #[must_use]
    pub fn with_led_size(mut self, size_px: f32) -> Self {
        self.layout_config.led_size_px = size_px;
        self
    }

    #[must_use]
    pub fn build(self) -> DesktopRenderer {
        DesktopRenderer {
            spectrum_rx: self.spectrum_rx,
            shutdown_token: self.shutdown_token,
            frame_rx: self.frame_rx,
            texture_handle: None,
            layout_config: self.layout_config,
            rgb_scratch: Vec::new(),
        }
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

    /// Total LED count around entire perimeter (if None, use EdgeSpectrum sample counts directly)
    pub total_led_count: Option<usize>,

    /// Target frames per second for rendering updates
    pub target_fps: u32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            led_strip_width_px: 60.0,
            border_width_px: 10.0,
            enable_border: true,
            led_size_px: 10.0,
            total_led_count: Some(300),
            target_fps: 30,
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
pub struct DesktopRenderer {
    spectrum_rx: tokio::sync::watch::Receiver<EdgeSpectra>,
    frame_rx: Option<tokio::sync::watch::Receiver<Frame>>,
    shutdown_token: Option<CancellationToken>,
    texture_handle: Option<egui::TextureHandle>,
    layout_config: LayoutConfig,
    /// Scratch buffer reused across frames to avoid a large per-frame allocation
    rgb_scratch: Vec<u8>,
}

/// Calculate LED counts per edge for uniform spacing
///
/// Distributes total LED count proportionally to edge lengths to achieve
/// consistent spacing around the perimeter (like real LED strips).
fn calculate_led_distribution(width: usize, height: usize, total_leds: usize) -> (usize, usize) {
    assert!(total_leds > 0, "Total LED count must be positive");

    let perimeter = 2.0 * (width as f32 + height as f32);
    let horizontal_fraction = width as f32 / perimeter;
    let vertical_fraction = height as f32 / perimeter;

    let horizontal_leds = (total_leds as f32 * horizontal_fraction).round() as usize;
    let vertical_leds = (total_leds as f32 * vertical_fraction).round() as usize;

    // Ensure at least 1 LED per edge
    (horizontal_leds.max(1), vertical_leds.max(1))
}

impl eframe::App for DesktopRenderer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self
            .shutdown_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            tracing::info!("Shutdown signal received, closing window");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Poll for new spectra (non-blocking)
        let spectra = self.spectrum_rx.borrow_and_update().clone();

        // Poll for new frames (non-blocking) and update texture when needed
        if let Some(frame_rx) = self.frame_rx.as_mut() {
            let has_changed = frame_rx.has_changed().unwrap_or_default();
            let needs_initial_texture = self.texture_handle.is_none();

            if has_changed || needs_initial_texture {
                // Clone is cheap (Arc'd data) and releases the watch lock
                // before the conversion below, so the sender is never blocked.
                let frame = frame_rx.borrow_and_update().clone();
                let rgb_data: Option<&[u8]> = match frame.format {
                    PixelFormat::RGB24 => Some(&frame.data),
                    PixelFormat::YUYV => {
                        prysm_capture::yuyv::yuyv_to_rgb_into(
                            &frame.data,
                            &mut self.rgb_scratch,
                            frame.width as usize,
                            frame.height as usize,
                        );
                        Some(&self.rgb_scratch)
                    }
                    PixelFormat::MJPEG | PixelFormat::BGR24 => None,
                };

                // Convert RGB data to ColorImage and update texture
                if let Some(rgb_data) = rgb_data {
                    let color_image = egui::ColorImage::from_rgb(
                        [frame.width as usize, frame.height as usize],
                        rgb_data,
                    );

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

        // Draw the UI
        egui::CentralPanel::default().show(ctx, |ui| {
            let available = ui.available_rect_before_wrap();

            // Calculate all layout dimensions
            let layout = LayoutDimensions::calculate(available, &self.layout_config);

            // Determine LED counts per edge based on actual rendered dimensions
            let (horizontal_display_leds, vertical_display_leds) =
                if let Some(total_leds) = self.layout_config.total_led_count {
                    // Use uniform spacing distribution based on actual GUI window size
                    let render_width = available.width() as usize;
                    let render_height = available.height() as usize;
                    calculate_led_distribution(render_width, render_height, total_leds)
                } else {
                    // Fall back to EdgeSpectrum sample counts (current behavior)
                    (spectra.top.len(), spectra.left.len())
                };

            // Render LED strips (excluding corners)
            self.render_discrete_leds(
                ui,
                &spectra.top,
                layout.top_strip,
                EdgePosition::Top,
                horizontal_display_leds,
            );
            self.render_discrete_leds(
                ui,
                &spectra.bottom,
                layout.bottom_strip,
                EdgePosition::Bottom,
                horizontal_display_leds,
            );
            self.render_discrete_leds(
                ui,
                &spectra.left,
                layout.left_strip,
                EdgePosition::Left,
                vertical_display_leds,
            );
            self.render_discrete_leds(
                ui,
                &spectra.right,
                layout.right_strip,
                EdgePosition::Right,
                vertical_display_leds,
            );

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

        // Request continuous repaints at target FPS to keep polling for new frames/spectra
        // This is necessary because egui only calls update() when requested - without
        // continuous repaints, we'd stop polling for new data when no UI events occur
        let frame_duration_ms = 1000 / self.layout_config.target_fps.max(1);
        ctx.request_repaint_after(std::time::Duration::from_millis(frame_duration_ms as u64));
    }
}

impl DesktopRenderer {
    /// Render discrete individual LEDs along an edge
    fn render_discrete_leds(
        &self,
        ui: &mut egui::Ui,
        spectrum: &Spectrum,
        rect: egui::Rect,
        edge: EdgePosition,
        display_led_count: usize,
    ) {
        let led_size = self.layout_config.led_size_px;

        // Resample spectrum to display LED count for uniform spacing
        let colors = spectrum.quantize(display_led_count);

        // Calculate spacing between LED centers along the edge
        let spacing = match edge {
            EdgePosition::Top | EdgePosition::Bottom => rect.width() / display_led_count as f32,
            EdgePosition::Left | EdgePosition::Right => rect.height() / display_led_count as f32,
        };

        for i in 0..display_led_count {
            let color = colors[i].to_srgb();
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
/// * `app` - The configured PrysmApp instance
/// * `shutdown_token` - Token to signal shutdown when window closes
pub fn run(
    app: DesktopRenderer,
    shutdown_token: &CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1920.0, 1080.0])
            .with_title("Prysm - Ambient Lighting"),
        ..Default::default()
    };

    let result = eframe::run_native("Prysm", options, Box::new(move |_cc| Ok(Box::new(app))));

    // Signal shutdown when window closes
    tracing::info!("Window closed, signaling shutdown to runtime thread");
    shutdown_token.cancel();

    result.map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}
