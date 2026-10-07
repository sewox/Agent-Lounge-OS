//! Windows DACL lockdown for secret files/dirs (PR-S).
//!
//! Applies a protected DACL with Full Control for the current user and SYSTEM only
//! (no inheritance). Fail-closed callers must propagate errors.
//!
//! Symbol paths target **windows-sys 0.59** (`GENERIC_ALL` in Foundation,
//! `SECURITY_NT_AUTHORITY` in Security, SystemServices RIDs are `i32`).

#![cfg(windows)]

use std::path::Path;
use std::ptr;

use anyhow::{bail, Context, Result};
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_SUCCESS, FALSE, GENERIC_ALL, HANDLE, INVALID_HANDLE_VALUE,
};
#[cfg(test)]
use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;
use windows_sys::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, SET_ACCESS, SE_FILE_OBJECT,
    TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    AllocateAndInitializeSid, CopySid, FreeSid, GetLengthSid, GetTokenInformation, IsValidSid,
    TokenUser, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION,
    PSID, SECURITY_NT_AUTHORITY, TOKEN_QUERY, TOKEN_USER,
};
#[cfg(test)]
use windows_sys::Win32::Security::{
    EqualSid, GetAce, ACCESS_ALLOWED_ACE, ACE_HEADER, PSECURITY_DESCRIPTOR,
    SECURITY_WORLD_SID_AUTHORITY,
};
use windows_sys::Win32::System::SystemServices::SECURITY_LOCAL_SYSTEM_RID;
#[cfg(test)]
use windows_sys::Win32::System::SystemServices::{
    DOMAIN_ALIAS_RID_USERS, SECURITY_AUTHENTICATED_USER_RID, SECURITY_BUILTIN_DOMAIN_RID,
    SECURITY_WORLD_RID,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(crate) fn apply_current_user_and_system_only(path: &Path) -> Result<()> {
    let wide = path_to_wide(path)?;
    unsafe {
        let user_sid = current_user_sid().context("resolve current user SID")?;
        let system_sid =
            well_known_sid_nt(SECURITY_LOCAL_SYSTEM_RID as u32).context("allocate SYSTEM SID")?;

        let mut entries = [
            explicit_access(user_sid.as_ptr()),
            explicit_access(system_sid.as_ptr()),
        ];

        let mut new_acl: *mut ACL = ptr::null_mut();
        let status = SetEntriesInAclW(2, entries.as_mut_ptr(), ptr::null_mut(), &mut new_acl);
        if status != ERROR_SUCCESS || new_acl.is_null() {
            bail!("SetEntriesInAclW failed: {status}");
        }

        let status = SetNamedSecurityInfoW(
            wide.as_ptr() as *mut u16,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            new_acl,
            ptr::null_mut(),
        );
        LocalFree(new_acl as *mut _);
        if status != ERROR_SUCCESS {
            bail!("SetNamedSecurityInfoW failed: {status}");
        }
    }
    Ok(())
}

/// True when the DACL contains an ACE for Everyone, Users, or Authenticated Users.
#[cfg(test)]
pub(crate) fn dacl_has_broad_aces(path: &Path) -> Result<bool> {
    let wide = path_to_wide(path)?;
    unsafe {
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let mut dacl: *mut ACL = ptr::null_mut();
        let status = GetNamedSecurityInfoW(
            wide.as_ptr() as *mut u16,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut sd,
        );
        if status != ERROR_SUCCESS {
            bail!("GetNamedSecurityInfoW failed: {status}");
        }

        let everyone = well_known_world_sid()?;
        let users = well_known_builtin_alias(DOMAIN_ALIAS_RID_USERS as u32)?;
        let auth_users = well_known_auth_users()?;
        let mut found = false;

        if !dacl.is_null() {
            let ace_count = (*dacl).AceCount;
            for i in 0..ace_count {
                let mut ace: *mut core::ffi::c_void = ptr::null_mut();
                if GetAce(dacl, i as u32, &mut ace) == FALSE || ace.is_null() {
                    continue;
                }
                let header = &*(ace as *const ACE_HEADER);
                // ACCESS_ALLOWED_ACE_TYPE = 0x00
                if header.AceType != 0 {
                    continue;
                }
                let allowed = &*(ace as *const ACCESS_ALLOWED_ACE);
                let sid = std::ptr::addr_of!(allowed.SidStart) as PSID;
                if sids_equal(sid, everyone.as_ptr())
                    || sids_equal(sid, users.as_ptr())
                    || sids_equal(sid, auth_users.as_ptr())
                {
                    found = true;
                    break;
                }
            }
        }

        if !sd.is_null() {
            LocalFree(sd as *mut _);
        }
        Ok(found)
    }
}

unsafe fn explicit_access(sid: PSID) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: SET_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: sid as *mut u16,
        },
    }
}

