//! Colours to WLED as DDP over UDP (port 4048): resample the edges to the LED
//! counts in linear light, encode them to 8 bits for the gamma WLED applies
//! ([`Transfer`]) and send one packet stream per frame, clockwise from the
//! bottom-left corner as seen from the front.
//!
//! WLED owns brightness, channel order and the realtime timeout, and raises
//! each byte to its colour gamma before driving the LEDs; the encoding undoes
//! that, so the light is the intended linear value. By default it dithers
//! over the few levels WLED shows at the dark end (see `dither`); with
//! dithering off it rounds to the nearest byte. WLED's current limiter, RGBW
//! auto-white and colour correction are not modelled. Sending never blocks and
//! never retries: a frame whose send fails is dropped or fails the output, as
//! [`classify::send`] says for its cause. Empty frames send nothing and
//! shorter frames leave the trailing LEDs unchanged. The LED counts are
//! validated settings ([`crate::MAX_LEDS`]).

use crate::LedCounts;
use crate::classify;
use crate::dither::Encoding;
use ddp_rs::connection::DDPConnection;
use ddp_rs::error::DDPError;
use ddp_rs::protocol::{ID, PixelConfig, PixelFormat};
use prysm_core::{Color, EdgeColors, LinearColor};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};

/// How WLED turns a byte into light, and so how a linear colour is encoded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Transfer {
    /// WLED's gamma is not known (not read yet, or refused): sRGB, which the
    /// usual colour gamma of 2.2 nearly undoes.
    Srgb,
    /// WLED shows `(byte / 255)^gamma`.
    Gamma(Gamma),
}

/// The power WLED raises a colour byte (as 0–1) to: 1 when it applies none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Gamma(f32);

impl Gamma {
    /// No gamma: bytes are linear light.
    pub(crate) const LINEAR: Self = Self(1.0);

    /// A gamma WLED accepts, 0.1–3 (it reports 1 when gamma is off); `None`
    /// outside that, where WLED itself applies none.
    pub(crate) fn new(value: f32) -> Option<Self> {
        (0.1..=3.0).contains(&value).then_some(Self(value))
    }

    /// WLED's gamma table entry for `byte`: `(int)(powf(i / 255, gamma) *
    /// 255 + 0.5)` (`calcGammaTable`), the byte itself at gamma 1.
    pub(crate) fn correct(self, byte: u8) -> u8 {
        quantize((f32::from(byte) / 255.0).powf(self.0))
    }
}

impl Transfer {
    /// The bytes that WLED shows as `color`. `powf` per channel, as the sRGB
    /// encoding does: a few hundred LEDs a frame need no table.
    pub(crate) fn encode(self, color: LinearColor) -> Color {
        match self {
            Self::Srgb => color.to_srgb(),
            Self::Gamma(Gamma(gamma)) => {
                let byte = |linear: f32| quantize(linear.clamp(0.0, 1.0).powf(gamma.recip()));
                Color::new(byte(color.r), byte(color.g), byte(color.b))
            }
        }
    }
}

/// `value` in 0–1 to the nearest of 0–255.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a value within 0–1 is rounded to an 8-bit channel"
)]
fn quantize(value: f32) -> u8 {
    (value * 255.0 + 0.5) as u8
}

const RGB8: PixelConfig = PixelConfig {
    data_type: ddp_rs::protocol::DataType::RGB,
    data_size: PixelFormat::Pixel8Bits,
    customer_defined: false,
};

/// A nonblocking RGB renderer for one WLED instance.
///
/// Sends every nonempty frame immediately, without a frame queue or retransmission.
/// A failed send drops the frame or fails, by [`classify::send`]; frames that
/// cannot be built are rejected at once. UDP delivery is not acknowledged.
/// Keep sending frames before WLED's realtime timeout expires, even for a
/// static picture. Stopping lets WLED resume its effect.
#[derive(Debug)]
pub(crate) struct WledRenderer {
    connection: DDPConnection,
    rgb: Vec<[u8; 3]>,
    /// Every send fails with this instead of going out.
    #[cfg(test)]
    refusal: Option<fn() -> std::io::Error>,
}

