//! Hero names, generated rather than numbered.
//!
//! A hero slot is named once, at registration, and the name is what every later
//! surface shows — the party screen, the battle cell, the roster. "Hero 1" reads as a
//! placeholder the game forgot to fill in, and a player who has to name four heroes
//! before their first dive is being charged admission for a form.
//!
//! Names are DETERMINISTIC in the account seed, so the same account always gets the
//! same four, and re-seeding an existing account is a no-op rather than a reshuffle.
//! They are class-neutral on purpose: a slot's class is chosen per dive and can change,
//! so a name that read as martial would be wrong the moment the slot became a Psyker.

/// The pool. Deliberately plain-spoken and pronounceable rather than high-fantasy —
/// these are working people who go into holes for a living.
const GIVEN: &[&str] = &[
    "Aldren", "Brisa", "Cald", "Danae", "Edrik", "Fenna", "Garrow", "Hesper", "Ivor",
    "Juno", "Kestrel", "Lira", "Marek", "Nessa", "Orin", "Perrin", "Quill", "Rook",
    "Sable", "Tamsin", "Ulric", "Vesna", "Wren", "Yarrow", "Zaid", "Alys", "Bram",
    "Corvin", "Dela", "Emrys", "Fable", "Gideon", "Halix", "Isolde", "Jarek", "Kova",
    "Lonan", "Mireth", "Nyx", "Ostrom", "Piet", "Rhen", "Saskia", "Teller", "Vane",
    "Wick", "Yael", "Zeph",
];

/// A stable 64-bit mix (splitmix64's finaliser) — the same one worldgen uses, so the
/// repo has one hashing idiom rather than two.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Turn any account identifier into a seed. Takes the string so callers do not have to
/// care whether they hold a UUID, a username, or a test fixture.
pub fn seed_of(account: &str) -> u64 {
    account.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

/// The name for `slot` on the account seeded by `seed`.
///
/// Distinct within an account: the pool is walked from the seeded start so two slots
/// never collide, which matters because the party screen lists them side by side.
pub fn hero_name(seed: u64, slot: usize) -> &'static str {
    let n = GIVEN.len();
    // A seeded start plus a seeded, coprime-to-`n` stride visits distinct entries for
    // every slot — picking each slot independently would collide often enough to be
    // noticed (birthday paradox: ~12% across four picks from 48).
    let start = (mix(seed) % n as u64) as usize;
    let stride = 1 + (mix(seed ^ 0xA5A5) % (n as u64 - 1)) as usize;
    let stride = if gcd(stride, n) == 1 { stride } else { 1 };
    GIVEN[(start + slot * stride) % n]
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The whole starting roster for an account, in slot order.
pub fn roster(account: &str, slots: usize) -> Vec<String> {
    let seed = seed_of(account);
    (0..slots).map(|i| hero_name(seed, i).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_always_gets_the_same_names() {
        assert_eq!(roster("don", 4), roster("don", 4));
        assert_ne!(roster("don", 4), roster("someone-else", 4));
    }

    #[test]
    fn the_four_slots_never_share_a_name() {
        // They sit next to each other on the party screen, so a duplicate is not a
        // cosmetic problem — it makes two heroes indistinguishable.
        for who in ["a", "b", "c", "don", "guest01", &"z".repeat(40)] {
            let r = roster(who, 4);
            let mut uniq = r.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), r.len(), "{who} got duplicate hero names: {r:?}");
        }
    }

    #[test]
    fn every_slot_of_a_full_pool_walk_stays_distinct() {
        // The pool is bigger than any party, but the stride has to be coprime with the
        // pool size or a long walk starts repeating early.
        let r = roster("stride-check", GIVEN.len());
        let mut uniq = r.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), GIVEN.len(), "the walk revisited a name");
    }

    #[test]
    fn names_are_real_words_not_placeholders() {
        for n in GIVEN {
            assert!(!n.is_empty() && !n.contains(char::is_numeric), "{n} is a placeholder");
        }
    }
}
