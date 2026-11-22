//! Pure bookkeeping for the app page and its operations, kept apart from the window so the
//! rules can be tested: which `AppPage` answer is current, whether a click may start an
//! operation, and which app an operation's result belongs to.
use std::time::{Duration, Instant};

/// Which app the user asked for and which one the page shows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PageKeys {
    open: Option<String>,
    page: Option<String>,
}

impl PageKeys {
    /// The user clicked a card: only an answer for this key is current from now on.
    pub fn request(&mut self, key: &str) {
        self.open = Some(key.to_string());
    }

    /// Whether an `AppPage(Some(..))` for `key` is the one asked for last. If so the page shows it.
    pub fn accept(&mut self, key: &str) -> bool {
        if self.open.as_deref() == Some(key) {
            self.page = Some(key.to_string());
            true
        } else {
            false
        }
    }

    /// Whether any answer (also a "not found") is still wanted.
    pub fn waiting(&self) -> bool {
        self.open.is_some()
    }

    /// The app is gone: nothing is shown, but a newer request may still be answered.
    pub fn not_found(&mut self) {
        self.page = None;
    }

    /// Every way of leaving the app page (Back, search, chips, Home, Installeds, Updates):
    /// answers still in flight become stale.
    pub fn leave(&mut self) {
        self.open = None;
        self.page = None;
    }

    pub fn page(&self) -> Option<&str> {
        self.page.as_deref()
    }
}

/// The longest the buttons wait, after an operation finished, for the page to show its result.
/// Past it they come back even if the reload never arrived (the status text still tells what
/// happened).
pub const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Clicks are accepted.
    #[default]
    Idle,
    /// A click was sent, or an operation runs.
    Pending,
    /// The operation finished; the engine has not reloaded its source yet.
    Settling { since: Instant },
    /// The source is reloaded; the page was asked for again (the `seq`-th `Open`) and its
    /// answer has not arrived yet.
    Reloading { since: Instant, seq: u64 },
}

/// Whether an install or remove may be clicked, and which app the last one was for. Owned only
/// by the click, `Started`, `Finished`, `OperationSettled`, the page answers and the timeout;
/// an unrelated error never touches it.
///
/// After `Finished` the buttons stay closed (settling) until the page shows the source as it is
/// after the operation: the engine's `OperationSettled`, then the answer to the `Open` sent after
/// it. Otherwise a second click on a page that still says "Install" would start another
/// privileged operation. [`SETTLE_TIMEOUT`] bounds the wait.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpGate {
    phase: Phase,
    key: Option<String>,
    /// Operations finished whose `OperationSettled` has not arrived. The engine settles them in
    /// order, so a late settle of an operation that timed out is not taken for the next one's.
    unsettled: u32,
}

impl OpGate {
    /// A click on a source button of the app `page_key`. Returns the key to operate on when the
    /// click is accepted (nothing in flight or settling, a page open).
    pub fn click(&mut self, page_key: Option<&str>, ui_active: bool) -> Option<String> {
        if self.phase != Phase::Idle || ui_active {
            return None;
        }
        let key = page_key?.to_string();
        self.phase = Phase::Pending;
        self.key = Some(key.clone());
        Some(key)
    }

    /// Whether clicks are refused (in flight or settling).
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// Whether the buttons show the "updating" look: the operation ended but the page does not
    /// show its result yet.
    pub fn settling(&self) -> bool {
        matches!(self.phase, Phase::Settling { .. } | Phase::Reloading { .. })
    }

    /// An operation started (also one queued earlier): it owns the buttons now.
    pub fn started(&mut self) {
        self.phase = Phase::Pending;
    }

    /// The operation ended (also a refusal that never started). Returns the app it was for.
    pub fn finished(&mut self, now: Instant) -> Option<String> {
        self.unsettled += 1;
        self.phase = Phase::Settling { since: now };
        self.key.clone()
    }

    /// The engine reloaded the source of the oldest unsettled operation. `reopen` is the
    /// sequence number of the `Open` the caller sends for the page on show, or `None` when no
    /// app page is open (then nothing on screen can be stale).
    pub fn settled(&mut self, reopen: Option<u64>) {
        self.unsettled = self.unsettled.saturating_sub(1);
        if self.unsettled > 0 {
            return;
        }
        if let Phase::Settling { since } = self.phase {
            self.phase = match reopen {
                Some(seq) => Phase::Reloading { since, seq },
                None => Phase::Idle,
            };
        }
    }

    /// The answer to the `answered`-th `Open` arrived (shown or not).
    pub fn page_answered(&mut self, answered: u64) {
        if let Phase::Reloading { seq, .. } = self.phase
            && answered >= seq
        {
            self.phase = Phase::Idle;
        }
    }

    /// Reopens the buttons when the settle took longer than [`SETTLE_TIMEOUT`]. Returns whether
    /// it did.
    pub fn tick(&mut self, now: Instant) -> bool {
        match self.phase {
            Phase::Settling { since } | Phase::Reloading { since, .. }
                if now.saturating_duration_since(since) >= SETTLE_TIMEOUT =>
            {
                self.phase = Phase::Idle;
                true
            }
            _ => false,
        }
    }
}

/// The page below the app page: where the user is, or where Back returns to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Home rows or search results.
    #[default]
    Home,
    Installed,
    Updates,
}

impl Place {
    /// Whether this page shows a list the engine can be asked for again.
    pub fn is_list(self) -> bool {
        self != Place::Home
    }
}

/// Where the user is. Opening an app puts its page on top without changing `place`, so Back
/// returns to the page the user came from, list contents included.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Nav {
    place: Place,
}

