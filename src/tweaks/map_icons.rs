//! Custom map icons added at runtime without a modded `regulation.bin`.
//!
//! The WorldMapPoint param is served to the map by a group *filler* (in the
//! 1.17 build the function at `0x140d5a4d0`, reached indirectly through a
//! wrapper). It resolves the WORLD_MAP_POINT param container
//! (`GetParamResCap(0x57)`), binary-searches the param's id table for the
//! requested base id, and writes a group struct
//! `{ vftable@0; base_id@+8; row_buffer@+0x10; count@+0x18 }`. The map walks
//! `count` consecutive rows from `row_buffer` at a `0x100` stride.
//!
//! The param block that `row_buffer` points at is a self-contained structure:
//! a small header at `[-0x10..0)` (`[-0x10]` = aligned data size, `[-0xc]` =
//! row count, `[+0xa]` = row-count word, `[+0x2d]/[+0x2e]` = stride flags), the
//! 0x100 rows from `[+0x0)`, then an id table of `{u32 id; u32 index}` entries
//! at `row_buffer + align16([row_buffer-0x10])`. The per-id lookup
//! `GetWorldMapPointParam` (`0x140d59e11`) resolves ids entirely relative to
//! `row_buffer` (header + id table), so a replacement `row_buffer` MUST carry
//! that surrounding structure or the lookup reads garbage (DL_PANIC).
//!
//! We resolve the WorldMapPoint param container the same way the game's
//! per-id getter does (`GetParamResCap(registry, 0x57, 0)` → `[+0x80]` →
//! `[+0x80]` = the rows base), then redirect that rows pointer at a mod-owned
//! contiguous block that faithfully reproduces the vanilla structure (header +
//! vanilla rows + our custom rows at the end + a consistent id table that also
//! resolves our new ids) and bump the row counts. The consumer sees a valid
//! extended param file, so both the map enumeration and the per-id getter keep
//! working. `base_id` is left as the real value, so id-based resolution works
//! both through the id table and through the per-id getter hook below.

use std::sync::{Once, OnceLock};

use serde::Deserialize;

use crate::hooks;
use crate::log::log;
use crate::memory;
use crate::scan;

/// Raw WORLD_MAP_POINT_PARAM_ST row size in the 1.16 regulation.
const ROW_SIZE: u64 = 0x100;

/// Group wrapper `FUN_140d58750`: `PUSH RBX; SUB RSP,0x20; LEA RAX,[vft];
/// MOV [RCX+8],-1; MOV [RCX],RAX; MOV RBX,RCX; XOR EAX,EAX; MOV [RCX+10],RAX;
/// MOV [RCX+18],EAX; CALL filler; ...; RET`. The two relative lea/mov immediates
/// (`??`) and the call target are the only varying bytes; the window of fixed
/// mnemonics is long enough to be unique.
const GROUP_BUILDER_PATTERN: &str = "53 48 83 EC 20 48 8D 05 ? ? ? ? C7 41 08 FF FF FF FF 48 89 01 48 8B D9 33 C0 \
     48 89 41 10 89 41 18 E8 ? ? ? ? 48 8B C3 48 83 C4 20 5B C3";

/// The pristine entry prologue of the getter, compared against live bytes to
/// tell "untouched" from "another hook owns this" (same approach as the
/// speffect getter). `PUSH RDI; SUB RSP,0x40; MOV [RSP+0x20],-2; ...`.
///
/// The bare prologue is shared by every param-type getter (they are all the
/// same compiled `GetParamResCap<T>` template, so the prologue alone matches
/// dozens of unrelated getters). The pattern is therefore extended through the
/// FD4Singleton-ctor/assert block that leads into the binary search, and we
/// anchor on `lea edx,[r8+0x57]` (the WORLD_MAP_POINT param id) as the
/// disambiguating tail. Wildcards cover the RIP-relative displacements.
const GETTER_PATTERN: &str = "57 48 83 EC 40 48 C7 44 24 20 FE FF FF FF 48 89 5C 24 50 48 89 6C 24 58 \
     48 89 74 24 60 8B FA 48 8B F1 33 DB 85 D2 0F 88 ? ? ? ? 48 8B 0D \
     ? ? ? ? 48 85 C9 75 2E 48 8D 0D ? ? ? ? E8 ? ? ? ? 4C 8B C8 \
     4C 8D 05 ? ? ? ? BA B4 00 00 00 48 8D 0D ? ? ? ? E8 ? ? ? ? \
     48 8B 0D ? ? ? ? 45 33 C0 41 8D 50 57";

