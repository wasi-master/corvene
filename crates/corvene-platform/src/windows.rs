//! Win32 pieces shared by the modules' Windows arms.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SPI_GETFONTSMOOTHING, SPI_GETFONTSMOOTHINGTYPE,
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

/// Corvene is a GUI program: a console program it starts would open a
/// console window of its own without this creation flag.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// For the programs that are meant to get a console (the shells).
pub const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// The AppUserModelID the installer's Start menu shortcut carries; toasts
/// are shown under it.
pub const APP_USER_MODEL_ID: &str = "com.wasimaster.corvene";

/// Settings › Accessibility › Contrast themes (`SPI_GETHIGHCONTRAST`).
pub fn high_contrast() -> bool {
    use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    let mut info = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // SAFETY: the call fills `info`, whose size it is told
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            Some((&raw mut info).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && info.dwFlags.contains(HCF_HIGHCONTRASTON)
}

/// Settings › Accessibility › Visual effects › Animation effects
/// (`SPI_GETCLIENTAREAANIMATION`); on when it can't be read.
pub fn client_area_animation() -> bool {
    let mut on = 1i32;
    // SAFETY: the call writes one BOOL to the pointer it is given
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut on).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_err() || on != 0
}

/// Whether ClearType is on (Chromium draws subpixel text then, grayscale
/// with plain font smoothing).
pub fn cleartype() -> bool {
    const FE_FONTSMOOTHINGCLEARTYPE: u32 = 2;
    let mut smoothing = 0i32;
    let mut kind = 0u32;
    // SAFETY: each call writes one BOOL / UINT to the pointer it is given
    unsafe {
        let on = SystemParametersInfoW(
            SPI_GETFONTSMOOTHING,
            0,
            Some((&raw mut smoothing).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let which = SystemParametersInfoW(
            SPI_GETFONTSMOOTHINGTYPE,
            0,
            Some((&raw mut kind).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        on.is_ok() && which.is_ok() && smoothing != 0 && kind == FE_FONTSMOOTHINGCLEARTYPE
    }
}

/// Settings › Time & language › Country or region, as ISO 3166-1 alpha-2.
pub fn country_code() -> Option<String> {
    let mut name = [0u16; 16];
    // SAFETY: the call writes at most `name.len()` UTF-16 units
    let len = unsafe { windows::Win32::Globalization::GetUserDefaultGeoName(&mut name) };
    let len = usize::try_from(len).ok()?.checked_sub(1)?;
    let code = String::from_utf16(name.get(..len)?).ok()?;
    (code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| code.to_ascii_uppercase())
}

/// Electron's `shell.showItemInFolder`: an Explorer window on the item's
/// folder with the item selected.
pub fn show_item_in_folder(path: &Path) -> std::io::Result<()> {
    // Explorer parses its own command line: the path is quoted as a whole,
    // after the comma
    Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// `HKEY_CURRENT_USER\Software\Classes\<scheme>` pointing at `exe`, the way
/// Electron's `app.setAsDefaultProtocolClient` writes it.
pub fn register_url_scheme(scheme: &str, exe: &Path) -> windows_registry::Result<()> {
    let key = windows_registry::CURRENT_USER.create(format!("Software\\Classes\\{scheme}"))?;
    key.set_string("", format!("URL:{scheme}"))?;
    key.set_string("URL Protocol", "")?;
    key.create("shell\\open\\command")?
        .set_string("", format!("\"{}\" \"%1\"", exe.display()))
}

/// The command registered for `scheme`, if any.
pub fn url_scheme_command(scheme: &str) -> Option<String> {
    windows_registry::CURRENT_USER
        .open(format!("Software\\Classes\\{scheme}\\shell\\open\\command"))
        .and_then(|key| key.get_string(""))
        .ok()
}

/// The CLSID of the toast activator (`notifications::serve_activator`), as
/// the installer's Start menu shortcut carries it
/// (`AppUserModelToastActivatorCLSID`) and `HKCU\Software\Classes\CLSID`
/// names this program as its local server.
pub const TOAST_ACTIVATOR_CLSID: windows::core::GUID =
    windows::core::GUID::from_u128(0x3405A3D0_042A_4FAB_A267_FD8A6E9608CF);
/// [`TOAST_ACTIVATOR_CLSID`] as the registry spells it.
pub const TOAST_ACTIVATOR_CLSID_TEXT: &str = "{3405A3D0-042A-4FAB-A267-FD8A6E9608CF}";

/// The mutex a running Corvene holds (`hold_running_mutex`); the uninstaller
/// checks for it (`packaging/windows/corvene.iss`, `InitializeUninstall`).
const RUNNING_MUTEX: windows::core::PCWSTR = windows::core::w!("CorveneRunning");

/// Whether this program was put here by the installer: its uninstaller sits
/// next to it (`packaging/windows/corvene.iss`).
pub fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("unins000.exe").is_file()))
        .unwrap_or(false)
}

/// `HKCU\Software\Classes\CLSID\<activator>\LocalServer32` pointing at
/// `exe`, as the installer writes it: a click on a toast once Corvene has
/// exited starts this program (COM, with `-Embedding`), which answers it
/// (`notifications::serve_activator`).
pub fn register_toast_activator(exe: &Path) -> windows_registry::Result<()> {
    let key = windows_registry::CURRENT_USER.create(format!(
        "Software\\Classes\\CLSID\\{TOAST_ACTIVATOR_CLSID_TEXT}"
    ))?;
    key.set_string("", "Corvene notification activator")?;
    key.create("LocalServer32")?
        .set_string("", format!("\"{}\"", exe.display()))
}

/// The program registered as the toast activator's server, if any.
pub fn toast_activator_server() -> Option<String> {
    windows_registry::CURRENT_USER
        .open(format!(
            "Software\\Classes\\CLSID\\{TOAST_ACTIVATOR_CLSID_TEXT}\\LocalServer32"
        ))
        .and_then(|key| key.get_string(""))
        .ok()
}

/// Restart Manager (`RegisterApplicationRestart`): when the installer
/// closes a running Corvene to replace its files (`CloseApplications`), it
/// starts it again afterwards (`RestartApplications`). Only for that: not
/// after a crash, a hang or a reboot.
pub fn register_application_restart() {
    use windows::Win32::System::Recovery::{
        RESTART_NO_CRASH, RESTART_NO_HANG, RESTART_NO_REBOOT, RegisterApplicationRestart,
    };
    // SAFETY: a null command line asks for a restart with no arguments
    let registered = unsafe {
        RegisterApplicationRestart(
            windows::core::PCWSTR::null(),
            RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT,
        )
    };
    if let Err(err) = registered {
        tracing::warn!(%err, "could not register for a restart by the installer");
    }
}

/// Hold the `CorveneRunning` mutex for the life of this process: the
/// uninstaller refuses to run while it exists (its files would be in use).
pub fn hold_running_mutex() {
    // SAFETY: creating (or opening) a named mutex; the handle is never
    // closed on purpose, so the mutex lives as long as the process
    match unsafe { windows::Win32::System::Threading::CreateMutexW(None, false, RUNNING_MUTEX) } {
        Ok(_handle) => {}
        Err(err) => tracing::warn!(%err, "could not create the running mutex"),
    }
}

/// Corvene (`1122-visual-studio-clone-folder`): whether the Visual Studio
/// IDE is installed. `vswhere` comes with the Visual Studio Installer and,
/// without `-products`, lists Community, Professional and Enterprise only
/// (Build Tools alone do not count). Asked once per run.
pub fn visual_studio_installed() -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        let Some(programs) = std::env::var_os("ProgramFiles(x86)") else {
            return false;
        };
        let vswhere = Path::new(&programs).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
        if !vswhere.is_file() {
            return false;
        }
        Command::new(vswhere)
            .args(["-nologo", "-property", "instanceId"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|out| out.status.success() && !out.stdout.trim_ascii().is_empty())
    })
}

