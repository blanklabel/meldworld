//! The `[T]` guided dive's pre-dive flow: a short welcome card, then a real
//! 4-class picker. Unlike the account-level town tour (`tutorial::render_town_tour`,
//! shown once ever) or the in-run walkthrough (`tutorial::*`, shown after the
//! dive has already begun), this is City-scoped and runs BEFORE
//! `ClientCmd::EnterMaze` is ever sent — pressing `[T]` now opens this instead
//! of diving directly (see `city_input`), and the dive only actually starts
//! once exactly 4 classes are confirmed.

use meld_client::glass;
use meld_client::net::ClientCmd;

use super::*;

/// City-scoped, pre-dive state for the welcome + 4-class picker. Cleared the
/// moment the dive is confirmed or skipped — never persisted, and (like a
/// normal dive's own party choice) never touches the account's real
/// party/unlocks.
#[derive(Resource, Default)]
pub(crate) struct TutorialPreDive {
    pub(crate) stage: Option<PreDiveStage>,
    picks: Vec<&'static str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreDiveStage {
    Welcome,
    Picking,
}

#[derive(Component)]
pub(crate) struct TutorialPreDiveRoot;
#[derive(Component)]
pub(crate) struct PreDiveAdvanceBtn;
#[derive(Component)]
pub(crate) struct PreDiveSkipBtn;
#[derive(Component)]
pub(crate) struct PreDiveClassChip(&'static str);

/// Each live class's overworld passive, in the player's own words — the
/// battle-role half already lives in `screens::CLASS_INFO`; this is the new
/// half. Written from the actual mechanics (`compute_perks`, server's
/// `game.rs`), not invented lore.
fn overworld_passive_blurb(key: &str) -> &'static str {
    match key {
        "explorer" => "Overworld: a corner minimap reveals monsters and the way out, then chests, then resources at range — plus a brighter lamp at night.",
        "hunter" => "Overworld: shows a monster's level over its head (then its HP, then more), flags dangerous ones, and spots monsters from farther away.",
        "psyker" => "Overworld: can tap a monster to pin it in place from range, and keeps co-op teammates visible however far off they wander.",
        "resonant" => "Overworld: regenerates its own HP passively just from walking around, no potions needed.",
        "shifter" => "Overworld: senses dungeon entrances and armed traps from range before you'd otherwise see them.",
        "phoenix_guard" => "Overworld: monsters notice the party from a shorter range — a calmer walk with one along.",
        "smithwright" => "Overworld: senses ore veins from farther away than anyone else can.",
        "keeper" => "Overworld: senses reagent beds from farther away than anyone else can.",
        "iron_hull" => "Overworld: shrinks monster aggro range and spots monsters from farther away, both at once.",
        "rift_knight" => "Overworld: can step off ledges directly instead of walking to a connector, and pulls in ground loot from range.",
        _ => "",
    }
}

/// Immediate-mode despawn/rebuild on stage/pick changes, mirroring
/// `tutorial::render_town_tour`'s own style.
pub(crate) fn render_tutorial_predive(
    mut commands: Commands,
    predive: Res<TutorialPreDive>,
    root_q: Query<Entity, With<TutorialPreDiveRoot>>,
) {
    let Some(stage) = predive.stage else {
        for e in &root_q {
            commands.entity(e).despawn();
        }
        return;
    };
    for e in &root_q {
        commands.entity(e).despawn();
    }
    commands
        .spawn((TutorialPreDiveRoot, GlobalZIndex(1000), glass::scrim()))
        .with_children(|root| match stage {
            PreDiveStage::Welcome => {
                root.spawn(glass::panel_capped(Val::Percent(90.0), Val::Px(460.0)))
                    .with_children(spawn_predive_welcome);
            }
            PreDiveStage::Picking => {
                root.spawn(glass::panel_capped(Val::Percent(94.0), Val::Px(720.0)))
                    .with_children(|p| spawn_predive_picker(p, &predive.picks));
            }
        });
}

fn spawn_predive_welcome(p: &mut ChildSpawnerCommands) {
    p.spawn(glass::text("Guided Practice Dive", 20.0, glass::TITLE));
    p.spawn(glass::divider());
    p.spawn(glass::text(
        "This is a real dive, but a gentle one. Next you'll pick 4 classes to field together just for this run — nothing here touches your real account or party.",
        14.0,
        glass::TEXT,
    ));
    spawn_predive_nav(p, "Continue", true);
}

