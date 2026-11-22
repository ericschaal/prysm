//! Entry points for the fuzz targets in `fuzz/` (feature `fuzzing`, never
//! enabled by a build that ships). Each one decodes untrusted input as a peer
//! would send it and asserts what the rest of the crate relies on, so a
//! target finds wrong answers as well as panics. The decoders stay
//! crate-private; only these checks are public.
use crate::offer::Offer;
use crate::request::{RemoteSettings, StreamEncoding};
use crate::wire::{
    self, Encoding, FromReceiver, FromSender, MAX_HEIGHT, MAX_PAYLOAD_BYTES, MAX_REASON_BYTES,
    MAX_WIDTH, Refusal, RefusalCause, ToReceiver, ToSender,
};

/// The flags byte of the frame header: decoding keeps only an H.264 stream's
/// keyframe bit, so re-encoding writes the others as 0.
const FLAGS: usize = 3;

/// A frame message; the first byte picks the stream's encoding. An accepted
/// frame is within the limits, its payload fits its header, and its header
/// re-encodes to the same bytes, apart from the ignored flags.
pub fn frame(data: &[u8]) {
    let Some((&selector, message)) = data.split_first() else {
        return;
    };
    let encoding = if selector & 1 == 0 {
        Encoding::Raw
    } else {
        Encoding::H264
    };
    let Ok(decoded) = wire::decode_frame(message, encoding) else {
        return;
    };
    let header = decoded.header;
    assert!((1..=MAX_WIDTH).contains(&header.width), "{header:?}");
    assert!((1..=MAX_HEIGHT).contains(&header.height), "{header:?}");
    assert!(decoded.payload.len() <= MAX_PAYLOAD_BYTES);
    assert_eq!(decoded.payload, &message[wire::HEADER_LEN..]);
    match encoding {
        Encoding::Raw => assert_eq!(
            header.format.expected_size(header.width, header.height),
            Ok(Some(decoded.payload.len())),
            "{header:?}"
        ),
        Encoding::H264 => {
            assert_eq!(header.format, prysm_h264::FORMAT);
            assert!(!decoded.payload.is_empty());
        }
    }
    let encoded = header
        .encode(encoding, decoded.payload.len())
        .expect("an accepted header encodes");
    let mut expected = [0; wire::HEADER_LEN];
    expected.copy_from_slice(&message[..wire::HEADER_LEN]);
    expected[FLAGS] = u8::from(header.keyframe);
    assert_eq!(encoded, expected, "{header:?}");
    assert_eq!(
        header.keyframe,
        encoding == Encoding::H264 && message[FLAGS] & 1 != 0
    );
}

/// A receiver's text message, as the sender reads it. An accepted message
/// survives encoding and decoding again; an accepted request is valid
/// settings that travel unchanged.
pub fn from_receiver(text: &str) {
    let Ok(message) = wire::decode_from_receiver(text) else {
        return;
    };
    let sent = match &message {
        FromReceiver::Request(request) => {
            if let Ok(settings) = RemoteSettings::try_from(request.clone()) {
                assert_eq!(settings.validate(), Ok(()));
                assert!(settings.width <= MAX_WIDTH && settings.height <= MAX_HEIGHT);
                assert_eq!(
                    RemoteSettings::try_from(wire::RequestMessage::from(settings)),
                    Ok(settings)
                );
            }
            ToSender::Request(request.clone())
        }
        FromReceiver::Keyframe => ToSender::Keyframe,
        FromReceiver::Unknown => return,
    };
    assert_eq!(
        wire::decode_from_receiver(&wire::encode_to_sender(&sent)),
        Ok(message)
    );
}

/// A sender's text message, as the receiver reads it. An accepted refusal's
/// reason is bounded and survives encoding and decoding again; any reason,
/// cut by [`Refusal::new`], is a bounded prefix on a character boundary.
pub fn from_sender(text: &str) {
    let cut = Refusal::new(RefusalCause::Request, text);
    assert!(cut.reason.len() <= MAX_REASON_BYTES);
    assert!(text.starts_with(&cut.reason));
    if text.len() <= MAX_REASON_BYTES {
        assert_eq!(cut.reason, text);
    }

    let Ok(message) = wire::decode_from_sender(text) else {
        return;
    };
    let FromSender::Refused(refusal) = &message else {
        return;
    };
    assert!(refusal.reason.len() <= MAX_REASON_BYTES);
    let sent = wire::encode_to_receiver(&ToReceiver::Refused(refusal.clone()));
    assert_eq!(wire::decode_from_sender(&sent), Ok(message));
}

/// An offer, as the receiver reads it from an announcement. An accepted
/// offer reads back the same from its own header, and checking a request
/// against it accepts only an encoding it names.
pub fn offer(data: &[u8]) {
    // The last 8 bytes, when there are that many, are a request to check.
    let (text, request) = match data.len().checked_sub(8) {
        Some(at) => data.split_at(at),
        None => (data, &[][..]),
    };
    let Ok(text) = std::str::from_utf8(text) else {
        return;
    };
    let Some(offer) = Offer::parse(text) else {
        return;
    };
    assert_eq!(Offer::parse(&offer.header()), Some(offer.clone()));
    if let [w0, w1, h0, h1, fps, ui, encoding, _] = *request {
        let settings = RemoteSettings {
            width: u32::from(u16::from_le_bytes([w0, w1])),
            height: u32::from(u16::from_le_bytes([h0, h1])),
            fps: u32::from(fps),
            include_ui: ui & 1 != 0,
            encoding: StreamEncoding::ALL[usize::from(encoding) % StreamEncoding::ALL.len()],
        };
        let checked = offer.check(&settings);
        let named = offer
            .header()
            .contains(&format!("\"{}\"", settings.encoding));
        if checked.is_ok() {
            assert!(named, "{settings} accepted by {}", offer.header());
        }
        if !named {
            assert_eq!(checked.map_err(|error| error.field), Err("encoding".into()));
        }
    }
}
