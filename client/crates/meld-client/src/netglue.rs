//! Net glue: pump server messages into game state/screens, the demo driver,
//! the shared `despawn::<T>` helper, and the UI font install.
//! Extracted from `main.rs` during the module reorg.


use meld_client::net::{ClientCmd, CombatantView, ServerMsg};

use super::*;

pub(crate) fn despawn<T: Component>(mut commands: Commands, q: Query<Entity, With<T>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// The bundled UI font — JetBrainsMono Nerd Font (OFL): a monospace text face with
/// Font-Awesome / Material-Design icons baked in at private-use codepoints, so the
/// HUD renders both Latin text and real icons instead of the ASCII-only default
/// (which drew `⚡`/`◆`/… as tofu boxes). See `assets/fonts/`.
#[derive(Resource)]
pub(crate) struct UiFont(Handle<Font>);

/// The bundled symbol-capable UI face, compiled in. Module scope so a test can ask the same
/// bytes the game installs whether a glyph is really in there — a Nerd Font codepoint the
/// face happens not to carry draws as a tofu box, and nothing at runtime complains.
pub(crate) const UI_FONT_BYTES: &[u8] =
    include_bytes!("../assets/fonts/JetBrainsMonoNerdFont-Regular.ttf");

pub(crate) fn load_ui_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    // Embed the Nerd Font bytes at COMPILE time and register it directly as a Font
    // asset, bypassing the async asset loader. Loading it as a loose/streamed asset was
    // fragile across build paths (loose-disk vs embedded) — when that load silently failed,
    // every text node fell back to Bevy's default face, which has the Latin glyphs but
    // none of the private-use icon codepoints, so all the HUD icons rendered as tofu
    // boxes. Compiling the bytes in makes the symbol-capable font ALWAYS present.
    commands.insert_resource(UiFont(fonts.add(Font::from_bytes(UI_FONT_BYTES.to_vec()))));
}

/// Retro-fit the bundled font onto every text node, so all UI (spawned across many
/// call sites with `TextFont { ..default() }`) picks it up without threading a handle
/// through each one. No call site sets its own font, so patching unconditionally is
/// safe — and it avoids depending on Bevy's internal default-font handle. Idempotent:
/// the id check means an already-patched node is never written again.
pub(crate) fn apply_ui_font(ui: Option<Res<UiFont>>, mut q: Query<&mut TextFont>) {
    let Some(ui) = ui else { return };
    for mut tf in &mut q {
        if tf.font != FontSource::Handle(ui.0.clone()) {
            tf.font = ui.0.clone().into();
        }
    }
}

// --------------------------------------------------------------- net pump --

