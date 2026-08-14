# Notes: adjusting the "heavy door" message (EMEVD msg 4200)

Status: research done, no edits made yet. Resume here tomorrow.

## Goal (CLARIFIED — this is the real task)

The mod's EMEVD script uses `DisplayGenericDialog(messageId=4200, ...)` to announce
when the tough start enemy dies / a door opens ("Somewhere, a heavy door has opened").

The actual task is **NOT** to edit text or swap the message ID. It is:

> Find the game function that is called to display the "Somewhere, a heavy door has
> opened" text (i.e. the map-event text / `EventTextForMap` display path), and redirect
> that function to display the same text through the **blinking message** function
> instead (the popup style used for e.g. item/notification messages).

So this is a **runtime / binary-level task in the game process**, not a data-file edit:
1. Attach Cheat Engine to `eldenring.exe`.
2. Find the function that renders msg 4200 (EventTextForMap text).
3. Find the "blinking message" display function.
4. Redirect/hook so the 4200 text is shown via the blinking-message function.

No progress made on this yet. CE bridge was down at end of last session.

## What we found (data-file research, done)

### Where the text lives

- `msg/common/menu.msgbnd.dcx` is a **176-byte stub** (decompresses to a 96-byte BND4
  with 0 files). Don't bother with it — the real data is per-language.
- The English bundle is `Game/msg/engus/menu.msgbnd.dcx`. It is a BND4 (version
  `07D7R6`) containing **18 FMGs** (79,626 entries total).
- Message **4200 lives in `EventTextForMap.fmg`** inside `engus/menu.msgbnd.dcx`:
  - `4200` → `Somewhere, a heavy door has opened`
  - `4010` → `Cannot open from this side`
  - `4000` → `Contraption does not move`
  - `4210` → `Guide and gatekeeper for those returning to the roots`
  - `108000`/`308000`/`408000`/`208000` → `Use X?` / `No X in inventory` /
    `Not enough X` / `X was lost with use` (item dialogs, `gdsparam@8000`)

### Other IDs (DLC bundles, probably not needed)

From `msg/engus/menu_dlc01.msgbnd.dcx` → `EventTextForMap_dlc01.fmg`:
- `2030000` → `Burn the sealing tree?`
- `1030040` → `Somewhere, a great rune has broken...`
- `1030041` → `And so too has a powerful charm.`
- `2020030` → `Somewhere, a spiritspring has been unsealed`

### Tooling

- `er-msgdumper` lives at `/mnt/BARGE/AIImprovements/er-msgdumper`
  (shared AI-agent workspace, not the ERQoL repo).
- Dump a bundle to TSV:
  ```
  cd /mnt/BARGE/AIImprovements/er-msgdumper
  LD_LIBRARY_PATH="$PWD/bin/Release/net10.0" \
    ./bin/Release/net10.0/er-msgdumper \
    "<game>/msg/engus/menu.msgbnd.dcx" -o /tmp/opencode/er/msgdump/engus
  # -> /tmp/opencode/er/msgdump/engus/messages.tsv  (tab: bundle \t fmg \t id \t text)
  ```
- Dumps are also saved under `/tmp/opencode/er/msgdump/engus{,_dlc01,_item}/messages.tsv`.
- CE MCP bridge lives at `/mnt/BARGE/AIImprovements/cheatengine-mcp-bridge` (was NOT
  reachable at end of last session — need to restart it before resuming).

## Notes / gotchas

- Earlier approach (edit the FMG text / swap the EMEVD messageId) is NOT what the
  user wants — this is a code redirection task in the running game.
- The terminal output stream got corrupted once (a dead Cheat Engine bridge process
  interleaved garbage into tool output). If output looks mangled, kill stray
  `python3`/`dotnet` processes and retry.

## Next steps (tomorrow)

1. Restart the CE MCP bridge (`/mnt/BARGE/AIImprovements/cheatengine-mcp-bridge`),
   open the game, attach to `eldenring.exe`.
2. Locate the "Somewhere, a heavy door has opened" string (UTF-16LE) in game memory
   and/or find references to `EventTextForMap` msg 4200 display code.
3. Identify the map-event text display function.
4. Identify the blinking-message display function.
5. Redirect the map-text display function → blinking-message function (hook/patch).
6. Verify in-game.
