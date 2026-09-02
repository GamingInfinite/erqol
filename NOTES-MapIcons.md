# NOTES — Adding custom map icons to ERQoL

Goal: make ERQoL add custom map icons (initially a single icon next to the
Church of Elleh grace in Limgrave), reusing the existing Site of Grace icon
type (iconId `83`) — no new icon/graphics types for the PoC.

This file captures verified reverse-engineering findings and the open design
decision. The consumption path is fully mapped; the append seam is the open
question.

---

## Deployment model (ERQoL)

- ERQoL is a **cdylib DLL** (`Cargo.toml`: `crate-type = ["cdylib"]`), injected
  at runtime. It does **not** ship a modded `regulation.bin`.
- It hooks code with `ilhook` and reads/writes params via `fromsoftware-rs`.
- Built DLL path:
  `/mnt/BARGE/Git/ERQoL/target/x86_64-pc-windows-gnu/debug/erqol.dll`
- Deploy: copy to `/mnt/BARGE/FromSoftModding/ERQoL/` (vanilla ER), or
  `/mnt/BARGE/FromSoftModding/ERRv2.2.9.6/dll/offline/` (Reforged).
- Reference append pattern that already works: `src/postures/speffects.rs`
  hooks the per-id getter `GetSpEffectParam` and serves verbatim row blobs for
  mod-owned ids (ROM size 912). NOTE: that id-getter hook does **not**
  transfer directly to the map-icon path (see below).

Contrast: **Original MapForGoblins ships a regbin replacement** (not a DLL),
proving that rows added to `WorldMapPointParam` in the regbin are iterated by
the game regardless of ID continuity. This is evidence that whatever rows are
present in the param are enumerated.

---

## Verified facts (1.16.2, Ghidra project `pc_eldenring_runtime.1.16.2.exe`)

### Param identity
- `WorldMapPointParam` type-id = `0x57` (name→id table `0x142bb3400`).
  `0x55` MultiMPEstusFlaskBonusParam, `0x56` MultiSoulBonusRateParam, `0x58` next.
- `GetWorldMapPointParam` = `FUN_140d580d0`: type-0x57 per-id getter (analog of
  `GetSpEffectParam`). Writes `{id @ +0x8, row @ +0x10}`.
- Row size = **0x100 = 256 bytes**, three-way confirmed:
  - disassembly (`row = group_data + index*0x100`),
  - raw dump (`0x1D800` = 472 × 0x100),
  - `static_assert(sizeof(world_map_point_param_st) == 256)` in the generated
    elden-x header.

### Map-icon enumeration (the important part)
- `FUN_1409c62c0` is the map-icon population routine. It builds a
  `WorldMapPointParamGroup` via `FUN_140d58750(group, start=1, end=100000000)`
  and then loops `index = 0 .. count`, resolving each row by DENSE INDEX and
  spawning a marker for visible ones.
- Group struct layout: `+0x00` vftable, `+0x08` base_id, `+0x10` row-buffer ptr,
  `+0x18` count.
- `FUN_140d58790(group, start, end)` fills the group from the WorldMapPointParam
  ParamFile inside `SoloParamRepository`:
  - fetches `GetParamResCap(SoloParamRepository, WorldMapPointParam, 0)`,
  - binary-searches the param file's RowLookupEntry table for the id range,
  - sets `count = endRowIndex - startRowIndex + 1` (i.e. the number of rows in
    range, NOT `(end-start)+1`),
  - sets `row_buffer` = base of the contiguous row block.
- Resolver `FUN_1409c2670(group, out, index)`:
  - `id = group->base_id + index`
  - `row_ptr = group->row_buffer + index*0x100`
  - valid only when `0 <= index < group->count`, else id = -1 / null row.
- Per-row consumption in `FUN_1409c62c0`: reads world pos `+0x24/+0x28/+0x2c`,
  calls `FUN_140886880` then visibility gate `FUN_1408877d0`; if visible,
  `HeapAlloc(0,8,0x310)`, `FUN_14087ba70`, add marker to collection
  `[+0x3948]` via `FUN_1409c8c50`. Loop `index in 0..count`.
