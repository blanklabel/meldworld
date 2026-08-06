//! Party synergies and ability combos (roadmap AD-2,
//! docs/proposals/adventure-depth.md Part C).
//!
//! Builds in MELDWORLD come from *composition*, not a stat sheet, so the depth has
//! to live in explicit interactions the player can see and assemble. Two kinds:
//!
//! - **Class-pair synergies** — passive, always-on while both classes are in the
//!   party (Phoenix Guard + Psyker stack into a fortress front; Resonant + Explorer
//!   turn a self-damaging kit into sustain; Shifter covers a fragile back line).
//!   Applied at battle assembly.
//! - **Combos** — *sequenced*: one hero's ability primes a target, and a specific
//!   follow-up cashes it in for amplified damage inside a short window. Snare the
//!   thing, then Backstab it. This is the layer that makes a turn order a
//!   decision rather than four independent menus, and it is deliberately
//!   cross-class: three of the four combos need two different heroes.
//!
//! Both are content and live here; magnitudes and the combo window are
//! `[adventure]` `[TUNABLE]`s.

use crate::enums::CharacterClass;

/// A sequenced ability combo: `setup` primes the target, `payoff` cashes it in
/// while the primer is live.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComboDef {
    pub key: &'static str,
    pub name: &'static str,
    /// The ability that primes the target.
    pub setup: &'static str,
    /// The class that performs the setup (for the party-screen description).
    pub setup_class: CharacterClass,
    /// The ability that cashes the primer in.
    pub payoff: &'static str,
    pub payoff_class: CharacterClass,
    /// Damage multiplier applied to the payoff when it lands on a primed target.
    pub damage_mult: f64,
    pub description: &'static str,
}

/// Every combo in the game. Ability keys match the skill kinds the battle engine
/// resolves, so a typo here is a combo that silently never fires — hence the test
/// that cross-checks them against the skills registry.
pub const COMBOS: &[ComboDef] = &[
    ComboDef {
        key: "cut_the_snare",
        name: "Cut the Snare",
        setup: "snare",
        setup_class: CharacterClass::Hunter,
        payoff: "backstab",
        payoff_class: CharacterClass::Shifter,
        damage_mult: 1.6,
        description: "Snare a foe, then Backstab it: the blade finds what cannot move.",
    },
    ComboDef {
        key: "crush_the_pinned",
        name: "Crush the Pinned",
        setup: "gravity_well",
        setup_class: CharacterClass::Psyker,
        payoff: "holy_censure",
        payoff_class: CharacterClass::PhoenixGuard,
        damage_mult: 1.5,
        description: "A foe held in a Gravity Well has nowhere to go when the censure lands.",
    },
    ComboDef {
        key: "follow_the_stagger",
        name: "Follow the Stagger",
        setup: "silvered_strike",
        setup_class: CharacterClass::PhoenixGuard,
        payoff: "frenzy",
        payoff_class: CharacterClass::Hunter,
        damage_mult: 1.5,
        description: "A Silvered Strike staggers; Frenzy arrives before it recovers.",
    },
    ComboDef {
        key: "press_the_slowed",
        name: "Press the Slowed",
        setup: "ransack",
        setup_class: CharacterClass::Shifter,
        payoff: "power_strike",
        payoff_class: CharacterClass::Hunter,
        damage_mult: 1.4,
        description: "Ransack robs a foe of its tempo. Power Strike takes the turn it lost.",
    },
];

/// Display name of a class, for a combo's sequence line.
pub fn pretty_class_name(class: CharacterClass) -> String {
    crate::affixes::pretty_class(crate::equipment::class_key(class))
}

/// The combo a given ability sets up, if any.
pub fn combo_for_setup(ability: &str) -> Option<&'static ComboDef> {
    COMBOS.iter().find(|c| c.setup == ability)
}

/// The combo a given ability cashes in, if any.
pub fn combo_for_payoff(ability: &str) -> Option<&'static ComboDef> {
    COMBOS.iter().find(|c| c.payoff == ability)
}

pub fn combo(key: &str) -> Option<&'static ComboDef> {
    COMBOS.iter().find(|c| c.key == key)
}

/// The timed-status token a primed target carries. Rides the existing
/// `timed_statuses` mechanism, so no new per-combatant state was needed.
pub fn primer_status(combo_key: &str) -> String {
    format!("primed:{combo_key}")
}

/// The combo key a primer token names, if it is one.
pub fn combo_from_primer(status: &str) -> Option<&'static ComboDef> {
    status.strip_prefix("primed:").and_then(combo)
}

/// What a class-pair synergy grants the party while both classes are present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynergyEffect {
    /// Every hero starts the fight holding this much Barrier.
    PartyBarrier,
    /// Every hero gains this much Regen.
    PartyRegen,
    /// Back-row heroes gain this much Evasion (percentage points).
    BackRowEvasion,
}

/// A passive interaction between two classes in the same party.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynergyDef {
    pub key: &'static str,
    pub name: &'static str,
    pub a: CharacterClass,
    pub b: CharacterClass,
    pub effect: SynergyEffect,
    pub description: &'static str,
}

