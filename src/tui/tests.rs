//! Rendered-frame snapshots and update-loop tests using ratatui's TestBackend.

use crate::core::config::ConfigStore;
use crate::core::paths::Paths;
use crate::tui::app::{Action, App, Modal};
use crate::tui::event::Event;
use crate::tui::views;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;

pub const FIXTURE: &str = r#"
[settings]
launch = "current"

[[server]]
name = "bastion"
host = "203.0.113.10"
user = "ata"
group = "work"
tags = ["infra"]

[[server]]
name = "db-prod"
host = "10.0.0.5"
user = "postgres"
jump = ["bastion"]
group = "work"
tags = ["db", "prod"]
favorite = true

  [[server.forward]]
  name = "pg"
  kind = "local"
  local_port = 5432
  remote = "localhost:5432"

[[server]]
name = "atasoy"
host = "home.example.com"
group = "personal"
"#;

pub struct Harness {
    pub app: App,
    pub rx: tokio::sync::mpsc::UnboundedReceiver<Event>,
    _dir: tempfile::TempDir,
}

pub fn harness(text: &str) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        config_dir: dir.path().into(),
        config_file: dir.path().join("servers.toml"),
        state_dir: dir.path().join("state"),
    };
    std::fs::write(&paths.config_file, text).unwrap();
    let store = ConfigStore::load(&paths.config_file).unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    Harness {
        app: App::new(paths, store, tx),
        rx,
        _dir: dir,
    }
}