/// Whether `ReadDirectoryChangesW` takes `dir`, asked the way the file
/// watcher (`notify`) asks: recursively, overlapped, with a 16 KiB buffer.
/// Some redirectors refuse it (a VDI user profile on an SMB share,
/// desktop/desktop#22972), and `notify` then reports no change at all
/// instead of an error. The request is cancelled at once.
pub fn directory_changes_supported(dir: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SECURITY,
        FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING, ReadDirectoryChangesW,
    };
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::CreateEventW;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated path that outlives the call
    let handle = match unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            None,
        )
    } {
        Ok(handle) => handle,
        Err(err) => {
            tracing::debug!(%err, dir = %dir.display(), "could not open the folder to watch");
            return false;
        }
    };
    // SAFETY: an unnamed manual-reset event, closed below
    let event = match unsafe { CreateEventW(None, true, false, PCWSTR::null()) } {
        Ok(event) => event,
        Err(_) => {
            // SAFETY: the handle opened above
            let _ = unsafe { CloseHandle(handle) };
            return true;
        }
    };
    let mut buffer = vec![0u32; 4096];
    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_CREATION
        | FILE_NOTIFY_CHANGE_SECURITY;
    // SAFETY: `buffer` and `overlapped` stay alive until the request has
    // completed or been cancelled (`GetOverlappedResult` waits for it)
    let started = unsafe {
        ReadDirectoryChangesW(
            handle,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 4) as u32,
            true,
            filter,
            None,
            Some(&raw mut overlapped),
            None,
        )
    };
    let supported = match started {
        Ok(()) => {
            let mut transferred = 0u32;
            // SAFETY: the pending request on `handle` uses `overlapped`
            unsafe {
                let _ = CancelIoEx(handle, Some(&raw const overlapped));
                let _ = GetOverlappedResult(handle, &raw const overlapped, &mut transferred, true);
            }
            true
        }
        Err(err) => {
            tracing::info!(%err, dir = %dir.display(), "the folder cannot be watched for changes");
            false
        }
    };
    // SAFETY: both handles were opened above and are not used after this
    unsafe {
        let _ = CloseHandle(event);
        let _ = CloseHandle(handle);
    }
    supported
}