/// Layout of the WorldMapPoint param block that `group.row_buffer` points at
/// (all offsets relative to the block base, i.e. to `row_buffer`):
///   [-0x10] u32 aligned data size  -> id table lives at base + align16(this)
///   [-0x0c] u32 row count
///   [+0x0a] u16 row count (word copy, read by the filler)
///   [+0x2d]/[+0x2e] stride/format flags (decide the row-offset indexing)
///   base+0x0 .. base+align16(data_size): the 0x100 rows
///   base+align16(data_size) ..: id table, entries {u32 id; u32 index}
const HDR_OFF_DATA_SIZE: isize = -0x10; // u32
const HDR_OFF_COUNT: isize = -0x0c; // u32
const HDR_OFF_COUNT_WORD: isize = 0x0a; // u16

/// A custom map icon to append. The row is cloned from the Church of Elleh
/// template (below) so it carries a real grace's gating fields, then its
/// location/icon fields are overridden by [`apply_point`].
#[derive(Clone)]
struct CustomPoint {
    /// World cell the icon lives in; must map to a known area (the visibility
    /// gate converts `area_no/grid_x/grid_z` + position to a world area).
    area_no: u8,
    grid_x_no: u8,
    grid_z_no: u8,
    /// World position `[x, y, z]` used to place the marker in that cell.
    pos: [f32; 3],
    /// Mod-allocated PlaceName message id backing the label lookup.
    msg_id: i32,
}

/// Byte offsets (WORLD_MAP_POINT_PARAM_ST, DataVersion 6) of the fields we set
/// for a custom icon.
const OFF_AREA_NO: usize = 0x20;
const OFF_GRID_X: usize = 0x21;
const OFF_GRID_Z: usize = 0x22;
const OFF_POS_X: usize = 0x24;
const OFF_POS_Y: usize = 0x28;
const OFF_POS_Z: usize = 0x2c;
const OFF_ICON_ID: usize = 0x0c; // u16
const OFF_FLAGS: usize = 0x10; // isEnableNoText = bit 2
const OFF_TEXT_ID1: usize = 0x30; // s32, -1 = render icon without a label
/// `text_type1` (u8). 0 routes the label through the PlaceName message path.
const OFF_TEXT_TYPE1: usize = 0x90;
/// PlaceName message bnd id. The map marker's label (text_type 0, text_id1)
/// resolves via `MsgRepositoryImp::LookupEntry(repo, lang, 0x13 PlaceName, id)`.
const MSGBND_PLACE_NAME: u32 = 0x13;
/// Icon id used for every custom point. Turns out 3 is a church marker (not the
/// Site of Grace icon the original name assumed); kept deliberately because it
/// stands out nicely next to real grace markers.
const CUSTOM_ICON_ID: u16 = 3;
/// Vanilla id of the real Church of Elleh grace, used as the custom-row
/// template. Cloning it carries valid `eventFlagId`/`clearedEventFlagId`/icon
/// fields, including the `eventFlagId` that gates whether the icon shows at all
/// (set true in nearly all saves), so custom icons render regardless.
const TEMPLATE_CHURCH_OF_ELLEH: u32 = 61_423_600;

/// On-disk / embedded JSON representation of a custom point. Kept human
/// readable and matching the field names the debug recorder emits so a recorded
/// point can be pasted straight into `map_icons_points.json`.
#[derive(Deserialize)]
struct JsonPoint {
    #[serde(default)]
    note: String,
    area_no: u8,
    grid_x_no: u8,
    grid_z_no: u8,
    pos: [f32; 3],
}

/// The base set of custom map icons, baked into the DLL from
/// `map_icons_points.json` (next to this file). This is the source of truth for
/// the icons shipped with the mod — it is embedded at compile time, not read
/// from disk at runtime. The debug `erqol_points.json` recorder is separate and
/// clears itself each launch.
const EMBEDDED_POINTS_JSON: &str = include_str!("map_icons_points.json");

/// Runtime-resolved custom point list. Parsed once from the embedded JSON on
/// the first `build_block` call, then cached. Empty if the embedded JSON fails
/// to parse (a build-time bug — should always have at least the baked set).
static CUSTOM_POINTS: OnceLock<Vec<CustomPoint>> = OnceLock::new();

/// Number of custom rows = number of custom points. Computed from the
/// runtime-resolved list so the row/offset/id-table sizes always match.
fn custom_row_count() -> usize {
    custom_points().len()
}

