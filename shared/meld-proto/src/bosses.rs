//! The ten NAMED bosses (FS-4) — one registry both sides read.
//!
//! A boss OVERLAYS a host creature: the spawn keeps `monster_kind` (whatever biome
//! creature it rode in on) and carries a separate boss identity that decides its
//! ability pool, its lineage, its sprite set and its title. That identity is what the
//! player is actually walking out there to meet, so it has to reach the client — on the
//! overworld snapshot (`mob:<kind>:<faction>:boss:<key>`) as well as in battle
//! (`boss:<key>` on the combatant's `statuses`).
//!
//! The key/title table lives HERE rather than server-side because the client needs the
//! title to draw a name plate, and a second copy of ten names is a second copy that goes
//! stale. `meld_world::boss_display_name` and `client::world_render::BOSS_KEYS` both read
//! this.

/// Every named boss as `(key, title)`, in tier order: elite (gloamhound, rustfang),
/// miniboss (choirmother, pyrewarden), dungeon (sepulcher, hollowbishop), region
/// (ironmaw, weepingcolossus), biome (miredrowned, ashenleviathan), then the
/// dungeon-only courts ([`DUNGEON_ONLY`]). Each key has a PixelLab sprite set under the
/// client's `assets/bosses/<key>/`.
pub const BOSSES: [(&str, &str); 11] = [
    ("gloamhound", "Gloamhound"),
    ("rustfang", "Rustfang"),
    ("choirmother", "Choirmother"),
    ("pyrewarden", "Pyrewarden"),
    ("sepulcher", "Sepulcher"),
    ("hollowbishop", "Hollow Bishop"),
    ("ironmaw", "Ironmaw"),
    ("weepingcolossus", "Weeping Colossus"),
    ("miredrowned", "Miredrowned"),
    ("ashenleviathan", "Ashen Leviathan"),
    ("briarlord", "The Briar Lord"),
];

/// Bosses that are **sealed behind a dungeon door** and never placed in the open world.
///
/// The distinction is load-bearing rather than flavour: the end fight draws its three
/// peers from "every named boss", so a boss whose whole identity is that you have to go
/// down into its barrow to find it would otherwise turn up standing in a field at d3200.
/// The open-world pools ask [`wanders_the_overworld`]; a dungeon claims its own by name
/// in its `[boss.B1] sprite`.
pub const DUNGEON_ONLY: &[&str] = &["briarlord"];

/// Can this boss be placed in the OPEN WORLD — an elite champion, a Gatekeeper in a
/// pass, an undead rite, a peer at the end fight? False for a dungeon's own boss.
pub fn wanders_the_overworld(key: &str) -> bool {
    !DUNGEON_ONLY.contains(&key)
}

/// The title a named boss is shown under, or `None` for anything that is not one of
/// the ten — a dungeon's authored `sprite`, say, which may be bespoke art with no boss
/// identity behind it. `None` means "draw no name plate" rather than "draw a guess":
/// a plate reading `Unknown Horror` over ordinary scenery is worse than no plate.
pub fn display_name(key: &str) -> Option<&'static str> {
    BOSSES.iter().find(|(k, _)| *k == key).map(|(_, title)| *title)
}

/// Is this one of the ten named bosses?
pub fn is_boss(key: &str) -> bool {
    display_name(key).is_some()
}

/// Every boss key, in tier order.
pub fn keys() -> impl Iterator<Item = &'static str> {
    BOSSES.iter().map(|(k, _)| *k)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key resolves to a title, and no two share either half — the table is the
    /// one place a boss is named, so a copy-paste slip here would show up as two
    /// different bosses wearing one name.
    #[test]
    fn every_boss_has_its_own_key_and_its_own_title() {
        for (key, title) in BOSSES {
            assert_eq!(display_name(key), Some(title));
            assert!(!key.is_empty() && !title.is_empty());
            assert_eq!(BOSSES.iter().filter(|(k, _)| *k == key).count(), 1, "{key} is listed twice");
            assert_eq!(BOSSES.iter().filter(|(_, t)| *t == title).count(), 1, "{title} is used twice");
        }
        assert_eq!(display_name("twingolem"), None, "a bespoke dungeon sprite is not a named boss");
        assert_eq!(display_name(""), None);
    }

    /// A dungeon-only boss has to BE a boss — the flag narrows where it is placed, it
    /// does not make it a second kind of thing with its own half of the registry.
    #[test]
    fn a_dungeon_only_boss_is_still_a_named_boss() {
        for key in DUNGEON_ONLY {
            assert!(is_boss(key), "{key} is dungeon-only but is not in the roster");
            assert!(!wanders_the_overworld(key));
        }
        assert!(wanders_the_overworld("ashenleviathan"));
        assert!(
            keys().any(wanders_the_overworld),
            "every boss is dungeon-only - the overworld pools would be empty"
        );
    }
}
