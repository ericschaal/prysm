use prysm_webos_protocol::WORKER_CONFIG_ENV;
use std::process::{Command, Stdio};

fn worker() -> Command {
    let binary = env!("CARGO_BIN_EXE_prysm-webos-capture");
    // Cargo's runner is not automatically applied to child processes.
    match std::env::var("CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABI_RUNNER") {
        Ok(runner) => {
            let mut args = runner.split_whitespace();
            let mut command = Command::new(args.next().expect("runner must not be empty"));
            command.args(args).arg(binary);
            command
        }
        Err(_) => Command::new(binary),
    }
}

#[test]
fn worker_rejects_cli_and_missing_supervisor_contract_before_native_startup() {
    for (argument, config, expected) in [
        (Some("--help"), None, "accepts no command-line arguments"),
        (None, None, "requires supervisor configuration"),
        (
            None,
            Some(
                r#"{"instance_url":"http://127.0.0.1:8787","source":"both","compression":"lz4","metrics":false}"#,
            ),
            "requires a supervisor event pipe",
        ),
    ] {
        let mut command = worker();
        command
            .env_remove(WORKER_CONFIG_ENV)
            .env_remove("PRYSM_EVENTS_FD")
            .stdin(Stdio::null());
        if let Some(argument) = argument {
            command.arg(argument);
        }
        if let Some(config) = config {
            command.env(WORKER_CONFIG_ENV, config);
        }
        let output = command.output().unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "argument={argument:?}, expected={expected:?}, output={output:?}"
        );
    }
}

#[test]
fn verify_instance_accepts_a_trusted_tls12_websocket() {
    use prysm_webos_protocol::{ControlMessage, HELLO};
    use rustls::{
        ServerConfig, ServerConnection, StreamOwned,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    };
    use std::{net::TcpListener, thread, time::Duration};

    let dir = tempfile::tempdir().unwrap();
    let cert = dir.path().join("cert.pem");
    let key = dir.path().join("key.pem");
    let issued = Command::new("openssl")
        .args([
            "req",
            "-new",
            "-x509",
            "-nodes",
            "-newkey",
            "rsa:2048",
            "-days",
            "1",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .output()
        .unwrap();
    assert!(
        issued.status.success(),
        "{}",
        String::from_utf8_lossy(&issued.stderr)
    );
    let server =
        ServerConfig::builder_with_provider(rustls::crypto::ring::default_provider().into())
            .with_protocol_versions(&[&rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from_pem_file(&cert).unwrap()],
                PrivateKeyDer::from_pem_file(key).unwrap(),
            )
            .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let tls = ServerConnection::new(server.into()).unwrap();
        let mut ws = tungstenite::accept(StreamOwned::new(tls, tcp)).unwrap();
        assert_eq!(
            ControlMessage::decode(&ws.read().unwrap().into_data()).unwrap(),
            HELLO
        );
        ws.send(tungstenite::Message::binary(
            ControlMessage::Idle.encode().unwrap(),
        ))
        .unwrap();
    });

    let result = worker()
        .env("SSL_CERT_FILE", cert)
        .args(["--verify-instance", &format!("https://localhost:{port}")])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    peer.join().unwrap();
}
