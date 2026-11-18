//! Thread priorities, so playback keeps up while a game loads the CPU.

/// For the decoder feeding the output ring: it must never fall behind.
pub(crate) fn raise_current_thread() {
    #[cfg(windows)]
    set(windows_sys::Win32::System::Threading::THREAD_PRIORITY_HIGHEST);
}

/// For background work (analysing the next track) that can wait.
pub(crate) fn lower_current_thread() {
    #[cfg(windows)]
    set(windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL);
}

#[cfg(windows)]
fn set(priority: windows_sys::Win32::System::Threading::THREAD_PRIORITY) {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};
    // SAFETY: the pseudo-handle of the calling thread is always valid.
    unsafe {
        SetThreadPriority(GetCurrentThread(), priority);
    }
}