impl WledRenderer {
    /// Resolve a WLED address (hostname or IP with port, normally 4048).
    ///
    /// Binds an ephemeral local port. Address resolution happens only here and
    /// may block; rendering uses a nonblocking socket.
    pub(crate) fn new(address: impl ToSocketAddrs) -> Result<Self, DDPError> {
        let destination = address
            .to_socket_addrs()?
            .next()
            .ok_or(DDPError::NoValidSocketAddr)?;
        let socket = bind(destination)?;
        socket.set_nonblocking(true)?;
        let connection = DDPConnection::try_new(destination, RGB8, ID::Default, socket)?;
        Ok(Self {
            connection,
            rgb: Vec::new(),
            #[cfg(test)]
            refusal: None,
        })
    }

    /// Every later send fails with `refusal()` and sends nothing.
    #[cfg(test)]
    fn refusing(mut self, refusal: fn() -> std::io::Error) -> Self {
        self.refusal = Some(refusal);
        self
    }

    /// Send 8-bit RGB colors in physical LED order, starting at LED zero.
    ///
    /// Empty input sends nothing. Use a fixed frame length: a shorter frame leaves
    /// trailing LEDs at their previous colors.
    #[cfg(test)]
    pub(crate) fn render(&mut self, colors: &[Color]) -> Result<(), DDPError> {
        if !self.prepare(colors.len()) {
            return Ok(());
        }
        for &Color { r, g, b } in colors {
            self.rgb.push([r, g, b]);
        }
        self.connection.write(self.rgb.as_flattened())?;
        Ok(())
    }

