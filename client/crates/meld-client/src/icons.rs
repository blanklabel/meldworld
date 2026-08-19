//! One icon per item kind, and one rule for choosing it.
//!
//! **If we drew art for it, show the art.** Every harvestable has a `resource_<kind>.png`
//! prop sprite, and a shrunk copy of the thing you actually pick up out of the world is
//! worth more than any symbol — the icon in the menu and the bush in the field are the same
//! picture. **If we did not, show a Nerd Font glyph for its TYPE**: a sword for a weapon, a
//! shield for an off-hand, a flask for a potion, an ingot for refined stock, a bone for a
//! trophy. Coloured by type as well, so the glyph carries two facts.
//!
//! What it never does is replace the words. An icon narrows the guess; the name and the
//! count are the answer, and both stay on the row.
//!
//! A flat colour-coded chip stood in for the glyphs here for a while, on the theory that a
//! 15px symbol reads as an ambiguous squiggle next to a 24px sprite. It is a real risk, and
//! the answer is size and colour rather than giving up the symbol: a coloured square says
//! only "consumable", where a flask says "potion" to anyone who has seen a potion.

use bevy::prelude::*;

use crate::world_render::WorldAssets;

/// Materials we have node art for. Held against the loaded prop sprites by a test, because
/// this list claiming a sprite that was never loaded shows up as a blank gap in a menu.
pub(crate) const SPRITE_MATERIALS: [&str; 10] = [
    "bloom_herb",
    "heartoak_bark",
    "sun_salts",
    "dune_iron",
    "ember_ash",
    "cinder_ore",
    "frost_lichen",
    "rime_ore",
    "bog_myrrh",
    "peat_iron",
];

/// The prop-sprite key for `kind`, if we drew it.
pub(crate) fn sprite_key(kind: &str) -> Option<String> {
    SPRITE_MATERIALS
        .contains(&kind)
        .then(|| format!("resource_{kind}"))
}

/// The Nerd Font icons this file draws, each named as the font names it.
///
/// Codepoints, not names, are what a `Text` node carries — and this font's Material Design
/// block is shifted from the upstream table, so a hand-copied codepoint lands on a
/// neighbour. It cost a keyboard where the chest armour should have been, and the first
/// version of the test below passed on it: the glyph WAS in the font, it just wasn't the
/// glyph. Every entry is now checked against the face's own glyph name, so the only way to
/// be wrong is to name the wrong thing.
mod nf {
    pub const CASH: (&str, &str) = ("\u{f0114}", "md-cash");
    pub const SCRIPT: (&str, &str) = ("\u{f0bc1}", "md-script");
    pub const FLASK: (&str, &str) = ("\u{f0093}", "md-flask");
    pub const SHIELD: (&str, &str) = ("\u{f0498}", "md-shield");
    pub const LEAF: (&str, &str) = ("\u{f032a}", "md-leaf");
    pub const RUN_FAST: (&str, &str) = ("\u{f046e}", "md-run_fast");
    pub const LIGHTNING: (&str, &str) = ("\u{f140b}", "md-lightning_bolt");
    pub const HEART_PLUS: (&str, &str) = ("\u{f142e}", "md-heart_plus");
    pub const BOOK: (&str, &str) = ("\u{f00bd}", "md-book_open");
    pub const GOLD_BARS: (&str, &str) = ("\u{f124f}", "md-gold");
    pub const BONE: (&str, &str) = ("\u{f00b9}", "md-bone");
    pub const PICKAXE: (&str, &str) = ("\u{f08b7}", "md-pickaxe");
    pub const SWORD: (&str, &str) = ("\u{f04e5}", "md-sword");
    pub const HARD_HAT: (&str, &str) = ("\u{f05b5}", "md-account_hard_hat");
    pub const TSHIRT: (&str, &str) = ("\u{f0a7b}", "md-tshirt_crew");
    pub const BOOT: (&str, &str) = ("\u{f0b47}", "md-shoe_formal");
    pub const RING: (&str, &str) = ("\u{f07eb}", "md-ring");
    pub const SACK: (&str, &str) = ("\u{f0d2e}", "md-sack");
    pub const KIT: (&str, &str) = ("\u{f18be}", "md-shield_sword");
    /// A THROWABLE — the francisca and the fire pot. Codepoint read out of the bundled face
    /// (by glyph name) rather than copied from an upstream table, which is the whole reason
    /// the identity test below exists.
    pub const BOMB: (&str, &str) = ("\u{f0691}", "md-bomb");

