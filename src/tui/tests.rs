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
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    Harness {
        app: App::new(paths, store, tx),
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
            assert_eq!(argv.last().unwrap(), "shhoba.db-prod");
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