impl Nav {
    /// The user went to `place` (search, chip, Home, Installeds, Updates).
    pub fn go(&mut self, place: Place) {
        self.place = place;
    }

    pub fn place(&self) -> Place {
        self.place
    }

    /// Where Back from an app page goes.
    pub fn back(&self) -> Place {
        self.place
    }
}

/// An operation's result is shown only on the page of the app it was started for.
pub fn status_visible_for(op_key: Option<&str>, page_key: Option<&str>) -> bool {
    matches!((op_key, page_key), (Some(a), Some(b)) if a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_requested_page_is_accepted() {
        let mut k = PageKeys::default();
        k.request("a");
        k.request("b");
        assert!(!k.accept("a"));
        assert_eq!(k.page(), None);
        assert!(k.accept("b"));
        assert_eq!(k.page(), Some("b"));
    }

    #[test]
    fn a_late_answer_after_leaving_is_stale() {
        let mut k = PageKeys::default();
        k.request("a");
        k.leave();
        assert!(!k.waiting());
        assert!(!k.accept("a"));
        assert_eq!(k.page(), None);
        // also when the page was already open
        k.request("b");
        assert!(k.accept("b"));
        k.leave();
        assert!(!k.accept("b"));
        assert_eq!(k.page(), None);
    }

    #[test]
    fn a_second_click_is_ignored_until_settled() {
        let now = Instant::now();
        let mut g = OpGate::default();
        assert_eq!(g.click(Some("a"), false), Some("a".to_string()));
        assert_eq!(g.click(Some("a"), false), None);
        g.started();
        assert_eq!(g.click(Some("a"), false), None);
        assert!(g.busy());
        assert!(!g.settling());
        assert_eq!(g.finished(now), Some("a".to_string()));
        // Finished: settling, a click is still ignored.
        assert!(g.settling() && g.busy());
        assert_eq!(g.click(Some("a"), false), None);
        // Reloaded, no page open: nothing on screen is stale.
        g.settled(None);
        assert!(!g.busy() && !g.settling());
        assert_eq!(g.click(Some("b"), false), Some("b".to_string()));
    }

    #[test]
    fn settling_waits_for_the_page_asked_after_the_reload() {
        let now = Instant::now();
        let mut g = OpGate::default();
        g.click(Some("a"), false);
        g.started();
        g.finished(now);
        // An answer to an earlier Open does not count: it may predate the reload.
        g.page_answered(3);
        assert!(g.settling());
        g.settled(Some(5));
        assert!(g.settling());
        g.page_answered(4);
        assert!(g.settling(), "an answer to an Open sent before the reload");
        assert_eq!(g.click(Some("a"), false), None);
        g.page_answered(5);
        assert!(!g.busy());
        assert_eq!(g.click(Some("a"), false), Some("a".to_string()));
    }

    #[test]
    fn settling_ends_after_the_timeout_at_the_latest() {
        let t0 = Instant::now();
        let mut g = OpGate::default();
        assert!(!g.tick(t0 + SETTLE_TIMEOUT), "idle: nothing to end");
        g.click(Some("a"), false);
        g.finished(t0);
        assert!(!g.tick(t0 + SETTLE_TIMEOUT - Duration::from_millis(1)));
        assert!(g.settling());
        assert!(g.tick(t0 + SETTLE_TIMEOUT));
        assert!(!g.busy());
        // Also while waiting for the page.
        g.click(Some("a"), false);
        g.finished(t0);
        g.settled(Some(1));
        assert!(g.tick(t0 + SETTLE_TIMEOUT));
        assert!(!g.busy());
    }

    #[test]
    fn a_late_settle_of_a_timed_out_operation_is_not_taken_for_the_next_one() {
        let t0 = Instant::now();
        let mut g = OpGate::default();
        g.click(Some("a"), false);
        g.finished(t0);
        assert!(g.tick(t0 + SETTLE_TIMEOUT));
        // A new operation starts and resets the settling state.
        g.click(Some("b"), false);
        g.started();
        assert!(!g.settling() && g.busy());
        g.finished(t0 + SETTLE_TIMEOUT);
        // The first operation's settle arrives late: the second is still settling.
        g.settled(Some(1));
        assert!(g.settling());
        g.settled(None);
        assert!(!g.busy());
    }

    #[test]
    fn a_started_operation_resets_settling() {
        let mut g = OpGate::default();
        g.click(Some("a"), false);
        g.finished(Instant::now());
        assert!(g.settling());
        // An operation queued earlier starts now.
        g.started();
        assert!(!g.settling() && g.busy());
    }

    #[test]
    fn click_needs_a_page_and_an_idle_ui() {
        let mut g = OpGate::default();
        assert_eq!(g.click(None, false), None);
        assert_eq!(g.click(Some("a"), true), None);
        assert!(!g.busy());
    }

    #[test]
    fn back_returns_to_where_the_user_came_from() {
        let mut n = Nav::default();
        assert_eq!(n.back(), Place::Home);
        n.go(Place::Installed);
        // opening an app does not change the place
        assert_eq!(n.back(), Place::Installed);
        n.go(Place::Updates);
        assert_eq!(n.back(), Place::Updates);
        n.go(Place::Home);
        assert_eq!(n.back(), Place::Home);
        assert!(!Place::Home.is_list());
        assert!(Place::Installed.is_list() && Place::Updates.is_list());
    }

    #[test]
    fn status_shows_only_on_the_operations_app() {
        assert!(status_visible_for(Some("a"), Some("a")));
        assert!(!status_visible_for(Some("a"), Some("b")));
        assert!(!status_visible_for(Some("a"), None));
        assert!(!status_visible_for(None, Some("a")));
        assert!(!status_visible_for(None, None));
    }
}
