//! Offline mock/demo setups (the `MELD_*` screenshot seeds) + class-flag apply.
//! Extracted from `main.rs` during the module reorg.


use bevy::prelude::*;

use meld_client::net::{CombatantView, GearLine, SkillLine};

use super::*;

/// The classes the party builder cycles through.
pub(crate) const PARTY_CLASSES: [&str; 8] = [
    "explorer",
    "hunter",
    "psyker",
    "resonant",
    "shifter",
    "phoenix_guard",
    "smithwright",
    "keeper",
];

/// Pre-fill the party builder from flags: `?party=` (whole party) wins, else
/// `?class=` sets the lead (slot 0). Both default to the diverse starting party.
pub(crate) fn apply_class_flag(mut session: ResMut<Session>) {
    if let Some(p) = party_flag() {
        let party: Vec<String> = p
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| PARTY_CLASSES.contains(&s.as_str()))
            .collect();
        if !party.is_empty() {
            session.party = party;
            session.party_chosen = true;
            session.party_from_flags = true;
        }
    } else if let Some(c) = class_flag() {
        if let Some(slot0) = session.party.first_mut() {
            *slot0 = c;
        }
        session.party_chosen = true;
        session.party_from_flags = true;
    }
}

/// If the battle-mockup flag is set, seed canned combatants and jump straight to
/// the Battle screen (no networking) so the battle subscreen is viewable on its
/// own. Runs once at startup; a no-op otherwise.
pub(crate) fn mock_battle_setup(
    mut battle: ResMut<BattleData>,
    mut hitfx: ResMut<HitFx>,
    mut target: ResMut<BattleTarget>,
    mut flash: ResMut<AtbFlash>,
    mut roster: ResMut<PartyRoster>,
    mut next: ResMut<NextState<Screen>>,
) {
    if !battle_mockup_flag() {
        return;
    }
    // Ability magnitudes normally arrive on `run.party`, resolved from balance, and
    // there is no server here. Canned so the tooltip's second line is screenshottable;
    // `MELD_BATTLE=skills` opens the page it lives on.
    for (key, effect) in [
        ("trailblaze", "1.15× damage · marked: every ally deals 120% to it for 6s"),
        ("field_dressing", "heals 12% of the target's max HP"),
        ("misdirection", "1.1× damage · 50% of its turn · distracted for 6s"),
        ("stable_ground", "Barrier for 12% of each ally's max HP (decays 2 per turn)"),
    ] {
        roster.ability_effects.insert(key.to_string(), effect.to_string());
    }
    // Seed a fresh ATB-ready flash on h1 so the "turn's up" pop shows statically.
    flash.age.insert("h1".to_string(), 0.06);
    // Freeze mid-impact so the hit flash + recoil (grendel) and the attacker lunge
    // (h1 stepping in) show statically. The fx systems no-op in the mock, so these
    // seeded ages don't advance.
    hitfx.acts.insert("h1".to_string(), 0.11);
    // Canned hit feedback so the floating numbers + flash are visible statically.
    hitfx.items.push(Hit {
        target: "grendel".into(),
        text: "-17".into(),
        color: Color::srgb(1.0, 0.5, 0.4),
        age: 0.06,
        scale: 1.0,
    });
    hitfx.items.push(Hit {
        target: "h3".into(),
        text: "+12".into(),
        color: Color::srgb(0.5, 1.0, 0.6),
        age: 0.0,
        scale: 1.0,
    });
    let hero = |id: &str, hp, gauge, class: &str, back: bool| {
        let mut statuses = vec![format!("class:{class}")];
        if back {
            statuses.push("row:back".into());
        }
        CombatantView {
            id: id.into(),
            name: "Hero".into(),
            hp,
            max_hp: 40,
            gauge,
            is_player: true,
            player_id: Some("me".into()),
            level: 1,
            statuses,
        }
    };
    battle.battle_id = "mock".to_string();
    battle.your_ids = vec!["h1".into(), "h2".into(), "h3".into(), "h4".into()];
    battle.monster_combatant = Some("grendel".to_string());
    battle.active = Some("h1".to_string());
    battle.ready.insert("h1".to_string());
    battle.ready.insert("h3".to_string());
    battle.queued.insert(
        "h2".to_string(),
        Order { kind: QueuedKind::Attack, target: Some("grendel".into()) },
    );
    battle.queued.insert(
        "h4".to_string(),
        Order { kind: QueuedKind::Skill("power_strike"), target: Some("grendel".into()) },
    );
    battle.combatants = vec![
        // A Explorer + Phoenix Guard hold the front; a Psyker + Resonant sit the back row.
        // (The Phoenix Guard makes the TACTICS tap toggle visible for screenshots.)
        hero("h1", 32, 1.0, "explorer", false),
        hero("h2", 40, 0.4, "psyker", true),
        hero("h3", 21, 1.0, "resonant", true),
        hero("h4", 36, 0.75, "phoenix_guard", false),
        CombatantView {
            id: "grendel".into(),
            name: "Grendel".into(),
            hp: 44,
            max_hp: 60,
            gauge: 0.65,
            is_player: false,
            player_id: None,
            level: 1,
            statuses: vec![],
        },
        // A second foe, downed — shows the KO gray-out (death indicator).
        CombatantView {
            id: "stalker".into(),
            name: "Stalker".into(),
            hp: 0,
            max_hp: 30,
            gauge: 0.0,
            is_player: false,
            player_id: None,
            level: 1,
            statuses: vec![],
        },
    ];
    // Seed status effects so the floating status-icon layer (buffs + DoT debuffs,
    // cycling when multiple) can be screenshotted: h1 shielded+regenerating, h3
    // evading, Grendel both poisoned and burning (so its icon alternates).
    let add = |battle: &mut BattleData, id: &str, toks: &[&str]| {
        if let Some(c) = battle.combatants.iter_mut().find(|c| c.id == id) {
            c.statuses.extend(toks.iter().map(|s| s.to_string()));
        }
    };
    add(&mut battle, "h1", &["barrier:8", "regen:3"]);

    add(&mut battle, "h3", &["evasion:20"]);
    // Grendel carries the Explorer's work too, so the new badges are screenshottable:
    // blazed by Trailblaze and distracted by Misdirection (the icon cycles through them).
    add(&mut battle, "grendel", &["poison", "burn", "marked", "distracted"]);
    // `MELD_BATTLE=coop` seeds a few joined allied parties so the surround layout
    // (each player's lineup on its own edge, enemies shrunk in the middle) can be
    // screenshotted. `MELD_BATTLE=1` stays a solo fight.
    if std::env::var("MELD_BATTLE").as_deref() == Ok("coop") {
        let ally = |id: &str, owner: &str, name: &str, class: &str, hp, gauge| CombatantView {
            id: id.into(),
            name: name.into(),
            hp,
            max_hp: 40,
            gauge,
            is_player: true,
            player_id: Some(owner.into()),
            level: 1,
            statuses: vec![format!("class:{class}")],
        };
        battle.combatants.extend([
            ally("a1", "ally_a", "Bram", "explorer", 34, 0.5),
            ally("a2", "ally_a", "Ivo", "psyker", 28, 0.2),
            ally("a3", "ally_a", "Sten", "resonant", 40, 0.9),
            ally("b1", "ally_b", "Wren", "psyker", 22, 0.7),
            ally("b2", "ally_b", "Cael", "explorer", 37, 0.35),
            ally("c1", "ally_c", "Doon", "resonant", 31, 0.6),
            ally("c2", "ally_c", "Fisk", "explorer", 40, 0.15),
        ]);
    }
    // Pre-pick a target so the shimmering reticle is visible in a static screenshot
    // (in play it's set by tapping an enemy — see `battle_click_target`).
    target.selected = Some("grendel".to_string());
    next.set(Screen::Battle);
}

