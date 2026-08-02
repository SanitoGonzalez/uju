use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use futures_util::{FutureExt, select};

#[derive(Clone)]
pub struct StopSource {
    tx: Option<crossfire::null::CloseHandle<crossfire::mpmc::Null>>,
    requested: Rc<Cell<bool>>,
}

#[derive(Clone)]
pub struct StopToken {
    rx: crossfire::MAsyncRx<crossfire::mpmc::Null>,
    requested: Rc<Cell<bool>>,
}

pub fn new() -> (StopSource, StopToken) {
    let (tx, rx) = crossfire::mpmc::Null::new().new_async();
    let requested = Rc::new(Cell::new(false));

    let source = StopSource {
        tx: Some(tx),
        requested: requested.clone(),
    };
    let token = StopToken { rx, requested };
    (source, token)
}

impl StopSource {
    pub fn request(&mut self) {
        if self.tx.take().is_some() {
            self.requested.set(true);
        }
    }

    pub fn is_requested(&self) -> bool {
        self.requested.get()
    }
}

impl StopToken {
    pub async fn wait(&self) {
        while self.rx.recv().await.is_ok() {}
    }

    /// Sleep for `duration`, returning early if stop already requested.
    ///
    /// Returns `true` if the full duration elapsed, `false` if stop interrupted the sleep.
    pub async fn wait_for(&self, duration: Duration) -> bool {
        if self.is_requested() {
            return false;
        }

        select! {
            _ = self.wait().fuse() => false,
            _ = compio::time::sleep(duration).fuse() => !self.is_requested(),
        }
    }

    pub fn is_requested(&self) -> bool {
        self.requested.get()
    }
}