pub const SYNERGIES: &[SynergyDef] = &[
    SynergyDef {
        key: "fortress_front",
        name: "Fortress Front",
        a: CharacterClass::PhoenixGuard,
        b: CharacterClass::Psyker,
        effect: SynergyEffect::PartyBarrier,
        description: "The hull holds the line while the Psyker plates it: the party opens each fight warded.",
    },
    SynergyDef {
        key: "blood_and_balm",
        name: "Blood and Balm",
        a: CharacterClass::Resonant,
        b: CharacterClass::Hunter,
        effect: SynergyEffect::PartyRegen,
        description: "A kit that pays in blood pairs with one that gives it back.",
    },
    SynergyDef {
        key: "covering_blink",
        name: "Covering Blink",
        a: CharacterClass::Shifter,
        b: CharacterClass::Resonant,
        effect: SynergyEffect::BackRowEvasion,
        description: "The Shifter's blink covers the back line; what it cannot stop, it moves out from under.",
    },
];

/// Which class-pair synergies a party composition activates. Both halves must be
/// present, and a pair of the *same* class never counts as a pair with itself.
pub fn active_synergies(classes: &[CharacterClass]) -> Vec<&'static SynergyDef> {
    SYNERGIES
        .iter()
        .filter(|s| classes.contains(&s.a) && classes.contains(&s.b))
        .collect()
}

/// The combos a party composition can actually perform: both the setup class and
/// the payoff class have to be in it. This is what the party screen shows so a
/// player can see the sequences their comp unlocks (and what a swap would cost).
pub fn available_combos(classes: &[CharacterClass]) -> Vec<&'static ComboDef> {
    COMBOS
        .iter()
        .filter(|c| classes.contains(&c.setup_class) && classes.contains(&c.payoff_class))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use CharacterClass::*;

    #[test]
    fn every_combo_names_abilities_the_engine_actually_has() {
        for c in COMBOS {
            assert!(
                crate::skills::is_hero_skill(c.setup),
                "{}: unknown setup ability {}",
                c.key,
                c.setup
            );
            assert!(
                crate::skills::is_hero_skill(c.payoff),
                "{}: unknown payoff ability {}",
                c.key,
                c.payoff
            );
            // The class a combo names must be the class that actually owns the
            // ability, or the party screen would promise a sequence no comp can run.
            assert_eq!(
                crate::skills::skill_owner(c.setup),
                Some(crate::equipment::class_key(c.setup_class)),
                "{}: setup class does not own {}",
                c.key,
                c.setup
            );
            assert_eq!(
                crate::skills::skill_owner(c.payoff),
                Some(crate::equipment::class_key(c.payoff_class)),
                "{}: payoff class does not own {}",
                c.key,
                c.payoff
            );
            assert!(c.damage_mult > 1.0, "{} does not reward the sequence", c.key);
            assert!(!c.description.is_empty());
            assert_eq!(combo(c.key), Some(c));
            assert_eq!(combo_for_setup(c.setup), Some(c));
            assert_eq!(combo_for_payoff(c.payoff), Some(c));
        }
    }

    #[test]
    fn combos_are_mostly_cross_class_because_that_is_the_point() {
        let cross = COMBOS.iter().filter(|c| c.setup_class != c.payoff_class).count();
        assert!(
            cross >= 3,
            "combos should make a PARTY sequence, not a solo rotation: {cross} of {}",
            COMBOS.len()
        );
    }

    #[test]
    fn a_primer_round_trips_through_its_status_token() {
        for c in COMBOS {
            let token = primer_status(c.key);
            assert_eq!(combo_from_primer(&token), Some(c));
        }
        assert!(combo_from_primer("barrier").is_none());
        assert!(combo_from_primer("primed:nonsense").is_none());
    }

    #[test]
    fn a_comp_only_gets_the_synergies_and_combos_it_can_field() {
        let default = [Hunter, Psyker, Resonant, Hunter];
        let names: Vec<&str> = active_synergies(&default).iter().map(|s| s.name).collect();
        assert!(names.contains(&"Blood and Balm"), "{names:?}");
        assert!(!names.contains(&"Fortress Front"), "{names:?}");
        assert!(!names.contains(&"Covering Blink"), "{names:?}");

        let tanky = [PhoenixGuard, Psyker, Resonant, Hunter];
        let names: Vec<&str> = active_synergies(&tanky).iter().map(|s| s.name).collect();
        assert!(names.contains(&"Fortress Front"), "{names:?}");

        let combos: Vec<&str> = available_combos(&default).iter().map(|c| c.name).collect();
        assert!(!combos.contains(&"Cut the Snare"), "{combos:?}");
        let rogueish = [Hunter, Shifter, Resonant, Psyker];
        let combos: Vec<&str> = available_combos(&rogueish).iter().map(|c| c.name).collect();
        assert!(combos.contains(&"Cut the Snare"), "{combos:?}");
        assert!(combos.contains(&"Press the Slowed"), "{combos:?}");
    }

    #[test]
    fn a_solo_class_party_fields_nothing() {
        let mono = [Hunter, Hunter, Hunter, Hunter];
        assert!(active_synergies(&mono).is_empty());
        assert!(available_combos(&mono).is_empty(), "a mono party has no cross-class sequence");
    }
}
