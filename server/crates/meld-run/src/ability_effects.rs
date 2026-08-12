//! The **numbers** behind every ability, as one line the player can read.
//!
//! [`meld_proto::skills`] says what an ability is *for*; it cannot say how much,
//! because magnitudes are `[TUNABLE]`s and `meld-proto` is shared with the client,
//! which has no `balance.toml`. So the registry's prose stopped at the category —
//! "A heavy blow. Spends Adrenaline." — and a player could not learn what Power
//! Strike costs, or that Frenzy costs twice as much, without pressing it and being
//! refused. Every row read as flavour because every row *was* flavour once you
//! asked the only question that decides between them.
//!
//! This fills that in from `balance.toml` server-side and ships it beside the
//! description (`run.party`), the same way [`meld_proto::synergies`] already ships a
//! formatted `effect` next to a synergy's prose. The numbers therefore cannot drift
//! from the ones the resolver uses: retune the `[TUNABLE]` and the tooltip retunes.

use meld_balance::Balance;

fn dmg(mult: f64) -> String {
    format!("{}× damage", trim(mult))
}

fn pct(f: f64) -> String {
    format!("{}%", (f * 100.0).round() as i64)
}

fn secs(ticks: u64, balance: &Balance) -> String {
    let ms = ticks * balance.battle.tick_ms.max(1);
    format!("{}s", (ms as f64 / 1000.0 * 10.0).round() / 10.0)
}

/// A float without a trailing `.0`, so `1.0×` reads as `1×`.
fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The ATB gauge runs 0..1, so a drain is naturally a fraction of one turn — which
/// is what the player is actually buying, and what "0.35" would not have told them.
fn turn(drain: f64) -> String {
    format!("{} of its turn", pct(drain))
}

fn join(parts: &[String]) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" · ")
}

/// What a Hunter ability costs, and the rate a basic Attack banks toward it. Every
/// Hunter skill is refused unless its cost is banked, so a row without both numbers
/// is a row the player cannot plan around.
fn adrenaline(cost: i32, balance: &Balance) -> String {
    let b = &balance.battle;
    format!(
        "{cost} of {} Adrenaline ({} per Attack)",
        b.hunter_adrenaline_max, b.hunter_adrenaline_per_attack
    )
}

