//! Skill/Manifestation unlock levels — the level a hero must reach before an
//! action becomes usable. Shared by the server (which rejects a locked skill,
//! authoritatively) and the client (which greys the menu row), so the two never
//! disagree. Structural content; the numbers are deliberately small for the
//! slice. Anything not listed is available from level 1 (Attack/Defend/Item and
//! the level-1 skills).

/// The level at which `skill` (a C2S `skill_kind`, or a Psyker manifestation
/// kind) unlocks. Returns 1 for always-available actions.
pub fn unlock_level(skill: &str) -> i32 {
    match skill {
        // Squire
        "second_wind" => 2,
        // Psyker manifestations
        "mind_spike" => 3,
        "temporal_anchor" => 5,
        // Resonant
        "regen_boon" => 2,
        "ward" => 3,
        // gravity_well, kinetic_aegis, power_strike, transfuse, attack, defend, item…
        _ => 1,
    }
}

/// Whether a hero at `level` may use `skill`.
pub fn is_unlocked(skill: &str, level: i32) -> bool {
    level >= unlock_level(skill)
}
