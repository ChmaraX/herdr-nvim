//! Windows processes through the Win32 API: which run and from what
//! executable, when one started, and ending one together with the
//! processes it started.

#![cfg(windows)]

use std::{
    ffi::OsString,
    io,
    os::windows::{ffi::OsStringExt, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Threading::{
            GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
            CREATE_NO_WINDOW, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        },
    },
};

pub struct Proc {
    pub pid: u32,
    pub image: PathBuf,
}

/// Every running process this user can open, with its executable: a
/// Toolhelp32 snapshot, then `QueryFullProcessImageNameW` on each. One that
/// cannot be opened (another user's, a protected one) or exits during the
/// scan is left out.
pub fn processes() -> io::Result<Vec<Proc>> {
    // SAFETY: plain Win32 calls; the snapshot handle is owned by `Handle`.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = Handle(snapshot);
    // SAFETY: an all-zero PROCESSENTRY32W is valid; dwSize is set below.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut image = vec![0u16; 32_768];
    let mut procs = Vec::new();
    // SAFETY: `entry` is a properly sized PROCESSENTRY32W.
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
        return Err(io::Error::last_os_error());
    }
    loop {
        let pid = entry.th32ProcessID;
        if let Some(image) =
            open(pid, PROCESS_QUERY_LIMITED_INFORMATION).and_then(|p| p.image(&mut image))
        {
            procs.push(Proc { pid, image });
        }
        // SAFETY: as above; it fails with ERROR_NO_MORE_FILES at the end.
        if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
            return Ok(procs);
        }
    }
}

/// When the running process `pid` started. `None` once it has exited (a
/// handle someone still holds keeps an exited process's pid taken), and when
/// no process this user can open has that pid.
pub fn started(pid: u32) -> Option<SystemTime> {
    let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE)?;
    // SAFETY: a valid process handle opened with SYNCHRONIZE.
    if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: a valid process handle and four writable FILETIMEs.
    let ok =
        unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    // FILETIME: 100 ns ticks since 1601-01-01 UTC.
    const TICKS_TO_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    let since_epoch = ticks.checked_sub(TICKS_TO_UNIX_EPOCH)?;
    Some(
        UNIX_EPOCH
            + Duration::new(
                since_epoch / 10_000_000,
                (since_epoch % 10_000_000) as u32 * 100,
            ),
    )
}

/// End `pid` and every process it started, theirs included:
/// `%SystemRoot%\System32\taskkill.exe /PID <pid> /T /F`. An error when
/// taskkill fails, say because the process has exited already.
pub fn terminate_tree(pid: u32) -> io::Result<()> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let output = Command::new(PathBuf::from(root).join("System32").join("taskkill.exe"))
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        // taskkill is a console program: started by a GUI app (herdr's
        // exec), it would flash a console window.
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    })
    .trim()
    .to_owned();
    Err(io::Error::other(format!(
        "taskkill /PID {pid} /T /F failed ({}): {said}",
        output.status
    )))
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Handle {
    fn image(&self, buffer: &mut [u16]) -> Option<PathBuf> {
        let mut len = buffer.len() as u32;
        // SAFETY: `buffer` holds `len` u16s; on success `len` is the path's length.
        let ok = unsafe {
            QueryFullProcessImageNameW(self.0, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len)
        };
        (ok != 0).then(|| PathBuf::from(OsString::from_wide(&buffer[..len as usize])))
    }
}

fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Option<Handle> {
    // SAFETY: OpenProcess returns null on failure, else a handle we own.
    let handle = unsafe { OpenProcess(access, 0, pid) };
    (!handle.is_null()).then(|| Handle(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_listed_running_from_its_executable() {
        let me = std::process::id();
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        let listed = processes().unwrap();
        let mine = listed.iter().find(|p| p.pid == me).expect("own pid listed");
        assert_eq!(mine.image.canonicalize().unwrap(), exe);
        let started = started(me).expect("running");
        assert!(started <= SystemTime::now());
    }
}