/// The one-line magnitude for `key`, empty for anything outside the registry
/// (Attack, Defend, Item). Keys are [`meld_proto::skills`] keys.
pub fn effect_line(key: &str, balance: &Balance) -> String {
    let b = &balance.battle;
    let sm = &balance.smithwright;
    let kp = &balance.keeper;
    match key {
        // ---- Explorer
        "trailblaze" => join(&[
            dmg(b.explorer_trailblaze_mult),
            format!(
                "marked: every ally deals {} to it for {}",
                pct(b.explorer_mark_damage_mult),
                secs(b.explorer_mark_ticks, balance)
            ),
        ]),
        "field_dressing" => {
            format!("heals {} of the target's max HP", pct(b.explorer_field_dressing_fraction))
        }
        "misdirection" => join(&[
            dmg(b.explorer_read_ground_mult),
            turn(b.explorer_read_ground_drain),
            format!(
                "distracted for {}: +{} to dodge its swings, +{} to flee",
                secs(b.explorer_misdirection_ticks, balance),
                pct(b.explorer_misdirection_miss),
                pct(b.explorer_misdirection_flee_bonus)
            ),
        ]),
        "stable_ground" => format!(
            "Barrier for {} of each ally's max HP (decays {} per turn)",
            pct(b.explorer_stable_ground_fraction),
            b.barrier_decay_per_turn
        ),
        "safe_passage" => format!("+{} dodge to every ally", pct(b.explorer_safe_passage_evasion)),
        "a_world_known" => format!(
            "every ally's gauge fills {}× faster for {}",
            trim(b.explorer_haste_mult),
            secs(b.explorer_haste_ticks, balance)
        ),
        "now" => "every living ally's gauge to full · 1 use per battle".to_string(),
        "the_world_entire" => join(&[
            format!(
                "marks EVERY enemy for {}",
                secs(b.explorer_world_entire_mark_ticks, balance)
            ),
            format!(
                "hastes the party {} for {}",
                dmg(b.explorer_haste_mult).replace("× damage", "× gauge"),
                secs(b.explorer_world_entire_haste_ticks, balance)
            ),
        ]),

        // ---- Hunter: the cost IS the class, so it leads every row.
        "power_strike" => {
            join(&[dmg(b.skill_power_mult), adrenaline(b.hunter_power_strike_cost, balance)])
        }
        "crushing_blow" => join(&[
            dmg(b.hunter_crushing_blow_mult),
            adrenaline(b.hunter_power_strike_cost, balance),
        ]),
        "second_wind" => join(&[
            format!("heals {} of your max HP", pct(b.skill_heal_fraction)),
            adrenaline(b.hunter_second_wind_cost, balance),
        ]),
        "snare" => join(&[
            dmg(b.explorer_snare_mult),
            turn(b.explorer_snare_drain),
            adrenaline(b.hunter_snare_cost, balance),
        ]),
        "pin_the_prey" => join(&[
            dmg(b.pin_the_prey_mult),
            turn(b.pin_the_prey_drain),
            adrenaline(b.hunter_snare_cost, balance),
        ]),
        "frenzy" => {
            join(&[dmg(b.explorer_frenzy_mult), adrenaline(b.hunter_frenzy_cost, balance)])
        }
        "iron_lung" => join(&[
            format!("heals {} of your max HP", pct(b.hunter_iron_lung_heal_fraction)),
            format!("{} Regen a turn", b.hunter_iron_lung_regen),
            adrenaline(b.hunter_second_wind_cost, balance),
        ]),
        "apex_predator" => join(&[
            format!("{} to EVERY enemy", dmg(b.hunter_apex_mult)),
            adrenaline(b.hunter_frenzy_cost, balance),
        ]),

        // ---- Psyker: a Focus fires again every Psyker turn it is held, so the
        // per-tick number is the one that decides between two Foci.
        "gravity_well" => focus(dmg(b.psyker_gravity_tick_mult) + ", ignoring armour"),
        "mind_spike" => focus(dmg(b.psyker_spike_tick_mult) + ", ignoring armour"),
        "kinetic_aegis" => {
            focus(format!("Barrier for {} of your max HP", pct(b.psyker_aegis_tick_fraction)))
        }
        "temporal_anchor" => focus(turn(b.psyker_anchor_gauge_drain)),
        "dominate_mind" => {
            focus(format!("{}, and its gauge to zero", turn(b.psyker_anchor_gauge_drain)))
        }
        "kinetic_wave" => focus(format!("{} to EVERY enemy", dmg(b.psyker_wave_tick_mult))),
        "reality_collapse" => focus(format!(
            "{} to EVERY enemy, ignoring armour",
            dmg(b.psyker_collapse_tick_mult)
        )),
        "thermal_flux" => focus(format!("{} as FIRE", dmg(b.psyker_thermal_tick_mult))),
        "matter_dissolution" => focus(format!(
            "{}, and {} armour off it",
            dmg(b.psyker_dissolution_tick_mult),
            b.psyker_dissolution_armour_shred
        )),
        "phase_shift" => focus(format!("+{} dodge", pct(b.psyker_phase_evasion))),

        // ---- Resonant
        "transfuse" => format!(
            "heals {} of the ally's max HP, and costs you {} of that",
            pct(b.resonant_transfuse_heal_fraction),
            pct(b.resonant_transfuse_cost_fraction)
        ),
        "regen_boon" => format!("+{} Regen per turn", b.resonant_boon_regen),
        "ward" => {
            format!("Barrier for {} of their max HP", pct(b.resonant_ward_barrier_fraction))
        }
        "mend_all" => {
            boon(b.resonant_mend_all_fraction, 0, 0.0, b.resonant_mend_all_self_cost, true)
        }
        "sanctuary" => boon(0.0, b.resonant_sanctuary_regen, 0.0, 0.0, true),
        "revitalize" => {
            boon(b.resonant_revitalize_fraction, 0, 0.0, b.resonant_revitalize_self_cost, false)
        }
        "lifewell" => boon(
            b.resonant_lifewell_fraction,
            b.resonant_lifewell_regen,
            0.0,
            b.resonant_lifewell_self_cost,
            true,
        ),
        "bloodbond" => boon(
            b.resonant_bloodbond_fraction,
            b.resonant_bloodbond_regen,
            b.resonant_bloodbond_barrier_fraction,
            b.resonant_bloodbond_self_cost,
            false,
        ),
        "martyr" => boon(b.resonant_martyr_fraction, 0, 0.0, b.resonant_martyr_self_cost, true),
        "eternal_bloom" => boon(
            b.resonant_bloom_fraction,
            0,
            b.resonant_bloom_barrier_fraction,
            b.resonant_bloom_self_cost,
            true,
        ),

        // ---- Shifter
        "backstab" => join(&[
            dmg(b.shifter_backstab_mult),
            format!("ignores {} of its armour", pct(b.shifter_backstab_pierce)),
        ]),
        "flicker" => format!(
            "+{} dodge, decaying {} each of your turns",
            pct(b.shifter_flicker_evasion),
            pct(b.shifter_flicker_decay)
        ),
        "ransack" => join(&[dmg(b.shifter_ransack_mult), turn(b.shifter_ransack_drain)]),
        "assassinate" => join(&[
            dmg(b.shifter_assassinate_mult),
            format!("ignores {} of its armour", pct(b.shifter_assassinate_pierce)),
        ]),
        "grand_larceny" => join(&[
            format!("{} to EVERY enemy", dmg(b.shifter_larceny_mult)),
            format!("{} each", turn(b.shifter_larceny_drain)),
        ]),
        "steal" => join(&[
            turn(b.shifter_steal_drain),
            format!(
                "{} chits per tier, {} chance of a material",
                b.shifter_steal_chits_per_tier,
                pct(b.shifter_steal_material_chance)
            ),
        ]),
        "mug" => join(&[
            dmg(b.shifter_mug_mult),
            turn(b.shifter_mug_drain),
            format!("{} chits per tier", b.shifter_steal_chits_per_tier),
        ]),

        // ---- Phoenix Guard: the undead multiplier is on every damaging row, because
        // it is the whole reason to field one.
        "silvered_strike" => join(&[
            dmg(b.phoenix_guard_swell_mult),
            turn(b.phoenix_guard_swell_drain),
            undead(balance),
        ]),
        "rite_of_rest" => format!(
            "Barrier for {} of your max HP",
            pct(b.phoenix_guard_root_barrier_fraction)
        ),
        "holy_censure" => join(&[
            dmg(b.phoenix_guard_shock_mult),
            "its gauge to zero".to_string(),
            undead(balance),
        ]),
        "purging_light" => {
            join(&[format!("{} to EVERY enemy", dmg(b.phoenix_guard_toll_mult)), undead(balance)])
        }
        "hallowed_ground" => join(&[
            format!("{} to EVERY enemy", dmg(b.phoenix_guard_hallowed_mult)),
            "zeroes every gauge".to_string(),
            undead(balance),
        ]),
        "phoenix_ascendant" => join(&[
            format!("{} to EVERY enemy", dmg(b.phoenix_guard_ascendant_mult)),
            undead(balance),
            format!(
                "Barrier for {} of each ally's max HP",
                pct(b.phoenix_guard_ascendant_barrier_fraction)
            ),
        ]),
        "unbroken_vigil" => format!(
            "Barrier for {} of each ally's max HP",
            pct(b.phoenix_guard_vigil_barrier_fraction)
        ),
        "eradication" => join(&[
            format!(
                "{} rising to {}× against a target at 1 HP",
                dmg(b.phoenix_guard_eradication_mult),
                trim(b.phoenix_guard_eradication_mult + b.phoenix_guard_eradication_missing_bonus)
            ),
            undead(balance),
        ]),

        // ---- Smithwright
        "hammer_fall" => join(&[dmg(sm.hammer_mult), turn(sm.hammer_gauge_drain)]),
        "quench" => format!("Barrier for {} of your max HP", pct(sm.quench_barrier_fraction)),
        "bulwark" => {
            format!("Barrier for {} of each ally's max HP", pct(sm.bulwark_barrier_fraction))
        }
        "tempering_blow" => format!("+{} attack for the rest of the fight", sm.temper_atk_bonus),
        "slag_spray" => format!("{} to EVERY enemy, ignoring armour", dmg(sm.slag_mult)),
        "anvil_chorus" => {
            format!("+{} attack for EVERY ally, for the rest of the fight", sm.chorus_atk_bonus)
        }
        "great_work" => join(&[
            format!(
                "heals {} and Barriers {} of every ally's max HP",
                pct(sm.great_work_heal_fraction),
                pct(sm.great_work_barrier_fraction)
            ),
            format!("+{} attack for the rest of the fight", sm.great_work_atk_bonus),
        ]),
        "one_true_forge" => format!(
            "heals {} and Barriers {} of every ally's max HP",
            pct(sm.forge_heal_fraction),
            pct(sm.forge_barrier_fraction)
        ),

        // ---- Keeper: its damage rides Mnd, so the row says so.
        "thornlash" => join(&[
            format!("{} (from Mnd)", dmg(kp.thornlash_mult)),
            turn(kp.thornlash_gauge_drain),
        ]),
        "poultice" => {
            format!("heals {} and grants +{} Regen", kp.poultice_heal, kp.poultice_regen)
        }
        "bloomfield" => format!("+{} Regen per turn to every ally", kp.bloomfield_regen),
        "root_snare" => join(&[
            format!("{} (from Mnd)", dmg(kp.root_snare_mult)),
            turn(kp.root_snare_gauge_drain),
        ]),
        "vital_draught" => {
            format!("{} Barrier and +{} Regen", kp.draught_barrier, kp.draught_regen)
        }
        "thorn_grove" => join(&[
            format!("{} (from Mnd) to EVERY enemy", dmg(kp.thorn_grove_mult)),
            format!("{} each", turn(kp.thorn_grove_gauge_drain)),
        ]),
        "world_tree" => format!(
            "heals {}, {} Barrier and {} Regen a turn to every ally",
            kp.world_tree_heal, kp.world_tree_barrier, kp.world_tree_regen
        ),
        "terras_gift" => format!(
            "heals {}, {} Barrier, and {} of a turn to every ally",
            kp.gift_heal,
            kp.gift_barrier,
            pct(kp.gift_gauge)
        ),

        _ => String::new(),
    }
}

