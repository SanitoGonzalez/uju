use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use crossfire::{mpmc, null::CloseHandle};
use futures_util::{FutureExt, select};
use smallvec::SmallVec;

pub struct StopSource {
    tx: Cell<Option<CloseHandle<mpmc::Null>>>,
    requested: Rc<Cell<bool>>,
}

impl StopSource {
    pub fn new() -> (StopSource, StopToken) {
        let (tx, rx) = mpmc::Null::new().new_async();
        let requested = Rc::new(Cell::new(false));
        let mut links = SmallVec::new();
        links.push(StopTokenLink { rx, requested: requested.clone() });

        let source = StopSource {
            tx: Cell::new(Some(tx)),
            requested,
        };
        let token = StopToken { links };
        (source, token)
    }

    pub fn request(&self) {
        if self.tx.take().is_some() {
            self.requested.set(true);
        }
    }

    pub fn is_requested(&self) -> bool {
        self.requested.get()
    }
}

impl Drop for StopSource {
    fn drop(&mut self) {
        self.request();
    }
}

#[derive(Clone)]
pub struct StopToken {
    links: SmallVec<[StopTokenLink; 2]>,
}

#[derive(Clone)]
struct StopTokenLink {
    rx: crossfire::MAsyncRx<mpmc::Null>,
    requested: Rc<Cell<bool>>,
}

impl StopToken {
    pub async fn wait(&self) {
        let waits = self
            .links
            .iter()
            .map(|link| async move { while link.rx.recv().await.is_ok() {} }.boxed_local());
        futures_util::future::select_all(waits).await;
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
        self.links.iter().any(|l| l.requested.get())
    }

    pub fn child(&self) -> (StopSource, StopToken) {
        let (source, token) = StopSource::new();
        let mut links = self.links.clone();
        links.extend(token.links);

        (source, StopToken { links })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[compio::test]
    async fn test_request() {
        let (source, token) = StopSource::new();
        assert!(!token.is_requested());
        source.request();
        assert!(token.is_requested());
        token.wait().await;
    }

    #[compio::test]
    async fn test_drop_requests() {
        let (source, token) = StopSource::new();
        drop(source);
        assert!(token.is_requested());
        token.wait().await;
    }

    #[compio::test]
    async fn test_child_sees_parent_stop_synchronously() {
        let (parent, token) = StopSource::new();
        let (_child, combined) = token.child();
        parent.request();
        assert!(combined.is_requested());
        combined.wait().await;
    }

    #[compio::test]
    async fn test_child_stop_does_not_affect_parent() {
        let (_parent, token) = StopSource::new();
        let (child, combined) = token.child();
        child.request();
        assert!(combined.is_requested());
        assert!(!token.is_requested());
        combined.wait().await;
    }

    #[compio::test]
    async fn test_wait_for_interrupted() {
        let (source, token) = StopSource::new();
        let waiter = compio::runtime::spawn({
            let token = token.clone();
            async move { token.wait_for(Duration::from_secs(10)).await }
        });
        source.request();
        assert!(!waiter.await.unwrap());
    }

    #[compio::test]
    async fn test_wait_for_elapsed() {
        let (_source, token) = StopSource::new();
        assert!(token.wait_for(Duration::from_millis(10)).await);
    }

    /// Fails instead of hanging the suite if `fut` never completes.
    async fn assert_completes<T>(fut: impl Future<Output = T>) -> T {
        select! {
            v = fut.fuse() => v,
            _ = compio::time::sleep(Duration::from_secs(1)).fuse() => panic!("lost wakeup: future did not complete"),
        }
    }

    #[compio::test]
    async fn test_wait_wakes_parked_waiter() {
        let (source, token) = StopSource::new();
        let waiter = compio::runtime::spawn(async move { token.wait().await });
        // yield so the waiter polls and parks in recv() before we stop
        compio::time::sleep(Duration::from_millis(1)).await;
        source.request();
        assert_completes(waiter).await.unwrap();
    }

    #[compio::test]
    async fn test_child_wait_wakes_parked_waiter_on_parent_stop() {
        let (parent, token) = StopSource::new();
        let (_child, combined) = token.child();
        let waiter = compio::runtime::spawn(async move { combined.wait().await });
        compio::time::sleep(Duration::from_millis(1)).await;
        parent.request();
        assert_completes(waiter).await.unwrap();
    }

    #[compio::test]
    async fn test_wait_for_wakes_parked_waiter() {
        let (source, token) = StopSource::new();
        let waiter =
            compio::runtime::spawn(async move { token.wait_for(Duration::from_secs(30)).await });
        compio::time::sleep(Duration::from_millis(1)).await;
        source.request();
        assert!(!assert_completes(waiter).await.unwrap());
    }
}