- Consumer chain: `Update (map-screen tick)` → `FUN_1403bd7d0` →
  `CSDistViewManagerImp::OpenDistViewMark` → `FUN_1408875a0` →
  `GetWorldMapPointParam`.

### Key insight: dense index iteration, NOT sparse-id iteration
- The iterator walks the row block by **dense consecutive index**:
  `row = row_buffer + index*0x100`, `id = base_id + index`. It iterates
  **every row present** in the WorldMapPointParam ParamFile, subject to
  visibility flags.
- Vanilla param: **472 rows**, IDs sparse (`78500`..`88524000`, with 340 id-gaps
  e.g. nothing between `430100` and `61413200`).
- Reforged (modded) param (`rows=4954`, captured live): added a **dense low-id
  block (ids 1..~4587)** with real icon/pos data, while keeping the vanilla
  sparse high ids. This confirms rows are appended by growing the row set, not
  by filling sparse-id gaps.
- Conclusion: **sparse id space ≠ spare row capacity.** The row block is dense
  (`count` slots × 0x100). "Empty ids" don't give you empty slots.

---

## The open design decision (append seam)

The map enumeration does **not** use the id getter (`GetWorldMapPointParam`).
It resolves rows by dense index from the ParamFile inside `SoloParamRepository`.
So the speffects id-getter hook does not apply directly.

For `FUN_1409c62c0` to spawn a new marker, a row must be present in the
WorldMapPointParam row block the group builder reads, so that some
`index < count` resolves to it. Options:

### Option A — extend the in-memory ParamFile (true append)
Allocate a new buffer containing vanilla `[RowDescriptor array + row block +
RowLookupEntry table]` plus appended rows; bump `metadata.row_count`,
`after_name_offset`; swap `FD4ParamResCap->paramFile`. Heaviest; touches shared
game state; riskiest. The map's hardcoded `index*0x100` stride only holds if
appended rows sit contiguously right after vanilla's last row at 256B stride.

### Option B — hook the group range-builder (`FUN_140d58790`)
Serve a mod-owned group whose `row_buffer` points at mod-owned 256B rows so
`row_buffer + index*0x100` resolves. Same contiguous-after-vanilla requirement
as A; roughly as hard.

### Option C — regbin replacement (like MapForGoblins)
Ship a modded `regulation.bin` with extra `WorldMapPointParam` rows. Proved to
work by MapForGoblins. Simplest conceptually, but displaces ERQoL's current
runtime-only deployment model and must merge with whatever Reforged's regbin
does.

### Next verifications before committing to a seam
1. Determine whether there is **spare allocated capacity after the vanilla row
   block** in the in-memory ParamFile buffer (would make an in-place append
   viable). Needs the running game (CE) — ask the user to launch ER.
2. Confirm exactly which per-row fields drive marker rendering/selection:
   `iconId` (`+0x??`), `eventFlagId`, `clearedEventFlagId`,
   `isEnableNoText`, `areaNo/gridXNo/gridZNo`, `textId1`, `distViewId`,
   `dispMinZoomStep`, `selectMinZoomStep`. This defines what a valid mod row
   blob must contain for the marker to render and be selectable.
3. Decide the icon: reuse iconId `83` (Site of Grace) — no new icon type.

---

## Related files
- `/mnt/BARGE/Git/ERQoL/src/postures/speffects.rs` — existing id-getter
  hook/append pattern (may not transfer to map enumeration).
- `/mnt/BARGE/Git/ERQoL/src/postures/speffect_ids.rs` — mod-owned id pattern.
- `/mnt/BARGE/Git/fromsoftware-rs/crates/eldenring/src/fd4/param_repository.rs`
  — `ParamFile` layout (`row_count` @ `+0xa`, dense `row block`, `RowLookupEntry`
  `{id,index}` table, `[ParamFileMetadata]` at `-0x10` with `after_name_offset`
  and `row_count`). No row-append primitive in fromsoftware-rs.
