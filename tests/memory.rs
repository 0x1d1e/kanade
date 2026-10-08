//! `scripts/memory.awk`, fed a sample: GPU buffers count once per DRM client, and a client id
//! is unique only per device (`drm-pdev`), so two GPUs may share one.

use std::io::Write;
use std::process::{Command, Stdio};

/// The `gpu` and `gpu-res` columns, in MiB, of the row the script prints for `sample`.
fn gpu(sample: &str) -> (String, String) {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/memory.awk");
    let mut awk = Command::new("awk")
        .args(["-v", "label=test", "-f", script])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("awk runs");
    awk.stdin
        .take()
        .unwrap()
        .write_all(sample.as_bytes())
        .unwrap();
    let out = awk.wait_with_output().unwrap();
    assert!(out.status.success());
    let row = String::from_utf8(out.stdout).unwrap();
    let columns: Vec<&str> = row.split_whitespace().collect();
    (columns[5].to_owned(), columns[6].to_owned())
}

/// The fdinfo of descriptor `fd`: one client on device `pdev`, holding `mib` of resident buffers
/// and 1 MiB more, in bytes, allocated in stolen memory.
fn fdinfo(fd: u32, pdev: Option<&str>, client: u32, mib: u32) -> String {
    let file = format!("/proc/1/fdinfo/{fd}");
    let pdev = pdev.map_or(String::new(), |pdev| {
        format!("fdinfo {file} drm-pdev:\t{pdev}\n")
    });
    format!(
        "fdinfo {file} drm-driver:\ti915\n{pdev}fdinfo {file} drm-client-id:\t{client}\n\
         fdinfo {file} drm-total-system0:\t{kib} KiB\nfdinfo {file} drm-resident-system0:\t{kib} KiB\n\
         fdinfo {file} drm-total-stolen-system0:\t1048576\nfdinfo {file} drm-total-cycles-rcs:\t999999\n",
        kib = mib * 1024
    )
}

#[test]
fn one_client_on_several_descriptors_counts_once() {
    // and a descriptor without DRM stats, as the NVIDIA driver's
    let sample = fdinfo(3, Some("0000:00:02.0"), 7, 100)
        + &fdinfo(4, Some("0000:00:02.0"), 7, 100)
        + "fdinfo /proc/1/fdinfo/5 pos:\t0\n";
    assert_eq!(gpu(&sample), ("101.0".into(), "100.0".into()));
}

#[test]
fn one_client_id_on_two_devices_counts_twice() {
    let sample = fdinfo(3, Some("0000:00:02.0"), 7, 100) + &fdinfo(4, Some("0000:01:00.0"), 7, 30);
    assert_eq!(gpu(&sample), ("132.0".into(), "130.0".into()));
}

#[test]
fn a_global_client_id_without_device_counts_once() {
    let sample = fdinfo(3, None, 7, 100) + &fdinfo(4, None, 7, 100) + &fdinfo(5, None, 8, 20);
    assert_eq!(gpu(&sample), ("122.0".into(), "120.0".into()));
}