/// Returns the runtime-resolved custom point list, parsing the embedded JSON
/// on first use.
fn custom_points() -> &'static [CustomPoint] {
    CUSTOM_POINTS.get_or_init(|| {
        let points: Vec<JsonPoint> = serde_json::from_str(EMBEDDED_POINTS_JSON)
            .unwrap_or_else(|e| {
                log(&format!(
                    "map_icons: ERROR parsing embedded map_icons_points.json: {e}"
                ));
                Vec::new()
            });
        let custom: Vec<CustomPoint> = points
            .into_iter()
            .map(|p| {
                let msg_id = crate::ezstate_menu::alloc_message_id();
                crate::ezstate_menu::register_message_in_bnd(MSGBND_PLACE_NAME, msg_id, &p.note);
                CustomPoint {
                    area_no: p.area_no,
                    grid_x_no: p.grid_x_no,
                    grid_z_no: p.grid_z_no,
                    pos: p.pos,
                    msg_id,
                }
            })
            .collect();
        log(&format!(
            "map_icons: loaded {} embedded custom point(s)",
            custom.len()
        ));
        custom
    })
}

/// The built combination of the vanilla structured block plus our appended
/// custom rows. `base` is the base of a mod-owned, contiguously-allocated
/// block that faithfully reproduces the vanilla WorldMapPoint structure
/// (header + vanilla rows + custom rows + id table), so the group filler can
/// point `row_buffer` at it and every consumer that reads header/id-table
/// fields relative to `row_buffer` keeps working.
#[derive(Clone, Copy)]
struct BuiltBlock {
    /// Base of the structured block (== what the group's `row_buffer` becomes).
    base: usize,
    /// Combined row count (vanilla + custom), mirroring `[base-0xc]`.
    count: u32,
}

/// `OnceLock` because the first invocation of the builder happens well after we
/// install the hook.
static ROWS: OnceLock<BuiltBlock> = OnceLock::new();

/// Runtime address of the per-id getter entry, used to decode the container
/// rows-base resolution (singleton RIP-relative load + `GetParamResCap` call).
static GETTER_ENTRY: OnceLock<u64> = OnceLock::new();

pub(crate) static MAP_ICONS_INSTALLER: Once = Once::new();

/// Forwarding target for the group builder used when another hook owns the
/// entry. Set before the entry patch goes live.
static GROUP_TRAMPOLINE: OnceLock<usize> = OnceLock::new();
/// Forwarding target for the per-id getter.
static GETTER_TRAMPOLINE: OnceLock<usize> = OnceLock::new();

/// Round `v` up to a multiple of 16 (matches the `add eax,0xf; and ~0xf` the
/// game uses to compute the id-table position from the data size).
fn align16(v: usize) -> usize {
    (v + 15) & !15
}

/// Reads a little-endian primitive at a pointer offset (helper to avoid pointer
/// arithmetic noise).
unsafe fn rd_u32(p: *const u8, off: isize) -> u32 {
    u32::from_le_bytes(unsafe { *((p.offset(off)) as *const [u8; 4]).cast() })
}
unsafe fn rd_u8(p: *const u8, off: isize) -> u8 {
    unsafe { *p.offset(off) }
}
unsafe fn rd_u64(p: *const u8, off: isize) -> u64 {
    u64::from_le_bytes(unsafe { *((p.offset(off)) as *const [u8; 8]).cast() })
}
unsafe fn wr_u32(p: *const u8, off: isize, v: u32) {
    unsafe { *((p.offset(off)) as *mut [u8; 4]).cast() = v.to_le_bytes() };
}
unsafe fn wr_u16(p: *const u8, off: isize, v: u16) {
    unsafe { *((p.offset(off)) as *mut [u8; 2]).cast() = v.to_le_bytes() };
}
unsafe fn wr_u64(p: *const u8, off: isize, v: u64) {
    unsafe { *((p.offset(off)) as *mut [u8; 8]).cast() = v.to_le_bytes() };
}

/// The offset-table row-addressing mode, decoded from the live stride flags
/// `[base+0x2d]` / `[base+0x2e]` exactly as the 1.17 getter `0x140d59e11` does.
/// The getter resolves row index `i` through a per-index entry located at
/// `base + table_off + i*stride` (`size` bytes), then `row = base + entry`.
struct StrideMode {
    table_off: usize, // byte offset from base where the entry for index 0 sits
    stride: usize,    // bytes between successive entries
    size: usize,      // 4 (relative u32) or 8 (relative u64)
}

