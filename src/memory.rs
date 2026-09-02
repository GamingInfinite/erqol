//! Shared memory access and patching helpers.

#![allow(dead_code)]

use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_NOACCESS,
    PAGE_PROTECTION_FLAGS, VirtualAlloc, VirtualProtect, VirtualQuery,
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
        VirtualProtect(
            addr as *const core::ffi::c_void,
            bytes.len(),
            old_prot,
            &mut old_prot,
        )
    };
    true
}

/// Like [`patch_bytes`], but writes the opcode byte LAST so no observing
/// thread ever decodes a partially-written branch instruction. The window a
/// concurrent fetch can tear shrinks to the non-opcode bytes only.
pub unsafe fn patch_bytes_ordered(addr: u64, bytes: &[u8], opcode_index: usize) -> bool {
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
    for (i, &b) in bytes.iter().enumerate() {
        if i == opcode_index {
            continue;
        }
        unsafe { std::ptr::write_volatile((addr as *mut u8).add(i), b) };
    }
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    unsafe { std::ptr::write_volatile((addr as *mut u8).add(opcode_index), bytes[opcode_index]) };
    let _ = unsafe {
        VirtualProtect(
            addr as *const core::ffi::c_void,
            bytes.len(),
            old_prot,
            &mut old_prot,
        )
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

/// Allocates a readable/writable/executable region of `size` bytes. Tries to
/// place it within ±2 GiB of `preferred_addr` so that a 32-bit relative jump
/// can reach it, then falls back to any available address.
pub unsafe fn alloc_executable(preferred_addr: u64, size: usize) -> Option<u64> {
    const GRANULARITY: u64 = 0x1_0000;
    const MAX_STEPS: usize = 0x8000; // 2 GiB / 64 KiB

    let aligned = preferred_addr & !(GRANULARITY - 1);

    // Search upward first: addresses >= the hook, up to +2 GiB.
    for i in 0..MAX_STEPS {
        let base = aligned.saturating_add(i as u64 * GRANULARITY);
        let mem = unsafe {
            VirtualAlloc(
                Some(base as *const core::ffi::c_void),
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        if !mem.is_null() {
            return Some(mem as u64);
        }
    }

    // Search downward: addresses < the hook, down to -2 GiB.
    for i in 1..MAX_STEPS {
        let base = aligned.saturating_sub(i as u64 * GRANULARITY);
        if base == 0 {
            break;
        }
        let mem = unsafe {
            VirtualAlloc(
                Some(base as *const core::ffi::c_void),
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        if !mem.is_null() {
            return Some(mem as u64);
        }
    }

    // Last resort: let the OS pick the address. This may be too far for a
    // 5-byte relative jmp, but callers can fall back to an absolute jump.
    let mem = unsafe { VirtualAlloc(None, size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) };
    if mem.is_null() {
        None
    } else {
        Some(mem as u64)
    }
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
