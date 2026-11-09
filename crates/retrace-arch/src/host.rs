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

#[cfg(test)]
mod tests {
    use super::*;

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