struct OwnedSid {
    ptr: PSID,
    _buf: Vec<u8>,
}

impl OwnedSid {
    fn as_ptr(&self) -> PSID {
        self.ptr
    }
}

unsafe fn current_user_sid() -> Result<OwnedSid> {
    let mut token: HANDLE = INVALID_HANDLE_VALUE;
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == FALSE {
        bail!("OpenProcessToken failed");
    }
    let mut needed = 0u32;
    GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
    let mut buf = vec![0u8; needed as usize];
    if GetTokenInformation(
        token,
        TokenUser,
        buf.as_mut_ptr() as *mut _,
        needed,
        &mut needed,
    ) == FALSE
    {
        CloseHandle(token);
        bail!("GetTokenInformation(TokenUser) failed");
    }
    CloseHandle(token);
    let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
    let src = token_user.User.Sid;
    if IsValidSid(src) == FALSE {
        bail!("invalid user SID");
    }
    copy_sid(src)
}

unsafe fn well_known_sid_nt(rid: u32) -> Result<OwnedSid> {
    let mut authority = SECURITY_NT_AUTHORITY;
    let mut sid: PSID = ptr::null_mut();
    if AllocateAndInitializeSid(&mut authority, 1, rid, 0, 0, 0, 0, 0, 0, 0, &mut sid) == FALSE
        || sid.is_null()
    {
        bail!("AllocateAndInitializeSid failed");
    }
    let owned = copy_sid(sid);
    FreeSid(sid);
    owned
}

#[cfg(test)]
unsafe fn well_known_world_sid() -> Result<OwnedSid> {
    let mut authority = SECURITY_WORLD_SID_AUTHORITY;
    let mut sid: PSID = ptr::null_mut();
    if AllocateAndInitializeSid(
        &mut authority,
        1,
        SECURITY_WORLD_RID as u32,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        &mut sid,
    ) == FALSE
        || sid.is_null()
    {
        bail!("AllocateAndInitializeSid(Everyone) failed");
    }
    let owned = copy_sid(sid);
    FreeSid(sid);
    owned
}

#[cfg(test)]
unsafe fn well_known_auth_users() -> Result<OwnedSid> {
    let mut authority = SECURITY_NT_AUTHORITY;
    let mut sid: PSID = ptr::null_mut();
    if AllocateAndInitializeSid(
        &mut authority,
        1,
        SECURITY_AUTHENTICATED_USER_RID as u32,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        &mut sid,
    ) == FALSE
        || sid.is_null()
    {
        bail!("AllocateAndInitializeSid(Authenticated Users) failed");
    }
    let owned = copy_sid(sid);
    FreeSid(sid);
    owned
}

#[cfg(test)]
unsafe fn well_known_builtin_alias(rid: u32) -> Result<OwnedSid> {
    let mut authority = SECURITY_NT_AUTHORITY;
    let mut sid: PSID = ptr::null_mut();
    if AllocateAndInitializeSid(
        &mut authority,
        2,
        SECURITY_BUILTIN_DOMAIN_RID as u32,
        rid,
        0,
        0,
        0,
        0,
        0,
        0,
        &mut sid,
    ) == FALSE
        || sid.is_null()
    {
        bail!("AllocateAndInitializeSid(builtin) failed");
    }
    let owned = copy_sid(sid);
    FreeSid(sid);
    owned
}

unsafe fn copy_sid(src: PSID) -> Result<OwnedSid> {
    let len = GetLengthSid(src) as usize;
    let mut sid_buf = vec![0u8; len];
    if CopySid(len as u32, sid_buf.as_mut_ptr() as PSID, src) == FALSE {
        bail!("CopySid failed");
    }
    let ptr = sid_buf.as_mut_ptr() as PSID;
    Ok(OwnedSid { ptr, _buf: sid_buf })
}

#[cfg(test)]
unsafe fn sids_equal(a: PSID, b: PSID) -> bool {
    EqualSid(a, b) != FALSE
}

fn path_to_wide(path: &Path) -> Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        bail!("path contains NUL");
    }
    wide.push(0);
    Ok(wide)
}
