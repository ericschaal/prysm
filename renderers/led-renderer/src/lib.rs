//! Send Prysm colors to WLED using DDP over UDP.
//!
//! Use port 4048 and enable DDP reception in WLED's Sync Interfaces settings.
//! WLED controls brightness, gamma, LED color order, and the realtime timeout.
//!
//! ```no_run
//! use led_renderer::WledRenderer;
//! use prysm_core::{Color, EdgeColors};
//!
//! # fn main() -> Result<(), ddp_rs::error::DDPError> {
//! let mut renderer = WledRenderer::new("wled.local:4048")?;
//! renderer.render(&[Color::new(255, 0, 0), Color::new(0, 0, 255)])?;
//! // Counts: top, right, bottom, left. Wiring starts at bottom-left, going up.
//! renderer.render_edges(&EdgeColors::default(), [96, 54, 96, 54])?;
//! # Ok(())
//! # }
//! ```

use ddp_rs::connection::DDPConnection;
use ddp_rs::error::DDPError;
use ddp_rs::protocol::{ID, PixelConfig, PixelFormat};
use prysm_core::{Color, EdgeColors};
use std::io;
use std::net::{ToSocketAddrs, UdpSocket};

/// A nonblocking RGB renderer for one WLED instance.
///
/// Each call sends immediately, without a frame queue or retransmission. Errors
/// are returned to the caller, including `WouldBlock` if the send buffer is full.
/// UDP delivery is not acknowledged. Keep sending frames before WLED's realtime
/// timeout expires, even for a static picture. Stopping lets WLED resume its effect.
#[derive(Debug)]
pub struct WledRenderer {
    connection: DDPConnection,
    rgb: Vec<u8>,
}

impl WledRenderer {
    /// Resolve a WLED address (hostname or IP with port, normally 4048).
    ///
    /// Binds an ephemeral local port. Address resolution happens only here and
    /// may block; rendering uses a nonblocking socket.
    pub fn new(address: impl ToSocketAddrs) -> Result<Self, DDPError> {
        let destination = address
            .to_socket_addrs()?
            .next()
            .ok_or(DDPError::NoValidSocketAddr)?;
        let socket = UdpSocket::bind(if destination.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        socket.set_nonblocking(true)?;
        let connection = DDPConnection::try_new(
            destination,
            PixelConfig {
                data_size: PixelFormat::Pixel8Bits,
                ..PixelConfig::default()
            },
            ID::Default,
            socket,
        )?;
        Ok(Self {
            connection,
            rgb: Vec::new(),
        })
    }

    /// Send 8-bit RGB colors in physical LED order, starting at LED zero.
    ///
    /// Empty input sends nothing. Use a fixed frame length: a shorter frame leaves
    /// trailing LEDs at their previous colors. WLED uses 16-bit LED indices, so
    /// frames with more than 65535 colors are rejected before sending.
    pub fn render(&mut self, colors: &[Color]) -> Result<(), DDPError> {
        self.prepare(colors.len())?;
        for color in colors {
            self.rgb.extend_from_slice(&[color.r, color.g, color.b]);
        }
        self.connection.write(&self.rgb)?;
        Ok(())
    }

    /// Resample linear edge gradients to the given top/right/bottom/left counts.
    ///
    /// Output starts at the bottom-left corner and runs clockwise, as viewed from
    /// the screen's front: left bottom-to-top, top left-to-right, right
    /// top-to-bottom, bottom right-to-left. Zero skips an edge. Other wiring layouts
    /// can provide their own ordered colors through [`Self::render`].
    /// Interpolation happens in linear light, followed by conversion to 8-bit sRGB.
    pub fn render_edges(
        &mut self,
        edges: &EdgeColors,
        led_counts: [usize; 4],
    ) -> Result<(), DDPError> {
        let total = led_counts
            .into_iter()
            .try_fold(0usize, usize::checked_add)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "LED count overflow"))?;
        self.prepare(total)?;
        for (strip, count, reverse) in [
            (&edges.left, led_counts[3], true),
            (&edges.top, led_counts[0], false),
            (&edges.right, led_counts[1], false),
            (&edges.bottom, led_counts[2], true),
        ] {
            for i in 0..count {
                let index = if reverse { count - 1 - i } else { i };
                let color = strip.color_at(index, count).to_srgb();
                self.rgb.extend_from_slice(&[color.r, color.g, color.b]);
            }
        }
        self.connection.write(&self.rgb)?;
        Ok(())
    }

    fn prepare(&mut self, count: usize) -> Result<(), DDPError> {
        if count > usize::from(u16::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WLED frames must contain at most 65535 LEDs",
            )
            .into());
        }
        self.rgb.clear();
        self.rgb.reserve(count * 3);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddp_rs::packet::PacketRef;
    use ddp_rs::protocol::DataType;
    use prysm_core::{ColorStrip, LinearColor};
    use std::time::Duration;

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
        ]);
        let edges = EdgeColors::new(
            gradient.clone(),
            ColorStrip::fill(LinearColor::new(1.0, 0.0, 0.0), 1),
            gradient,
            ColorStrip::new(vec![LinearColor::black(), LinearColor::new(0.0, 0.0, 1.0)]),
        );
        renderer.render_edges(&edges, [3, 1, 2, 2]).unwrap();
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
            bottom: ColorStrip::new(vec![LinearColor::black(), LinearColor::new(1.0, 1.0, 1.0)]),
            ..EdgeColors::default()
        };
        renderer.render_edges(&edges, [0, 0, 1, 0]).unwrap();
        let bytes = receive(&socket);
        assert_eq!(PacketRef::from_bytes(&bytes).unwrap().data, [188, 188, 188]);
    }

    #[test]
    fn empty_frames_send_nothing() {
        let (mut renderer, socket) = receiver();
        renderer.render(&[]).unwrap();
        renderer
            .render_edges(&EdgeColors::default(), [0; 4])
            .unwrap();
        assert_no_packet(&socket);
    }

    #[test]
    fn rejects_oversized_and_overflowing_layouts_before_sending() {
        let (mut renderer, socket) = receiver();
        let edges = EdgeColors::default();
        for counts in [[65536, 0, 0, 0], [usize::MAX, 1, 0, 0]] {
            assert!(matches!(
                renderer.render_edges(&edges, counts),
                Err(DDPError::Disconnect(e)) if e.kind() == io::ErrorKind::InvalidInput
            ));
        }
        assert!(renderer.render(&vec![Color::black(); 65536]).is_err());
        assert_no_packet(&socket);
    }
}