/// If an overlay-mockup flag is set, seed canned inventory/progress data and jump
/// to the overworld with that screen open — so the overlays are viewable on their
/// own without a server.
#[allow(clippy::too_many_arguments)]
/// Seed the extraction tally for a screenshot: a few stacks with real material keys so
/// the icons resolve, plus a piece of gear. Runs every frame under the flag and re-arms
/// itself, because the real tally rolls off on a timer — which is exactly long enough to
/// be gone by the time a capture lands.
pub(crate) fn mock_tally_setup(mut report: ResMut<LootReport>) {
    if !crate::flags::tally_preview_flag() {
        return;
    }
    if report.active && report.elapsed < 1.0 {
        return;
    }
    *report = LootReport {
        active: true,
        title: "Extracted".to_string(),
        xp: Some(248),
        chits: 137,
        items: vec![
            ("bloom_herb".to_string(), 7),
            ("heartoak_bark".to_string(), 4),
            ("myconid_cap".to_string(), 2),
            ("town_portal".to_string(), 1),
        ],
        gear: vec!["Rare Dune Ingot Blade".to_string()],
        elapsed: 0.0,
        gate_return: false,
    };
}

pub(crate) fn mock_overlay_setup(
    mut overlay: ResMut<Overlay>,
    mut tab: ResMut<OverlayTab>,
    mut inv: ResMut<InventoryData>,
    mut prog: ResMut<ProgressData>,
    mut world: ResMut<Overworld>,
    mut levelup: ResMut<LevelUpQueue>,
    mut roster: ResMut<PartyRoster>,
    mut stats: ResMut<RunStats>,
    mut backpack: ResMut<RunBackpack>,
    mut picker: ResMut<EquipPicker>,
    mut unlocks: ResMut<UnlocksRes>,
    mut main_menu: ResMut<MainMenu>,
    mut next: ResMut<NextState<Screen>>,
) {
    // Open the cascade at a given depth for screenshots.
    if let Some(spec) = menu_flag() {
        let (section, pane) = match spec.as_str() {
            "items" => (Some(MenuSection::Items), None),
            "materials" => (Some(MenuSection::Materials), None),
            "map" => (Some(MenuSection::Map), None),
            "guide" => (Some(MenuSection::Guide), None),
            "party.abilities" => (Some(MenuSection::Party), Some(MenuPane::Abilities)),
            "party.equipment" => (Some(MenuSection::Party), Some(MenuPane::Equipment)),
            _ => (Some(MenuSection::Party), None),
        };
        main_menu.section = section;
        main_menu.pane = pane;
    }
    // CL-1: queue an unlock banner with no server behind it, and show the roster
    // with only what a fresh account owns, so both surfaces are screenshottable.
    if let Some(key) = unlock_mockup_flag() {
        if let Some(def) = meld_proto::unlocks::unlock(&key) {
            let owned: Vec<String> = meld_proto::unlocks::starting_unlocks()
                .iter()
                .map(|s| s.to_string())
                .collect();
            unlocks.pending.push_back(crate::net::UnlockLine {
                key: def.key.to_string(),
                name: def.name.to_string(),
                kind: match def.kind {
                    meld_proto::unlocks::UnlockKind::PartySlot(_) => "party_slot".into(),
                    meld_proto::unlocks::UnlockKind::Class(_) => "class".into(),
                },
                class_key: match def.kind {
                    meld_proto::unlocks::UnlockKind::Class(c) => {
                        Some(meld_proto::equipment::class_key(c).to_string())
                    }
                    _ => None,
                },
                slot: match def.kind {
                    meld_proto::unlocks::UnlockKind::PartySlot(n) => Some(n),
                    _ => None,
                },
                trigger_text: def.trigger_text.to_string(),
                banner: def.banner.to_string(),
            });
            roster.locked = crate::overlays::locked_roster_lines(&owned);
            roster.party_slots = meld_proto::unlocks::party_slots(&owned);
            unlocks.owned = owned;
            unlocks.hold = true;
        }
    }
    if inventory_mockup_flag() {
        inv.loaded = true;
        // AD-2 depth lines, so the party screen's synergy/combo block is visible in
        // mock frames the way the server would describe it.
        roster.synergies = vec![meld_client::net::DepthLine {
            name: "Blood and Balm".into(),
            detail: "every hero gains 3 Regen".into(),
            description: "A kit that pays in blood pairs with one that gives it back.".into(),
            bonus_pct: 0,
        }];
        roster.combos = vec![meld_client::net::DepthLine {
            name: "Cut the Snare".into(),
            detail: "Snare (Hunter) then Backstab (Shifter)".into(),
            description: "Snare a foe, then Backstab it.".into(),
            bonus_pct: 60,
        }];
        // Seed a party roster so the party screen (+ formation toggle) is visible.
        let level = crate::flags::hero_level_flag();
        let hero = |name: &str, class: &str, back_row| meld_client::net::HeroLine {
            name: name.into(),
            class_key: class.into(),
            level,
            str_: 24,
            mnd: 4,
            dex: 12,
            wll: 20,
            max_hp: 40,
            hp: 40,
            xp: 0,
            xp_to_next: 80,
            back_row,
        };
        roster.heroes = vec![
            hero("Rurik", "explorer", false),
            hero("Ivo", "psyker", true),
            hero("Sten", "resonant", true),
            hero("Bram", "explorer", false),
        ];
        inv.chits = 1240;
        // Real material keys (match `resource_<key>.png`) so the Items tab shows
        // their harvest-node sprites; `bloom_salve` has no node art and exercises
        // the glyph fallback.
        inv.materials = vec![
            ("bloom_herb".into(), 7),
            ("heartoak_bark".into(), 3),
            ("dune_iron".into(), 5),
            ("bloom_salve".into(), 2),
        ];
        inv.pending = vec![("bloom_herb".into(), 2)];
        inv.gear = vec![
            // A heavy chest piece the demo's non-Iron-Hull lead cannot wear, so the
            // Equip tab shows a class-blocked row (GR-5) in demo frames.
            GearLine {
                gear_id: "mock-unique".into(),
                name: "Reaver's Edge".into(),
                slot: "chest".into(),
                class_key: String::new(),
                insurance: "insured".into(),
                family: String::new(),
                armor_weight: "medium".into(),
                affixes: vec![meld_proto::affixes::Affix {
                    key: "atk".into(),
                    magnitude: 22,
                    element: None,
                    ally_class: None,
                }],
                unique_key: "reaver_edge".into(),
                set_key: String::new(),
                tier: 11,
                equipped_hero_slot: None,
                max_durability: 100,
                base_max_durability: 100,
                atk_bonus: 0,
                def_bonus: 0,
                spd_bonus: 0,
                reroll_cost: 3,
            },
            GearLine {
                gear_id: "mock-legendary".into(),
                name: "Legendary Warblade of the Bulwark".into(),
                slot: "chest".into(),
                class_key: String::new(),
                insurance: "insured".into(),
                family: String::new(),
                armor_weight: "medium".into(),
                affixes: vec![
                    meld_proto::affixes::Affix {
                        key: "barrier".into(),
                        magnitude: 14,
                        element: None,
                        ally_class: None,
                    },
                    meld_proto::affixes::Affix {
                        key: "resist".into(),
                        magnitude: 24,
                        element: Some("FIRE".into()),
                        ally_class: None,
                    },
                    meld_proto::affixes::Affix {
                        key: "ally_atk".into(),
                        magnitude: 5,
                        element: None,
                        ally_class: Some("resonant".into()),
                    },
                ],
                unique_key: String::new(),
                set_key: "kiln_chorus".into(),
                tier: 9,
                equipped_hero_slot: None,
                max_durability: 90,
                base_max_durability: 90,
                atk_bonus: 0,
                def_bonus: 11,
                spd_bonus: 0,
                reroll_cost: 3,
            },
            GearLine {
                gear_id: "mock-plate".into(),
                name: "Bulwark Plate".into(),
                slot: "chest".into(),
                class_key: String::new(),
                insurance: "insured".into(),
                family: String::new(),
                armor_weight: "heavy".into(),
                affixes: Vec::new(),
                unique_key: String::new(),
                set_key: String::new(),
                tier: 3,
                equipped_hero_slot: None,
                max_durability: 70,
                base_max_durability: 70,
                atk_bonus: 0,
                def_bonus: 6,
                spd_bonus: 0,
                reroll_cost: 3,
            },
            GearLine {
                gear_id: "mock-weapon".into(),
                name: "Chipped Blade".into(),
                slot: "main_hand".into(),
                class_key: String::new(),
                insurance: "insured".into(),
                family: "sword".into(),
                armor_weight: String::new(),
                affixes: Vec::new(),
                unique_key: String::new(),
                set_key: String::new(),
                tier: 0,
                equipped_hero_slot: Some(0),
                max_durability: 90,
                base_max_durability: 100,
                atk_bonus: 3,
                def_bonus: 0,
                spd_bonus: 0,
                reroll_cost: 3,
            },
            GearLine {
                gear_id: "mock-accessory".into(),
                name: "Duneglass Charm".into(),
                slot: "accessory".into(),
                class_key: "explorer".into(),
                insurance: "ephemeral".into(),
                family: String::new(),
                armor_weight: String::new(),
                affixes: Vec::new(),
                unique_key: String::new(),
                set_key: String::new(),
                tier: 3,
                equipped_hero_slot: None,
                max_durability: 60,
                base_max_durability: 60,
                atk_bonus: 0,
                def_bonus: 0,
                spd_bonus: 1,
                reroll_cost: 9,
            },
        ];
        // Seed the run readouts that moved off the HUD into the Status tab, and open
        // there so `MELD_INVENTORY` screenshots the distance/biome/backpack-in-menu.
        stats.distance = 342;
        stats.tier = 3;
        stats.biome = "Ashfall".into();
        backpack.items = vec![
            ("town_portal".into(), 2),
            ("bloom_herb".into(), 7),
            ("cinder_ore".into(), 4),
        ];
        backpack.chits = 1240;
        backpack.gear = vec![("Duneglass Charm".into(), 0)];
        overlay.kind = Some(OverlayKind::Inventory);
        *tab = OverlayTab::Status;
        // `MELD_INVENTORY_TAB=equip` lands on the Equip tab with a category picker
        // already open, which is the only place gear rows (and their class-block
        // labels) render.
        match inventory_tab_flag().as_deref() {
            Some("equip") => {
                *tab = OverlayTab::Equip;
                picker.category = Some("chest");
            }
            Some("items") => *tab = OverlayTab::Items,
            _ => {}
        }
    } else if levelup_mockup_flag() {
        prog.loaded = true;
        prog.skills = vec![
            SkillLine { kind: "alchemy".into(), level: 3, xp: 245 },
            SkillLine { kind: "forging".into(), level: 2, xp: 130 },
            SkillLine { kind: "gatekeeping".into(), level: 1, xp: 20 },
        ];
        prog.classes = vec!["explorer".into(), "dragoon".into()];
        overlay.kind = Some(OverlayKind::LevelUp);
    } else if levelup_anim_mockup_flag() {
        use meld_client::net::HeroLevelUpLine;
        levelup.run_level = 5;
        levelup.hold = true; // demo: hold each hero on screen for screenshots
        levelup.pending.extend([
            HeroLevelUpLine {
                name: "Rurik".into(),
                class_key: "explorer".into(),
                level: 5,
                max_hp: (52, 62),
                str_: (24, 27),
                mnd: (4, 4),
                dex: (12, 13),
                wll: (20, 22),
            },
            HeroLevelUpLine {
                name: "Yselle".into(),
                class_key: "psyker".into(),
                level: 5,
                max_hp: (42, 46),
                str_: (6, 6),
                mnd: (32, 36),
                dex: (14, 15),
                wll: (12, 13),
            },
        ]);
    } else {
        return;
    }
    // A minimal overworld behind the overlay.
    world.entities.insert("me".into(), OwEntity::player(0.0, 0.0));
    world.entities.insert("grendel".into(), OwEntity::monster(10.0, 0.0, "forest_bloom_stalker", "beast"));
    world.entities.insert("portal".into(), OwEntity::portal(14.0, 0.0));
    next.set(Screen::Overworld);
}