pub fn render(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| views::draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

pub fn key(app: &mut App, code: KeyCode) -> Option<Action> {
    app.update(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

pub fn ctrl(app: &mut App, c: char) -> Option<Action> {
    app.update(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL,
    )))
}

pub fn typ(app: &mut App, s: &str) {
    for c in s.chars() {
        key(app, KeyCode::Char(c));
    }
}

#[test]
fn main_screen_wide() {
    let h = harness(FIXTURE);
    insta::assert_snapshot!(render(&h.app, 100, 16));
}

#[test]
fn main_screen_narrow_hides_details() {
    let h = harness(FIXTURE);
    insta::assert_snapshot!(render(&h.app, 70, 12));
}

#[test]
fn empty_state() {
    let h = harness("");
    insta::assert_snapshot!(render(&h.app, 80, 10));
}

#[test]
fn favorites_first_and_groups_sorted() {
    let h = harness(FIXTURE);
    let names: Vec<String> = h.app.rows.iter().map(|r| format!("{r:?}")).collect();
    assert_eq!(
        names,
        [
            "Group { name: \"personal\", count: 1, collapsed: false }",
            "Server(\"atasoy\")",
            "Group { name: \"work\", count: 2, collapsed: false }",
            "Server(\"db-prod\")",
            "Server(\"bastion\")",
        ]
    );
}

#[test]
fn search_filters_and_esc_clears() {
    let mut h = harness(FIXTURE);
    key(&mut h.app, KeyCode::Char('/'));
    typ(&mut h.app, "prod");
    assert_eq!(h.app.selected_server().unwrap().name, "db-prod");
    insta::assert_snapshot!(render(&h.app, 100, 8));
    key(&mut h.app, KeyCode::Esc);
    assert!(h.app.search.is_empty());
    assert_eq!(h.app.rows.len(), 5);
}

#[test]
fn collapse_group() {
    let mut h = harness(FIXTURE);
    h.app.select_server("db-prod");
    key(&mut h.app, KeyCode::Char(' '));
    assert_eq!(h.app.rows.len(), 3);
    key(&mut h.app, KeyCode::Char(' '));
    assert_eq!(h.app.rows.len(), 5);
}

#[test]
fn enter_in_current_mode_execs_ssh() {
    let mut h = harness(FIXTURE);
    h.app.select_server("db-prod");
    match key(&mut h.app, KeyCode::Enter) {
        Some(Action::Exec(argv)) => {
            assert_eq!(argv[0], "ssh");
            assert_eq!(argv.last().unwrap(), "sshoba.db-prod");
        }
        other => panic!("expected exec, got {other:?}"),
    }
}

#[test]
fn add_via_form() {
    let mut h = harness(FIXTURE);
    key(&mut h.app, KeyCode::Char('a'));
    typ(&mut h.app, "web");
    key(&mut h.app, KeyCode::Tab);
    h.app.update(Event::Paste("me@1.2.3.4:2200".into()));
    insta::assert_snapshot!(render(&h.app, 100, 30));
    ctrl(&mut h.app, 's');
    assert!(h.app.modal.is_none(), "{:?}", h.app.modal);
    let web = h.app.config().get("web").unwrap();
    assert_eq!(web.user.as_deref(), Some("me"));
    assert_eq!(web.port, Some(2200));
    assert_eq!(h.app.selected_server().unwrap().name, "web");
    let text = std::fs::read_to_string(&h.app.paths.config_file).unwrap();
    assert!(text.contains("name = \"web\""));
}

#[test]
fn edit_rename_keeps_forwards() {
    let mut h = harness(FIXTURE);
    h.app.select_server("bastion");
    key(&mut h.app, KeyCode::Char('e'));
    ctrl(&mut h.app, 'u');
    typ(&mut h.app, "jump");
    ctrl(&mut h.app, 's');
    assert!(h.app.modal.is_none());
    assert_eq!(h.app.config().get("db-prod").unwrap().jump, ["jump"]);
    assert_eq!(h.app.config().get("db-prod").unwrap().forward.len(), 1);
}

#[test]
fn delete_used_jump_asks_then_removes_refs() {
    let mut h = harness(FIXTURE);
    h.app.select_server("bastion");
    key(&mut h.app, KeyCode::Char('d'));
    match &h.app.modal {
        Some(Modal::Confirm { message, .. }) => assert!(message.contains("db-prod")),
        other => panic!("{other:?}"),
    }
    key(&mut h.app, KeyCode::Char('y'));
    assert!(h.app.config().get("bastion").is_none());
    assert!(h.app.config().get("db-prod").unwrap().jump.is_empty());
}

#[test]
fn rename_prompt_and_favorite() {
    let mut h = harness(FIXTURE);
    h.app.select_server("atasoy");
    key(&mut h.app, KeyCode::Char('r'));
    ctrl(&mut h.app, 'u');
    typ(&mut h.app, "home");
    key(&mut h.app, KeyCode::Enter);
    assert!(h.app.config().get("home").is_some());
    assert_eq!(h.app.selected_server().unwrap().name, "home");
    key(&mut h.app, KeyCode::Char('s'));
    assert!(h.app.config().get("home").unwrap().favorite);
}

#[test]
fn help_overlay_lists_every_binding() {
    let mut h = harness(FIXTURE);
    key(&mut h.app, KeyCode::Char('?'));
    let screen = render(&h.app, 100, 40);
    for b in crate::tui::keys::MAIN {
        assert!(screen.contains(b.help), "missing {}", b.help);
    }
    key(&mut h.app, KeyCode::Esc);
    assert!(h.app.modal.is_none());
}

#[test]
fn reloads_when_file_changes() {
    let mut h = harness(FIXTURE);
    h.app.select_server("atasoy");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut text = std::fs::read_to_string(&h.app.paths.config_file).unwrap();
    text.push_str("\n[[server]]\nname = \"new\"\nhost = \"n\"\n");
    std::fs::write(&h.app.paths.config_file, text).unwrap();
    h.app.update(Event::Tick);
    assert!(h.app.config().get("new").is_some());
    assert_eq!(h.app.selected_server().unwrap().name, "atasoy");
}

#[tokio::test(flavor = "multi_thread")]
async fn ping_updates_dots_and_caches() {
    use tokio::io::AsyncWriteExt;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let _ = s.write_all(b"SSH-2.0-test\r\n").await;
        }
    });
    let mut h = harness(&format!(
        "[settings]\nping_interval = 0\n[[server]]\nname = \"local\"\nhost = \"127.0.0.1\"\nport = {port}\n"
    ));
    key(&mut h.app, KeyCode::Char('P'));
    assert!(h.app.checking.contains("local"));
    assert!(render(&h.app, 80, 6).contains("◌"));
    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), h.rx.recv())
        .await
        .unwrap()
        .unwrap();
    h.app.update(ev);
    assert!(h.app.checking.is_empty());
    let screen = render(&h.app, 80, 6);
    assert!(screen.contains("●") && screen.contains("ms"), "{screen}");
    let state = crate::core::state::State::load(&h.app.paths);
    assert_eq!(
        state.ping["local"].status,
        crate::core::state::PingStatus::Up
    );
}

#[test]
fn tunnels_screen_add_and_delete_forward() {
    let mut h = harness(FIXTURE);
    h.app.select_server("db-prod");
    key(&mut h.app, KeyCode::Char('t'));
    insta::assert_snapshot!(render(&h.app, 100, 8));
    key(&mut h.app, KeyCode::Char('a'));
    typ(&mut h.app, "web -L 8080:localhost:80 auto");
    key(&mut h.app, KeyCode::Enter);
    assert!(h.app.modal.is_none(), "{:?}", h.app.modal);
    let f = h
        .app
        .config()
        .get("db-prod")
        .unwrap()
        .forward("web")
        .unwrap()
        .clone();
    assert!(f.auto);
    assert_eq!(h.app.tunnel_rows().len(), 2);
    key(&mut h.app, KeyCode::Char('j'));
    key(&mut h.app, KeyCode::Char('d'));
    key(&mut h.app, KeyCode::Char('y'));
    assert!(h
        .app
        .config()
        .get("db-prod")
        .unwrap()
        .forward("web")
        .is_none());
    key(&mut h.app, KeyCode::Char('?'));
    assert!(render(&h.app, 100, 20).contains("Start / stop tunnel"));
    key(&mut h.app, KeyCode::Esc);
    key(&mut h.app, KeyCode::Esc);
    assert!(matches!(h.app.screen, crate::tui::app::Screen::Main));
}

