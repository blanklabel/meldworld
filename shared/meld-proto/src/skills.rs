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
        // Hunter (martial baseline): basic attacks bank Adrenaline, all skills spend
        // it. Power Strike is L1; the costlier releases gate by level too.
        "second_wind" => 2,
        "snare" => 2,
        "frenzy" => 3,
        // Psyker manifestations
        "mind_spike" => 3,
        "temporal_anchor" => 5,
        // Resonant
        "regen_boon" => 2,
        "ward" => 3,
        // Shifter (rogue)
        "flicker" => 2,
        "ransack" => 3,
        // Iron Hull (monk / tank): Swell Strike is L1; the rest gate by level.
        "root" => 2,
        "kinetic_shock" => 3,
        "toll_of_the_deep" => 5,
        // power_strike, backstab, swell_strike, gravity_well, transfuse, attack…
        _ => 1,
    }
}

/// Whether a hero at `level` may use `skill`.
pub fn is_unlocked(skill: &str, level: i32) -> bool {
    level >= unlock_level(skill)
}