    /// Everything the icon table can draw, for the identity test.
    #[cfg(test)]
    pub const ALL: [(&str, &str); 20] = [
        CASH, SCRIPT, FLASK, SHIELD, LEAF, RUN_FAST, LIGHTNING, HEART_PLUS, BOOK, GOLD_BARS,
        BONE, PICKAXE, SWORD, HARD_HAT, TSHIRT, BOOT, RING, SACK, KIT, BOMB,
    ];
}

/// The glyph and colour standing in for a kind we have no art for, chosen by what it IS.
pub(crate) fn glyph(kind: &str) -> (&'static str, Color) {
    use meld_proto::consumables::ConsumableEffect as E;
    use meld_proto::materials::MaterialClass as M;

    let gold = Color::srgb(0.95, 0.82, 0.35);
    let steel = Color::srgb(0.78, 0.84, 0.95);
    let green = Color::srgb(0.52, 0.88, 0.6);
    let bone = Color::srgb(0.86, 0.80, 0.66);
    let arcane = Color::srgb(0.72, 0.58, 0.98);

    if kind == "chits" {
        return (nf::CASH.0, gold);
    }
    if kind == "town_portal" {
        return (nf::SCRIPT.0, arcane);
    }
    if let Some(c) = meld_proto::consumables::consumable(kind) {
        return match c.effect {
            E::Heal | E::FullHeal => (nf::FLASK.0, green),
            E::Barrier => (nf::SHIELD.0, steel),
            E::Regen => (nf::LEAF.0, green),
            E::Evasion => (nf::RUN_FAST.0, green),
            E::Adrenaline => (nf::LIGHTNING.0, Color::srgb(0.95, 0.72, 0.35)),
            E::Revive => (nf::HEART_PLUS.0, gold),
            E::Experience => (nf::BOOK.0, arcane),
            // A cure reads as a cure: one family, or the lot.
            E::Cleanse => (nf::LEAF.0, arcane),
            E::Panacea => (nf::HEART_PLUS.0, arcane),
            // Thrown at the whole encounter — not a bottle you drink, so not a flask.
            E::ThrownAll => (nf::BOMB.0, Color::srgb(0.95, 0.55, 0.3)),
        };
    }
    if let Some(m) = meld_proto::materials::material(kind) {
        return match m.class {
            M::Refined => (nf::GOLD_BARS.0, steel),
            M::Trophy => (nf::BONE.0, bone),
            M::Ore => (nf::PICKAXE.0, Color::srgb(0.72, 0.66, 0.58)),
            M::Reagent => (nf::LEAF.0, green),
        };
    }
    // A gear SLOT rather than an item kind, which is how the party and Vault screens ask.
    match kind {
        "main_hand" => (nf::SWORD.0, steel),
        "off_hand" => (nf::SHIELD.0, steel),
        "head" => (nf::HARD_HAT.0, steel),
        "chest" => (nf::TSHIRT.0, steel),
        "legs" => (nf::BOOT.0, steel),
        "accessory" => (nf::RING.0, arcane),
        // Unknown, and saying so is better than guessing: a plain sack reads as "an item".
        _ => (nf::SACK.0, Color::srgb(0.72, 0.78, 0.9)),
    }
}

/// A kind's own name, from whichever registry owns it. `bog_myrrh` is a key, not a word.
pub(crate) fn display_name(kind: &str) -> String {
    if kind == "chits" {
        return "chits".to_string();
    }
    if let Some(m) = meld_proto::materials::material(kind) {
        return m.name.to_string();
    }
    if let Some(c) = meld_proto::consumables::consumable(kind) {
        return c.name.to_string();
    }
    kind.replace('_', " ")
}