/// Decode the stride mode for `base` from its flag bytes, mirroring the 1.17
/// getter's dispatch exactly:
///   c==2 -> 4-byte offsets at `base + i*12 + 0x34`
///   c==3 -> 4-byte offsets at `base + i*12 + 0x44`
///   c==4/c==5 with flag>=0x80 and `[+0x2e]&2` -> 8-byte offsets at `base + (i+3)*24`
///   c==4/c==5 otherwise -> 4-byte offsets at `base + i*12 + 0x44`
fn stride_mode(base: *const u8) -> StrideMode {
    let flag = unsafe { rd_u8(base, 0x2d) };
    let f2 = (unsafe { rd_u8(base, 0x2e) }) & 2;
    let c = flag & 0x7f;
    match c {
        2 => StrideMode {
            table_off: 0x34,
            stride: 12,
            size: 4,
        },
        3 => StrideMode {
            table_off: 0x44,
            stride: 12,
            size: 4,
        },
        _ => {
            if flag >= 0x80 && f2 != 0 {
                StrideMode {
                    table_off: 0x48,
                    stride: 24,
                    size: 8,
                }
            } else {
                StrideMode {
                    table_off: 0x44,
                    stride: 12,
                    size: 4,
                }
            }
        }
    }
}

fn read_off(base: *const u8, mode: &StrideMode, idx: usize) -> usize {
    unsafe {
        let p = base.add(mode.table_off + idx * mode.stride);
        if mode.size == 4 {
            rd_u32(p, 0) as usize
        } else {
            rd_u64(p, 0) as usize
        }
    }
}

