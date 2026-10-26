//! Video rectangles and proportional LED placement.
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

    /// Exact total LED count; zero hides all LEDs. None uses each strip's sample count.
    pub total_led_count: Option<usize>,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            led_strip_width_px: 60.0,
            border_width_px: 10.0,
            enable_border: true,
            led_size_px: 10.0,
            total_led_count: Some(300),
        }
    }
}

/// Calculated layout dimensions for all UI elements
pub(super) struct LayoutDimensions {
    pub(super) top_strip: egui::Rect,
    pub(super) bottom_strip: egui::Rect,
    pub(super) left_strip: egui::Rect,
    pub(super) right_strip: egui::Rect,
    pub(super) top_border: Option<egui::Rect>,
    pub(super) bottom_border: Option<egui::Rect>,
    pub(super) left_border: Option<egui::Rect>,
    pub(super) right_border: Option<egui::Rect>,
    pub(super) video_rect: egui::Rect,
}

impl LayoutDimensions {
    /// Calculate layout from available space and configuration
    pub(super) fn calculate(available: egui::Rect, config: &LayoutConfig) -> Self {
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

/// Calculate LED counts per edge for uniform spacing
///
/// Distributes total LED count proportionally to edge lengths to achieve
/// consistent spacing around the perimeter (like real LED strips).
pub(super) fn calculate_led_distribution(
    width: usize,
    height: usize,
    total_leds: usize,
) -> [usize; 4] {
    let width = width.max(1) as f64;
    let height = height.max(1) as f64;
    let horizontal =
        ((total_leds as f64 * width / (width + height)).round() as usize).min(total_leds);
    let vertical = total_leds - horizontal;

    // Top, right, bottom, left; give odd remainders to top and right.
    [
        horizontal.div_ceil(2),
        vertical.div_ceil(2),
        horizontal / 2,
        vertical / 2,
    ]
}