- `/mnt/BARGE/Git/fromsoftware-rs/crates/eldenring/src/cs/solo_param_repository.rs`
  — `SoloParamHolder`, `WorldMapPointParam` (index 87).
- `/tmp/opencode/rawdump/wmp.bin` — vanilla in-memory WorldMapPointParam dump
  (row_count=472, strings_offset=0x20480).
- `/tmp/opencode/v_wp.csv` — vanilla rows (472, ids 78500..88524000, sparse).
- `/tmp/opencode/m_wp.csv` — Reforged rows (4954, dense low-id block 1..4587 +
  vanilla high ids).
- Vanilla regulation: `/mnt/BARGE/SteamLibrary/steamapps/common/ELDEN RING/Game/regulation.bin`
- regdumper: `/mnt/BARGE/AIImprovements/er-regdumper/`
- Ghidra program `pc_eldenring_runtime.1.16.2.exe` (MCP socket
  `/run/user/1000/ghidra-mcp/ghidra-1590860.sock`).

## Immediate next step (PoC)
Add one icon next to the Church of Elleh grace in Limgrave, reusing iconId `83`.
Confirm the field layout / idle slot strategy (Option A/B/C) first, then produce
a 256-byte `world_map_point_param_st` row blob with the Elleh world coordinates
and site-of-grace icon.

## 1.17 signature verification + fixes (2026-09-01)

Root cause of the runtime "group builder signature not found": two hand-typo
bytes in `GROUP_BUILDER_PATTERN`:
  - `48 89 CB` -> `48 8B D9`  (mov rbx, rcx, NOT mov [rbx],rcx)
  - `48 89 D8` -> `48 8B C3`  (mov rax, rbx, NOT mov [rax],rbx)
Corrected pattern is UNIQUE (1 hit) against the 1.17 exe -> VA `0x140d5a491`.

The 1.17 group wrapper is byte-for-byte identical to 1.16.2: `PUSH RBX; SUB
RSP,0x20; LEA RAX,[vftable]; MOV [RCX+8],-1; MOV [RCX],RAX; MOV RBX,RCX; XOR
EAX,EAX; MOV [RCX+0x10],EAX; MOV [RCX+0x18],EAX; CALL filler; MOV RAX,RBX;
...; RET`. So field offsets hold: base_id `[+8]`, row_buffer `[+0x10]`,
count `[+0x18]`. Hook design is valid on 1.17.

The getter: 1.16.2 getter prologue + body template is preserved in 1.17, but
the body uses `48 8B F1` (mov rsi,rcx), NOT `4C 8B F1` (mov r14,rcx) — a second
hand-typo fixed in `GETTER_PATTERN`.

IMPORTANT: the getter prologue alone is NOT unique in 1.17 — it matches 65
functions, because every param-type getter is the same compiled
`GetParamResCap<T>` template. `GETTER_PATTERN` was therefore extended through
the FD4Singleton-ctor/assert block and anchored on `lea edx,[r8+0x57]`
(WORLD_MAP_POINT param id, 0x57) as the disambiguating tail. Extended pattern
is UNIQUE -> VA `0x140d59e11`.

Verified with the corrected patterns:
  - GROUP_BUILDER_PATTERN -> 1 hit, VA `0x140d5a491`
  - GETTER_PATTERN (extended) -> 1 hit, VA `0x140d59e11`

After fixing both, the DLL rebuilt clean and was copied to
`/mnt/BARGE/FromSoftModding/ERQoL/`. Next in-game test: open Limgrave map
around Church of Elleh and inspect `/mnt/BARGE/FromSoftModding/ERQoL/logs/erqol.log`
for `map_icons:` lines ("group builder at 0x140d5a491", "built N rows ...
appended 1 custom", "group builder redirected") and crash-free run.
