//! Checks against the real OpenSSH client (no server needed).

mod common;

use common::Env;

/// OpenSSH must pass `-F` on to the ssh it spawns for jump hosts, so hops resolve in
/// the generated file (section 7.1).
#[test]
fn ssh_passes_config_file_to_jump_hosts() {
    if sshoba::core::launcher::which("ssh").is_none() {
        eprintln!("ssh not installed; skipping");
        return;
    }
    let env = Env::new();
    env.write_config(
        r#"
[settings]
control_master = false

[[server]]
name = "hop"
host = "127.0.0.1"
port = 1

[[server]]
name = "target"
host = "10.255.255.1"
jump = ["hop"]
"#,
    );
    env.cmd().args(["cmd", "target"]).assert().success();
    let file = env.state_dir().join("ssh_config");
    let out = std::process::Command::new("ssh")
        .arg("-v")
        .arg("-F")
        .arg(&file)
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=3"])
        .arg("sshoba.target")
        .arg("true")
        .env("HOME", env.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected = format!("exec ssh -F {}", file.display());
    assert!(
        stderr.contains(&expected),
        "jump ssh did not get -F; stderr:\n{stderr}"
    );
}
