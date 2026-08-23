# ERQoL Notes — Injecting HKS variables into the runtime name→ID dictionary

Status: **investigation complete, implementation tabled.** Any of the 3 options below can be
implemented directly from this document without re-doing the reverse engineering.

## Goal

The AuraFarmingPostures mod (see `/mnt/BARGE/FromSoftModding/ERQoL/ogpostures/` and our active
copy in `.../ERQoL/postures/`) needs 11 new character-script variables (`ERAF*`, see list at the
bottom). The vanilla game does not know these names; the mod therefore ships an edited
`action/variablenameid.txt`. We want to eventually stop shipping that file and instead add the
entries to the game's **in-memory dictionary** (or otherwise satisfy name resolution), so that
the only remaining file replacements are genuine assets (`chr/*.anibnd.dcx`, `chr/c0000.behbnd.dcx`,
regulation, etc.).

## How variable/event name resolution actually works

All addresses are for `eldenring.exe` **1.16.2** (Ghidra project
`/mnt/BARGE/GhidraProjects/ELDENRINGLOCAL`, program `pc_eldenring_runtime.1.16.2.exe`,
image base 0x140000000).

- Script side: c0000.hks defines
  - `function GetVariable(name) return hkbGetVariable(name) end` (line ~90)
  - `function SetVariable(name, v) act(SetHavokVariable, name, v) end` (line ~381)
- Engine side:
  - `hkbGetVariable` Lua impl = `FUN_14145b980`
    (find via string `"hkbGetVariable"` @ `142d48d20`, xrefs `14145b992`, `14145ce70`).
    Flow: name string → hash map lookup → int id → second map id → value; asserts
    `"Variable '%s' was not found"` on failure.
  - `hkbFireEvent`-style event raise = `FUN_14145a940`; same pattern with
    `"Event '%s' not found."`.
- Object chain used by both:
  - `hkbSelf +0x10` → Behavior Context (`FUN_141451740`)
  - `[BC]` → Character (`FUN_141451710`)
  - `Character +0xe0` → Project Asset Manager ("PAM", `FUN_141451690`)
  - `S = *(PAM + 0x100)`
  - `S +0x18` → owner of the **event** name→id map (seeded from `action:/eventNameId.txt`)
  - `S +0x20` → owner of the **variable** name→id map (seeded from `action:/variableNameId.txt`)
  - actual map struct sits at `owner + 0x28` in both cases.
- The txt files themselves are registered as resources at startup by `FUN_1400a8620`
  (paths built inline as wide strings: `L"action:/eventNameId.txt"`, `L"action:/variableNameId.txt"`
  → stored @ `DAT_143b39d48`, `L"action:/stateNameId.txt"` → `DAT_143b39d88`).
- Map container (shared template, also used for CSHkBehWorld named allocators):
  - find = `FUN_140c142c0(map*, key*)`:
    - `mask   = *(u32*)(map + 0x18)` (power-of-two minus 1)
    - `slots  = *(void**)(map + 0x10)`, slot stride 8 bytes `{int a; int b;}`
    - NOTE: probe reads the **second** dword: `idx = *(int*)(slots + 4 + i*8)`; linear probing
      `i = (i+1) & mask`; empty when `idx < 0`
    - `entries = *(void**)(map + 0x00)`, entry stride 0x10: `{void* key; int id; int pad}`
    - key equality: pointer-equal OR `FUN_1416950d0(...)`; hash: `FUN_1416a28c0(key)`
  - example caller using the same container for named allocators: `FUN_140c14130`
    (`GLOBAL_CSHkBehManager->hkBehWorld → FUN_141450a00 → +0x18 → +0x28`).

### The 11 entries we must make resolvable (ids from the mod's variablenameid.txt)

| id | name |
|----|------|
| 659 | ERAFPostureOverlayWeight |
| 660 | ERAFIdleBlendUpperBody |
| 661 | ERAFMainPostureSelectorIndex |
| 662 | ERAFIdleBlendRightArm |
| 663 | ERAFIdleBlendLeftLeg |
| 664 | ERAFIdleBlendRightLeg |
| 669 | ERAFRightArmSelectorIndex |
| 677 | ERAFIdleBlendLeftArm |
| 678 | ERAFLeftArmSelectorIndex |
| 679 | ERAFTwoLaneProofRightArmWeight |
| 680 | ERAFTwoLaneProofRightArmSelectorIndex |

(These ids were chosen to match variables defined inside the modified `chr/c0000.behbnd.dcx`
Havok behavior graph; the graph consumes them by id. The script only ever *writes* them,
via `SetVariable`, ~32 call sites.)

## Option A — keep shipping the txt (current plan)

Zero work. Keep the `action/variablenameid.txt` redirect in the me3 profile. Only worth
revisiting if we later want the `postures/action/` folder gone entirely.

## Option B — insert entries into the runtime maps post-load

Preferred end state. Sketch:

1. Capture the PAM/S pointers at runtime. Two candidate methods:
   - Detour `FUN_14145b980` once, read the chain off the live call, store `S`, restore.
   - Or walk `GLOBAL_CSHkBehManager` (named global in Ghidra) → world → characters.
2. Locate or replicate the map's **insert/rehash** routine (open item — see "Open items").
3. Insert the 11 entries (id table above) before the first character executes c0000.hks.
4. Verify from injected Lua: `GetVariable("ERAFPostureOverlayWeight")` must not assert.

Key unknowns/risks:

- Key encoding of map entries (interned char* vs wide; how strings are interned — see
  `FUN_1416916a0` / `FUN_141693010` used when building names elsewhere).
- Allocator for entries/slots (likely Havok heap: `GetHavokHeapMemAllocator()`).
- Timing: whether S/maps exist at DLL-init time or are built lazily at first character load;
  insertion must happen after build but before first `hkbGetVariable/SetHavokVariable` miss.
- Whether PAM is shared across all characters (expected yes — "project" asset manager).
  First runtime check: compare PAM pointers between two different characters.

## Option C — detour name resolution, hardcode ERAF* ids

Smallest surface, no container mutation:

- Detour `FUN_14145b980` (`hkbGetVariable`): if the name argument starts with `ERAF`, push the
  hardcoded id/value path directly (mimic its success tail) and return; else run original.
- Find the `SetHavokVariable` action implementation (search the act-dispatch registration for the
  literal `"SetHavokVariable"` at runtime / in Ghidra) and special-case the same 11 names there,
  writing through the normal id-based write helper.
- Immune to map rebuilds/timing; slightly hacky but robust.

## Open items (to resolve during implementation)

1. Find the map insert/populate function. Fastest method: attach Cheat Engine to a running ER,
   hardware-write breakpoint on the slots array of the live variable map (walk the object chain
   above from any character), boot to title/first spawn, and read the writer PC. Cross-check in
   Ghidra.
2. Determine key representation (encoding/interning) — inspect a few live entries.
3. Confirm shared-PAM assumption (two characters → same pointer).

## Verification recipe

With option B or C implemented and the me3 profile pointing at a *vanilla* `variablenameid.txt`
(or no redirect):

1. Game boots without asserts mentioning `Variable '...' was not found` for ERAF names.
2. Injected Lua probe returns sensible values for the 11 ids.
3. In-game: posture overlay blending behaves identically to the file-shipping setup
   (compare against the `ogpostures` instance launched via `posturetest.me3`).
