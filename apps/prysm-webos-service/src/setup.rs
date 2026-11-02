//! One-shot root setup; normal service operation needs neither Node nor shell IPC.
use crate::files::Files;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::process::{Command, Stdio};

pub fn run(files: &Files, service_name: &str) -> Result<()> {
    files.manifest(true)?;
    files.register_supervisor()?;
    let status = Command::new("/usr/bin/timeout")
        .args(["-k", "1", "10"])
        .arg(&files.scanner)
        .arg("scan-services")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .context("rescan Luna services")?;
    ensure!(status.success(), "Luna service rescan failed: {status}");
    let mut child = Command::new("/usr/bin/timeout")
        .args([
            "-k",
            "1",
            "10",
            "/usr/bin/luna-send",
            "-w",
            "5000",
            "-n",
            "1",
        ])
        .arg(format!("luna://{service_name}/status"))
        .arg("{}")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("activate Luna service")?;
    // luna-send on this TV exits without replying on EOF. Own stdin outside
    // wait_with_output so it stays open until the reply or external timeout.
    let input = child.stdin.take().context("Luna activation stdin")?;
    let output = child.wait_with_output()?;
    drop(input);
    ensure!(
        output.status.success(),
        "Luna service activation failed: {}",
        output.status
    );
    let value = activation_reply(&output.stdout)?;
    println!("{value}");
    Ok(())
}
fn activation_reply(bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= 65536, "Oversized Luna activation reply");
    let value: Value = serde_json::from_slice(bytes).context("invalid Luna activation reply")?;
    ensure!(
        value["returnValue"] == true,
        "Luna service activation failed: {value}"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    #[test]
    fn activation_requires_an_explicit_success_reply() {
        assert!(super::activation_reply(br#"{"returnValue":true}"#).is_ok());
        for value in [
            b"".as_slice(),
            b"null",
            b"{}",
            br#"{"returnValue":false,"errorText":"denied"}"#,
        ] {
            assert!(super::activation_reply(value).is_err());
        }
        assert!(super::activation_reply(&vec![b' '; 65537]).is_err());
    }
}
