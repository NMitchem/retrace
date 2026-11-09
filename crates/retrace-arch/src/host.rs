//! M57: the host's OS version, read only to name it in a refusal.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};

extern "C" {
    fn sysctlbyname(name: *const c_char, oldp: *mut c_void, oldlenp: *mut usize, newp: *mut c_void, newlen: usize) -> c_int;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostOs {
    pub product: String,
    pub build: String,
}

fn sysctl_string(name: &str) -> String {
    let cname = CString::new(name).unwrap();
    let mut len = 0usize;
    // SAFETY: a size query with a null buffer, then a read into a buffer of that size.
    unsafe {
        if sysctlbyname(cname.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 || len == 0 {
            return "?".into();
        }
        let mut buf = vec![0u8; len];
        if sysctlbyname(cname.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return "?".into();
        }
        buf.truncate(len);
        while buf.last() == Some(&0) {
            buf.pop();
        }
        String::from_utf8(buf).unwrap_or_else(|_| "?".into())
    }
}

/// `kern.osproductversion` and `kern.osversion`, `"?"` when unreadable.
pub fn host_os() -> HostOs {
    HostOs { product: sysctl_string("kern.osproductversion"), build: sysctl_string("kern.osversion") }
}

/// M57 §3e: the macOS majors retrace has been measured on.
pub const SUPPORTED_MAJORS: [u32; 2] = [26, 27];

/// `macOS 26 and 27`.
pub fn supported_text() -> String {
    let v: Vec<String> = SUPPORTED_MAJORS.iter().map(|m| m.to_string()).collect();
    match v.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("macOS {} and {last}", rest.join(", ")),
        _ => format!("macOS {}", v.join("")),
    }
}

/// The startup gate: `None` iff the product's major is one retrace has been measured on.
pub fn os_refusal(os: &HostOs) -> Option<crate::Unsupported> {
    let major = os.product.split('.').next().and_then(|m| m.parse::<u32>().ok());
    if major.is_some_and(|m| SUPPORTED_MAJORS.contains(&m)) { return None; }
    Some(crate::Unsupported::new(
        crate::WallCategory::MacosVersion,
        format!("macOS {} ({}) is not supported: retrace runs on {}", os.product, os.build, supported_text()),
    ))
}

/// `<product>,<build>`, both non-empty, as `RETRACE_DEBUG_MACOS` spells a stand-in host.
pub fn parse_os_override(v: &str) -> Option<HostOs> {
    let (product, build) = v.split_once(',')?;
    if product.is_empty() || build.is_empty() { return None; }
    Some(HostOs { product: product.into(), build: build.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_majors_pass_and_others_are_refused() {
        let os = |p: &str| HostOs { product: p.into(), build: "27A100".into() };
        for p in ["26.0", "26.5.2", "26.7.2", "27.2", "27.10"] {
            assert_eq!(os_refusal(&os(p)), None, "{p}");
        }
        for p in ["25.6", "28.0", "", "x"] {
            assert!(os_refusal(&os(p)).is_some(), "{p:?}");
        }
        let u = os_refusal(&os("28.0")).unwrap();
        assert_eq!(u.what, "macOS 28.0 (27A100) is not supported: retrace runs on macOS 26 and 27");
        assert_eq!(u.category, crate::WallCategory::MacosVersion);
    }

    #[test]
    fn the_override_parses_product_and_build() {
        assert_eq!(parse_os_override("28.0,27A100"), Some(HostOs { product: "28.0".into(), build: "27A100".into() }));
        for bad in ["", "28.0", ",x", "x,"] { assert_eq!(parse_os_override(bad), None, "{bad:?}"); }
    }

    fn sw_vers(flag: &str) -> String {
        let o = std::process::Command::new("sw_vers").arg(flag).output().unwrap();
        String::from_utf8(o.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn host_os_matches_sw_vers() {
        let h = host_os();
        assert_eq!(h.product, sw_vers("-productVersion"));
        assert_eq!(h.build, sw_vers("-buildVersion"));
    }
}