/// Reconstruct a mod-owned structured block from the live WorldMapPoint
/// container rows base `base` (the getter's `rdx`, i.e. `[container+0x80][0x80]`),
/// where the header lives at `[-0x10..0)` and rows are addressed through the
/// offset table. Returns the new block base and combined count, or `None` if
/// the live structure is unusable (e.g. empty).
///
/// The new block reproduces the vanilla layout exactly: header verbatim, the
/// control zone `[0..c)`, a rebuilt offset table, every vanilla row relocated
/// verbatim, our custom rows appended, then the id table — with `data_size`
/// and `count` patched so the getter's binsearch + offset resolution work.
fn build_block(base: *const u8) -> Option<BuiltBlock> {
    if let Some(b) = ROWS.get() {
        return Some(*b);
    }
    if base.is_null() {
        log("map_icons: build_block: null base");
        return None;
    }
    let mode = stride_mode(base);
    let vanilla_data_size = (unsafe { rd_u32(base, HDR_OFF_DATA_SIZE) }) as usize;
    let vanilla_count = (unsafe { rd_u32(base, HDR_OFF_COUNT) }) as usize;
    let vanilla_data_size_al = align16(vanilla_data_size);
    log(&format!(
        "map_icons: build_block base={base:p} data_size={vanilla_data_size:#x} \
         aligned={vanilla_data_size_al:#x} count={vanilla_count} \
         mode=off:{:#x},stride:{},size:{} flags=[{:#x},{:#x}]",
        mode.table_off,
        mode.stride,
        mode.size,
        unsafe { rd_u8(base, 0x2d) },
        unsafe { rd_u8(base, 0x2e) }
    ));
    if vanilla_count == 0 || vanilla_data_size_al == 0 {
        log("map_icons: build_block: empty/unreadable rows");
        return None;
    }

    let new_count = vanilla_count + custom_row_count();
    let table_bytes = new_count * mode.stride;
    let rows_bytes = new_count * (ROW_SIZE as usize);
    let control = mode.table_off;
    let new_data_size = control + table_bytes + rows_bytes;
    let new_data_size_al = align16(new_data_size);
    let id_bytes = new_count * 8;

    let mut buf: Vec<u8> = Vec::with_capacity(0x10 + new_data_size_al + id_bytes);
    // 1. Header (16 bytes before base), verbatim.
    unsafe {
        buf.extend_from_slice(std::slice::from_raw_parts(base.sub(0x10), 0x10));
    }
    // 2. Control zone `[base, base+control)` verbatim (flags/header fields).
    unsafe {
        buf.extend_from_slice(std::slice::from_raw_parts(base, control));
    }
    let nb = unsafe { buf.as_mut_ptr().add(0x10) }; // new block base

    // 2b. Find the vanilla row index for the custom-row template (the real
    //     Church of Elleh grace) via the id table. Fall back to vanilla row 0
    //     if the id is absent.
    let template_idx = find_row_index(
        base,
        vanilla_data_size_al,
        vanilla_count,
        TEMPLATE_CHURCH_OF_ELLEH,
    )
    .unwrap_or(0);

    // 2c. Grow the Vec to its final data size now, BEFORE the raw writes below.
    //     The rows/offset-table writes use raw pointers (they don't advance
    //     `len`), so a later `resize` would zero-fill the region they occupy.
    //     Resizing here zero-fills the pad, then every raw write lands inside.
    buf.resize(0x10 + new_data_size_al, 0);

    // 3. Rebuilt offset table + relocated rows. Vanilla rows are copied from
    //    their live locations (via the vanilla offset table); custom rows are
    //    appended after them.
    let rows_start = control + table_bytes;
    for i in 0..vanilla_count {
        let row_dst = unsafe { nb.add(rows_start + i * (ROW_SIZE as usize)) };
        let row_src = unsafe { base.add(read_off(base, &mode, i)) };
        unsafe {
            std::ptr::copy_nonoverlapping(row_src, row_dst, ROW_SIZE as usize);
        }
    }
    for i in 0..custom_row_count() {
        let row_dst = unsafe { nb.add(rows_start + (vanilla_count + i) * (ROW_SIZE as usize)) };
        let row_src = unsafe { base.add(read_off(base, &mode, template_idx)) };
        unsafe { std::ptr::copy_nonoverlapping(row_src, row_dst, ROW_SIZE as usize) };
        let pt = &custom_points()[i];
        // NOTE: written via a byte slice, not a `&mut WmpRow` — custom rows
        // start at `rows_start = control + table_bytes`, whose alignment is not
        // guaranteed 16-byte (it flips with the parity of the row count), so an
        // aligned `WmpRow` deref would misalign-panic. Byte-level writes need no
        // alignment.
        let row_bytes = unsafe { std::slice::from_raw_parts_mut(row_dst, ROW_SIZE as usize) };
        apply_point(row_bytes, pt);
    }
    // 4. Write the offset table entries (relative to the new base).
    for i in 0..new_count {
        let dst_off = rows_start + i * (ROW_SIZE as usize);
        let entry = unsafe { nb.add(mode.table_off + i * mode.stride) };
        unsafe {
            if mode.size == 4 {
                wr_u32(entry, 0, dst_off as u32);
            } else {
                wr_u64(entry, 0, dst_off as u64);
            }
        }
    }

    // 5. Id table: clone vanilla entries, then append custom id entries.
    //    Entries are `{u32 id, u32 row_index}` (id at +0, row_index at +4).
    //    Custom ids continue after the highest vanilla id so the table stays
    //    sorted for the getter's binsearch; custom row_index = vanilla_count + i.
    let old_id_table = unsafe { base.add(vanilla_data_size_al) };
    let mut max_vanilla_id = 0u32;
    unsafe {
        for i in 0..vanilla_count {
            let e = std::slice::from_raw_parts(old_id_table.add(i * 8), 8);
            let vid = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
            if vid > max_vanilla_id {
                max_vanilla_id = vid;
            }
            buf.extend_from_slice(e);
        }
    }
    log(&format!(
        "map_icons: custom template row index {template_idx} (id {TEMPLATE_CHURCH_OF_ELLEH}), max vanilla id {max_vanilla_id}"
    ));
    for i in 0..custom_row_count() {
        let id = max_vanilla_id.wrapping_add(1 + i as u32);
        let row_index = (vanilla_count + i) as u32;
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&row_index.to_le_bytes());
    }

    // 6. Patch sizes positionally (mirrors vanilla: u32 data_size at -0x10,
    //    u32 count at -0xc, u16 count-word at +0xa).
    unsafe {
        wr_u32(nb, HDR_OFF_DATA_SIZE, new_data_size as u32);
        wr_u32(nb, HDR_OFF_COUNT, new_count as u32);
        wr_u16(nb, HDR_OFF_COUNT_WORD, new_count as u16);
    }

    let block = BuiltBlock {
        base: nb as usize,
        count: new_count as u32,
    };
    std::mem::forget(buf);
    let _ = ROWS.set(block);
    let custom_count = custom_row_count();
    log(&format!(
        "map_icons: built block base={nb:p} vanilla_count={vanilla_count} \
         new_count={new_count} data_size={new_data_size_al:#x} \
         custom={custom_count}"
    ));
    Some(block)
}

fn apply_point(row: &mut [u8], pt: &CustomPoint) {
    row[OFF_AREA_NO] = pt.area_no;
    row[OFF_GRID_X] = pt.grid_x_no;
    row[OFF_GRID_Z] = pt.grid_z_no;
    write_f32(row, OFF_POS_X, pt.pos[0]);
    write_f32(row, OFF_POS_Y, pt.pos[1]);
    write_f32(row, OFF_POS_Z, pt.pos[2]);
    // Force the church icon (3) with no text label, using the same icon-only
    // pattern a marker row uses: isEnableNoText (bit 2) set + textId1 = -1.
    // The row's `eventFlagId` (the flag that gates whether the icon shows at
    // all) is preserved untouched from the template clone, so these render
    // under the same flag gating as the test icon.
    write_u16(row, OFF_ICON_ID, CUSTOM_ICON_ID);
    row[OFF_FLAGS] |= 1 << 2;
    // Route the label through the PlaceName message path (text_type 0) and
    // point it at our registered note text. `isEnableNoText` (bit 2) is kept ON;
    // verified it does NOT prevent the label from rendering, so it stays.
    row[OFF_TEXT_TYPE1] = 0;
    write_i32(row, OFF_TEXT_ID1, pt.msg_id);
}