/// Drain server messages every frame, update resources, drive transitions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pump_net(
    net: NonSend<NetRes>,
    mut session: ResMut<Session>,
    mut world: ResMut<Overworld>,
    mut battle: ResMut<BattleData>,
    mut end: ResMut<EndInfo>,
    mut menu: ResMut<BattleMenu>,
    mut hitfx: ResMut<HitFx>,
    mut inv: ResMut<InventoryData>,
    mut prog: ResMut<ProgressData>,
    mut lobby: ResMut<LobbyData>,
    mut backpack: ResMut<RunBackpack>,
    // Grouped as one tuple param to stay within Bevy's 16-param system limit.
    mut world_res: (
        ResMut<WorldPath>,
        ResMut<WorldFrame>,
        ResMut<Terrain>,
        ResMut<LootReport>,
        ResMut<PerksRes>,
        ResMut<AccountHeroNames>,
        ResMut<LoadoutData>,
        ResMut<RunGearData>,
        ResMut<WorldWeb>,
        ResMut<crate::world_render::DungeonSceneRes>,
        ResMut<VanguardBoardData>,
        ResMut<ShopData>,
        ResMut<Notice>,
        Res<Time>,
        ResMut<CraftData>,
        // Nested so the tuple stays inside Bevy's 16-element system-param limit.
        (
            ResMut<crate::overworld::ExploredMap>,
            ResMut<crate::overworld::StationUi>,
            ResMut<crate::overworld::HeatUi>,
            ResMut<HarvestPops>,
            ResMut<HuntBoardData>,
            ResMut<BountyData>,
            ResMut<crate::ShiftTell>,
        ),
    ),
    mut roster: ResMut<PartyRoster>,
    mut announce: Announce,
    state: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    let (world_path, world_frame, terrain, report, perks, hero_names, loadouts, run_gear, world_web, dungeon_scene, vanguard, shop, notice, clock, craft, (explored, station, heat, pops, hunts, bounties, tell)) = &mut world_res;
    net.0.poll();
    while let Some(msg) = net.0.try_recv() {
        match msg {
            ServerMsg::Backpack { items, chits, gear } => {
                backpack.items = items;
                backpack.chits = chits;
                backpack.gear = gear;
            }
            ServerMsg::Pouches { pouches, capacity } => {
                backpack.pouches = pouches;
                backpack.pouch_capacity = capacity;
            }
            ServerMsg::Party { heroes, synergies, combos, abilities, ability_costs } => {
                roster.heroes = heroes;
                // A party message with an empty roster is a formation/rename echo;
                // don't let it wipe the depth lines the full roster carried.
                if !synergies.is_empty() || !combos.is_empty() || !roster.heroes.is_empty() {
                    roster.synergies = synergies;
                    roster.combos = combos;
                }
                // Same rule for the same reason: a formation/rename echo carries no
                // ability table, and dropping it would blank every tooltip mid-run.
                if !abilities.is_empty() {
                    roster.ability_effects = abilities.into_iter().collect();
                    roster.ability_costs = ability_costs.into_iter().collect();
                }
            }
            ServerMsg::Perks { perks: p } => perks.0 = p,
            ServerMsg::OnboardingStatus { town_seen, run_seen } => {
                announce.tutorial.loaded = true;
                announce.tutorial.town_seen = town_seen;
                announce.tutorial.run_seen = run_seen;
            }
            ServerMsg::Unlocked { newly, owned, party_slots, banner, deepest_ever } => {
                // `owned` is the server's full set every time, so this is a
                // replace, never a merge.
                roster.locked = crate::overlays::locked_roster_lines(&owned);
                roster.party_slots = party_slots;
                announce.unlocks.owned = owned;
                announce.unlocks.party_slots = party_slots;
                announce.unlocks.deepest_ever = deepest_ever;
                if banner {
                    announce.unlocks.pending.extend(newly);
                }
            }
            ServerMsg::LevelUp { new_run_level, heroes, .. } => {
                // Enqueue each leveled hero for the old-school stat screen.
                announce.levelup.run_level = new_run_level;
                announce.levelup.pending.extend(heroes);
            }
            ServerMsg::WorldPath { points } => {
                world_path.points = points.iter().map(|(x, y)| (*x as f32, *y as f32)).collect();
                world_path.drawn = false;
            }
            ServerMsg::WorldWeb { edges } => {
                world_web.edges = edges
                    .iter()
                    .map(|((ax, ay), (bx, by))| ((*ax as f32, *ay as f32), (*bx as f32, *by as f32)))
                    .collect();
                world_web.drawn = false;
            }
            ServerMsg::TerrainSection { section } => {
                // A streamed section extends the clear-path trail (initial-chain
                // sections carry no path — that already rode run.started).
                if !section.path.is_empty() {
                    for (x, y) in &section.path {
                        world_path.points.push((*x as f32, *y as f32));
                    }
                    world_path.drawn = false;
                }
                // Streamed sections carry their own authored mountains, keyed by section so
                // the ground shader + entity Y raise them as the player walks out — and so
                // a RE-sent section (how a Shift retiles the ground) replaces its own
                // mountains rather than growing a second one beside each of the first.
                crate::world_render::set_section_peaks(section.index, &section.peaks);
                // …and its STRAITS (WG-7 continents), keyed the same way and for the same
                // reason. ⚠️ A Shift does NOT re-cut the coastline (a continent does not
                // wander), so the server re-sends a retiled section's straits unchanged —
                // if it ever stops, this drops a sea it is still colliding against.
                crate::world_render::set_section_straits(section.index, &section.straits);
                terrain.sections.insert(section.index, section);
            }
            ServerMsg::DungeonScene { active, theme, floor, width, height } => {
                // DG-6b: flip the client-only dungeon re-skin. Mark dirty only on a real
                // change so the enclosure builder rebuilds on descent / floor-change /
                // exit, not every message.
                let changed = dungeon_scene.active != active
                    || dungeon_scene.theme != theme
                    || dungeon_scene.floor != floor
                    || dungeon_scene.width != width
                    || dungeon_scene.height != height;
                dungeon_scene.active = active;
                dungeon_scene.theme = theme;
                dungeon_scene.floor = floor;
                dungeon_scene.width = width;
                dungeon_scene.height = height;
                dungeon_scene.dirty |= changed;
            }
            ServerMsg::WorldFrame { x_min, x_max, lateral, west_return_border, radial_arc_degrees, seams } => {
                world_frame.have = true;
                world_frame.x_min = x_min as f32;
                world_frame.x_max = x_max as f32;
                world_frame.lateral = lateral as f32;
                world_frame.west_return_border = west_return_border as f32;
                world_frame.radial_arc_degrees = radial_arc_degrees as f32;
                world_frame.seams = seams;
            }
            ServerMsg::Connected { player_id } => {
                session.player_id = player_id;
                // Post-auth home is the hub city (The Last City). From there the player
                // steps through The Threshold to dive (solo or co-op). This is what
                // makes the city the always-there base of the extract-or-die loop.
                session.status = "arrived in The Last City".to_string();
                if *state.get() == Screen::Join {
                    next.set(Screen::City);
                }
            }
            ServerMsg::RunStarted { terrain_off, peaks, straits, world_seed, tutorial } => {
                // Seed this run's terrain BEFORE the ground/entities render, so the shader
                // + every entity Y grow the same per-run-varied hills (no "same hill by the
                // hub every run").
                crate::world_render::set_terrain_offset(terrain_off.0, terrain_off.1);
                // Replace any prior run's mountains with this run's authored peaks (the
                // initial-chain sections' peaks all ride here on run.started).
                crate::world_render::set_peaks(peaks);
                // …and this world's CONTINENTS (WG-7): the straits its ground shader ramps a
                // beach over and its prop placement culls against. The initial chain's all
                // ride here, as the peaks do.
                crate::world_render::set_straits(straits);
                // …and remember which WORLD this is, so the player can read and share its
                // name (CANON D19). Taken from the server rather than from whatever we
                // asked for — see `RunStarted::world_seed`.
                crate::world_render::set_world_seed(world_seed);
                // Fresh dive: drop any terrain from the previous run before the new
                // section stream arrives (server sends them right after this).
                terrain.sections.clear();
                // A new dive is a blank map: the previous run's walk belonged to a
                // world that no longer exists (instances are discarded on close).
                explored.forget();
                // The dive can start from the City (solo, via The Threshold) or
                // the Lobby (co-op).
                lobby.in_lobby = false;
                if matches!(*state.get(), Screen::City | Screen::Lobby) {
                    next.set(Screen::Overworld);
                }
                // Arm the first-dive briefing only once the server has actually
                // confirmed the run — never at the Dive button press, since the
                // server hasn't ruled yet at that point. Optimistic ack: mark it
                // seen and send the C2S now rather than waiting on dismiss, so a
                // disconnect mid-briefing doesn't re-show it every reconnect.
                if announce.tutorial.loaded && !announce.tutorial.run_seen {
                    announce.tutorial.run_seen = true;
                    net.0.send(ClientCmd::OnboardingRunSeen);
                    announce.tutorial.show_run_popup = true;
                }
                // Arm the guided walkthrough from the SERVER's answer about the world we
                // actually landed in — never from our own [T] keypress. The old intent flag
                // was cleared only when a dive started, so a T-press whose `enter_maze` was
                // refused stayed armed and put a walkthrough over the player's next,
                // randomized dive. And the world's tutorial-ness was never the caller's to
                // decide: the flag is set when a world is CREATED, so a normal dive that
                // joins a live tutorial world IS a tutorial run whatever it asked for, and
                // a `[T]` dive into a world that already exists is not one.
                if tutorial {
                    announce.tutorial_run.step = Some(TutorialStep::Harvest);
                    announce.tutorial_run.harvested = false;
                    announce.tutorial_run.chest_opened = false;
                    announce.tutorial_run.battle_intro = None;
                    announce.tutorial_run.chest_explain = false;
                    announce.tutorial_run.chest_explained = false;
                } else {
                    announce.tutorial_run.step = None;
                }
            }
            ServerMsg::LobbyState { code, host, members } => {
                lobby.in_lobby = true;
                lobby.code = code;
                lobby.my_ready = members
                    .iter()
                    .find(|(id, _, _)| id == &session.player_id)
                    .map(|(_, _, r)| *r)
                    .unwrap_or(false);
                lobby.host = host;
                lobby.members = members;
                if matches!(*state.get(), Screen::Join | Screen::City) {
                    next.set(Screen::Lobby);
                }
            }
            ServerMsg::LobbyClosed => {
                lobby.in_lobby = false;
                lobby.members.clear();
                lobby.code.clear();
            }
            ServerMsg::Snapshot { entities } => {
                world.entities.clear();
                for e in entities {
                    world.entities.insert(
                        e.id,
                        OwEntity {
                            x: e.x as f32,
                            y: e.y as f32,
                            kind: e.kind,
                            name: e.monster_kind,
                            faction: e.faction,
                            radius: e.radius as f32,
                            battling: e.battling,
                            clashing: e.clashing,
                            level: e.level,
                            opened: e.opened,
                            mob_level: e.mob_level,
                            hp: e.hp,
                            max_hp: e.max_hp,
                            encounter_class: e.encounter_class,
                            aggression: e.aggression,
                            quarry: e.quarry,
                            expects_parties: e.expects_parties,
                            held: e.held,
                            boss: e.boss,
                            bodies_required: e.bodies_required,
                        },
                    );
                }
                // Mark a fresh snapshot so the interpolation buffer captures it.
                world.seq = world.seq.wrapping_add(1);
            }
            ServerMsg::BattleStarted {
                battle_id,
                your_combatant_id: _,
                your_combatant_ids,
                combatants,
                monster_combatant,
                spectating,
            } => {
                battle.battle_id = battle_id;
                battle.your_ids = your_combatant_ids;
                battle.monster_combatant = monster_combatant;
                battle.combatants = combatants;
                battle.ready.clear();
                battle.queued.clear();
                battle.spectating = spectating;
                battle.active = battle.your_ids.first().cloned();
                reset_menu(&mut menu);
                if *state.get() != Screen::Battle {
                    next.set(Screen::Battle);
                }
            }
            ServerMsg::WatchEnded { battle_id, .. } => {
                // Only ever leaves a WATCHED screen. A fight of our own can start while a
                // feed is still closing (the server sends our `battle.started` first, the
                // sweep drops the watch a tick later) — acting on this unconditionally
                // would walk us straight back out of the fight we were just pulled into.
                if battle.spectating && battle.battle_id == battle_id {
                    battle.spectating = false;
                    battle.combatants.clear();
                    battle.ready.clear();
                    battle.queued.clear();
                    battle.active = None;
                    if *state.get() == Screen::Battle {
                        next.set(Screen::Overworld);
                    }
                }
            }
            ServerMsg::LootPickedUp { items } => {
                // The same banner a chest raises, because it answers the same question.
                // Auto-pickup used to be silent, so a creature that died fighting another
                // creature and left something behind was indistinguishable from one that
                // left nothing.
                report.raise("SPOILS", None, 0, items, Vec::new());
            }
            ServerMsg::TurnReady { combatant_id } => {
                // A hero's gauge filled; it can now act (its queued order fires).
                battle.ready.insert(combatant_id);
            }
            ServerMsg::Telegraph { combatant_id, text } => {
                // A monster shouted a channeled cast (spec §3/§6): flash the
                // bubble for the channel window and put the caster in its
                // charging pose. Bubble TTL ~ the longest telegraph (3 s).
                hitfx.callouts.retain(|c| c.combatant_id != combatant_id);
                hitfx.callouts.push(Callout {
                    combatant_id: combatant_id.clone(),
                    text,
                    age: 0.0,
                    ttl: 3.0,
                    flashing: true,
                });
                hitfx.act_clip.insert(combatant_id, "attack".to_string());
            }
            ServerMsg::ActionResolved {
                actor,
                action,
                callout,
                effects,
            } => {
                // An instant monster ability's shout pops briefly over the
                // arena (telegraphed ones already arrived via `Telegraph`).
                if let Some(text) = callout {
                    hitfx.callouts.retain(|c| c.combatant_id != actor);
                    hitfx.callouts.push(Callout {
                        combatant_id: actor.clone(),
                        text,
                        age: 0.0,
                        ttl: 1.4,
                        flashing: false,
                    });
                }
                // Elemental WEAK!/RESIST!/IMMUNE!/ABSORB! feedback is Psyker
                // threat-sight (spec §6): unlocked when the party's Psyker perk
                // is live, plain numbers otherwise.
                let show_elements = perks.0.hunter_threat > 0;
                let mut did_damage = false;
                for e in effects {
                    // Reflect the authoritative HP immediately + spawn feedback.
                    if let Some(c) = battle.combatants.iter_mut().find(|c| c.id == e.target) {
                        c.hp = e.hp_after;
                    }
                    if e.kind.eq_ignore_ascii_case("damage") && e.amount.unwrap_or(0) > 0 {
                        did_damage = true;
                    }
                    push_hit_fx(&mut hitfx, &e, show_elements);
                }
                // A damaging action makes its actor lunge in to strike.
                if did_damage {
                    hitfx.acts.insert(actor.clone(), 0.0);
                }
                // Pick the sprite clip: the basic `attack`, or the exact skill the
                // client last fired (the wire `action` is only Attack/Skill/…). A
                // non-damaging skill (heal/buff) still plays its clip, just no lunge.
                let clip = match action.as_str() {
                    "attack" => Some("attack".to_string()),
                    "skill" => Some(
                        battle
                            .last_skill
                            .get(&actor)
                            .cloned()
                            .unwrap_or_else(|| "attack".to_string()),
                    ),
                    _ => None,
                };
                if let Some(clip) = clip {
                    hitfx.act_clip.insert(actor, clip);
                }
            }
            ServerMsg::CombatantsJoined { combatants } => {
                for c in combatants {
                    if !battle.combatants.iter().any(|x| x.id == c.id) {
                        battle.combatants.push(c);
                    }
                }
            }
            // CR-11: say what just walked in. The leader's own shout arrived with its
            // resolution; this is the answer to it, over the same body — creatures
            // appearing mid-fight with nothing to explain them reads as the game cheating,
            // which is the same argument the gang-up mark is shouted for.
            ServerMsg::Reinforcements { called_by, arrived } => {
                if arrived > 0 {
                    hitfx.callouts.retain(|c| c.combatant_id != called_by);
                    hitfx.callouts.push(Callout {
                        combatant_id: called_by,
                        text: if arrived == 1 {
                            "THE PACK ANSWERS!".to_string()
                        } else {
                            format!("THE PACK ANSWERS \u{2014} {arrived} MORE!")
                        },
                        age: 0.0,
                        ttl: 2.2,
                        flashing: true,
                    });
                }
            }
            ServerMsg::Gauge { updates } => {
                for (id, gauge, hp, statuses) in updates {
                    if let Some(c) = battle.combatants.iter_mut().find(|c| c.id == id) {
                        c.gauge = gauge;
                        c.hp = hp;
                        c.statuses = statuses;
                    }
                }
            }
            ServerMsg::BattleEnded { outcome, xp, chits, items, gear_drops, worn } => {
                // Victory returns to the overworld (go extract!) and pops up the
                // after-action report; defeat ends the run.
                if outcome == "victory" {
                    // Stay on the battle screen and show the tally THERE; dismissing
                    // it is what walks you back out (`render_loot_report`).
                    report.raise("VICTORY", Some(xp), chits, items, gear_drops);
                    report.worn = worn;
                    report.gate_return = *state.get() == Screen::Battle;
                } else if outcome == "fled" {
                    // Fleeing keeps the run alive — back to the overworld, not the
                    // death screen. The server already charged the toll and mirrored
                    // it into the backpack; here we just surface what it cost. (For
                    // the Fled outcome, `chits`/`items` carry what was DROPPED.)
                    if *state.get() == Screen::Battle {
                        next.set(Screen::Overworld);
                    }
                    let dropped: i32 = items.iter().map(|(_, q)| *q).sum();
                    let mut line = if chits > 0 || dropped > 0 {
                        format!("Fled — dropped {chits} chits, {dropped} item(s)")
                    } else {
                        "Fled the battle".to_string()
                    };
                    // You still paid for whoever went down before you got out.
                    if !worn.is_empty() {
                        let names: Vec<&str> = worn.iter().map(|(n, ..)| n.as_str()).collect();
                        line.push_str(&format!(" - {} fell; kit worn", names.join(", ")));
                        // Fleeing shows no report card, so this line is the ONLY place the
                        // burn would be spoken — and an ephemeral piece lost on the way out
                        // of a fight you ran from is exactly the loss a player would
                        // otherwise discover much later, in a menu, with no explanation.
                        let burned: usize = worn.iter().map(|(.., b)| b.len()).sum();
                        if burned > 0 {
                            line.push_str(&format!(", {burned} ephemeral piece(s) burned"));
                        }
                    }
                    session.status = line;
                } else {
                    end.outcome = outcome;
                    end.banked = 0;
                    end.chits = 0;
                    end.gear = 0;
                    end.worn = worn;
                    next.set(Screen::Ended);
                }
            }
            ServerMsg::ChestOpened { chits, items, gear } => {
                report.raise("TREASURE!", None, chits, items, gear);
                // Order-independent: a curious player can open the chest before
                // harvesting (nothing blocks it, and a chest can't be reopened to
                // fix a missed advance later), so this must not require
                // `harvested` first — only gate on it to decide whether BOTH are
                // now done.
                if announce.tutorial_run.step == Some(TutorialStep::Harvest)
                    && !announce.tutorial_run.chest_opened
                {
                    announce.tutorial_run.chest_opened = true;
                    if announce.tutorial_run.harvested {
                        announce.tutorial_run.arm_fight();
                    }
                }
                // A one-shot explainer, first chest EVER this dive — additive to the
                // loot toast above, not a replacement for it (see `chest_explain_card`).
                // Gated on `chest_explained` (never re-armed), not `chest_explain`
                // (which clears again once dismissed) — otherwise a LATER chest, e.g.
                // the dungeon's own loot chest, would show it a second time.
                if announce.tutorial_run.step.is_some() && !announce.tutorial_run.chest_explained {
                    announce.tutorial_run.chest_explained = true;
                    announce.tutorial_run.chest_explain = true;
                }
            }
            ServerMsg::ChannelStarted { fill_ms, method, .. } => {
                session.channeling = true;
                // The bar restarts from empty on every fill, so the client only needs
                // the fill length and when this one began.
                session.channel_fill_ms = fill_ms;
                session.status = if method.starts_with("harvest") {
                    "gathering...".to_string()
                } else {
                    "extracting...".to_string()
                };
            }
            ServerMsg::ChannelInterrupted => {
                session.channeling = false;
                session.channel_fill_ms = 0;
                session.status = String::new();
            }
            ServerMsg::RunEnded { result, banked, chits, gear } => {
                session.channeling = false;
                end.outcome = result;
                end.banked = banked;
                end.chits = chits;
                end.gear = gear;
                next.set(Screen::Ended);
                // Dying, fleeing, or extracting some other way than the tutorial's
                // own "Go back to town" button must not leave stale walkthrough
                // state armed for the player's next, non-tutorial dive.
                announce.tutorial_run.step = None;
                announce.tutorial_run.battle_intro = None;
                announce.tutorial_run.chest_explain = false;
                announce.tutorial_run.chest_explained = false;
            }
            ServerMsg::InventoryData {
                chits,
                materials,
                gear,
                pending,
            } => {
                inv.chits = chits;
                inv.materials = materials;
                inv.gear = gear;
                inv.pending = pending;
                inv.loaded = true;
            }
            ServerMsg::ProgressData { skills, classes } => {
                prog.skills = skills;
                prog.classes = classes;
                prog.loaded = true;
            }
            ServerMsg::ShopStock { vendor, items } => {
                shop.vendor = vendor;
                shop.items = items;
                shop.loaded = true;
            }
            ServerMsg::GearShopStock { gear } => {
                shop.gear = gear;
            }
            ServerMsg::BrokerQuotes { quotes } => {
                shop.quotes = quotes;
            }
            ServerMsg::Recipes { recipes } => {
                craft.recipes = recipes;
                craft.loaded = true;
                craft.cursor = craft.cursor.min(craft.recipes.len().saturating_sub(1));
            }
            ServerMsg::TempoStarted { job_id, service, strikes, sweep_ms, bands } => {
                heat.job_id = Some(job_id);
                heat.service = service;
                heat.strikes = strikes;
                heat.sweep_ms = sweep_ms;
                heat.bands = bands;
                heat.struck = 0;
                heat.opened_at = clock.elapsed_secs_f64();
            }
            ServerMsg::SmithResult { message, ok, uses_left } => {
                heat.job_id = None;
                // The field bench has no panel of its own to print into, so the smith's
                // answer lands where every other field refusal does — the notice line.
                notice.say(message, clock.elapsed_secs_f64());
                if ok {
                    station.jobs = uses_left.max(0) as u8;
                    if uses_left <= 0 {
                        station.open = None;
                    }
                }
            }
            ServerMsg::Harvested { kind, qty } => {
                pops.banked(&kind, qty);
                // Order-independent (see the matching comment on ChestOpened): a
                // player may open the chest before harvesting, so this only
                // requires this half to advance the shared step.
                if announce.tutorial_run.step == Some(TutorialStep::Harvest)
                    && !announce.tutorial_run.harvested
                {
                    announce.tutorial_run.harvested = true;
                    if announce.tutorial_run.chest_opened {
                        announce.tutorial_run.arm_fight();
                    }
                }
            }
            ServerMsg::VaultNotice { text } => notice.say(text, clock.elapsed_secs_f64()),
            ServerMsg::CraftResult { text } => {
                craft.last = text;
                // The book's `craftable` flags are the server's answer at fetch time, so
                // a craft that changed a level or a stack invalidates them.
                net.0.fetch_recipes();
            }
            ServerMsg::VanguardBoard { season, entries, you } => {
                vanguard.season = season;
                vanguard.entries = entries;
                vanguard.you = you;
                vanguard.loaded = true;
            }
            ServerMsg::Bounties { board } => {
                bounties.rank = board.rank;
                bounties.rank_title = board.rank_title;
                bounties.rank_xp_to_next = board.rank_xp_to_next;
                bounties.active = board.active;
                bounties.history = board.history;
                bounties.loaded = true;
            }
            ServerMsg::HuntBoard { hunts: rows } => {
                let mut rows = rows;
                // Finished work first, then what is still in hand, then what has been paid.
                // Sorted HERE and not in `hunts_view`, because the board's row order IS the
                // claim order: the digit keys and `CounterRowButton` both index straight into
                // this list, so a view that sorted its own rows would claim a different hunt
                // than the one under the number. Stable, so the server's ordering survives
                // inside each group.
                rows.sort_by_key(|h| h.board_order());
                hunts.cursor = hunts.cursor.min(rows.len().saturating_sub(1));
                hunts.hunts = rows;
                hunts.loaded = true;
            }
            ServerMsg::HuntProgress { name, progress, target, complete } => {
                notice.say(
                    if complete {
                        format!("{name} complete - claim it at the Bounty Board")
                    } else {
                        format!("{name}  {progress}/{target}")
                    },
                    clock.elapsed_secs_f64(),
                );
            }
            ServerMsg::ShiftHeld { anchors } => {
                tell.armed = false;
                let lost = anchors.iter().filter(|a| a.destroyed).count();
                let hurt: i32 = anchors.iter().map(|a| a.damage).sum();
                // What it has LEFT is the whole point of BD-3: an anchor is permanence you
                // keep paying for, so the cost has to be legible while it is still standing.
                // The server has always sent this; the decoder used to drop it.
                let left: i32 = anchors.iter().filter(|a| !a.destroyed).map(|a| a.hp).sum();
                let cap: i32 = anchors.iter().filter(|a| !a.destroyed).map(|a| a.max_hp).sum();
                notice.say(
                    if lost > 0 {
                        "The anchor held - and fell. The ground is loose again.".to_string()
                    } else {
                        format!("Your anchor held the Shift  (-{hurt} to it, {left}/{cap} left)")
                    },
                    clock.elapsed_secs_f64(),
                );
            }
            ServerMsg::ShiftWarning { inner_radius, outer_radius, biome, lands_in_ms, caught } => {
                let now = clock.elapsed_secs_f64();
                let secs = (lands_in_ms as f64 / 1000.0).round().max(1.0) as u64;
                tell.inner = inner_radius as f32;
                tell.outer = outer_radius as f32;
                tell.lands_at = now + lands_in_ms as f64 / 1000.0;
                tell.biome = crate::world_render::title_case(&biome);
                tell.caught = caught;
                tell.armed = true;
                notice.say(
                    if caught {
                        format!("THE LAND IS SHIFTING - {} in {secs}s. MOVE.", tell.biome)
                    } else {
                        format!("The land is shifting to {} in {secs}s", tell.biome)
                    },
                    now,
                );
            }
            ServerMsg::PositionCorrection { x, y } => {
                world.snap = Some((x as f32, y as f32));
            }
            ServerMsg::Shifted { biome, from_biome, damage } => {
                tell.armed = false;
                tell.flash_until = clock.elapsed_secs_f64() + crate::SHIFT_FLASH_SECS;
                let hurt: i32 = damage.iter().sum();
                let what = if from_biome.is_empty() {
                    format!("The land shifted to {}", crate::world_render::title_case(&biome))
                } else {
                    format!("{} became {}", crate::world_render::title_case(&from_biome), crate::world_render::title_case(&biome))
                };
                notice.say(
                    if hurt > 0 { format!("{what} - {hurt} Force damage") } else { what },
                    clock.elapsed_secs_f64(),
                );
            }
            ServerMsg::Loadouts { list } => {
                loadouts.list = list;
                loadouts.loaded = true;
            }
            ServerMsg::HeroNames { names, classes } => {
                hero_names.names = names;
                hero_names.classes = classes;
                hero_names.loaded = true;
            }
            ServerMsg::RunGear { gear } => {
                run_gear.gear = gear;
            }
            ServerMsg::Error { message } => {
                // A login/auth error while still on the Join screen unlocks it for
                // another attempt (e.g. wrong password) rather than dead-ending.
                if session.connecting && !session.entered {
                    session.connecting = false;
                    session.status = if message == "wrong-password" {
                        "Wrong password for that account (or choose a new username).".to_string()
                    } else {
                        // Already a clear reason from the API, e.g. "Password must be
                        // 8–128 chars." or "Username must be 3–20 chars of [a-zA-Z0-9_]."
                        message.clone()
                    };
                } else {
                    session.status = format!("error: {message}");
                    // In a run, a refusal has to be VISIBLE: the player pressed a key
                    // and is owed a reason. The server's own wording is the message.
                    notice.say(message.clone(), clock.elapsed_secs_f64());
                }
            }
            ServerMsg::Disconnected => {
                session.status = "disconnected".to_string();
            }
        }
    }
}

