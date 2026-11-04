//! Private receiver for the bounded webOS native smoke test.
use anyhow::{Context, Result, ensure};
use prysm_webos_protocol::{Codec, ControlMessage, HEADER_LEN, HELLO, Header};
use std::{fs, net::TcpListener, time::Duration};
use tungstenite::Message;

fn main() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    fs::write("receiver.port", listener.local_addr()?.port().to_string())?;
    // The configuration probe connects first and opens no native resources.
    for verify in [true, false] {
        let (tcp, _) = listener.accept()?;
        tcp.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut tv = tungstenite::accept(tcp).map_err(|error| anyhow::anyhow!("{error}"))?;
        ensure!(
            ControlMessage::decode(&tv.read()?.into_data())? == HELLO,
            "bad hello"
        );
        if verify {
            tv.send(Message::binary(ControlMessage::Idle.encode()?))?;
            continue;
        }
        tv.send(Message::binary(
            ControlMessage::CaptureSettings {
                refresh_rate_hz: 15,
                capture_resolution: [320, 180],
            }
            .encode()?,
        ))?;
        for _ in 0..3 {
            let frame = tv.read()?.into_data();
            let header = Header::decode(
                frame
                    .get(..HEADER_LEN)
                    .context("missing header")?
                    .try_into()?,
            )?;
            ensure!(
                header.codec == Codec::Raw
                    && header.width == 320
                    && header.height == 180
                    && frame.len() == HEADER_LEN + header.payload_len,
                "unexpected capture format or size"
            );
        }
        fs::write("receiver.complete", "3\n")?;
        while tv.read().is_ok() {}
    }
    Ok(())
}
