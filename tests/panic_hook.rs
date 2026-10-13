//! The panic hook lives in its own test binary because the panic hook is
//! process-global and would leak into unrelated tests.

use std::panic;
use std::sync::atomic::{AtomicUsize, Ordering};

use ptal::tui::install_restore_hook;

static RESTORED: AtomicUsize = AtomicUsize::new(0);
static PREVIOUS: AtomicUsize = AtomicUsize::new(0);

#[test]
fn a_panic_restores_the_terminal_once_and_still_reports_through_the_old_hook() {
    panic::set_hook(Box::new(|_| {
        PREVIOUS.fetch_add(1, Ordering::SeqCst);
    }));
    install_restore_hook(|| {
        // The terminal must be back before the message is printed.
        assert_eq!(PREVIOUS.load(Ordering::SeqCst), 0);
        RESTORED.fetch_add(1, Ordering::SeqCst);
    });

    let result = panic::catch_unwind(|| panic!("boom"));

    assert!(result.is_err());
    assert_eq!(RESTORED.load(Ordering::SeqCst), 1);
    assert_eq!(PREVIOUS.load(Ordering::SeqCst), 1);
}
