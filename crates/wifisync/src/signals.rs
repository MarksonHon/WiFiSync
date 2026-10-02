//! Signal handling: only a flag is set; the actual teardown (restoring the network) is done
//! synchronously in the main loop.

use std::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_shutdown(_signal: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

extern "C" fn on_reload(_signal: libc::c_int) {
    RELOAD.store(true, Ordering::SeqCst);
}

/// Install the SIGTERM / SIGINT / SIGHUP handlers.
pub fn install() {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_shutdown as extern "C" fn(libc::c_int) as *const () as usize;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());

        let mut reload: libc::sigaction = std::mem::zeroed();
        reload.sa_sigaction = on_reload as extern "C" fn(libc::c_int) as *const () as usize;
        reload.sa_flags = 0;
        libc::sigemptyset(&mut reload.sa_mask);
        libc::sigaction(libc::SIGHUP, &reload, std::ptr::null_mut());
    }
}

pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Take and clear the "SIGHUP received, configuration reload needed" flag.
pub fn take_reload_request() -> bool {
    RELOAD.swap(false, Ordering::SeqCst)
}

/// Sleep for the given milliseconds, but wake every 100 ms to check the shutdown flag.
pub fn sleep_interruptible(millis: u64) {
    let step = 100;
    let mut left = millis;
    while left > 0 && !shutdown_requested() {
        let chunk = left.min(step);
        std::thread::sleep(std::time::Duration::from_millis(chunk));
        left -= chunk;
    }
}
