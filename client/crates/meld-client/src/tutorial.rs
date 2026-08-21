//! Onboarding: the town welcome tour (shown once in Last City) and the
//! first-dive briefing (shown once on a brand-new account's first real run).
//! Both gate on `Tutorial.loaded` (set once `onboarding.status` lands from the
//! server) so a returning player never sees a one-frame flash of first-timer
//! UI while their real "already seen it" flags are still in flight.

use meld_client::glass;
use meld_client::net::{ClientCmd, Net};

use super::*;

// ------------------------------------------------------------- town tour ---

/// The tour step that highlights the travel column — styled by
/// `city::render_travel_column` itself (a brighter, thicker border on the
/// REAL element while this step is active) rather than an approximated
/// overlay box, so the highlight always covers exactly what's actually
/// there regardless of how many districts the list holds.
pub(crate) const TRAVEL_COLUMN_STEP: u8 = 2;
/// The tour step that highlights the tap-action bar — styled the same way,
/// by `city::highlight_tap_action_bar` mutating the real container's border.
pub(crate) const TAP_ACTION_BAR_STEP: u8 = 3;

struct TourStep {
    caption: &'static str,
}

const TOUR_STEPS: [TourStep; 6] = [
    // Steps 0-1: the two welcome pages (their own layout, not caption-driven).
    TourStep { caption: "" },
    TourStep { caption: "" },
    TourStep {
        caption: "This is your travel list — tap a district (or press its number) to walk straight to it.",
    },
    TourStep {
        caption: "Quick actions live here: Party, Run, Vault, Co-op, and Exit — no walking required.",
    },
    // Step 4: the "Tutorial" tip card (its own layout).
    TourStep { caption: "" },
    // Step 5: the "Got it!" card.
    TourStep { caption: "" },
];

/// Marks the tour's centred card (scrim + panel) — despawned/rebuilt each step.
#[derive(Component)]
pub(crate) struct TownTourRoot;
/// The "don't show this tutorial again" toggle, present on both welcome pages
/// (steps 0-1).
#[derive(Component)]
pub(crate) struct TourSkipCheckbox;
/// Advance/finish (label reads "Next" or "Got it!" depending on the step).
#[derive(Component)]
pub(crate) struct TourAdvanceBtn;
/// Step back one (hidden on step 0 — nothing to go back to).
#[derive(Component)]
pub(crate) struct TourBackBtn;

/// Immediate-mode despawn/rebuild each changed step. Arms itself the moment
/// the server sync has landed for an account that hasn't seen the tour yet.
/// The travel-column and tap-action-bar highlight steps don't spawn anything
/// of their own here — `city::render_travel_column` and
/// `city::highlight_tap_action_bar` style the REAL elements directly (see
/// `TRAVEL_COLUMN_STEP`/`TAP_ACTION_BAR_STEP`), so the highlight always
/// covers exactly what's actually on screen.
pub(crate) fn render_town_tour(
    mut commands: Commands,
    mut tutorial: ResMut<Tutorial>,
    city: Res<CityUi>,
    session: Res<Session>,
    card_q: Query<Entity, With<TownTourRoot>>,
) {
    // AUTOPLAY NEVER SEES THE TOUR. It is a modal card that swallows the dive key, and
    // autoplay cannot press the button that dismisses it — so every autoplay launch sat in
    // the hub forever, which made screenshotting anything past town impossible on the
    // embedded build (an in-memory account is always a fresh one, so the tour always fires).
    // The tour is for a person; autoplay is an instrument.
    if crate::flags::autoplay_flag() {
        tutorial.town_step = None;
        tutorial.town_seen = true;
    } else if tutorial.loaded && !tutorial.town_seen && tutorial.town_step.is_none() {
        tutorial.town_step = Some(0);
    }
    let Some(step_idx) = tutorial.town_step else {
        for e in &card_q {
            commands.entity(e).despawn();
        }
        return;
    };
    let step = &TOUR_STEPS[step_idx as usize];
    // Defensive: don't point at the travel column while it's hidden (a
    // shop/craft/board/hunts/party panel open, or a run underway) — wait
    // rather than despawn, so the tour simply resumes once it's visible again.
    if step_idx == TRAVEL_COLUMN_STEP
        && (city.party_open
            || city.shop_open
            || city.craft_open
            || city.board_open
            || city.hunts_open
            || session.entered)
    {
        return;
    }
    for e in &card_q {
        commands.entity(e).despawn();
    }

    commands
        .spawn((TownTourRoot, GlobalZIndex(1000), glass::scrim()))
        .with_children(|root| {
            root.spawn(glass::panel_capped(Val::Percent(90.0), Val::Px(440.0)))
                .with_children(|p| match step_idx {
                    0 => spawn_welcome_page(
                        p,
                        Some("Welcome to Meldworld"),
                        "This is your hub — where you'll spend your time between dives, away from the grind for gear and items.",
                        tutorial.skip_checked,
                        0,
                    ),
                    1 => spawn_welcome_page(
                        p,
                        Some("Controls"),
                        "WASD moves you, [E] interacts with a district, [Enter] dives. Every control here is also listed in the pause menu — nothing here is hidden.",
                        tutorial.skip_checked,
                        1,
                    ),
                    4 => spawn_tutorial_tip_card(p),
                    5 => spawn_gotit_card(p),
                    _ => spawn_step_card(p, step.caption, step_idx),
                });
        });
}

