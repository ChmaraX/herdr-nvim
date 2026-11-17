//! Windows gives a child every inheritable handle of its parent, not only the
//! stdio it is told. When the app's `exec` runs herdr-nvim, this process's
//! stdin, stdout and stderr are the exec's pipes, and the app's `exec` does
//! not return until every holder has closed them. A tab's nvim daemon,
//! started (through the launcher and the runtime) from such an exec, would
//! hold them for its whole life, and the tree click that started it would
//! never finish (the real app, 0.1.16: winprobe's detached child kept its
//! exec waiting 21 s, its whole life).
//!
//! So both binaries make their own std handles non-inheritable first thing.
//! Rust gives a child duplicates of what it inherits (`Stdio::inherit`), so
//! children that should share this process's stdio still do.

/// Makes this process's std handles non-inheritable (Windows; elsewhere
/// nothing). First thing in `main`.
pub fn keep_std_handles_to_self() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE},
            System::Console::{
                GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
            },
        };
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: GetStdHandle and SetHandleInformation on this process's
            // own handles; a missing or invalid one is skipped, a failure
            // ignored (a console handle can't be changed, and isn't a pipe).
            unsafe {
                let handle = GetStdHandle(id);
                if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}