fn spawn_predive_picker(p: &mut ChildSpawnerCommands, picks: &[&'static str]) {
    p.spawn(glass::text("Choose 4 Classes", 20.0, glass::TITLE));
    p.spawn(glass::divider());
    p.spawn(glass::text(format!("{} of 4 selected", picks.len()), 13.0, glass::DIM));
    // No scroll-wheel wiring exists anywhere in this client (`Overflow::scroll_y`
    // only ever clips, it doesn't add an interactive scrollbar) — so rather than
    // relying on scrolling that doesn't actually work, this fits all 10 classes
    // in a fixed 2-column grid, compact enough to stay within `panel_capped`'s
    // own 92vh cap on ordinary window sizes without needing to clip at all.
    p.spawn(Node {
        display: Display::Grid,
        grid_template_columns: RepeatedGridTrack::flex(2, 1.0),
        column_gap: Val::Px(10.0),
        row_gap: Val::Px(8.0),
        margin: UiRect::vertical(Val::Px(6.0)),
        ..default()
    })
    .with_children(|grid| {
        for c in crate::screens::CLASS_INFO.iter() {
            let on = picks.contains(&c.key);
            grid.spawn((
                Button,
                PreDiveClassChip(c.key),
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                    border: UiRect::all(Val::Px(if on { 2.0 } else { 1.0 })),
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(if on { glass::ACTIVE } else { glass::GLASS_THIN }),
                BorderColor::all(if on { glass::ACTIVE_EDGE } else { glass::EDGE_SOFT }),
            ))
            .with_children(|card| {
                card.spawn(glass::text(c.name, 14.0, glass::TITLE));
                card.spawn(glass::text(c.role, 11.0, glass::TEXT));
                card.spawn(glass::text(overworld_passive_blurb(c.key), 10.0, glass::DIM));
            });
        }
    });
    spawn_predive_nav(p, "Confirm", picks.len() == 4);
}

fn spawn_predive_nav(p: &mut ChildSpawnerCommands, advance_label: &str, advanceable: bool) {
    p.spawn(Node {
        flex_direction: FlexDirection::Row,
        column_gap: Val::Px(12.0),
        justify_content: JustifyContent::SpaceBetween,
        margin: UiRect::top(Val::Px(10.0)),
        ..default()
    })
    .with_children(|row| {
        row.spawn((Button, PreDiveSkipBtn, glass::chip(false)))
            .with_children(|b| {
                b.spawn(glass::text("Skip and close the tutorial", 13.0, glass::TEXT));
            });
        row.spawn((Button, PreDiveAdvanceBtn, glass::chip(advanceable)))
            .with_children(|b| {
                b.spawn(glass::text(advance_label, 15.0, glass::TITLE));
            });
    });
}

/// Chip toggles, Continue/Confirm, and Skip — Confirm is the only place a
/// tutorial dive's `ClientCmd::EnterMaze` is ever sent (see `city_input`'s
/// `[T]` branch, which no longer sends it directly).
pub(crate) fn tutorial_predive_buttons(
    mut predive: ResMut<TutorialPreDive>,
    mut session: ResMut<Session>,
    net: NonSend<NetRes>,
    advance_q: Query<&Interaction, (With<PreDiveAdvanceBtn>, Changed<Interaction>)>,
    skip_q: Query<&Interaction, (With<PreDiveSkipBtn>, Changed<Interaction>)>,
    chip_q: Query<(&Interaction, &PreDiveClassChip), Changed<Interaction>>,
) {
    if skip_q.iter().any(|i| *i == Interaction::Pressed) {
        predive.stage = None;
        predive.picks.clear();
        return;
    }
    for (interaction, chip) in &chip_q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Some(pos) = predive.picks.iter().position(|k| *k == chip.0) {
            predive.picks.remove(pos);
        } else if predive.picks.len() < 4 {
            predive.picks.push(chip.0);
        }
        // Past 4, a chip press is a silent no-op — the same "refused rather
        // than erroring" idiom a disabled menu row uses elsewhere.
    }
    if advance_q.iter().any(|i| *i == Interaction::Pressed) {
        match predive.stage {
            Some(PreDiveStage::Welcome) => predive.stage = Some(PreDiveStage::Picking),
            Some(PreDiveStage::Picking) if predive.picks.len() == 4 => {
                session.entered = true;
                session.coop = false;
                session.status = "beginning the guided run...".to_string();
                net.0.send(ClientCmd::EnterMaze {
                    party: predive.picks.iter().map(|s| s.to_string()).collect(),
                    tutorial: true,
                    hub: session.hub.clone(),
                });
                predive.stage = None;
                predive.picks.clear();
            }
            _ => {}
        }
    }
}