fn write_f32(row: &mut [u8], off: usize, val: f32) {
    row[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn write_i32(row: &mut [u8], off: usize, val: i32) {
    row[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn write_u16(row: &mut [u8], off: usize, val: u16) {
    row[off..off + 2].copy_from_slice(&val.to_le_bytes());
}

/// Look up the vanilla row index for `want` in the id table (entries
/// `{u32 id; u32 row_index}` at `base + id_table_off`), returning `None` when
/// the id is absent.
fn find_row_index(base: *const u8, id_table_off: usize, count: usize, want: u32) -> Option<usize> {
    unsafe {
        let tab = base.add(id_table_off);
        for i in 0..count {
            let id = rd_u32(tab, (i * 8) as isize);
            if id == want {
                return Some(rd_u32(tab, (i * 8 + 4) as isize) as usize);
            }
        }
    }
    None
}

/// Group struct filled by the wrapper `0x140d5a491` + filler `0x140d5a4d0`:
/// `[+0]  vftable`
/// `[+8]  first-row offset` (the filler binary-searches the id table for the
///       requested base id, then stores the resolved row offset here; the base
///       of the block is `row_buffer - this_byte_offset`)
/// `[+0x10] row_buffer` = block base + first-row offset (-> first vanilla row)
/// `[+0x18] count`      = number of rows served to the map
#[repr(C)]
struct Group {
    vftable: usize,
    row_offset: u32,
    _pad: u32,
    row_buffer: usize,
    count: u32,
}

/// The wrapper `0x140d5a490` receives `(group, edx, r8d)` from the map
/// population (`mov r8d,0x5f5e100; mov edx,1; call wrapper`) and forwards edx/r8d
/// untouched to the filler, which binsearches the id table for that id range.
/// Our detour must preserve that signature or the filler searches garbage ids.
type GroupBuilderFn = extern "system" fn(*mut Group, u32, u32) -> *mut Group;

/// Set once after the first group fill that carries our extended row set, so
/// we can prove the map-enumeration path actually consumes our block.
static GROUP_FILL_LOGGED: Once = Once::new();

/// Build the extended block and point the container's rows pointer at it, so
/// both the group filler (map enumeration) and the per-id getter resolve
/// against our structure. Idempotent after the first call. Returns whether the
/// redirect is (or already was) in place.
fn ensure_redirected() -> bool {
    if ROWS.get().is_some() {
        return true;
    }
    let Some((rows_base, hop1)) = (unsafe { resolve_rows_base() }) else {
        log("map_icons: redirect: resolve_rows_base failed (registry/container not ready)");
        return false;
    };
    let Some(block) = build_block(rows_base) else {
        log("map_icons: redirect: build_block failed");
        return false;
    };
    unsafe {
        *(hop1.add(0x80) as *mut usize) = block.base;
    }
    log(&format!(
        "map_icons: redirected container rows base -> {:#x} (count={})",
        block.base, block.count
    ));
    true
}

/// Function-replacement detour for the group wrapper. Ensures the extended
/// block is built and redirected BEFORE the original fill runs, so the filler
/// (which reads the param file through the redirected `[hop1+0x80]`) binsearches
/// our extended id table and serves all 473 rows to the map population. The
/// `base`/`end` id range args are forwarded untouched.
unsafe extern "system" fn group_builder_detour(
    group: *mut Group,
    base_id: u32,
    end_id: u32,
) -> *mut Group {
    let _ = ensure_redirected();
    let trampoline = *GROUP_TRAMPOLINE.get().unwrap_or(&0);
    let original_fn: GroupBuilderFn = unsafe { std::mem::transmute(trampoline) };
    let ret = original_fn(group, base_id, end_id);
    // Diagnostic: after a fill exposes our extended row set, log it once.
    if let Some(b) = ROWS.get() {
        if let Some(g) = unsafe { group.as_ref() } {
            if g.count >= b.count {
                GROUP_FILL_LOGGED.call_once(|| {
                    log(&format!(
                        "map_icons: group builder served extended rows row_buffer={:#x} \
                         count={} (block count {})",
                        g.row_buffer, g.count, b.count
                    ));
                });
            }
        }
    }
    ret
}

/// The per-id getter result structure, per the 1.17 getter `0x140d59e11` which
/// writes `[result+8]=id` and `[result+0x10]=row` (and returns a leftover value
/// in rax, NOT the result pointer).
#[repr(C)]
struct GetterResult {
    _unk0: usize,
    id: u32,
    _pad: u32,
    row: usize,
}

type GetterFn = extern "system" fn(*mut GetterResult, u32) -> *mut GetterResult;

/// `GetParamResCap(registry, param_id, index)` — `registry` is `[0x143d85f58]`
/// (a per-param table of 72-byte entries), `param_id` in edx, `index` (r8d) 0.
/// Returns the param container, or null when out of bounds / not loaded yet.
type GetParamResCapFn = extern "system" fn(*const u8, u32, u32) -> *mut u8;

/// Resolve the getter's container rows base the same way the getter itself
/// does, by decoding the RIP-relative `mov rcx,[singleton]` and the
/// `call GetParamResCap` directly out of the hooked getter entry. Returns the
/// `(rows_base, hop1)` pair, where the redirect writes `[hop1+0x80]`.
unsafe fn resolve_rows_base() -> Option<(*const u8, *mut u8)> {
    let entry = *GETTER_ENTRY.get()? as *const u8;
    // RIP-relative disp32: target = next_ip + (i64)disp. Use i64 wrapping_add so
    // backward references (the call) don't overflow u64 in a debug build.
    // `mov rcx,[rip+disp]` at entry+0x2c (7 bytes); next_ip = entry+0x33.
    let singleton_global =
        (entry_add(entry, 0x2c + 7) as i64).wrapping_add(disp_at(entry, 0x2c + 3) as i64) as u64;
    let registry = unsafe { *(singleton_global as *const u8 as *const u64) } as *const u8;
    if registry.is_null() {
        log("map_icons: getter param registry not initialised yet");
        return None;
    }
    // `call GetParamResCap` at entry+0x6d (5 bytes); next_ip = entry+0x72.
    let getparam_addr =
        (entry_add(entry, 0x6d + 5) as i64).wrapping_add(disp_at(entry, 0x6d + 1) as i64) as u64;
    let getparam: GetParamResCapFn = unsafe { std::mem::transmute(getparam_addr as usize) };
    let container = getparam(registry, 0x57, 0);
    if container.is_null() {
        log("map_icons: getter container null");
        return None;
    }
    let hop1 = unsafe { *(container.add(0x80) as *const *mut u8) };
    if hop1.is_null() {
        log("map_icons: getter container hop1 null");
        return None;
    }
    let rows_base = unsafe { *(hop1.add(0x80) as *const *const u8) };
    log(&format!(
        "map_icons: resolved registry={registry:p} container={container:p} \
         hop1={hop1:p} rows_base={rows_base:p}"
    ));
    Some((rows_base, hop1))
}

/// `(ptr + n)` without overflow panics on a `*const u8`.
fn entry_add(p: *const u8, n: usize) -> *const u8 {
    unsafe { p.add(n) }
}
fn disp_at(p: *const u8, off: usize) -> i32 {
    let b = unsafe { std::slice::from_raw_parts(p.add(off), 4) };
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The per-id getter hook. On the first call (whichever detour gets there
/// first) it ensures the extended block is built and the container's rows
/// pointer (`[hop1+0x80]`) is redirected, so every later lookup — vanilla and
/// custom — hits our extended structure. Then it passes through to the
/// original so the build is fully transparent.
unsafe extern "system" fn getter_detour(result: *mut GetterResult, id: u32) -> *mut GetterResult {
    let _ = ensure_redirected();
    let trampoline = *GETTER_TRAMPOLINE.get().unwrap_or(&0);
    let original_fn: GetterFn = unsafe { std::mem::transmute(trampoline) };
    original_fn(result, id)
}

/// The pristine entry prologue of the group wrapper: `PUSH RBX; SUB RSP,0x20`.
/// The 5-byte E9 entry patch overwrites only these bytes; the `LEA RAX,[rip+..]`
/// at entry+5 is left untouched, so the relocation window ends cleanly at
/// entry+5 (a `sub` instruction boundary). No branches live in the window.
const GBE_ENTRY_EXPECTED: [u8; 5] = [0x53, 0x48, 0x83, 0xEC, 0x20];

/// Installs the group-builder hook race-safely, mirroring the speffect helper
/// exactly (pristine prologue -> relocated trampoline; otherwise chain behind
/// an existing hook's target; otherwise bail to avoid clobbering).
fn install_group_race_safe(entry: u64) -> bool {
    let orig = unsafe { std::slice::from_raw_parts(entry as *const u8, 5) }.to_vec();

    let forward_target: u64;
    if orig[..] == GBE_ENTRY_EXPECTED {
        let jump_back = entry + 5;
        let mut trampoline = Vec::with_capacity(24);
        trampoline.extend_from_slice(&orig[0..5]);
        trampoline.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
        trampoline.extend_from_slice(&jump_back.to_le_bytes());
        let Some(addr) = (unsafe { memory::alloc_executable(entry, trampoline.len()) }) else {
            log("map_icons: ERROR: failed to allocate group builder trampoline");
            return false;
        };
        unsafe {
            std::ptr::copy_nonoverlapping(trampoline.as_ptr(), addr as *mut u8, trampoline.len());
        }
        forward_target = addr;
        log(&format!(
            "map_icons: group builder pristine; trampoline at {addr:#x} bytes={trampoline:02x?}"
        ));
    } else if let Some(existing) = hooks::existing_hook_target(entry, &orig) {
        forward_target = existing;
        log(&format!(
            "map_icons: group builder already hooked; chaining behind hook target {existing:#x}"
        ));
    } else {
        log(&format!(
            "map_icons: group builder entry has an incompatible/unrecognised patch ({orig:02x?}); skipping to avoid clobbering another hook"
        ));
        return false;
    }

    hooks::finalize_e9_entry(
        "map_icons: group builder",
        entry,
        group_builder_detour as *const () as usize,
        forward_target as usize,
        &GROUP_TRAMPOLINE,
    )
}

/// Locates the group builder via its unique prologue signature.
fn locate_group_builder() -> Option<u64> {
    scan::scan_pattern(GROUP_BUILDER_PATTERN)
}

pub(crate) fn install() {
    let Some(target) = locate_group_builder() else {
        log("map_icons: ERROR: group builder signature not found");
        return;
    };
    log(&format!("map_icons: group builder at {target:#x}"));
    if !install_group_race_safe(target) {
        log("map_icons: ERROR: group builder hook not installed");
    }

    // The per-id getter hook is a robustness extra for id-based lookups; the
    // map-population path alone already works through the group builder. It is
    // not essential, so a failure here is non-fatal.
    if let Some(getter) = scan::scan_pattern(GETTER_PATTERN) {
        log(&format!("map_icons: getter at {getter:#x}"));
        let _ = GETTER_ENTRY.set(getter);
        if !install_getter_race_safe(getter) {
            log("map_icons: ERROR: getter hook not installed");
        }
    } else {
        log("map_icons: WARNING getter signature not found; skipping id-lookup hook");
    }
}

/// The pristine entry prologue of `GetWorldMapPointParam` — `PUSH RDI; SUB
/// RSP,0x40` (this getter is identified by the shared param-getter tail
/// captured in `GETTER_PATTERN`).
const GETTER_ENTRY_EXPECTED: [u8; 5] = [0x57, 0x48, 0x83, 0xEC, 0x40];

/// Per-id getter hook, mirroring the speffect race-safe installer.
fn install_getter_race_safe(entry: u64) -> bool {
    let orig = unsafe { std::slice::from_raw_parts(entry as *const u8, 5) }.to_vec();

    let forward_target: u64;
    if orig[..] == GETTER_ENTRY_EXPECTED {
        let jump_back = entry + 5;
        let mut trampoline = Vec::with_capacity(24);
        trampoline.extend_from_slice(&orig[0..5]);
        trampoline.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
        trampoline.extend_from_slice(&jump_back.to_le_bytes());
        let Some(addr) = (unsafe { memory::alloc_executable(entry, trampoline.len()) }) else {
            log("map_icons: ERROR: failed to allocate getter trampoline");
            return false;
        };
        unsafe {
            std::ptr::copy_nonoverlapping(trampoline.as_ptr(), addr as *mut u8, trampoline.len());
        }
        forward_target = addr;
        log(&format!(
            "map_icons: getter pristine; trampoline at {addr:#x} bytes={trampoline:02x?}"
        ));
    } else if let Some(existing) = hooks::existing_hook_target(entry, &orig) {
        forward_target = existing;
        log(&format!(
            "map_icons: getter already hooked; chaining behind hook target {existing:#x}"
        ));
    } else {
        log(&format!(
            "map_icons: getter entry has an incompatible/unrecognised patch ({orig:02x?}); skipping to avoid clobbering another hook"
        ));
        return false;
    }

    hooks::finalize_e9_entry(
        "map_icons: getter",
        entry,
        getter_detour as *const () as usize,
        forward_target as usize,
        &GETTER_TRAMPOLINE,
    )
}
