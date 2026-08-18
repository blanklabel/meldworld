//! Which lasting conditions are AFFLICTIONS and which are BOONS — one classification both
//! sides read.
//!
//! It decides two rules that pull in opposite directions:
//!
//! - **An affliction does not wear off.** Poison, a web, a mark: these hold until something
//!   removes them, so a party that catches one has to spend a turn on it instead of waiting
//!   out a timer. Outlasting a debuff by standing still is not a decision.
//! - **A boon does.** Haste, a Barrier, Regen — these expire and decay on purpose. A boon that
//!   never faded would make the opening turns of a fight the whole fight, and every buff in
//!   the game a thing you stack once and forget.
//!
//! It lives in `meld-proto` because the client colours a cell by condition and the server
//! decides expiry, and those two disagreeing is how a hero ends up rendered as poisoned by an
//! effect the engine already dropped. The client's palette used to carry its own hard-coded
//! list of names — the same "two lists, one goes stale" trap that shipped an ability menu
//! naming a class's abandoned kit.

/// Conditions that are done TO a fighter and must be cured.
pub const AFFLICTIONS: &[&str] = &[
    // Damage over time.
    "poison", "burn",
    // Slows — every rate cut the engine applies (`status_slow_mult` and the Psyker's tiers).
    "slowed", "web", "chill", "bind",
    // Being easier to hit, or worse at hitting.
    "marked", "distracted", "blinded",
    // Fear and rage: someone else is steering you.
    "dread", "frenzied", "confused",
    // Held still. Its own thing, and the most dangerous: a whole party paralyzed is dead.
    "paralyzed",
];

/// Conditions a fighter WANTS, which fade on purpose.
pub const BOONS: &[&str] = &["hasted", "barrier", "regen", "evasion", "insight"];

/// Whether `name` is an affliction — and therefore whether it needs curing rather than
/// waiting out. Unknown conditions are treated as BOONS, because the failure mode matters:
/// a new boon mistaken for an affliction becomes permanent and breaks every fight, while a
/// new affliction mistaken for a boon merely keeps the old expiring behaviour.
pub fn is_affliction(name: &str) -> bool {
    AFFLICTIONS.contains(&name)
}

pub fn is_boon(name: &str) -> bool {
    BOONS.contains(&name)
}


/// What KIND of affliction something is, and therefore what lifts it.
///
/// A cure answers a condition, not a checklist: a poultice draws venom out and has nothing to
/// say about being blinded, and a draught that cleared everything for the price of a salve
/// would make every affliction in the game a non-event. Only a **Panacea** answers all four,
/// and it is priced like it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Something is in your blood or on your skin, and it is burning through you.
    Venom,
    /// Something is holding you: webbed, chilled, bound, slowed.
    Bindings,
    /// You cannot see straight, or you are lit up for everyone else.
    Senses,
    /// Someone else is steering: dread, or a rage that is not yours.
    Mind,
    /// Everything. A Panacea, and nothing cheaper.
    All,
}

/// Which family an affliction belongs to. `None` for anything that is not an affliction.
pub fn family_of(name: &str) -> Option<Family> {
    Some(match name {
        "poison" | "burn" => Family::Venom,
        // Paralysis is a binding taken to its end — the same answer frees you.
        "slowed" | "web" | "chill" | "bind" | "paralyzed" => Family::Bindings,
        "marked" | "distracted" | "blinded" => Family::Senses,
        // Someone else is steering: afraid, enraged, or turned around.
        "dread" | "frenzied" | "confused" => Family::Mind,
        _ => return None,
    })
}

/// Whether a cure of `family` lifts `name`.
pub fn cures(family: Family, name: &str) -> bool {
    match family_of(name) {
        None => false,
        Some(_) if family == Family::All => true,
        Some(f) => f == family,
    }
}


/// Conditions a **physical blow** knocks out of someone — theirs or yours.
///
/// Being struck brings you round. It is the answer that is not a bottle: a martial party with
/// no mender can still slap a frightened ally back into the fight, and the same is true of the
/// creature you are hitting, so it cuts both ways.
pub const CLEARED_BY_A_HIT: &[&str] = &["dread", "confused"];

/// Conditions that **healing** ends. A frenzy is someone else steering; care takes the wheel
/// back, which is what makes a mender the answer to it rather than a damage race.
pub const CLEARED_BY_HEALING: &[&str] = &["frenzied"];

#[cfg(test)]
mod tests {
    use super::*;

    /// A condition cannot be both, and the two lists are the whole vocabulary — a name in
    /// neither silently keeps the old timer, so this is what makes adding one deliberate.
    #[test]
    fn nothing_is_both_a_curse_and_a_blessing() {
        for a in AFFLICTIONS {
            assert!(!BOONS.contains(a), "{a} is listed as both");
            assert!(is_affliction(a) && !is_boon(a));
        }
        for b in BOONS {
            assert!(is_boon(b) && !is_affliction(b));
        }
    }

    /// Every affliction must be curable BY SOMETHING, or it is a permanent condition wearing
    /// a cure's clothes.
    #[test]
    fn every_affliction_belongs_to_a_family() {
        for a in AFFLICTIONS {
            let f = family_of(a).unwrap_or_else(|| panic!("{a} has no family, so nothing cures it"));
            assert!(cures(f, a), "{a}'s own family does not cure it");
            assert!(cures(Family::All, a), "a Panacea should answer {a}");
            // …and a cure is SPECIFIC: some other family must not lift it.
            let other = [Family::Venom, Family::Bindings, Family::Senses, Family::Mind]
                .into_iter()
                .find(|o| *o != f)
                .unwrap();
            assert!(!cures(other, a), "{a} is lifted by {other:?}, which cures everything");
        }
        assert!(family_of("hasted").is_none(), "a boon should have no cure family");
    }

    /// The safe default is "boon", so an unrecognised condition keeps expiring rather than
    /// becoming a permanent one nobody can remove.
    #[test]
    fn an_unknown_condition_is_not_a_permanent_curse() {
        assert!(!is_affliction("something_new"));
    }
}
