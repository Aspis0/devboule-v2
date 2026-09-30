//! Selects and loads the ConPTY function table: a `conpty.dll` staged beside
//! the running executable, or kernel32's inbox implementation.
//!
//! The app-local lookup never uses a bare library name: a bare name would let
//! a `conpty.dll` in any PATH folder (or the current directory) load into
//! whichever process owns the pseudoconsoles. Only an absolute path derived
//! from `current_exe` is ever attempted, and `LoadLibraryExW` is pinned to
//! the DLL's own directory plus System32 for its dependencies. The bundle is
//! all-or-nothing — `conpty.dll` and the architecture's `OpenConsole.exe`
//! host together or not at all, in the `conpty/` subdirectory layout herdr's
//! installer deploys and verifies — and a reparse point is rejected when the
//! bundle is checked: immediately before the load, not atomically with it,
//! and a hard link is not a reparse point. Anything else falls back to
//! kernel32 without panicking and without leaving a partially populated
//! table. The crate has no logger; `conpty_source` reports the one-time
//! choice, including why a staged bundle was not used.

use lazy_static::lazy_static;
use std::ffi::OsStr;
use std::io::Error as IoError;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::{mem, ptr};
use winapi::shared::minwindef::{DWORD, HMODULE};
use winapi::shared::winerror::HRESULT;
use winapi::um::libloaderapi::{
    FreeLibrary, GetModuleHandleW, GetProcAddress, LoadLibraryExW,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use winapi::um::wincon::COORD;
use winapi::um::winnt::HANDLE;

type CreatePseudoConsoleFn = unsafe extern "system" fn(
    size: COORD,
    h_input: HANDLE,
    h_output: HANDLE,
    flags: DWORD,
    hpc: *mut HANDLE,
) -> HRESULT;
type ResizePseudoConsoleFn = unsafe extern "system" fn(hpc: HANDLE, size: COORD) -> HRESULT;
type ClosePseudoConsoleFn = unsafe extern "system" fn(hpc: HANDLE);

// These field names intentionally mirror the exported Win32 symbol names.
#[allow(non_snake_case)]
pub(super) struct ConPtyFuncs {
    pub(super) CreatePseudoConsole: CreatePseudoConsoleFn,
    pub(super) ResizePseudoConsole: ResizePseudoConsoleFn,
    pub(super) ClosePseudoConsole: ClosePseudoConsoleFn,
    module: HMODULE,
    owned: bool,
}

// HMODULE and the function pointers are plain code addresses, valid as long as
// the module stays loaded; the loader keeps it loaded for the process lifetime.
unsafe impl Send for ConPtyFuncs {}
unsafe impl Sync for ConPtyFuncs {}

impl Drop for ConPtyFuncs {
    fn drop(&mut self) {
        if self.owned {
            unsafe { FreeLibrary(self.module) };
        }
    }
}

enum ConPtyChoice {
    AppLocal,
    System,
    /// A complete bundle was staged, but the DLL would not load or was missing
    /// a symbol; the reason travels with the choice for the one startup line.
    SystemAfterFailedLoad(String),
}

impl ConPtyFuncs {
    unsafe fn from_module(module: HMODULE, owned: bool) -> Result<Self, String> {
        if module.is_null() {
            return Err(IoError::last_os_error().to_string());
        }
        Ok(Self {
            module,
            owned,
            CreatePseudoConsole: load_symbol(module, b"CreatePseudoConsole\0")?,
            ResizePseudoConsole: load_symbol(module, b"ResizePseudoConsole\0")?,
            ClosePseudoConsole: load_symbol(module, b"ClosePseudoConsole\0")?,
        })
    }
}

unsafe fn load_symbol<T: Copy>(module: HMODULE, name: &'static [u8]) -> Result<T, String> {
    debug_assert_eq!(mem::size_of::<T>(), mem::size_of::<*mut std::ffi::c_void>());
    let symbol = unsafe { GetProcAddress(module, name.as_ptr().cast()) };
    if symbol.is_null() {
        return Err(format!(
            "missing symbol {}",
            String::from_utf8_lossy(&name[..name.len() - 1])
        ));
    }
    Ok(unsafe { mem::transmute_copy(&symbol) })
}

fn load_system_conpty() -> Result<ConPtyFuncs, String> {
    let kernel = wide_string(OsStr::new("kernel32.dll"));
    let module = unsafe { GetModuleHandleW(kernel.as_ptr()) };
    unsafe { ConPtyFuncs::from_module(module, false) }
}

fn load_app_local_conpty(path: &Path) -> Result<ConPtyFuncs, String> {
    let wide = wide_string(path.as_os_str());
    let module = unsafe {
        LoadLibraryExW(
            wide.as_ptr(),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    match unsafe { ConPtyFuncs::from_module(module, true) } {
        Ok(funcs) => Ok(funcs),
        Err(error) => {
            // from_module can fail after a successful load; release the
            // module so no half-populated table survives.
            if !module.is_null() {
                unsafe { FreeLibrary(module) };
            }
            Err(error)
        }
    }
}

fn conpty_dll_candidate(exe_dir: &Path) -> PathBuf {
    exe_dir.join("conpty").join("conpty.dll")
}

fn openconsole_candidate(exe_dir: &Path) -> PathBuf {
    exe_dir.join("conpty").join("x64").join("OpenConsole.exe")
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

fn has_reparse_attribute(attributes: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

// A reparse point (symlink, junction, mount point) in the executable's own
// directory is not trusted: it could resolve the load somewhere else entirely.
// Checked immediately before the load, not atomically with it, and a hard
// link is not a reparse point.
fn is_regular_file(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_file() && !has_reparse_attribute(metadata.file_attributes()),
        Err(_) => false,
    }
}

// A DLL without its OpenConsole host would load and then fail every
// pseudoconsole; the two files are staged (and trusted) together or not at all.
fn app_local_bundle_complete(exe_dir: &Path) -> bool {
    is_regular_file(&conpty_dll_candidate(exe_dir)) && is_regular_file(&openconsole_candidate(exe_dir))
}

fn choose_conpty(exe_dir: Option<&Path>) -> (ConPtyFuncs, ConPtyChoice) {
    // Without kernel32's own exports the system is too old to run at all.
    let system = load_system_conpty().unwrap_or_else(|error| {
        panic!(
            "this system does not support conpty. Windows 10 October 2018 or newer is required: {}",
            error
        )
    });
    // The pinned package ships an x64 shim only, matching the architecture
    // this check pins; on any other architecture the staged pair could never
    // load into this process.
    if std::env::consts::ARCH != "x86_64" {
        return (system, ConPtyChoice::System);
    }
    let Some(dir) = exe_dir else {
        return (system, ConPtyChoice::System);
    };
    if !app_local_bundle_complete(dir) {
        // The normal case: no complete bundle staged, nothing to attempt.
        return (system, ConPtyChoice::System);
    }
    match load_app_local_conpty(&conpty_dll_candidate(dir)) {
        Ok(funcs) => (funcs, ConPtyChoice::AppLocal),
        Err(error) => (system, ConPtyChoice::SystemAfterFailedLoad(error)),
    }
}

static CONPTY_SOURCE: OnceLock<String> = OnceLock::new();

fn source_phrase(choice: ConPtyChoice) -> String {
    match choice {
        ConPtyChoice::AppLocal => "the app-local ConPTY bundle beside the executable".to_string(),
        ConPtyChoice::System => "the Windows inbox ConPTY (kernel32)".to_string(),
        ConPtyChoice::SystemAfterFailedLoad(error) => format!(
            "the Windows inbox ConPTY (a staged ConPTY bundle was present but could not be loaded: {error})"
        ),
    }
}

fn load_conpty() -> ConPtyFuncs {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    let (funcs, choice) = choose_conpty(exe_dir.as_deref());
    let _ = CONPTY_SOURCE.set(source_phrase(choice));
    funcs
}

/// Which ConPTY implementation the loader selected, as a human-readable
/// phrase. Forcing the one-time selection, so this is the same choice every
/// pseudoconsole in this process will use. A staged-but-unloadable bundle
/// names the reason; no path of the user is ever included.
pub fn conpty_source() -> String {
    // Deref forces the one-time selection this reports on.
    let _ = &*CONPTY;
    CONPTY_SOURCE
        .get()
        .expect("forcing the selection above always records its source")
        .clone()
}

fn wide_string(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

lazy_static! {
    pub(super) static ref CONPTY: ConPtyFuncs = load_conpty();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // One test changes the process current directory; the tests must not run
    // while another is mid-lookup.
    static PROCESS_LOCK: Mutex<()> = Mutex::new(());

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "portable-pty-conpty-loader-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory is created");
        dir
    }

    fn test_exe_dir() -> PathBuf {
        std::env::current_exe()
            .expect("test executable path is known")
            .parent()
            .expect("test executable has a parent directory")
            .to_path_buf()
    }

    // The staged layout herdr's installer deploys and verifies:
    // conpty/conpty.dll plus conpty/x64/OpenConsole.exe.
    fn stage_bundle(dir: &Path, dll_bytes: &[u8]) {
        let dll = dir.join("conpty").join("conpty.dll");
        std::fs::create_dir_all(dll.parent().unwrap()).expect("bundle dir is created");
        std::fs::write(dll, dll_bytes).expect("dll is written");
        let host = dir.join("conpty").join("x64").join("OpenConsole.exe");
        std::fs::create_dir_all(host.parent().unwrap()).expect("host dir is created");
        std::fs::write(host, b"placeholder").expect("host is written");
    }

    fn system_dll_bytes() -> Vec<u8> {
        let system_dll = "C:\\Windows\\System32\\version.dll";
        if !Path::new(system_dll).is_file() {
            eprintln!("skipping: {system_dll} is not present on this machine");
            return Vec::new();
        }
        std::fs::read(system_dll).expect("a real, loadable, symbol-less library is read")
    }

    #[test]
    fn absent_bundle_falls_back_to_the_system_conpty() {
        let dir = scratch_dir("absent");
        let (funcs, choice) = choose_conpty(Some(&dir));
        assert!(matches!(choice, ConPtyChoice::System));
        assert!(!funcs.owned);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_executable_directory_falls_back_to_the_system_conpty() {
        let (funcs, choice) = choose_conpty(None);
        assert!(matches!(choice, ConPtyChoice::System));
        assert!(!funcs.owned);
    }

    #[test]
    fn conpty_bundle_outside_the_executable_directory_is_not_considered() {
        let _lock = PROCESS_LOCK.lock().unwrap();
        let decoy = scratch_dir("decoy");
        stage_bundle(&decoy, b"not a dll");
        let original = std::env::current_dir().expect("current directory is known");
        std::env::set_current_dir(&decoy).expect("current directory is changed");
        let exe_dir = test_exe_dir();
        let candidate = conpty_dll_candidate(&exe_dir);
        // Resolved while the current directory still holds the decoy: a bare
        // name resolves into the decoy and exists, an absolute exe-side path
        // does not.
        let candidate_resolves_into_decoy = candidate.exists();
        let (funcs, choice) = choose_conpty(Some(&exe_dir));
        std::env::set_current_dir(original).expect("current directory is restored");
        assert!(
            !candidate_resolves_into_decoy,
            "the candidate must be the absolute path beside the executable, never \
             a name resolved against the current directory or PATH"
        );
        assert!(matches!(choice, ConPtyChoice::System));
        assert!(!funcs.owned);
        std::fs::remove_dir_all(&decoy).unwrap();
    }

    #[test]
    fn unloadable_dll_in_a_complete_bundle_downgrades_with_the_reason() {
        let dir = scratch_dir("invalid");
        stage_bundle(&dir, b"not a dll");
        let (funcs, choice) = choose_conpty(Some(&dir));
        let phrase = source_phrase(choice);
        assert!(phrase.starts_with(
            "the Windows inbox ConPTY (a staged ConPTY bundle was present but could not be loaded: "
        ));
        assert!(phrase.ends_with(')'));
        // No path may ride along with the error: neither separator can appear.
        assert!(!phrase.contains('\\'), "{phrase}");
        assert!(!phrase.contains('/'), "{phrase}");
        assert!(!funcs.owned);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn library_without_the_conpty_symbols_downgrades_with_the_reason() {
        let bytes = system_dll_bytes();
        if bytes.is_empty() {
            return;
        }
        let dir = scratch_dir("symbol-less");
        stage_bundle(&dir, &bytes);
        let (funcs, choice) = choose_conpty(Some(&dir));
        let phrase = source_phrase(choice);
        // Pins the full static phrase plus the error producer behind it.
        assert!(phrase.starts_with("the Windows inbox ConPTY (a staged ConPTY bundle was present but could not be loaded: missing symbol "));
        assert!(!phrase.contains('\\'), "{phrase}");
        assert!(!phrase.contains('/'), "{phrase}");
        assert!(!funcs.owned);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bundle_without_its_openconsole_host_is_not_chosen() {
        let bytes = system_dll_bytes();
        if bytes.is_empty() {
            return;
        }
        let dir = scratch_dir("no-host");
        // A loadable library as the DLL: the only reason to stay on the inbox
        // path is the missing host, not the DLL content.
        let dll = dir.join("conpty").join("conpty.dll");
        std::fs::create_dir_all(dll.parent().unwrap()).expect("bundle dir is created");
        std::fs::write(dll, &bytes).expect("dll is written");
        let (funcs, choice) = choose_conpty(Some(&dir));
        assert!(matches!(choice, ConPtyChoice::System));
        assert!(!funcs.owned);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_bundle_gate_requires_both_files() {
        let dir = scratch_dir("gate");
        assert!(!app_local_bundle_complete(&dir));
        let dll = dir.join("conpty").join("conpty.dll");
        std::fs::create_dir_all(dll.parent().unwrap()).expect("bundle dir is created");
        std::fs::write(dll, b"dll bytes").expect("dll is written");
        assert!(!app_local_bundle_complete(&dir));
        let host = dir.join("conpty").join("x64").join("OpenConsole.exe");
        std::fs::create_dir_all(host.parent().unwrap()).expect("host dir is created");
        std::fs::write(host, b"host bytes").expect("host is written");
        assert!(app_local_bundle_complete(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // The rule that needs no privilege: the reparse mask itself, so the
    // security predicate cannot lose its clause on a machine where symlinks
    // cannot be created.
    #[test]
    fn the_reparse_mask_rejects_reparse_attributes() {
        assert!(has_reparse_attribute(FILE_ATTRIBUTE_REPARSE_POINT));
        assert!(has_reparse_attribute(FILE_ATTRIBUTE_REPARSE_POINT | 0x20));
        assert!(!has_reparse_attribute(0x20));
        assert!(!has_reparse_attribute(0));
    }

    #[test]
    fn a_reparse_point_is_not_trusted_as_the_bundle() {
        let dir = scratch_dir("reparse");
        let target = scratch_dir("reparse-target");
        std::fs::write(target.join("elsewhere.dll"), b"dll bytes").expect("target is written");
        let host = dir.join("conpty").join("x64").join("OpenConsole.exe");
        std::fs::create_dir_all(host.parent().unwrap()).expect("host dir is created");
        std::fs::write(host, b"placeholder").expect("host is written");
        // The mask test above pins the rule itself; this end-to-end run needs
        // a privilege this machine may not grant.
        let linked = match std::os::windows::fs::symlink_file(
            target.join("elsewhere.dll"),
            dir.join("conpty").join("conpty.dll"),
        ) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("symlink end-to-end leg skipped, symbolic links unavailable: {error}");
                false
            }
        };
        if linked {
            assert!(!is_regular_file(&dir.join("conpty").join("conpty.dll")));
            assert!(!app_local_bundle_complete(&dir));
            let (funcs, choice) = choose_conpty(Some(&dir));
            assert!(matches!(choice, ConPtyChoice::System));
            assert!(!funcs.owned);
        }
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&target).unwrap();
    }
}
