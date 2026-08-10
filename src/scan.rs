use fromsoftware_shared::program::Program;
use pelite::pattern;
use pelite::pe64::Pe;

pub fn scan_pattern(pattern_str: &str) -> Option<u64> {
    let program = Program::current();
    let atoms = pattern::parse(pattern_str).ok()?;
    let mut captures = [0u32];
    if program.scanner().matches_code(&atoms).next(&mut captures) {
        program.rva_to_va(captures[0]).ok()
    } else {
        None
    }
}

/// Scans for a pattern that ends with a relative call (`e8 ?? ?? ?? ??`) and
/// returns the VA of the call *target* rather than the match start. Patterns
/// passed here should end in `e8 $ '` so the `$` follows the call and the
/// bookmark captures the target RVA.
pub fn scan_pattern_call(pattern_str: &str) -> Option<u64> {
    let program = Program::current();
    let atoms = pattern::parse(pattern_str).ok()?;
    let mut captures = [0u32; 2];
    if program.scanner().matches_code(&atoms).next(&mut captures) {
        program.rva_to_va(captures[1]).ok()
    } else {
        None
    }
}
