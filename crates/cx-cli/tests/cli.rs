//! End to end through the real binary: a background `cx serve`, then a
//! second identity pairs, lists, uploads and downloads.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const CX: &str = env!("CARGO_BIN_EXE_cx");

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cx(state: &Path, args: &[&str]) -> String {
    let out = Command::new(CX).arg("--state-dir").arg(state).args(args).output().unwrap();
    assert!(out.status.success(), "cx {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn serve_pair_ls_put_get() {
    let tmp = tempfile::tempdir().unwrap();
    let (server_state, client_state) = (tmp.path().join("server"), tmp.path().join("client"));
    let share = tmp.path().join("share");
    std::fs::create_dir_all(&share).unwrap();
    std::fs::write(share.join("readme.txt"), b"hi from the server").unwrap();

    let mut child = Command::new(CX)
        .arg("--state-dir")
        .arg(&server_state)
        .args(["--name", "nas", "serve", "--bind", "127.0.0.1", "--port", "0", "--share"])
        .arg(format!("Docs={}", share.display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let _server = Server(child);

    // Read the startup banner on a thread (with a timeout on our side).
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let (mut addr, mut first_code) = (None, None);
    while addr.is_none() || first_code.is_none() {
        let line = rx.recv_timeout(Duration::from_secs(20)).expect("cx serve did not start");
        if let Some(a) = line.strip_prefix("listening on ") {
            addr = Some(a.trim().to_string());
        }
        if let Some(c) = line.strip_prefix("pairing code: ") {
            first_code = Some(c[..6].to_string());
        }
    }
    let addr = addr.unwrap();
    let port = addr.rsplit(':').next().unwrap().to_string();

    // A fresh code from the running service (same state dir = same key).
    let code = cx(&server_state, &["pair-code", "--port", &port]).trim().to_string();
    assert_eq!(code.len(), 6);
    assert_ne!(Some(&code), first_code.as_ref());

    // Unpaired clients are turned away.
    let out = Command::new(CX).arg("--state-dir").arg(&client_state).args(["ls", &format!("peer://{addr}/Docs")]).output().unwrap();
    assert!(!out.status.success());

    let paired = cx(&client_state, &["--name", "laptop", "pair", &addr, &code]);
    assert!(paired.contains("paired with \"nas\""), "{paired}");

    let listing = cx(&client_state, &["ls", &format!("peer://{addr}/Docs")]);
    assert!(listing.contains("readme.txt"), "{listing}");
    let root = cx(&client_state, &["ls", &format!("peer://{addr}/")]);
    assert!(root.contains("Docs/"), "{root}");

    // Upload 3 MiB, download it again, compare.
    let data: Vec<u8> = (0..3u32 << 20).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let local = tmp.path().join("upload.bin");
    std::fs::write(&local, &data).unwrap();
    cx(&client_state, &["put", local.to_str().unwrap(), &format!("peer://{addr}/Docs/")]);
    assert_eq!(std::fs::read(share.join("upload.bin")).unwrap(), data);

    let back = tmp.path().join("download.bin");
    cx(&client_state, &["get", &format!("peer://{addr}/Docs/upload.bin"), back.to_str().unwrap()]);
    assert_eq!(std::fs::read(&back).unwrap(), data);

    // Addressing the server by device id works too (address from the trust store).
    let devices = cx(&client_state, &["devices"]);
    let device_id = devices.split_whitespace().next().unwrap().to_string();
    let listing = cx(&client_state, &["ls", &format!("peer://{device_id}/Docs")]);
    assert!(listing.contains("upload.bin"), "{listing}");

    // The server logged what the client did.
    let audit = std::fs::read_to_string(server_state.join("audit.jsonl")).unwrap();
    assert!(audit.contains("\"op\":\"write\"") && audit.contains("/Docs/upload.bin"), "{audit}");
}