#[test]
fn forward_prompt_parsing() {
    use crate::tui::app::parse_forward_input;
    assert_eq!(parse_forward_input("s -D 1080").unwrap().spec(), "1080");
    assert!(parse_forward_input("pg 5432").is_err());
    assert!(parse_forward_input("pg -X 1:h:2").is_err());
}

#[test]
fn toggle_tunnel_up_and_down() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-ssh");
    std::fs::write(&fake, "#!/bin/sh\nwhile true; do sleep 1; done\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut h = harness(&format!(
        "[settings]\nssh_bin = \"{}\"\nping_interval = 0\n[[server]]\nname = \"s\"\nhost = \"h\"\n[[server.forward]]\nname = \"r\"\nkind = \"remote\"\nlocal_port = 9000\nremote = \"localhost:9000\"\n",
        fake.display()
    ));
    key(&mut h.app, KeyCode::Char('t'));
    key(&mut h.app, KeyCode::Enter); // runs synchronously without a runtime
    let ev = h.rx.try_recv().unwrap();
    h.app.update(ev);
    assert!(h.app.running_tunnel("s/r").is_some());
    let screen = render(&h.app, 100, 8);
    assert!(
        screen.contains("● up") && screen.contains("1 tunnel up"),
        "{screen}"
    );
    key(&mut h.app, KeyCode::Enter);
    let ev = h.rx.try_recv().unwrap();
    h.app.update(ev);
    assert!(h.app.running_tunnel("s/r").is_none());
}

#[test]
fn transfer_dialog_send_via_rsync() {
    use crate::tui::views::transfer::Step;
    use std::os::unix::fs::PermissionsExt;
    if crate::core::launcher::which("rsync").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote");
    std::fs::create_dir_all(&remote).unwrap();
    let fake = dir.path().join("fake-ssh");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nshift 3\ncd '{}' || exit 1\nexec sh -c \"$*\"\n",
            remote.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let src = dir.path().join("payload.bin");
    std::fs::write(&src, vec![7u8; 4096]).unwrap();
    let mut h = harness(&format!(
        "[settings]\nssh_bin = \"{}\"\ntransfer_tool = \"rsync\"\ncontrol_master = false\nping_interval = 0\n[[server]]\nname = \"box\"\nhost = \"h\"\n",
        fake.display()
    ));
    key(&mut h.app, KeyCode::Char('x'));
    key(&mut h.app, KeyCode::Char('s'));
    h.app.update(Event::Paste(src.display().to_string()));
    key(&mut h.app, KeyCode::Enter);
    let screen = render(&h.app, 80, 14);
    assert!(
        screen.contains("Send to box") && screen.contains("remote dir"),
        "{screen}"
    );
    key(&mut h.app, KeyCode::Enter); // start; runs synchronously without a runtime
    while let Ok(ev) = h.rx.try_recv() {
        h.app.update(ev);
    }
    match &h.app.modal {
        Some(Modal::Transfer(t)) => assert_eq!(t.step, Step::Done(Ok(())), "{:?}", t.step),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        std::fs::read(remote.join("payload.bin")).unwrap().len(),
        4096
    );
    key(&mut h.app, KeyCode::Char('z'));
    assert!(h.app.modal.is_none());
}

#[test]
fn transfer_dialog_missing_source() {
    use crate::tui::views::transfer::Step;
    let mut h = harness(FIXTURE);
    key(&mut h.app, KeyCode::Char('x'));
    key(&mut h.app, KeyCode::Char('s'));
    typ(&mut h.app, "/definitely/not/here");
    key(&mut h.app, KeyCode::Enter);
    key(&mut h.app, KeyCode::Enter);
    match &h.app.modal {
        Some(Modal::Transfer(t)) => {
            assert!(matches!(&t.step, Step::Done(Err(e)) if e.contains("not/here")))
        }
        other => panic!("{other:?}"),
    }
}