/// One of the two welcome pages: an optional title, a body line, the skip
/// checkbox (present on both pages), and the Back/Next row.
fn spawn_welcome_page(
    p: &mut ChildSpawnerCommands,
    title: Option<&str>,
    body: &str,
    skip_checked: bool,
    step_idx: u8,
) {
    if let Some(title) = title {
        p.spawn(glass::text(title.to_string(), 20.0, glass::TITLE));
        p.spawn(glass::divider());
    }
    p.spawn(glass::text(body.to_string(), 14.0, glass::TEXT));
    p.spawn(Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(8.0),
        margin: UiRect::top(Val::Px(4.0)),
        ..default()
    })
    .with_children(|row| {
        row.spawn((Button, TourSkipCheckbox, glass::chip(skip_checked)))
            .with_children(|b| {
                b.spawn(glass::text(if skip_checked { "[x]" } else { "[ ]" }, 13.0, glass::TEXT));
            });
        row.spawn(glass::text("Don't show this tutorial again", 13.0, glass::DIM));
    });
    spawn_nav_row(p, step_idx > 0);
}

fn spawn_step_card(p: &mut ChildSpawnerCommands, caption: &str, step_idx: u8) {
    p.spawn(glass::text(caption.to_string(), 15.0, glass::TEXT));
    spawn_nav_row(p, step_idx > 0);
}

fn spawn_tutorial_tip_card(p: &mut ChildSpawnerCommands) {
    p.spawn(glass::text("Tutorial", 20.0, glass::TITLE));
    p.spawn(glass::divider());
    p.spawn(glass::text(
        "Not sure where to start? Press [T] at The Threshold for a guided practice dive — a gentler run to test the game out before you risk a real one.",
        14.0,
        glass::TEXT,
    ));
    spawn_nav_row(p, true);
}

fn spawn_gotit_card(p: &mut ChildSpawnerCommands) {
    p.spawn(glass::text("You're all set", 20.0, glass::TITLE));
    p.spawn(glass::divider());
    p.spawn(glass::text(
        "Explore town, then step through The Threshold whenever you're ready to dive.",
        14.0,
        glass::TEXT,
    ));
    spawn_nav_row(p, true);
}

fn spawn_nav_row(p: &mut ChildSpawnerCommands, show_back: bool) {
    p.spawn(Node {
        flex_direction: FlexDirection::Row,
        column_gap: Val::Px(12.0),
        margin: UiRect::top(Val::Px(8.0)),
        justify_content: JustifyContent::FlexEnd,
        ..default()
    })
    .with_children(|row| {
        if show_back {
            row.spawn((Button, TourBackBtn, glass::chip(false)))
                .with_children(|b| {
                    b.spawn(glass::text("Back", 15.0, glass::TEXT));
                });
        }
        row.spawn((Button, TourAdvanceBtn, glass::chip(true)))
            .with_children(|b| {
                b.spawn(glass::text("Next", 15.0, glass::TITLE));
            });
    });
}