fn focus(body: String) -> String {
    format!("every Psyker turn while held: {body}")
}

fn undead(balance: &Balance) -> String {
    format!("{}× vs undead", trim(balance.battle.phoenix_guard_undead_mult))
}

/// The Resonant's deep kit is one shape with seven sets of numbers, so it gets one
/// formatter rather than seven near-identical arms.
fn boon(heal: f64, regen: i32, barrier: f64, self_cost: f64, party: bool) -> String {
    let who = if party { "every ally" } else { "one ally" };
    let mut parts = Vec::new();
    if heal > 0.0 {
        parts.push(format!("heals {} of {who}'s max HP", pct(heal)));
    }
    if regen > 0 {
        parts.push(format!("+{regen} Regen to {who}"));
    }
    if barrier > 0.0 {
        parts.push(format!("Barrier for {} of max HP", pct(barrier)));
    }
    if self_cost > 0.0 {
        parts.push(format!("costs you {} of the healing", pct(self_cost)));
    }
    join(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ability in the registry gets a number, or the row is back to flavour —
    /// which is the bug this module exists to fix. A new ability with no arm here
    /// fails loudly rather than shipping a blank line.
    #[test]
    fn every_ability_states_its_magnitude() {
        let b = Balance::load_default().unwrap();
        for s in meld_proto::skills::SKILLS {
            let line = effect_line(s.key, &b);
            assert!(!line.is_empty(), "{} ({}) has no numbers", s.name, s.key);
            assert!(
                line.chars().any(|c| c.is_ascii_digit()),
                "{}'s effect line states no magnitude: {line:?}",
                s.name
            );
        }
        // An action that is not a registry ability (Attack/Defend) has nothing to say.
        assert_eq!(effect_line("attack", &b), "");
        assert_eq!(effect_line("nonsense", &b), "");
    }

    /// The Hunter's whole economy was invisible: every skill is refused unless its
    /// cost is banked, so the cost AND the rate a basic Attack banks at have to be
    /// on the row or the class cannot be played by reading it.
    #[test]
    fn a_hunter_row_states_what_it_costs_and_how_you_bank_it() {
        let b = Balance::load_default().unwrap();
        for key in
            ["power_strike", "crushing_blow", "second_wind", "snare", "pin_the_prey", "frenzy"]
        {
            let line = effect_line(key, &b);
            assert!(line.contains("Adrenaline"), "{key}: {line:?}");
            assert!(
                line.contains(&format!("{}", b.battle.hunter_adrenaline_per_attack)),
                "{key} does not say how Adrenaline is banked: {line:?}"
            );
        }
        assert!(b.battle.hunter_frenzy_cost > b.battle.hunter_power_strike_cost);
        assert!(effect_line("frenzy", &b).contains(&format!("{}", b.battle.hunter_frenzy_cost)));
    }

    /// The prose and the numbers are two descriptions of the same resolver, so they
    /// must not contradict each other. This is what the registry had shipped: Sanctuary
    /// promised Barrier and granted Regen, Revitalize advertised "no HP cost to you"
    /// and charged 30% of the heal, Lifewell said "Regen for the WHOLE party" for an
    /// ability that also heals and bills you, and Martyr said "an ally" for a party heal.
    #[test]
    fn the_prose_and_the_numbers_agree() {
        let b = Balance::load_default().unwrap();
        for s in meld_proto::skills::SKILLS {
            let line = effect_line(s.key, &b).to_lowercase();
            let desc = s.description.to_lowercase();
            if line.contains("costs you") {
                assert!(
                    !desc.contains("no hp cost") && !desc.contains("costs you nothing"),
                    "{} charges the caster but says it does not: {:?}",
                    s.name,
                    s.description
                );
                assert!(
                    desc.contains("your own") || desc.contains("cost") || desc.contains("paid"),
                    "{} charges the caster and never mentions it: {:?}",
                    s.name,
                    s.description
                );
            }
            let party_line = line.contains("every ally") || line.contains("every enemy");
            let party_desc =
                desc.contains("party") || desc.contains("every") || desc.contains("all-enemy");
            if party_line {
                assert!(party_desc, "{} lands on everyone but does not say so: {:?}", s.name, s.description);
            }
            if line.contains("one ally") {
                assert!(
                    !desc.contains("whole party") && !desc.contains("every ally"),
                    "{} lands on ONE ally but claims the party: {:?}",
                    s.name,
                    s.description
                );
            }
            // Regen and Barrier are different statuses with different rules; a row that
            // names the wrong one is worse than a row that names neither.
            for word in ["regen", "barrier"] {
                if desc.contains(word) && !line.is_empty() {
                    assert!(
                        line.contains(word) || line.contains("ward"),
                        "{} promises {word} and grants none: {:?}",
                        s.name,
                        line
                    );
                }
            }
        }
    }

    /// The numbers come from balance, not from a second copy of them — retune the
    /// `[TUNABLE]` and the tooltip has to move with it.
    #[test]
    fn the_numbers_are_the_ones_the_resolver_uses() {
        let mut b = Balance::load_default().unwrap();
        b.battle.explorer_trailblaze_mult = 9.75;
        assert!(
            effect_line("trailblaze", &b).contains("9.75×"),
            "{:?}",
            effect_line("trailblaze", &b)
        );
        b.smithwright.temper_atk_bonus = 41;
        assert!(effect_line("tempering_blow", &b).contains("41"));
    }

    #[test]
    fn magnitudes_read_as_a_player_would_say_them() {
        assert_eq!(trim(1.0), "1");
        assert_eq!(trim(1.15), "1.15");
        assert_eq!(trim(3.5), "3.5");
        assert_eq!(pct(0.125), "13%");
        let b = Balance::load_default().unwrap();
        // Ticks are an engine unit; the player is told seconds.
        assert_eq!(secs(60, &b), "6s");
        assert_eq!(secs(100, &b), "10s");
    }

}
