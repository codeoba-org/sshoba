//! Events: terminal input, ticks and results from background tasks, all on one channel.

use ratatui::crossterm::event::{self, Event as CtEvent, KeyEvent, KeyEventKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Paste(String),
    Resize,
    Tick,
    Msg(Msg),
}

/// Results from background work.
#[derive(Debug)]
pub enum Msg {
    Ping {
        name: String,
        outcome: crate::core::ping::Outcome,
    },
    /// A tunnel start (`up = true`) or stop finished.
    Tunnel {
        id: String,
        up: bool,
        result: Result<(), String>,
    },
    TabOpened {
        name: String,
        result: Result<(), String>,
    },
}

/// Reads terminal input on its own thread. Can be paused while a child process (the
/// user's editor) owns the terminal.
pub struct Input {
    paused: Arc<AtomicBool>,
    parked: Arc<AtomicBool>,
}

impl Input {
    pub fn spawn(tx: UnboundedSender<Event>) -> Input {
        let paused = Arc::new(AtomicBool::new(false));
        let parked = Arc::new(AtomicBool::new(false));
        let (p, k) = (paused.clone(), parked.clone());
        std::thread::spawn(move || loop {
            if p.load(Ordering::SeqCst) {
                k.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            k.store(false, Ordering::SeqCst);
            match event::poll(Duration::from_millis(50)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => return,
            }
            let ev = match event::read() {
                Ok(CtEvent::Key(k)) if k.kind != KeyEventKind::Release => Event::Key(k),
                Ok(CtEvent::Paste(s)) => Event::Paste(s),
                Ok(CtEvent::Resize(..)) => Event::Resize,
                Ok(_) => continue,
                Err(_) => return,
            };
            if tx.send(ev).is_err() {
                return;
            }
        });
        Input { paused, parked }
    }

    /// Stop reading input and wait until the reader thread is idle.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        let start = Instant::now();
        while !self.parked.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}