/// Checking the skip box (on either welcome page) and advancing finishes the
/// tour immediately rather than stepping forward — the only "skip" path, per
/// the spec (no separate Skip button). Shared by both the mouse and keyboard
/// paths.
fn tour_advance(tutorial: &mut Tutorial, net: &Net) {
    let step = tutorial.town_step.unwrap_or(0);
    let last = step + 1 >= TOUR_STEPS.len() as u8;
    if tutorial.skip_checked || last {
        tutorial.town_step = None;
        tutorial.town_seen = true;
        net.send(ClientCmd::OnboardingTownSeen);
    } else {
        tutorial.town_step = Some(step + 1);
    }
}

fn tour_back(tutorial: &mut Tutorial) {
    let step = tutorial.town_step.unwrap_or(0);
    tutorial.town_step = Some(step.saturating_sub(1));
}

/// Next/Back/Got-it clicks.
pub(crate) fn tour_buttons(
    mut tutorial: ResMut<Tutorial>,
    net: NonSend<NetRes>,
    advance_q: Query<&Interaction, (With<TourAdvanceBtn>, Changed<Interaction>)>,
    back_q: Query<&Interaction, (With<TourBackBtn>, Changed<Interaction>)>,
) {
    if advance_q.iter().any(|i| *i == Interaction::Pressed) {
        tour_advance(&mut tutorial, &net.0);
        return;
    }
    if back_q.iter().any(|i| *i == Interaction::Pressed) {
        tour_back(&mut tutorial);
    }
}

/// Keyboard twin of `tour_buttons`: Enter/Space advances (or finishes, matching
/// the button's own rule), Backspace steps back — the same keys/roles
/// `battle.rs`'s command menu uses for its own Enter-selects/Backspace-backs-out.
pub(crate) fn tour_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut tutorial: ResMut<Tutorial>,
    net: NonSend<NetRes>,
) {
    if tutorial.town_step.is_none() {
        return;
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        tour_advance(&mut tutorial, &net.0);
    } else if keys.just_pressed(KeyCode::Backspace) {
        tour_back(&mut tutorial);
    }
}

/// Toggles the "don't show this tutorial again" checkbox.
pub(crate) fn tour_checkbox_click(
    mut tutorial: ResMut<Tutorial>,
    q: Query<&Interaction, (With<TourSkipCheckbox>, Changed<Interaction>)>,
) {
    if q.iter().any(|i| *i == Interaction::Pressed) {
        // `render_town_tour` rebuilds every frame regardless, so this flips
        // the checkbox glyph on the very next frame without any extra signal.
        tutorial.skip_checked = !tutorial.skip_checked;
    }
}

// ---------------------------------------------------------- first dive -----

/// Marks the first-dive briefing's card.
#[derive(Component)]
pub(crate) struct FirstRunPopupRoot;
/// Its single dismiss button.
#[derive(Component)]
pub(crate) struct FirstRunDismissBtn;

