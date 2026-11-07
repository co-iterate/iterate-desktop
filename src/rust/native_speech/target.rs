//! AppKit-backed target capture and activation for desktop speech writeback.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontmostApplication {
    pub bundle_id: String,
    pub pid: i32,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn speech_bridge_copy_current_bundle_identifier(
        bundle_id: *mut std::ffi::c_char,
        capacity: usize,
    ) -> bool;
    fn speech_bridge_copy_frontmost_application(
        bundle_id: *mut std::ffi::c_char,
        bundle_id_capacity: usize,
        pid: *mut i32,
    ) -> bool;
    fn speech_bridge_activate_application(bundle_id: *const std::ffi::c_char, pid: i32) -> bool;
    fn speech_bridge_application_matches_identity(
        bundle_id: *const std::ffi::c_char,
        pid: i32,
    ) -> bool;
}

/// Read the running bundle, including identities preserved by the installer.
#[cfg(target_os = "macos")]
pub fn current_bundle_identifier() -> Option<String> {
    let mut buffer = [0_i8; 1024];
    if !unsafe { speech_bridge_copy_current_bundle_identifier(buffer.as_mut_ptr(), buffer.len()) } {
        return None;
    }
    let identifier = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }.to_str().ok()?;
    (!identifier.is_empty()).then(|| identifier.to_owned())
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn matches_own_bundle(candidate: &str, current: Option<&str>) -> bool {
    current.is_some_and(|identifier| !identifier.is_empty() && candidate == identifier)
}

#[cfg(test)]
mod bundle_identity_tests {
    use super::matches_own_bundle;

    #[test]
    fn uses_runtime_identity_without_recognizing_other_installations() {
        assert!(matches_own_bundle("dev.example.custom", Some("dev.example.custom")));
        assert!(!matches_own_bundle("com.kexin94yyds.iterate", Some("dev.example.custom")));
        assert!(!matches_own_bundle("dev.example.custom.other", Some("dev.example.custom")));
        assert!(!matches_own_bundle("dev.example.custom", None));
        assert!(!matches_own_bundle("", Some("")));
    }
}

#[cfg(target_os = "macos")]
pub fn capture_frontmost_application() -> Result<FrontmostApplication, String> {
    let mut bundle_id = vec![0_i8; 1024];
    let mut pid = 0_i32;
    let copied = unsafe {
        speech_bridge_copy_frontmost_application(bundle_id.as_mut_ptr(), bundle_id.len(), &mut pid)
    };
    if !copied {
        return Err("NSWorkspace did not return a frontmost application".into());
    }
    let bundle_id = unsafe { std::ffi::CStr::from_ptr(bundle_id.as_ptr()) }
        .to_str()
        .map_err(|_| "frontmost bundle identifier is not valid UTF-8")?
        .trim()
        .to_owned();
    if bundle_id.is_empty() || pid <= 0 {
        return Err("frontmost application identity is incomplete".into());
    }
    Ok(FrontmostApplication { bundle_id, pid })
}

#[cfg(not(target_os = "macos"))]
pub fn capture_frontmost_application() -> Result<FrontmostApplication, String> {
    Err("frontmost application capture is only available on macOS".into())
}

#[cfg(target_os = "macos")]
pub fn activate_application(target: &FrontmostApplication) -> Result<(), String> {
    let bundle_id = std::ffi::CString::new(target.bundle_id.as_str())
        .map_err(|_| "bundle identifier contains an interior NUL")?;
    if unsafe { speech_bridge_activate_application(bundle_id.as_ptr(), target.pid) } {
        Ok(())
    } else {
        Err(format!(
            "NSWorkspace could not activate pid {} ({})",
            target.pid, target.bundle_id
        ))
    }
}

#[cfg(target_os = "macos")]
pub fn application_matches_identity(target: &FrontmostApplication) -> bool {
    let Ok(bundle_id) = std::ffi::CString::new(target.bundle_id.as_str()) else {
        return false;
    };
    unsafe { speech_bridge_application_matches_identity(bundle_id.as_ptr(), target.pid) }
}

#[cfg(not(target_os = "macos"))]
pub fn activate_application(_target: &FrontmostApplication) -> Result<(), String> {
    Err("application activation is only available on macOS".into())
}

#[cfg(not(target_os = "macos"))]
pub fn application_matches_identity(_target: &FrontmostApplication) -> bool {
    false
}
