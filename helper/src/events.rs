//! Turns libalpm's callbacks into protocol events, and answers its questions safely.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use alpm::{Alpm, DownloadEvent, Event as AlpmEvent, Progress, Question};
use store_protocol::{Event, Stage};

use crate::CancelGate;

/// Where events go. libalpm callbacks must be `'static`, so this is a shared handle rather than
/// a borrowed closure. It is not `Send`: libalpm calls back on the thread that drives it.
#[derive(Clone)]
pub struct Emitter(Rc<RefCell<dyn FnMut(Event)>>);

impl Emitter {
    pub fn new(sink: impl FnMut(Event) + 'static) -> Emitter {
        Emitter(Rc::new(RefCell::new(sink)))
    }

    pub fn emit(&self, event: Event) {
        (self.0.borrow_mut())(event);
    }
}

/// Questions libalpm asked that this helper will not answer "yes" to.
#[derive(Default)]
pub(crate) struct QuestionLog {
    /// A package signature key that is not trusted was met (the helper never imports keys).
    pub unknown_key: bool,
    /// Dependencies with several providers: the helper never picks one silently.
    pub ambiguous: Vec<String>,
}

/// Emits a progress event only when the percentage for that stage and package changes.
#[derive(Default)]
struct Throttle(HashMap<String, u8>);

impl Throttle {
    fn changed(&mut self, stage: Stage, name: &str, percent: u8) -> bool {
        let key = format!("{stage:?}/{name}");
        if self.0.get(&key) == Some(&percent) {
            return false;
        }
        self.0.insert(key, percent);
        true
    }
}

fn progress(stage: Stage, package: Option<&str>, percent: Option<u8>) -> Event {
    Event::Progress {
        stage,
        package: package.map(str::to_owned),
        percent,
    }
}

/// `tree-2.3.2-1-x86_64.pkg.tar.zst` becomes `tree`; database files and odd names become `None`.
pub(crate) fn package_from_filename(file: &str) -> Option<String> {
    let stem = &file[..file.find(".pkg.tar")?];
    let parts: Vec<&str> = stem.rsplitn(4, '-').collect();
    (parts.len() == 4).then(|| parts[3].to_owned())
}

fn percent_of(done: i64, total: i64) -> u8 {
    if total <= 0 {
        0
    } else {
        (done.saturating_mul(100) / total).clamp(0, 100) as u8
    }
}

/// Registers every callback on `alpm`. The returned log records the questions that need a human.
pub(crate) fn attach(
    alpm: &Alpm,
    emitter: &Emitter,
    gate: &CancelGate,
) -> Rc<RefCell<QuestionLog>> {
    let questions = Rc::new(RefCell::new(QuestionLog::default()));
    let throttle = Rc::new(RefCell::new(Throttle::default()));

    alpm.set_event_cb((emitter.clone(), gate.clone()), |event, (emitter, gate)| {
        match event.event() {
            AlpmEvent::RetrieveStart => emitter.emit(progress(Stage::Refresh, None, None)),
            AlpmEvent::ResolveDepsStart => emitter.emit(progress(Stage::Resolve, None, None)),
            AlpmEvent::PkgRetrieveStart(_) => emitter.emit(progress(Stage::Download, None, None)),
            AlpmEvent::IntegrityStart
            | AlpmEvent::KeyringStart
            | AlpmEvent::LoadStart
            | AlpmEvent::FileConflictsStart
            | AlpmEvent::InterConflictsStart => emitter.emit(progress(Stage::Verify, None, None)),
            // Pre-transaction hooks run before the first package operation and already change
            // the system (snapshots, kernel images), so they are the point of no return too.
            AlpmEvent::HookStart(_) => {
                gate.commit_started();
                emitter.emit(progress(Stage::Hooks, None, None));
            }
            AlpmEvent::TransactionStart | AlpmEvent::PackageOperationStart(_) => {
                gate.commit_started();
            }
            _ => {}
        }
    });

    alpm.set_progress_cb(
        (emitter.clone(), throttle.clone()),
        |kind, name, percent, _count, _current, (emitter, throttle)| {
            let stage = match kind {
                Progress::AddStart => Stage::Install,
                Progress::UpgradeStart | Progress::DowngradeStart | Progress::ReinstallStart => {
                    Stage::Upgrade
                }
                Progress::RemoveStart => Stage::Remove,
                _ => Stage::Verify,
            };
            // libalpm reports its verify steps with an empty name; the stage start event already
            // told the client what is happening.
            if name.is_empty() {
                return;
            }
            let percent = percent.clamp(0, 100) as u8;
            if throttle.borrow_mut().changed(stage, name, percent) {
                emitter.emit(progress(stage, Some(name), Some(percent)));
            }
        },
    );

    alpm.set_dl_cb(
        (emitter.clone(), throttle),
        |file, event, (emitter, throttle)| {
            if file.ends_with(".sig") {
                return;
            }
            let percent = match event.event() {
                DownloadEvent::Init(_) => 0,
                DownloadEvent::Progress(p) => percent_of(p.downloaded, p.total),
                DownloadEvent::Completed(_) => 100,
                DownloadEvent::Retry(_) => return,
            };
            let (stage, package) = if file.ends_with(".db") || file.ends_with(".files") {
                (Stage::Refresh, None)
            } else {
                (Stage::Download, package_from_filename(file))
            };
            let key = package.as_deref().unwrap_or(file);
            if throttle.borrow_mut().changed(stage, key, percent) {
                emitter.emit(progress(stage, package.as_deref(), Some(percent)));
            }
        },
    );

    alpm.set_question_cb(questions.clone(), |question, log| {
        match question.question() {
            Question::InstallIgnorepkg(mut q) => q.set_install(false),
            Question::Replace(q) => q.set_replace(false),
            Question::Conflict(mut q) => q.set_remove(false),
            // A package that failed verification is deleted from the cache (as pacman does by
            // default), so the next attempt downloads it again instead of failing the same way.
            Question::Corrupted(mut q) => q.set_remove(true),
            Question::RemovePkgs(mut q) => q.set_skip(false),
            Question::SelectProvider(mut q) => {
                log.borrow_mut()
                    .ambiguous
                    .push("a dependency has several providers".to_string());
                q.set_index(0);
            }
            Question::ImportKey(mut q) => {
                log.borrow_mut().unknown_key = true;
                q.set_import(false);
            }
        }
    });

    questions
}