/// A single static card, modelled on `overlays::unlock_banner`: waits to be
/// dismissed (button or Space/Enter/Escape) rather than timing out. `run_seen`
/// and the C2S ack already happened at arm-time (`netglue.rs`'s `RunStarted`
/// handler), so dismissing here only clears the local "show it" flag.
pub(crate) fn first_run_popup(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut tutorial: ResMut<Tutorial>,
    dismiss_q: Query<&Interaction, (With<FirstRunDismissBtn>, Changed<Interaction>)>,
    root_q: Query<Entity, With<FirstRunPopupRoot>>,
) {
    if !tutorial.show_run_popup {
        for e in &root_q {
            commands.entity(e).despawn();
        }
        return;
    }
    let dismiss = keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::Escape)
        || dismiss_q.iter().any(|i| *i == Interaction::Pressed);
    if dismiss {
        tutorial.show_run_popup = false;
        for e in &root_q {
            commands.entity(e).despawn();
        }
        return;
    }
    if root_q.is_empty() {
        commands
            .spawn((FirstRunPopupRoot, GlobalZIndex(1000), glass::scrim()))
            .with_children(|root| {
                root.spawn(glass::panel_capped(Val::Percent(90.0), Val::Px(460.0)))
                    .with_children(|p| {
                        p.spawn(glass::text("Before You Dive", 20.0, glass::TITLE));
                        p.spawn(glass::divider());
                        for line in [
                            "Push as deep as you dare — gear and chits are the reward for the fights you win.",
                            "Death is real, and it will happen. You lose this run's gear and level — never your account, classes, or Vault.",
                            "Every run starts fresh at hero level 1. What you loot from monsters is what keeps you alive further down.",
                            "New classes and party slots are earned permanently by playing — they're never lost to a bad run.",
                        ] {
                            p.spawn(glass::text(line, 14.0, glass::TEXT));
                        }
                        p.spawn(Node {
                            justify_content: JustifyContent::FlexEnd,
                            margin: UiRect::top(Val::Px(8.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn((Button, FirstRunDismissBtn, glass::chip(true)))
                                .with_children(|b| {
                                    b.spawn(glass::text("Let's go", 15.0, glass::TITLE));
                                });
                        });
                    });
            });
    }
}

// ------------------------------------------------------ guided practice dive --
//
// The `[T]` dive's step-by-step walkthrough (`TutorialRun`), layered on top of
// real gameplay rather than static city UI. Unlike the town tour/briefing
// above, this is never persisted — see `TutorialRun`'s own doc comment.

/// The always-present, non-blocking coaching line for the walkthrough — ONE
/// fixed spot on screen (top-centre) across every step, including the Battle
/// screen, so the player never has to re-locate it. No scrim: this must never
/// block clicks on the world/HUD beneath it.
#[derive(Component)]
pub(crate) struct TutorialCaptionRoot;

pub(crate) fn render_tutorial_caption(
    mut commands: Commands,
    tutorial_run: Res<TutorialRun>,
    card_q: Query<Entity, With<TutorialCaptionRoot>>,
) {
    let caption = match tutorial_run.step {
        Some(TutorialStep::Harvest) if !tutorial_run.harvested => {
            Some("Walk up to the glowing node and press [E] to gather it.")
        }
        Some(TutorialStep::Harvest) => {
            Some("Now open the chest the same way — walk up and press [E].")
        }
        Some(TutorialStep::Fight) => Some(
            "A creature is near — when the fight starts, press Attack (or click it) to strike.",
        ),
        Some(TutorialStep::Dungeon) => {
            Some("Look for the dungeon entrance ahead and press [E] to descend.")
        }
        _ => None,
    };
    let Some(caption) = caption else {
        for e in &card_q {
            commands.entity(e).despawn();
        }
        return;
    };
    for e in &card_q {
        commands.entity(e).despawn();
    }
    commands
        .spawn((
            TutorialCaptionRoot,
            GlobalZIndex(900),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn(glass::panel_capped(Val::Percent(70.0), Val::Px(420.0)))
                .with_children(|p| {
                    p.spawn(glass::text("Guided Dive", 12.0, glass::DIM));
                    p.spawn(glass::text(caption, 15.0, glass::TEXT));
                });
        });
}

/// Marks the "What is a Dungeon?" card.
#[derive(Component)]
pub(crate) struct TutorialExplainRoot;
/// Its single dismiss button.
#[derive(Component)]
pub(crate) struct TutorialExplainDismissBtn;

/// Fires the dungeon-entry explanation the instant the client's dungeon
/// re-skin flips on (`DungeonSceneRes.active` false→true) while the
/// walkthrough is waiting on the Dungeon step. Tracked via a local edge
/// rather than `DungeonSceneRes.dirty`, which `world_render::manage_dungeon_scene`
/// consumes — reading it here would race against that system's own ordering.
pub(crate) fn dungeon_explain_card(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut tutorial_run: ResMut<TutorialRun>,
    scene: Res<crate::world_render::DungeonSceneRes>,
    mut was_active: Local<bool>,
    dismiss_q: Query<&Interaction, (With<TutorialExplainDismissBtn>, Changed<Interaction>)>,
    root_q: Query<Entity, With<TutorialExplainRoot>>,
) {
    let just_entered = scene.active && !*was_active;
    *was_active = scene.active;
    if just_entered && tutorial_run.step == Some(TutorialStep::Dungeon) {
        tutorial_run.step = Some(TutorialStep::Explain);
    }
    if tutorial_run.step != Some(TutorialStep::Explain) {
        for e in &root_q {
            commands.entity(e).despawn();
        }
        return;
    }
    let dismiss = keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::Escape)
        || dismiss_q.iter().any(|i| *i == Interaction::Pressed);
    if dismiss {
        tutorial_run.step = Some(TutorialStep::Done);
        for e in &root_q {
            commands.entity(e).despawn();
        }
        return;
    }
    if root_q.is_empty() {
        commands
            .spawn((TutorialExplainRoot, GlobalZIndex(1000), glass::scrim()))
            .with_children(|root| {
                root.spawn(glass::panel_capped(Val::Percent(90.0), Val::Px(440.0)))
                    .with_children(|p| {
                        p.spawn(glass::text("What is a Dungeon?", 20.0, glass::TITLE));
                        p.spawn(glass::divider());
                        for line in [
                            "A dungeon is a hand-built space reached only through entrances like the one you just walked through.",
                            "No Town Portal works inside — it's a committed trip. Floors are fixed layouts, not the open, procedural overworld.",
                            "You leave through the dungeon's own exit, deeper in.",
                        ] {
                            p.spawn(glass::text(line, 14.0, glass::TEXT));
                        }
                        p.spawn(Node {
                            justify_content: JustifyContent::FlexEnd,
                            margin: UiRect::top(Val::Px(8.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn((Button, TutorialExplainDismissBtn, glass::chip(true)))
                                .with_children(|b| {
                                    b.spawn(glass::text("Got it", 15.0, glass::TITLE));
                                });
                        });
                    });
            });
    }
}

/// Marks the completion popup's card.
#[derive(Component)]
pub(crate) struct TutorialCompleteRoot;
/// Dismiss-and-continue: the dive carries on with no further walkthrough UI.
#[derive(Component)]
pub(crate) struct TutorialKeepGoingBtn;
/// Dismiss and channel a Town Portal back to Last City.
#[derive(Component)]
pub(crate) struct TutorialGoTownBtn;

/// The walkthrough's finish line: shown once step 4's explanation card is
/// dismissed. `Keep going` just clears the walkthrough state; `Go back to
/// town` sends the same `ClientCmd::TownPortal` the Map column's own
/// "Return to town" row sends, then clears it too — the existing extraction
/// channel → `RunEnded` → `Screen::Ended` flow does the rest, unchanged.
pub(crate) fn tutorial_complete_popup(
    mut commands: Commands,
    tutorial_run: Res<TutorialRun>,
    root_q: Query<Entity, With<TutorialCompleteRoot>>,
) {
    if tutorial_run.step != Some(TutorialStep::Done) {
        for e in &root_q {
            commands.entity(e).despawn();
        }
        return;
    }
    if root_q.is_empty() {
        commands
            .spawn((TutorialCompleteRoot, GlobalZIndex(1000), glass::scrim()))
            .with_children(|root| {
                root.spawn(glass::panel_capped(Val::Percent(90.0), Val::Px(440.0)))
                    .with_children(|p| {
                        p.spawn(glass::text("you have completed the tutorial", 20.0, glass::TITLE));
                        p.spawn(glass::divider());
                        p.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(12.0),
                            justify_content: JustifyContent::FlexEnd,
                            margin: UiRect::top(Val::Px(8.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn((Button, TutorialKeepGoingBtn, glass::chip(false)))
                                .with_children(|b| {
                                    b.spawn(glass::text("Keep going", 15.0, glass::TEXT));
                                });
                            row.spawn((Button, TutorialGoTownBtn, glass::chip(true)))
                                .with_children(|b| {
                                    b.spawn(glass::text("Go back to town", 15.0, glass::TITLE));
                                });
                        });
                    });
            });
    }
}

/// "Keep going"/"Go back to town" clicks.
pub(crate) fn tutorial_complete_buttons(
    mut tutorial_run: ResMut<TutorialRun>,
    net: NonSend<NetRes>,
    keep_q: Query<&Interaction, (With<TutorialKeepGoingBtn>, Changed<Interaction>)>,
    town_q: Query<&Interaction, (With<TutorialGoTownBtn>, Changed<Interaction>)>,
) {
    if keep_q.iter().any(|i| *i == Interaction::Pressed) {
        tutorial_run.step = None;
        return;
    }
    if town_q.iter().any(|i| *i == Interaction::Pressed) {
        net.0.send(ClientCmd::TownPortal);
        tutorial_run.step = None;
    }
}
