use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::Write,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

static DLL_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Records the path of the loaded `erqol.dll` so logs can be written next to it.
pub fn init_dll_path(hmodule: usize) {
    let _ = DLL_PATH.get_or_init(|| unsafe {
        let mut buffer = [0u16; 1024];
        let handle = HMODULE(hmodule as *mut core::ffi::c_void);
        let len = GetModuleFileNameW(Some(handle), &mut buffer) as usize;
        PathBuf::from(OsString::from_wide(&buffer[..len.min(buffer.len())]))
    });
}

fn log_file() -> &'static Mutex<Option<File>> {
    static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
    LOG_FILE.get_or_init(|| {
        let path = DLL_PATH
            .get()
            .and_then(|p| p.parent())
            .map(|dir| dir.join("logs").join("erqol.log"));
        let file = path.and_then(|p| open_log(&p));
        Mutex::new(file)
    })
}

fn open_log(path: &Path) -> Option<File> {
    std::fs::create_dir_all(path.parent()?).ok()?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Appends a line to `logs/erqol.log`. Failures are silently ignored so logging
/// never takes down the game.
pub fn log(msg: impl AsRef<str>) {
    let mut guard = log_file().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "{}", msg.as_ref());
    }
}