/// Put `kind`'s icon at the head of a row: its sprite if we drew one, else its type glyph.
pub(crate) fn spawn_icon(
    row: &mut ChildSpawnerCommands,
    wa: Option<&WorldAssets>,
    kind: &str,
    px: f32,
) {
    if let Some(tex) = sprite_key(kind).and_then(|k| wa.and_then(|w| w.prop_sprites.get(&k))) {
        row.spawn((
            ImageNode::new(tex.clone()),
            Node { width: Val::Px(px), height: Val::Px(px), ..default() },
        ));
        return;
    }
    let (g, colour) = glyph(kind);
    row.spawn((
        // A fixed-width box so a glyph and a sprite occupy the same slot and the names down
        // a column still line up. Nerd Font icons all advance the same monospace cell, but
        // their INK runs wider than it, so centring the advance is what leaves them looking
        // off — see `battle::status_icon_left` for the same problem with a measured fix.
        Node {
            width: Val::Px(px),
            height: Val::Px(px),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
    ))
    .with_children(|slot| {
        slot.spawn((
            Text::new(g),
            TextFont { font_size: px * 0.82, ..default() },
            TextColor(colour),
        ));
    });
}

/// `<icon> xN Name` — the whole row for one stack, in the order the eye wants it: what it
/// is, how many, what it is called. The count and the name are never dropped for the icon.
pub(crate) fn spawn_stack(
    row: &mut ChildSpawnerCommands,
    wa: Option<&WorldAssets>,
    kind: &str,
    qty: i32,
    px: f32,
) {
    spawn_icon(row, wa, kind, px);
    row.spawn((
        Text::new(format!("x{qty}")),
        TextFont { font_size: px * 0.70, ..default() },
        TextColor(Color::srgb(0.95, 0.85, 0.5)),
    ));
    row.spawn((
        Text::new(display_name(kind)),
        TextFont { font_size: px * 0.66, ..default() },
        TextColor(Color::srgb(0.88, 0.92, 1.0)),
    ));
}

/// A piece of GEAR, in its rarity's colour. The slot is what you would rather show — a sword
/// for a weapon, boots for legs — but a haul only reports names, so this stands for "a piece
/// of kit" and the colour carries the part the player is looking for. Where the slot IS
/// known, ask `spawn_icon` with it instead.
pub(crate) fn spawn_gear_icon(row: &mut ChildSpawnerCommands, rarity: Color, px: f32) {
    row.spawn((
        Node {
            width: Val::Px(px),
            height: Val::Px(px),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
    ))
    .with_children(|slot| {
        slot.spawn((
            Text::new(nf::KIT.0),
            TextFont { font_size: px * 0.82, ..default() },
            TextColor(rarity),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every material claiming a sprite must have one loaded, or the row draws a hole.
    #[test]
    fn a_material_that_claims_art_has_art_loaded() {
        for kind in SPRITE_MATERIALS {
            let key = sprite_key(kind).expect("in the list, so it claims a sprite");
            assert!(
                crate::world_render::PROP_KEYS.contains(&key.as_str()),
                "{kind} claims {key}, which nothing loads"
            );
            assert!(
                meld_proto::materials::is_material(kind),
                "{kind} has art but is not a material anything can hold"
            );
        }
    }

    /// Each codepoint must be the glyph we NAMED, not merely a glyph that exists.
    ///
    /// Checking presence alone is what let a hand-copied codepoint through: this font's
    /// Material Design block is shifted from the upstream table, so `md-tshirt_crew` landed
    /// on a keyboard — present, rendered, and wrong. The face knows its own glyph names, so
    /// ask it.
    #[test]
    fn every_icon_is_the_glyph_it_claims_to_be() {
        let face = ttf_parser::Face::parse(crate::netglue::UI_FONT_BYTES, 0)
            .expect("the bundled UI font parses");
        for (g, name) in nf::ALL {
            let ch = g.chars().next().expect("a glyph is at least one char");
            let gid = face
                .glyph_index(ch)
                .unwrap_or_else(|| panic!("{name} (U+{:X}) is not in the font at all", ch as u32));
            assert_eq!(
                face.glyph_name(gid),
                Some(name),
                "U+{:X} is {:?}, not {name} — the codepoint is off",
                ch as u32,
                face.glyph_name(gid)
            );
        }
    }

    /// Every kind anything can hold resolves to an icon: art if we drew it, else a glyph
    /// from the checked table above. Never nothing.
    #[test]
    fn every_item_kind_gets_exactly_one_icon() {
        let kinds: Vec<String> = meld_proto::materials::MATERIALS
            .iter()
            .map(|m| m.key.to_string())
            .chain(meld_proto::consumables::CONSUMABLES.iter().map(|c| c.key.to_string()))
            .chain(["chits".into(), "town_portal".into()])
            .chain(meld_proto::equipment::SLOTS.iter().map(|s| s.to_string()))
            .collect();
        for kind in &kinds {
            if sprite_key(kind).is_some() {
                continue;
            }
            let (g, _) = glyph(kind);
            assert!(
                nf::ALL.iter().any(|(t, _)| *t == g),
                "{kind} draws {g:?}, which is not in the checked icon table"
            );
        }
    }

    /// The icon narrows the guess; the words are the answer. A stack always says both.
    #[test]
    fn a_stack_always_keeps_its_name_and_its_count() {
        for kind in ["bog_myrrh", "peat_ingot", "bloom_salve", "chits", "main_hand"] {
            let name = display_name(kind);
            assert!(!name.is_empty(), "{kind} has no name to show");
            assert!(!name.contains('_'), "{kind} shows a wire key, not a word: {name}");
        }
    }
}