/// Offline demo timeline (no networking): walk the overworld, then fight and win.
#[allow(clippy::too_many_arguments)]
pub(crate) fn demo_driver(
    time: Res<Time>,
    mut demo: ResMut<Demo>,
    mut world: ResMut<Overworld>,
    mut battle: ResMut<BattleData>,
    mut end: ResMut<EndInfo>,
    mut session: ResMut<Session>,
    state: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    if !demo.on {
        return;
    }
    demo.t += time.delta_secs();
    let t = demo.t;
    session.player_id = "me".to_string();

    // 0–3s: overworld, hero walking east toward Grendel.
    if t < 3.0 {
        if !demo.started {
            demo.started = true;
            next.set(Screen::Overworld);
        }
        let x = t / 3.0 * 9.0;
        world.entities.clear();
        world.entities.insert("me".to_string(), OwEntity::player(x, 0.0));
        world.entities.insert("grendel".to_string(), OwEntity::monster(10.0, 0.0, "forest_bloom_stalker", "beast"));
        world.entities.insert("portal".to_string(), OwEntity::portal(14.0, 0.0));
        return;
    }

    // 3s+: battle. Grendel's HP falls to 0 over ~5s; gauges animate.
    if *state.get() == Screen::Overworld {
        battle.your_ids = vec!["me".to_string()];
        battle.active = Some("me".to_string());
        battle.monster_combatant = Some("g".to_string());
        battle.combatants = vec![
            CombatantView { id: "me".into(), name: "Hero".into(), hp: 40, max_hp: 40, gauge: 0.0, is_player: true, player_id: Some("me".into()), level: 1, statuses: vec![] },
            CombatantView { id: "g".into(), name: "forest bloom stalker".into(), hp: 60, max_hp: 60, gauge: 0.0, is_player: false, player_id: None, level: 1, statuses: vec![] },
        ];
        next.set(Screen::Battle);
    }
    let phase = t - 3.0;
    let hp = (60.0 * (1.0 - phase / 5.0)).max(0.0) as i32;
    for c in battle.combatants.iter_mut() {
        let p = phase as f64;
        c.gauge = if c.is_player { (p * 0.9) % 1.0 } else { (p * 0.6) % 1.0 };
        if c.id == "g" {
            c.hp = hp;
        }
    }
    if hp <= 0 && *state.get() == Screen::Battle {
        end.outcome = "victory".to_string();
        next.set(Screen::Ended);
    }
}
