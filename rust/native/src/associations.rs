use crate::{Result, collection, error};

pub const PRODUCT: &str = "Clip Image Viewer";

pub fn portable() -> bool {
    std::env::var_os("PORTABLE_EXECUTABLE_FILE").is_some()
        || std::env::current_exe().ok().and_then(|exe| exe.parent().map(|p| p.join("portable.flag")))
            .is_some_and(|path| path.is_file())
}

pub fn supported() -> bool {
    cfg!(windows) && !cfg!(debug_assertions) && !portable() && !std::env::args_os().any(|arg| arg == "--smoke-test")
}

pub fn preference() -> Option<bool> {
    #[cfg(windows)]
    {
        use winreg::{RegKey, enums::HKEY_CURRENT_USER};
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\ClipImageViewer\Settings")
            .ok()?.get_value::<u32, _>("AssociationsEnabled").ok().map(|n| n != 0)
    }
    #[cfg(not(windows))]
    { None }
}

pub fn register(request: Vec<String>, remove_unselected: bool) -> Result<()> {
    if !supported() { return Err("이 실행 환경은 파일 연결을 지원하지 않습니다.".into()); }
    let mut allowed: Vec<_> = collection::extensions().into_iter().map(|ext| format!(".{ext}")).collect();
    allowed.sort();
    let mut selected: Vec<_> = request.iter().map(|ext| ext.to_ascii_lowercase())
        .filter(|ext| allowed.binary_search(ext).is_ok()).collect();
    selected.sort();
    selected.dedup();
    #[cfg(windows)] register_windows(&allowed, &selected, remove_unselected)?;
    Ok(())
}

#[cfg(windows)]
fn register_windows(allowed: &[String], selected: &[String], remove_unselected: bool) -> Result<()> {
    use std::io;
    use winreg::{RegKey, enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE}};

    const CLASSES: &str = r"Software\Classes";
    const CAP: &str = r"Software\ClipImageViewer\Capabilities";
    const PROG_ID_PREFIX: &str = "ClipImageViewer";
    fn missing_ok(result: io::Result<()>) -> Result<()> {
        match result {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(error(e)),
            _ => Ok(()),
        }
    }
    let user = RegKey::predef(HKEY_CURRENT_USER);
    let exe = std::env::current_exe().map_err(error)?;
    let icon = format!("{},0", exe.display());
    let command = format!(r#""{}" "%1""#, exe.display());
    let (registered, _) = user.create_subkey(r"Software\RegisteredApplications").map_err(error)?;

    if remove_unselected {
        for ext in allowed.iter().filter(|ext| selected.binary_search(ext).is_err()) {
            for prefix in [PROG_ID_PREFIX, "ClipView"] {
                let prog = format!("{prefix}{ext}");
                if let Ok(key) = user.open_subkey_with_flags(format!(r"{CLASSES}\{ext}\OpenWithProgids"), KEY_WRITE) {
                    missing_ok(key.delete_value(&prog))?;
                }
                if let Ok(key) = user.open_subkey_with_flags(format!(r"{CLASSES}\{ext}"), KEY_READ | KEY_WRITE)
                    && key.get_value::<String, _>("").is_ok_and(|v| v.eq_ignore_ascii_case(&prog))
                {
                    missing_ok(key.delete_value(""))?;
                }
                missing_ok(user.delete_subkey_all(format!(r"{CLASSES}\{prog}")))?;
            }
        }
    }

    if selected.is_empty() && remove_unselected {
        missing_ok(registered.delete_value(PRODUCT))?;
        missing_ok(user.delete_subkey_all(CAP))?;
    } else if !selected.is_empty() {
        let (cap, _) = user.create_subkey(CAP).map_err(error)?;
        cap.set_value("ApplicationName", &PRODUCT).map_err(error)?;
        cap.set_value("ApplicationDescription", &"다양한 이미지, 동영상, CLIP STUDIO PAINT 문서를 보는 미디어 뷰어").map_err(error)?;
        cap.set_value("ApplicationIcon", &icon).map_err(error)?;
        let (mapping, _) = cap.create_subkey("FileAssociations").map_err(error)?;
        if remove_unselected {
            for ext in allowed.iter().filter(|ext| selected.binary_search(ext).is_err()) {
                missing_ok(mapping.delete_value(ext))?;
            }
        }
        for ext in selected {
            let prog = format!("{PROG_ID_PREFIX}{ext}");
            let (key, _) = user.create_subkey(format!(r"{CLASSES}\{prog}")).map_err(error)?;
            key.set_value("", &"Clip Image Viewer 미디어").map_err(error)?;
            key.create_subkey("DefaultIcon").map_err(error)?.0.set_value("", &icon).map_err(error)?;
            key.create_subkey(r"shell\open\command").map_err(error)?.0.set_value("", &command).map_err(error)?;
            user.create_subkey(format!(r"{CLASSES}\{ext}\OpenWithProgids")).map_err(error)?.0
                .set_value(&prog, &"").map_err(error)?;
            mapping.set_value(ext, &prog).map_err(error)?;
        }
        registered.set_value(PRODUCT, &CAP).map_err(error)?;
    }
    if remove_unselected || !selected.is_empty() {
        user.create_subkey(r"Software\ClipImageViewer\Settings").map_err(error)?.0
            .set_value("AssociationsEnabled", &u32::from(!selected.is_empty())).map_err(error)?;
    }
    Ok(())
}
