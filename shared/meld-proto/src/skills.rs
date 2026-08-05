//! Skill/Manifestation unlock levels — the level a hero must reach before an
//! action becomes usable. Shared by the server (which rejects a locked skill,
//! authoritatively) and the client (which greys the menu row), so the two never
//! disagree. Structural content; the numbers are deliberately small for the
//! slice. Anything not listed is available from level 1 (Attack/Defend/Item and
//! the level-1 skills).

/// Every hero skill key the engine resolves, per class. `unlock_level` returning 1
/// for anything unlisted means a typo elsewhere cannot be caught by that function
/// alone — this is the list to check a key against (see the AD-2 combo registry,
/// whose whole effect depends on its ability keys being real).
pub const HERO_SKILLS: &[(&str, &[&str])] = &[
    (
        "explorer",
        &["power_strike", "second_wind", "snare", "frenzy"],
    ),
    (
        "psyker",
        &["gravity_well", "kinetic_aegis", "mind_spike", "temporal_anchor"],
    ),
    ("resonant", &["transfuse", "regen_boon", "ward"]),
    ("shifter", &["backstab", "flicker", "ransack"]),
    (
        "iron_hull",
        &["swell_strike", "root", "kinetic_shock", "toll_of_the_deep"],
    ),
];

/// Whether `skill` is a hero skill the engine knows how to resolve.
pub fn is_hero_skill(skill: &str) -> bool {
    HERO_SKILLS.iter().any(|(_, ks)| ks.contains(&skill))
}

/// The class whose kit `skill` belongs to, as a `class_key`.
pub fn skill_owner(skill: &str) -> Option<&'static str> {
    HERO_SKILLS
        .iter()
        .find(|(_, ks)| ks.contains(&skill))
        .map(|(class, _)| *class)
}

/// The level at which `skill` (a C2S `skill_kind`, or a Psyker manifestation
/// kind) unlocks. Returns 1 for always-available actions.
pub fn unlock_level(skill: &str) -> i32 {
    match skill {
        // Explorer (martial baseline): basic attacks bank Adrenaline, all skills spend
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

/// The player-facing name of a skill key (`swell_strike` → `Swell Strike`).
pub fn pretty_skill(skill: &str) -> String {
    skill
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a hero at `level` may use `skill`.
pub fn is_unlocked(skill: &str, level: i32) -> bool {
    level >= unlock_level(skill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gated_skill_is_a_known_hero_skill() {
        // An unlock level for a key no class owns would gate a skill that does not
        // exist — a silent typo, since `unlock_level` answers 1 for anything else.
        for skill in [
            "second_wind", "snare", "frenzy", "mind_spike", "temporal_anchor",
            "regen_boon", "ward", "flicker", "ransack", "root", "kinetic_shock",
            "toll_of_the_deep",
        ] {
            assert!(is_hero_skill(skill), "{skill} is gated but owned by no class");
            assert!(unlock_level(skill) > 1, "{skill} is listed but not gated");
        }
        assert!(!is_hero_skill("attack"));
        assert!(!is_hero_skill("nonsense"));
        assert_eq!(skill_owner("backstab"), Some("shifter"));
        assert_eq!(skill_owner("snare"), Some("explorer"));
        assert_eq!(skill_owner("nope"), None);
    }
}
