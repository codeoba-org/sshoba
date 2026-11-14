mod common;

use common::Env;
use predicates::prelude::*;

const FIXTURE: &str = include_str!("fixtures/servers.toml");

#[test]
fn help_and_version() {
    let env = Env::new();
    env.cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("connect"));
    env.cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn first_run_creates_config() {
    let env = Env::new();
    env.cmd().arg("ls").assert().success();
    let text = std::fs::read_to_string(env.config_file()).unwrap();
    assert!(text.contains("[settings]"));
}

#[test]
fn add_list_show() {
    let env = Env::new();
    env.cmd()
        .args([
            "add",
            "web",
            "ata@1.2.3.4:2222",
            "--tag",
            "x",
            "--group",
            "g",
        ])
        .assert()
        .success();
    env.cmd()
        .args(["add", "web", "1.2.3.4"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already exists"));
    env.cmd()
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("ata@1.2.3.4:2222"));
    env.cmd()
        .args(["show", "web"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ssh -p 2222 ata@1.2.3.4"));
}

#[test]
fn list_json_snapshot() {
    let env = Env::new();
    env.write_config(FIXTURE);
    let out = env.cmd().args(["ls", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    insta::assert_json_snapshot!(v);
}

#[test]
fn list_filters() {
    let env = Env::new();
    env.write_config(FIXTURE);
    let names = |args: &[&str]| -> Vec<String> {
        let out = env
            .cmd()
            .args(["ls", "--json"])
            .args(args)
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&["--fav"]), ["db-prod"]);
    assert_eq!(names(&["--group", "work"]), ["bastion", "web-2"]);
    assert_eq!(names(&["--tag", "db"]), ["db-prod"]);
}

#[test]
fn not_found_and_ambiguous_exit_codes() {
    let env = Env::new();
    env.write_config(FIXTURE);
    env.cmd()
        .args(["show", "bastoin"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("did you mean \"bastion\""));
    env.cmd().args(["connect", "zzzz"]).assert().code(2);
    env.cmd()
        .args(["connect", "o"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("bastion").and(predicate::str::contains("db-prod")));
}

#[test]
fn invalid_config_exit_code_4() {
    let env = Env::new();
    for bad in [
        "[[server]]\nname = \"a\"\nhost = \"h\"\n[[server]]\nname = \"a\"\nhost = \"h\"\n",
        "[[server]]\nname = \"a\"\nhost = \"h\"\njump = [\"nope\"]\n",
        "[[server]]\nname = \"a\"\nhost = \"h\"\njump = [\"b\"]\n[[server]]\nname = \"b\"\nhost = \"h\"\njump = [\"a\"]\n",
        "this is not toml",
    ] {
        env.write_config(bad);
        env.cmd().arg("ls").assert().code(4);
    }
}

#[test]
fn rename_updates_jumps() {
    let env = Env::new();
    env.write_config(FIXTURE);
    env.cmd()
        .args(["rename", "bastion", "jump1"])
        .assert()
        .success();
    let text = std::fs::read_to_string(env.config_file()).unwrap();
    assert!(text.contains("jump = [\"jump1\"]"));
    assert!(text.contains("# Fixture used by CLI tests."));
    assert!(text.contains("note = \"Main jump box\""));
}

#[test]
fn rm_rules() {
    let env = Env::new();
    env.write_config(FIXTURE);
    env.cmd()
        .args(["rm", "bastion", "-y"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("db-prod"));
    env.cmd()
        .args(["rm", "web-2"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("-y"));
    env.cmd().args(["rm", "web-2", "-y"]).assert().success();
    env.cmd()
        .args(["rm", "bastion", "-y", "--force"])
        .assert()
        .success();
    let text = std::fs::read_to_string(env.config_file()).unwrap();
    assert!(!text.contains("bastion"));
    assert!(!text.contains("web-2"));
}

#[test]
fn set_field() {
    let env = Env::new();
    env.write_config(FIXTURE);
    env.cmd()
        .args(["set", "web-2", "port", "2222"])
        .assert()
        .success();
    env.cmd()
        .args(["set", "web-2", "port", "nope"])
        .assert()
        .code(1);
    env.cmd()
        .args(["set", "web-2", "colour", "x"])
        .assert()
        .failure();
    let text = std::fs::read_to_string(env.config_file()).unwrap();
    assert!(text.contains("port = 2222"));
}

#[test]
fn cmd_prints_exact_command() {
    let env = Env::new();
    env.write_config(FIXTURE);
    let out = env.cmd().args(["cmd", "db-prod"]).output().unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.starts_with("ssh -F "));
    assert!(s
        .trim_end()
        .ends_with("-o ServerAliveInterval=30 sshoba.db-prod"));
    let generated = std::fs::read_to_string(env.state_dir().join("ssh_config")).unwrap();
    assert!(generated.contains("ProxyJump sshoba.bastion"));
}

#[test]
fn config_path() {
    let env = Env::new();
    env.cmd()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            env.config_file().display().to_string(),
        ));
}

#[test]
fn completions_scripts() {
    let env = Env::new();
    for sh in ["bash", "zsh", "fish"] {
        env.cmd()
            .args(["completions", sh])
            .assert()
            .success()
            .stdout(predicate::str::contains("sshoba"));
    }
}

#[test]
fn dynamic_completion_lists_servers() {
    let env = Env::new();
    env.write_config(FIXTURE);
    let out = env
        .cmd()
        .env("COMPLETE", "fish")
        .args(["--", "sshoba", "connect", ""])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("bastion"), "{s}");
    assert!(s.contains("db-prod"), "{s}");
}

#[cfg(unix)]
fn fake_bin(env: &Env, name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = env.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = env.path().join(format!("{name}.args"));
    let script = format!(
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{}'\n",
        log.display()
    );
    let p = bin.join(name);
    std::fs::write(&p, script).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    log
}

fn path_with(env: &Env) -> String {
    format!(
        "{}:{}",
        env.path().join("bin").display(),
        std::env::var("PATH").unwrap()
    )
}

#[test]
fn connect_in_tmux_opens_window() {
    let env = Env::new();
    env.write_config(FIXTURE);
    let log = fake_bin(&env, "tmux");
    env.cmd()
        .env("PATH", path_with(&env))
        .env("TMUX", "/tmp/tmux-1/default,1,0")
        .args(["c", "db-prod"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Opened db-prod"));
    let args = std::fs::read_to_string(log).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(&args[..4], ["new-window", "-n", "db-prod", "--"]);
    assert_eq!(args[4], "ssh");
    assert_eq!(*args.last().unwrap(), "sshoba.db-prod");
    let state = std::fs::read_to_string(env.state_dir().join("state.json")).unwrap();
    assert!(state.contains("\"connect_count\": 1"));
}

#[test]
fn connect_in_zellij_opens_tab() {
    let env = Env::new();
    env.write_config(FIXTURE);
    let log = fake_bin(&env, "zellij");
    env.cmd()
        .env("PATH", path_with(&env))
        .env("ZELLIJ", "0")
        .args(["c", "web-2", "--with-forwards"])
        .assert()
        .success();
    let args = std::fs::read_to_string(log).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(&args[..4], ["action", "new-tab", "--name", "web-2"]);
    let layout = std::fs::read_to_string(args[5]).unwrap();
    assert!(layout.contains("\"sshoba.web-2\""));
}

#[test]
fn tab_without_multiplexer_fails() {
    let env = Env::new();
    env.write_config(FIXTURE);
    env.cmd()
        .args(["c", "web-2", "--tab"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("tmux, Zellij"));
}

#[test]
fn connect_here_needs_tty() {
    let env = Env::new();
    env.write_config(FIXTURE);
    env.cmd()
        .args(["c", "web-2", "--here"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("TTY"));
}

#[test]
fn ping_exit_codes() {
    let env = Env::new();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let up = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let up_port = up.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::Write;
        for s in up.incoming().flatten() {
            let mut s = s;
            let _ = s.write_all(b"SSH-2.0-test\r\n");
            std::thread::sleep(std::time::Duration::from_millis(600));
        }
    });
    env.write_config(&format!(
        "[[server]]\nname = \"down-host\"\nhost = \"127.0.0.1\"\nport = {port}\n\n[[server]]\nname = \"up-host\"\nhost = \"127.0.0.1\"\nport = {up_port}\n"
    ));
    env.cmd()
        .args(["ping", "down-host"])
        .assert()
        .code(5)
        .stdout(predicate::str::contains("down"));
    let out = env
        .cmd()
        .args(["ping", "up-host", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v[0]["status"], "up");
    assert_eq!(v[0]["name"], "up-host");
    let state = std::fs::read_to_string(env.state_dir().join("state.json")).unwrap();
    assert!(state.contains("\"up-host\""));
    env.cmd().args(["ping", "--all"]).assert().code(5);
}

#[test]
fn import_ssh_config_fixture() {
    let env = Env::new();
    env.write_config(
        "[settings]\nlaunch = \"auto\"\n\n[[server]]\nname = \"db-replica\"\nhost = \"10.0.0.6\"\n",
    );
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ssh/config");
    let out = env
        .cmd()
        .args(["import", "ssh-config", "--dry-run", "--file"])
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("bastion") && stdout.contains("db-replica"),
        "{stdout}"
    );
    assert!(!std::fs::read_to_string(env.config_file())
        .unwrap()
        .contains("bastion"));

    env.cmd()
        .args(["import", "ssh-config", "--file"])
        .arg(&fixture)
        .assert()
        .success();
    let text = std::fs::read_to_string(env.config_file()).unwrap();
    insta::assert_snapshot!(text);
}

#[test]
fn fwd_crud() {
    let env = Env::new();
    env.write_config(FIXTURE);
    env.cmd()
        .args([
            "fwd",
            "add",
            "web-2",
            "app",
            "-L",
            "127.0.0.1:8080:web:80",
            "--auto",
        ])
        .assert()
        .success();
    env.cmd()
        .args(["fwd", "add", "web-2", "socks", "-D", "1080"])
        .assert()
        .success();
    env.cmd()
        .args(["fwd", "add", "web-2", "bad", "-L", "8080"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("invalid -L forward spec"));
    env.cmd()
        .args(["fwd", "add", "web-2", "app", "-R", "9000:localhost:9000"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already has"));
    let out = env
        .cmd()
        .args(["fwd", "ls", "web-2", "--json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v[0]["spec"], "127.0.0.1:8080:web:80");
    assert_eq!(v[0]["auto"], true);
    assert_eq!(v[1]["kind"], "socks");
    assert_eq!(v[1]["running"], false);
    env.cmd()
        .args(["fwd", "rm", "web-2", "nope"])
        .assert()
        .code(2);
    env.cmd()
        .args(["fwd", "rm", "web-2", "socks"])
        .assert()
        .success();
    let text = std::fs::read_to_string(env.config_file()).unwrap();
    assert!(text.contains("name = \"app\"") && !text.contains("name = \"socks\""));
    // connect --with-forwards adds auto forwards.
    let log = fake_bin(&env, "tmux");
    env.cmd()
        .env("PATH", path_with(&env))
        .env("TMUX", "x")
        .args(["c", "web-2", "--with-forwards"])
        .assert()
        .success();
    let args = std::fs::read_to_string(log).unwrap();
    assert!(args.contains("-L\n127.0.0.1:8080:web:80\n"), "{args}");
}

#[test]
fn tunnel_up_ls_down() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let fake = env.path().join("fake-ssh");
    std::fs::write(&fake, "#!/bin/sh\nwhile true; do sleep 1; done\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    env.write_config(&format!(
        "[settings]\nssh_bin = \"{}\"\n\n[[server]]\nname = \"db\"\nhost = \"h\"\n\n  [[server.forward]]\n  name = \"pg\"\n  kind = \"local\"\n  local_port = {port}\n  remote = \"localhost:5432\"\n",
        fake.display()
    ));
    env.cmd()
        .args(["tunnel", "up", "db", "pg"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Tunnel db/pg up"));
    let out = env.cmd().args(["tunnel", "ls", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v[0]["id"], "db/pg");
    assert_eq!(v[0]["spec"], format!("-L {port}:localhost:5432"));
    env.cmd()
        .args(["tunnel", "up", "db", "pg"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already running"));
    env.cmd()
        .args(["tunnel", "up", "db", "nope"])
        .assert()
        .code(2);
    env.cmd()
        .args(["tunnel", "down", "--all"])
        .assert()
        .success();
    let out = env.cmd().args(["tunnel", "ls", "--json"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
}

/// rsync through a fake "ssh" that runs the remote command in a local shell, like sshd.
#[test]
fn send_and_get_with_spaces_via_rsync() {
    use std::os::unix::fs::PermissionsExt;
    if sshoba::core::launcher::which("rsync").is_none() {
        return;
    }
    let env = Env::new();
    let remote_home = env.path().join("remote home");
    std::fs::create_dir_all(remote_home.join("up loads")).unwrap();
    let fake = env.path().join("fake-ssh");
    // argv: -F <file> sshoba.<name> <remote command words...>
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nshift 3\ncd '{}' || exit 1\nexec sh -c \"$*\"\n",
            remote_home.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    env.write_config(&format!(
        "[settings]\nssh_bin = \"{}\"\ntransfer_tool = \"rsync\"\ncontrol_master = false\n\n[[server]]\nname = \"box\"\nhost = \"h\"\n",
        fake.display()
    ));
    let local = env.path().join("local dir");
    std::fs::create_dir_all(&local).unwrap();
    let file = local.join("my file.txt");
    std::fs::write(&file, "hello").unwrap();

    env.cmd()
        .current_dir(&local)
        .args(["send", "box", "my file.txt", "~/up loads"])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(remote_home.join("up loads/my file.txt")).unwrap(),
        "hello"
    );

    std::fs::write(remote_home.join("up loads/back file.txt"), "back").unwrap();
    let down = env.path().join("down here");
    std::fs::create_dir_all(&down).unwrap();
    env.cmd()
        .args(["get", "box", "up loads/back file.txt"])
        .arg(&down)
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(down.join("back file.txt")).unwrap(),
        "back"
    );

    env.cmd()
        .args(["send", "box", "does-not-exist.txt"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("does-not-exist.txt"));
}

#[test]
fn export_ssh_config_to_file() {
    let env = Env::new();
    env.write_config(FIXTURE);
    let out_file = env.path().join("out").join("sshoba");
    env.cmd()
        .args(["export", "ssh-config", "--file"])
        .arg(&out_file)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "Include {}",
            out_file.display()
        )));
    let text = std::fs::read_to_string(&out_file).unwrap();
    assert!(text.contains("Host db-prod\n"));
    assert!(text.contains("ProxyJump bastion"));
}
