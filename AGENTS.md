# Working Rules

- After finishing a plan for a change, present the plan to the user and wait
  for their approval before implementing it.
- Do NOT use the Task tool (subagents) — they stall out in the harness. Do
  all work directly with Read/Edit/Grep/Glob/Bash tools.
- When using GhidraMCP tools, send only ONE command at a time and wait for
  its response before sending the next. The Ghidra bridge does not handle
  concurrent requests safely.
- Do NOT launch Elden Ring (or any game) yourself. When a change needs an
  in-game test run, ask the user to run it, then inspect the logs afterwards
  (`/mnt/BARGE/FromSoftModding/ERQoL/logs/erqol.log` and `crash.log`).

# Building

- Always build the debug build:
  `cargo build` (configured via `.cargo/config.toml` to target
  `x86_64-pc-windows-gnu`).
- The built DLL is produced at:
  `/mnt/BARGE/Git/ERQoL/target/x86_64-pc-windows-gnu/debug/erqol.dll`
- After making a new debug build, copy the DLL to:
  `/mnt/BARGE/FromSoftModding/ERQoL/`
- When testing with **Elden Ring Reforged**, copy the DLL to Reforged's own
  third-party DLL folder instead:
  `/mnt/BARGE/FromSoftModding/ERRv2.2.9.6/dll/offline/`
