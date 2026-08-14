# Working Rules

- After finishing a plan for a change, present the plan to the user and wait
  for their approval before implementing it.

# Building

- Always build the debug build:
  `cargo build` (configured via `.cargo/config.toml` to target
  `x86_64-pc-windows-gnu`).
- The built DLL is produced at:
  `/mnt/BARGE/Git/ERQoL/target/x86_64-pc-windows-gnu/debug/erqol.dll`
- After making a new debug build, copy the DLL to:
  `/mnt/BARGE/FromSoftModding/ERQoL/`