    /// Resample linear edge gradients to the given top/right/bottom/left counts.
    ///
    /// Output starts at the bottom-left corner and runs clockwise, as viewed from
    /// the screen's front: left bottom-to-top, top left-to-right, right
    /// top-to-bottom, bottom right-to-left. Zero skips an edge.
    /// Interpolation happens in linear light, followed by `encoding`, which
    /// is given each LED's place in that order.
    pub(crate) fn render_edges(
        &mut self,
        edges: &EdgeColors,
        leds: LedCounts,
        encoding: &Encoding,
    ) -> Result<(), DDPError> {
        if !self.prepare(leds.total() as usize) {
            return Ok(());
        }
        let [top, right, bottom, left] = leds.edges();
        let colors = edges
            .left
            .iter_resampled(left)
            .rev()
            .chain(edges.top.iter_resampled(top))
            .chain(edges.right.iter_resampled(right))
            .chain(edges.bottom.iter_resampled(bottom).rev());
        for (led, color) in (0..).zip(colors) {
            let Color { r, g, b } = encoding.encode(color, led);
            self.rgb.push([r, g, b]);
        }
        match self.send() {
            Ok(_) => Ok(()),
            Err(DDPError::Disconnect(error))
                if classify::send(&error) == classify::Send::FrameDropped =>
            {
                tracing::trace!(%error, "DDP frame dropped");
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// The payload to WLED, one datagram per packet.
    fn send(&mut self) -> Result<usize, DDPError> {
        #[cfg(test)]
        if let Some(refusal) = self.refusal {
            return Err(refusal().into());
        }
        self.connection.write(self.rgb.as_flattened())
    }

    /// Clears the payload for `count` LEDs; `false` for none, which sends
    /// nothing.
    fn prepare(&mut self, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        self.rgb.clear();
        self.rgb.reserve(count);
        true
    }
}

fn bind(destination: SocketAddr) -> std::io::Result<UdpSocket> {
    UdpSocket::bind(if destination.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
}

/// Ask the kernel which local address it currently uses for the DDP route.
/// UDP connect does not send a packet, and the streaming socket stays unconnected.
pub(crate) fn route_address(destination: SocketAddr) -> Option<IpAddr> {
    let socket = bind(destination).ok()?;
    socket.connect(destination).ok()?;
    let local = socket.local_addr().ok()?.ip().to_canonical();
    (!local.is_unspecified()).then_some(local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddp_rs::packet::PacketRef;
    use ddp_rs::protocol::DataType;
    use prysm_core::{ColorStrip, LinearColor};
    use std::io;
    use std::time::Duration;

    /// Top, right, bottom, left.
    fn leds([top, right, bottom, left]: [u16; 4]) -> LedCounts {
        LedCounts {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Undithered sRGB: the layout tests check where colours go, not how
    /// they are encoded.
    const NEAREST: Encoding = Encoding::Nearest(Transfer::Srgb);

    fn receiver() -> (WledRenderer, UdpSocket) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let renderer = WledRenderer::new(socket.local_addr().unwrap()).unwrap();
        (renderer, socket)
    }

    fn receive(socket: &UdpSocket) -> Vec<u8> {
        let mut bytes = [0u8; 1500];
        let size = socket.recv(&mut bytes).unwrap();
        bytes[..size].to_vec()
    }

    fn assert_no_packet(socket: &UdpSocket) {
        socket.set_nonblocking(true).unwrap();
        let error = socket.recv(&mut [0u8; 1500]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }

    #[test]
    fn sends_rgb_with_wled_header() {
        let (mut renderer, socket) = receiver();
        renderer
            .render(&[Color::new(12, 34, 56), Color::new(78, 90, 123)])
            .unwrap();
        let bytes = receive(&socket);
        assert_eq!(
            bytes,
            [0x41, 1, 0x0b, 1, 0, 0, 0, 0, 0, 6, 12, 34, 56, 78, 90, 123]
        );
        let packet = PacketRef::from_bytes(&bytes).unwrap();
        assert_eq!(packet.header.pixel_config, RGB8);
        assert_eq!(packet.header.pixel_config.data_type, DataType::RGB);
        assert_eq!(
            packet.header.pixel_config.data_size,
            PixelFormat::Pixel8Bits
        );
        assert_no_packet(&socket);
    }

    #[test]
    fn splits_frames_at_480_leds_and_pushes_only_the_last_packet() {
        for count in [480usize, 481, 960, 961] {
            let (mut renderer, socket) = receiver();
            let colors: Vec<_> = (0..count)
                .map(|i| Color::new(u8::try_from(i % 256).unwrap(), 2, 3))
                .collect();
            renderer.render(&colors).unwrap();
            let mut received = Vec::new();
            let chunks = count.div_ceil(480);
            for i in 0..chunks {
                let bytes = receive(&socket);
                let packet = PacketRef::from_bytes(&bytes).unwrap();
                assert_eq!(packet.header.offset, u32::try_from(i * 1440).unwrap());
                assert_eq!(usize::from(packet.header.length), packet.data.len());
                assert_eq!(packet.header.packet_type.push, i + 1 == chunks);
                assert!(bytes.len() <= 1450);
                received.extend_from_slice(packet.data);
            }
            let expected: Vec<_> = colors.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
            assert_eq!(received, expected);
            assert_no_packet(&socket);
        }
    }

    #[test]
    fn starts_at_bottom_left_resamples_clockwise_and_encodes_srgb() {
        let (mut renderer, socket) = receiver();
        let gradient = ColorStrip::new(vec![
            LinearColor::new(0.0, 0.0, 0.0),
            LinearColor::new(1.0, 1.0, 1.0),
        ])
        .unwrap();
        let edges = EdgeColors::new(
            gradient.clone(),
            ColorStrip::fill(LinearColor::new(1.0, 0.0, 0.0), std::num::NonZeroUsize::MIN),
            gradient,
            ColorStrip::new(vec![LinearColor::black(), LinearColor::new(0.0, 0.0, 1.0)]).unwrap(),
        );
        renderer
            .render_edges(&edges, leds([3, 1, 2, 2]), &NEAREST)
            .unwrap();
        let bytes = receive(&socket);
        let packet = PacketRef::from_bytes(&bytes).unwrap();
        assert_eq!(
            packet.data,
            [
                0, 0, 255, 0, 0, 0, // left bottom-to-top
                0, 0, 0, 188, 188, 188, 255, 255, 255, // top
                255, 0, 0, // right
                255, 255, 255, 0, 0, 0, // bottom reversed
            ]
        );
    }

    #[test]
    fn skips_missing_edges_and_samples_single_led_at_midpoint() {
        let (mut renderer, socket) = receiver();
        let edges = EdgeColors {
            bottom: ColorStrip::new(vec![LinearColor::black(), LinearColor::new(1.0, 1.0, 1.0)])
                .unwrap(),
            ..EdgeColors::default()
        };
        renderer
            .render_edges(&edges, leds([0, 0, 1, 0]), &NEAREST)
            .unwrap();
        let bytes = receive(&socket);
        assert_eq!(PacketRef::from_bytes(&bytes).unwrap().data, [188, 188, 188]);
    }

    #[test]
    fn empty_frames_send_nothing() {
        let (mut renderer, socket) = receiver();
        renderer.render(&[]).unwrap();
        renderer
            .render_edges(&EdgeColors::default(), leds([0; 4]), &NEAREST)
            .unwrap();
        assert_no_packet(&socket);
    }

    #[test]
    fn sends_consecutive_frames_without_throttling() {
        let (mut renderer, socket) = receiver();
        for color in [Color::new(255, 0, 0), Color::new(0, 255, 0)] {
            renderer.render(&[color]).unwrap();
            let bytes = receive(&socket);
            assert_eq!(
                PacketRef::from_bytes(&bytes).unwrap().data,
                [color.r, color.g, color.b]
            );
            renderer
                .render_edges(&EdgeColors::default(), leds([1; 4]), &NEAREST)
                .unwrap();
            let bytes = receive(&socket);
            assert_eq!(PacketRef::from_bytes(&bytes).unwrap().data, [0; 12]);
        }
        assert_no_packet(&socket);
    }

    /// Sends that fail with a cause that drops the frame never fail a
    /// held renderer, however many frames and however long.
    #[test]
    fn frames_whose_send_cannot_leave_now_are_dropped_for_ever() {
        let refusals: [fn() -> io::Error; 2] = [
            || io::ErrorKind::WouldBlock.into(),
            || io::Error::from_raw_os_error(libc::ENOBUFS),
        ];
        for refusal in refusals {
            let (renderer, socket) = receiver();
            let mut renderer = renderer.refusing(refusal);
            for _ in 0..1_000 {
                renderer
                    .render_edges(&EdgeColors::default(), leds([1, 0, 0, 0]), &NEAREST)
                    .unwrap();
            }
            assert_no_packet(&socket);
        }
    }

    #[test]
    fn a_send_error_with_no_row_fails_the_first_frame() {
        let mut renderer = WledRenderer::new("127.0.0.1:0").unwrap();
        let error = renderer
            .render_edges(&EdgeColors::default(), leds([1, 0, 0, 0]), &NEAREST)
            .unwrap_err();
        let DDPError::Disconnect(error) = error else {
            panic!("{error:?}");
        };
        assert_eq!(classify::send(&error), classify::Send::OutputFails);
    }

    /// What WLED emits for `byte` at `gamma`, as 0–255 light.
    fn emitted(byte: u8, gamma: f32) -> f32 {
        (255.0 * (f32::from(byte) / 255.0).powf(gamma)).round()
    }

    #[test]
    fn gamma_encoding_makes_wled_emit_the_intended_light() {
        for gamma in [1.0, 2.2, 2.8] {
            let transfer = Transfer::Gamma(Gamma::new(gamma).unwrap());
            for step in 0..=1000 {
                let linear = f32::from(u16::try_from(step).unwrap()) / 1000.0;
                let byte = transfer.encode(LinearColor::new(linear, 0.0, 0.0)).r;
                // The light lies within one LED step of the intent (plus
                // WLED's own rounding): no other byte would be nearer.
                let wanted = 255.0 * linear;
                let below = emitted(byte.saturating_sub(1), gamma);
                let above = emitted(byte.saturating_add(1), gamma);
                assert!(
                    below - 0.5 <= wanted && wanted <= above + 0.5,
                    "gamma {gamma}, {linear}: byte {byte} emits {}, wanted {wanted}",
                    emitted(byte, gamma)
                );
            }
            let encode = |value| transfer.encode(LinearColor::new(value, value, value));
            assert_eq!(encode(0.0), Color::black(), "gamma {gamma}");
            assert_eq!(encode(1.0), Color::new(255, 255, 255), "gamma {gamma}");
            assert_eq!(encode(-0.5), Color::black(), "gamma {gamma}");
            assert_eq!(encode(1.5), Color::new(255, 255, 255), "gamma {gamma}");
        }
    }

    #[test]
    fn no_gamma_sends_linear_bytes_and_unknown_gamma_sends_srgb() {
        let linear = Transfer::Gamma(Gamma::LINEAR);
        for byte in 0..=255 {
            let value = f32::from(byte) / 255.0;
            let color = LinearColor::new(value, value, value);
            assert_eq!(linear.encode(color), Color::new(byte, byte, byte));
        }
        for step in 0..=1000 {
            let value = f32::from(u16::try_from(step).unwrap()) / 1000.0;
            let color = LinearColor::new(value, value / 2.0, 1.0 - value);
            assert_eq!(Transfer::Srgb.encode(color), color.to_srgb());
        }
    }

    #[test]
    fn gamma_is_one_wled_accepts() {
        for value in [0.1, 0.5, 1.0, 2.2, 3.0] {
            assert_eq!(Gamma::new(value), Some(Gamma(value)));
        }
        for value in [0.0, 0.09, 3.1, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(Gamma::new(value), None, "{value}");
        }
    }

    #[test]
    fn gamma_table_is_wleds() {
        // WLED 0.15's built-in gamma 2.8 table (`colors.cpp`), which 16
        // computes with the same formula.
        #[rustfmt::skip]
        const WLED_2_8: [u8; 256] = [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1,
            1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2,
            2, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 5, 5, 5,
            5, 6, 6, 6, 6, 7, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10,
            10, 10, 11, 11, 11, 12, 12, 13, 13, 13, 14, 14, 15, 15, 16, 16,
            17, 17, 18, 18, 19, 19, 20, 20, 21, 21, 22, 22, 23, 24, 24, 25,
            25, 26, 27, 27, 28, 29, 29, 30, 31, 32, 32, 33, 34, 35, 35, 36,
            37, 38, 39, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 50,
            51, 52, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 66, 67, 68,
            69, 70, 72, 73, 74, 75, 77, 78, 79, 81, 82, 83, 85, 86, 87, 89,
            90, 92, 93, 95, 96, 98, 99, 101, 102, 104, 105, 107, 109, 110, 112, 114,
            115, 117, 119, 120, 122, 124, 126, 127, 129, 131, 133, 135, 137, 138, 140, 142,
            144, 146, 148, 150, 152, 154, 156, 158, 160, 162, 164, 167, 169, 171, 173, 175,
            177, 180, 182, 184, 186, 189, 191, 193, 196, 198, 200, 203, 205, 208, 210, 213,
            215, 218, 220, 223, 225, 228, 231, 233, 236, 239, 241, 244, 247, 249, 252, 255,
        ];
        let gamma = Gamma::new(2.8).unwrap();
        for byte in 0..=u8::MAX {
            assert_eq!(gamma.correct(byte), WLED_2_8[usize::from(byte)], "{byte}");
            assert_eq!(Gamma::LINEAR.correct(byte), byte);
        }
    }

    #[test]
    fn edges_are_encoded_with_the_transfer_given() {
        let (mut renderer, socket) = receiver();
        let edges = EdgeColors {
            bottom: ColorStrip::fill(
                LinearColor::new(0.18, 0.0, 1.0),
                std::num::NonZeroUsize::MIN,
            ),
            ..EdgeColors::default()
        };
        let mut send = |transfer| {
            renderer
                .render_edges(&edges, leds([0, 0, 1, 0]), &Encoding::Nearest(transfer))
                .unwrap();
            PacketRef::from_bytes(&receive(&socket))
                .unwrap()
                .data
                .to_vec()
        };
        assert_eq!(
            send(Transfer::Gamma(Gamma::new(2.8).unwrap())),
            [138, 0, 255]
        );
        assert_eq!(send(Transfer::Srgb), [118, 0, 255]);
        assert_eq!(send(Transfer::Gamma(Gamma::LINEAR)), [46, 0, 255]);
    }
}
