//! Shared memory access and patching helpers.

#![allow(dead_code)]

use windows::Win32::System::Memory::{
    VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE,
    PAGE_NOACCESS, PAGE_PROTECTION_FLAGS,
};

// ---- Volatile reads ----

pub unsafe fn read_byte(addr: u64) -> u8 {
    unsafe { std::ptr::read_volatile(addr as *const u8) }
}

pub unsafe fn read_qword(addr: u64) -> u64 {
    unsafe { std::ptr::read_volatile(addr as *const u64) }
}

pub unsafe fn read_dword(addr: u64) -> u32 {
    unsafe { std::ptr::read_volatile(addr as *const u32) }
}

pub unsafe fn read_word(addr: u64) -> u16 {
    unsafe { std::ptr::read_volatile(addr as *const u16) }
}

pub unsafe fn read_f32(addr: u64) -> f32 {
    unsafe { std::ptr::read_volatile(addr as *const f32) }
}

// ---- Volatile writes ----

pub unsafe fn write_dword(addr: u64, value: u32) {
    unsafe { std::ptr::write_volatile(addr as *mut u32, value) }
}

// ---- Code-page patches ----

/// Makes the page containing `addr` writable, writes `bytes`, then restores the
/// original protection. Mirrors the code-page write pattern ilhook uses
/// internally.
pub unsafe fn patch_bytes(addr: u64, bytes: &[u8]) -> bool {
    let mut old_prot = PAGE_PROTECTION_FLAGS(0);
    if unsafe {
        VirtualProtect(
            addr as *const core::ffi::c_void,
            bytes.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old_prot,
        )
    }
    .is_err()
    {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
    }
    let _ = unsafe {
        VirtualProtect(addr as *const core::ffi::c_void, bytes.len(), old_prot, &mut old_prot)
    };
    true
}

// ---- Safe reads (VirtualQuery-guarded) ----

/// Reads a `u64` from `addr` after checking the backing page is committed and
/// readable via `VirtualQuery`. Used by crash handlers so they never fault while
/// trying to dump context around an access violation.
pub fn safe_read_u64(addr: usize) -> Option<u64> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let base = addr as *const core::ffi::c_void;
    let size = std::mem::size_of::<MEMORY_BASIC_INFORMATION>();
    if unsafe { VirtualQuery(Some(base), &mut info, size) } == 0 || info.State != MEM_COMMIT {
        return None;
    }
    const READ_PROT: u32 = 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80;
    if info.Protect.0 & READ_PROT == 0 {
        return None;
    }
    Some(unsafe { *(base.cast::<u64>()) })
}

// ---- Memory region enumeration ----

#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub base: usize,
    pub size: usize,
}

/// Enumerates all committed, readable memory regions in the current process.
pub fn enum_memory_regions() -> Vec<MemoryRegion> {
    let mut regions = Vec::new();
    let mut addr = 0usize;
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };

    loop {
        let bytes = unsafe {
            VirtualQuery(
                Some(addr as *const core::ffi::c_void),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if bytes == 0 {
            break;
        }

        if info.State == MEM_COMMIT && info.Protect != PAGE_NOACCESS {
            const READ_PROT: u32 = 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80;
            if info.Protect.0 & READ_PROT != 0 {
                regions.push(MemoryRegion {
                    base: info.BaseAddress as usize,
                    size: info.RegionSize,
                });
            }
        }

        let next = (info.BaseAddress as usize).saturating_add(info.RegionSize);
        if next <= addr {
            break;
        }
        addr = next;
    }

    regions
}