/// A fake ssh that runs the remote command in a shell inside `remote` (as $HOME).
fn fake_remote_ssh(dir: &std::path::Path, remote: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let fake = dir.join("fake-ssh");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in sshoba.*) shift; [ \"$1\" = \"--\" ] && shift; break;; *) shift;; esac; done\nexport HOME='{r}'\ncd '{r}' || exit 1\nexec sh -c \"$*\"\n",
            r = remote.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    fake
}

fn drain(h: &mut Harness) {
    while let Ok(ev) = h.rx.try_recv() {
        h.app.update(ev);
    }
}

#[test]
fn file_browser_lists_and_copies_both_ways() {
    use crate::tui::app::Screen;
    if crate::core::launcher::which("rsync").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote");
    let local = dir.path().join("local");
    std::fs::create_dir_all(remote.join("logs")).unwrap();
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(remote.join("server file.txt"), "from server").unwrap();
    std::fs::write(remote.join(".hidden"), "").unwrap();
    std::fs::write(local.join("a.txt"), "local a").unwrap();
    std::fs::write(local.join("b.txt"), "local b").unwrap();
    let fake = fake_remote_ssh(dir.path(), &remote);
    let mut h = harness(&format!(
        "[settings]\nssh_bin = \"{}\"\ntransfer_tool = \"rsync\"\ncontrol_master = false\nping_interval = 0\n[[server]]\nname = \"box\"\nhost = \"h\"\n",
        fake.display()
    ));
    key(&mut h.app, KeyCode::Char('f'));
    drain(&mut h);
    let remote_real = remote.display().to_string();
    {
        let Screen::Files(b) = &mut h.app.screen else {
            panic!()
        };
        assert_eq!(b.remote.error, None);
        assert_eq!(b.remote.path, remote_real);
        let names: Vec<&str> = b
            .remote
            .visible(false)
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, ["logs", "server file.txt"]);
        b.local.path = local.display().to_string();
    }
    key(&mut h.app, KeyCode::Char('r')); // refresh local pane at the new path
                                         // Mark both local files and copy them to the remote pane.
    key(&mut h.app, KeyCode::Char(' '));
    key(&mut h.app, KeyCode::Char(' '));
    key(&mut h.app, KeyCode::Char('c'));
    drain(&mut h);
    assert!(h.app.modal.is_none(), "{:?}", h.app.modal);
    assert_eq!(
        std::fs::read_to_string(remote.join("a.txt")).unwrap(),
        "local a"
    );
    assert_eq!(
        std::fs::read_to_string(remote.join("b.txt")).unwrap(),
        "local b"
    );
    {
        let Screen::Files(b) = &h.app.screen else {
            panic!()
        };
        assert!(
            b.remote.entries.iter().any(|e| e.name == "b.txt"),
            "remote pane refreshed"
        );
        assert!(b.local.marked.is_empty());
    }
    // Remote → local: highlight "server file.txt" and copy.
    key(&mut h.app, KeyCode::Tab);
    {
        let Screen::Files(b) = &mut h.app.screen else {
            panic!()
        };
        b.remote.selected = b
            .remote
            .visible(false)
            .iter()
            .position(|e| e.name == "server file.txt")
            .unwrap();
    }
    key(&mut h.app, KeyCode::Char('c'));
    drain(&mut h);
    assert_eq!(
        std::fs::read_to_string(local.join("server file.txt")).unwrap(),
        "from server"
    );
    // Hidden files toggle, directory navigation and back.
    key(&mut h.app, KeyCode::Char('.'));
    {
        let Screen::Files(b) = &mut h.app.screen else {
            panic!()
        };
        assert!(b.remote.visible(true).iter().any(|e| e.name == ".hidden"));
        b.remote.selected = b
            .remote
            .visible(true)
            .iter()
            .position(|e| e.name == "logs")
            .unwrap();
    }
    key(&mut h.app, KeyCode::Enter);
    drain(&mut h);
    {
        let Screen::Files(b) = &h.app.screen else {
            panic!()
        };
        assert!(b.remote.path.ends_with("/logs"), "{}", b.remote.path);
    }
    key(&mut h.app, KeyCode::Backspace);
    drain(&mut h);
    {
        let Screen::Files(b) = &h.app.screen else {
            panic!()
        };
        assert_eq!(b.remote.path, remote_real);
        assert_eq!(b.remote.current(true).unwrap().name, "logs");
    }
    assert!(render(&h.app, 120, 12).contains("server file.txt"));
    key(&mut h.app, KeyCode::Esc);
    assert!(matches!(h.app.screen, Screen::Main));
}

#[test]
fn y_copies_readable_command() {
    let mut h = harness(FIXTURE);
    h.app.select_server("db-prod");
    assert_eq!(
        key(&mut h.app, KeyCode::Char('y')),
        Some(Action::Copy(
            "ssh -J ata@203.0.113.10 postgres@10.0.0.5".into()
        ))
    );
}
