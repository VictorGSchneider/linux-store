//! Pure bookkeeping for the app page and its operations, kept apart from the window so the
//! rules can be tested: which `AppPage` answer is current, whether a click may start an
//! operation, and which app an operation's result belongs to.

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

/// Whether an install or remove is in flight, and for which app. Owned only by the click and by
/// `Started`/`Finished`; an unrelated error never touches it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpGate {
    pending: bool,
    key: Option<String>,
}

impl OpGate {
    /// A click on a source button of the app `page_key`. Returns the key to operate on when the
    /// click is accepted (nothing in flight, a page open).
    pub fn click(&mut self, page_key: Option<&str>, ui_active: bool) -> Option<String> {
        if self.pending || ui_active {
            return None;
        }
        let key = page_key?.to_string();
        self.pending = true;
        self.key = Some(key.clone());
        Some(key)
    }

    pub fn busy(&self) -> bool {
        self.pending
    }

    pub fn started(&mut self) {
        self.pending = true;
    }

    /// The operation ended (also a refusal that never started). Returns the app it was for.
    pub fn finished(&mut self) -> Option<String> {
        self.pending = false;
        self.key.clone()
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
    fn a_second_click_is_ignored_until_finished() {
        let mut g = OpGate::default();
        assert_eq!(g.click(Some("a"), false), Some("a".to_string()));
        assert_eq!(g.click(Some("a"), false), None);
        g.started();
        assert_eq!(g.click(Some("a"), false), None);
        assert!(g.busy());
        assert_eq!(g.finished(), Some("a".to_string()));
        assert_eq!(g.click(Some("b"), false), Some("b".to_string()));
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
