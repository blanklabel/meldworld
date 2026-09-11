//! Simple full-screen screens: Join (party builder), co-op Lobby, and the
//! Ended (extract/death) summary.
//! Extracted from `main.rs` during the module reorg.


use meld_client::glass;

use meld_client::net::{self, ClientCmd};

use super::*;

// ---------------------------------------------------------------- join -----

/// Picker metadata for a class (client-side; the authoritative stats live in
/// `balance.toml`, but the Join screen is pre-connection so it shows an at-a-glance
/// role + relative 0..5 ratings + kit). Ratings are qualitative, tuned to read the
/// class taxonomy (CLAUDE.md) at a glance, not the exact balance numbers.
pub(crate) struct ClassInfo {
    pub key: &'static str,
    pub name: &'static str,
    pub role: &'static str,
    pub hp: u8,
    pub atk: u8,
    pub spd: u8,
    pub mag: u8,
    pub def: u8,
}

pub(crate) const CLASS_INFO: [ClassInfo; 10] = [
    ClassInfo { key: "explorer", name: "Explorer", role: "The order that maps and anchors the world \u{2014} tempo and stability, not burst.", hp: 4, atk: 3, spd: 3, mag: 2, def: 3 },
    ClassInfo { key: "hunter", name: "Hunter", role: "The guild that disposes of dangerous creatures \u{2014} the martial baseline.", hp: 4, atk: 4, spd: 3, mag: 1, def: 3 },
    ClassInfo { key: "psyker", name: "Psyker", role: "Psychic channeler. Weaves persistent Foci from the back row.", hp: 2, atk: 1, spd: 3, mag: 5, def: 2 },
    ClassInfo { key: "resonant", name: "Resonant", role: "Healer. Innate Regen keeps the party standing.", hp: 3, atk: 2, spd: 3, mag: 4, def: 2 },
    ClassInfo { key: "shifter", name: "Shifter", role: "Rogue skirmisher. Fast, fragile, the only innate dodge.", hp: 2, atk: 4, spd: 5, mag: 1, def: 1 },
    ClassInfo { key: "phoenix_guard", name: "Phoenix Guard", role: "The Last City's anti-undead order \u{2014} a wall that hits the risen hardest.", hp: 5, atk: 3, spd: 1, mag: 1, def: 5 },
    ClassInfo { key: "smithwright", name: "Smithwright", role: "The Foundry's builder \u{2014} raises the field forge, and buys the party time.", hp: 4, atk: 4, spd: 2, mag: 1, def: 4 },
    ClassInfo { key: "keeper", name: "Keeper", role: "Open Flower grower \u{2014} sets up the still, and keeps everyone standing.", hp: 2, atk: 1, spd: 4, mag: 5, def: 2 },
    ClassInfo { key: "iron_hull", name: "Iron Hull Monk", role: "Ascetic of the rusting vessel \u{2014} staggers, roots, and answers a back rank with sound.", hp: 4, atk: 3, spd: 3, mag: 2, def: 3 },
    ClassInfo { key: "rift_knight", name: "Rift Knight", role: "Wall Defense drop-trooper \u{2014} teleports out of reach, then lands on one target.", hp: 3, atk: 5, spd: 3, mag: 1, def: 3 },
];

pub(crate) fn class_info(key: &str) -> &'static ClassInfo {
    CLASS_INFO.iter().find(|c| c.key == key).unwrap_or(&CLASS_INFO[0])
}

/// The kit as a multi-line "Skills\n  Name — what it does" block for the detail panel,
/// read from [`meld_proto::skills`] — the one registry the server gates on and the battle
/// menu builds from. It used to be a hand-copied list on each `ClassInfo`, which had
/// already drifted: the Explorer's card still named "Set Anchor" and put its rungs at
/// Lv2/5/9 when the registry says 4/9/16.
pub(crate) fn kit_text(ci: &ClassInfo) -> String {
    let mut s = String::from("Skills");
    for def in meld_proto::skills::skills_for_class(ci.key).iter().take(KIT_ROWS) {
        let at = if def.unlock > 1 {
            format!(" (Lv{})", def.unlock)
        } else {
            String::new()
        };
        s.push_str(&format!("\n  {} \u{2014} {}{at}", def.name, def.description));
    }
    // Say what is behind the cut. Every ladder now runs to level 100, so a card that
    // silently showed four of eight read as "this is the whole class".
    let rest = meld_proto::skills::skills_for_class(ci.key).len().saturating_sub(KIT_ROWS);
    if rest > 0 {
        let top = meld_proto::skills::ladder_top(meld_proto::skills::archetype(ci.key));
        s.push_str(&format!("\n  ...and {rest} more, out to level {top}"));
    }
    s
}

/// Kit rows the Join screen's card shows before it runs out of room.
const KIT_ROWS: usize = 4;


/// Which account-login field is being typed into: 0 = username, 1 = password, None =
/// no field (so 1-4 / arrows still drive the class picker).
#[derive(Resource, Default)]
pub(crate) struct LoginFocus(pub Option<u8>);

/// The login screen's looping backdrop. Bevy plays no video, so the source clip is
/// baked into a WebP frame sequence (`assets/loginscreens/`) and stepped here. The
/// clip is a slow push-in, which would jump on a plain loop, so it plays **ping-pong**
/// — forwards, then backwards — and joins itself seamlessly.
///
/// The handles live here only while the Join screen is up, so the frame textures are
/// handed back to the GPU on log-in.
#[derive(Resource, Default)]
pub(crate) struct LoginBg {
    frames: Vec<Handle<Image>>,
    idx: usize,
    forward: bool,
    t: f32,
}

const LOGIN_BG_FRAMES: usize = 120;
const LOGIN_BG_FPS: f32 = 12.0;
const LOGIN_BG_ASPECT: f32 = 16.0 / 9.0;

#[derive(Component)]
pub(crate) struct LoginBgImage;

#[derive(Component)]
pub(crate) struct JoinUserField; // clickable username box
#[derive(Component)]
pub(crate) struct JoinPassField; // clickable password box
#[derive(Component)]
pub(crate) struct JoinUserText;
#[derive(Component)]
pub(crate) struct JoinPassText;

/// KeyCode → character for typing an account username/password: lowercase letters
/// (uppercase with Shift), digits, and a couple of safe symbols.
pub(crate) fn typed_char(key: KeyCode, shift: bool) -> Option<char> {
    use KeyCode::*;
    let base = match key {
        KeyA => 'a', KeyB => 'b', KeyC => 'c', KeyD => 'd', KeyE => 'e', KeyF => 'f',
        KeyG => 'g', KeyH => 'h', KeyI => 'i', KeyJ => 'j', KeyK => 'k', KeyL => 'l',
        KeyM => 'm', KeyN => 'n', KeyO => 'o', KeyP => 'p', KeyQ => 'q', KeyR => 'r',
        KeyS => 's', KeyT => 't', KeyU => 'u', KeyV => 'v', KeyW => 'w', KeyX => 'x',
        KeyY => 'y', KeyZ => 'z',
        Digit0 => '0', Digit1 => '1', Digit2 => '2', Digit3 => '3', Digit4 => '4',
        Digit5 => '5', Digit6 => '6', Digit7 => '7', Digit8 => '8', Digit9 => '9',
        Minus => '-', Period => '.',
        _ => return None,
    };
    Some(if shift && base.is_ascii_alphabetic() { base.to_ascii_uppercase() } else { base })
}

/// One labelled, clickable text field (username / password).
fn field_box(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    field_tag: impl Bundle,
    text_tag: impl Bundle,
) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            ..default()
        })
        .with_children(|r| {
            r.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: FontSize::Px(14.0), ..default() },
                TextColor(glass::DIM),
            ));
            r.spawn((
                Button,
                field_tag,
                Node {
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    width: Val::Px(180.0),
                    height: Val::Px(30.0),
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    border: UiRect::all(Val::Px(1.5)),
                    ..default()
                },
                BorderColor::all(glass::EDGE_SOFT),
                BackgroundColor(glass::GLASS_DEEP),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new(String::new()),
                    text_tag,
                    TextFont { font_size: FontSize::Px(15.0), ..default() },
                    TextColor(glass::TEXT),
                ));
            });
        });
}

#[allow(clippy::type_complexity)]
pub(crate) fn join_ui(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut bg: ResMut<LoginBg>,
    mut login: ResMut<LoginFocus>,
) {
    bg.frames = (0..LOGIN_BG_FRAMES)
        .map(|i| assets.load(format!("loginscreens/gears_and_forest/frame{i:03}.webp")))
        .collect();
    bg.idx = 0;
    bg.forward = true;
    bg.t = 0.0;

    // A field is focused up front, so the first thing a player types lands in the
    // username instead of being swallowed (they should not have to find the click).
    login.0 = Some(0);

    commands
        .spawn((
            JoinRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                LoginBgImage,
                ImageNode::new(bg.frames[0].clone()).with_color(BG_TINT),
                Node { position_type: PositionType::Absolute, ..default() },
                BackgroundColor(Color::BLACK),
                ZIndex(0),
            ));
            // The backdrop is a busy painting, so the panel needs a real scrim under
            // it — glass alone leaves 12 px hint text fighting bark and gear teeth.
            p.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(glass::SCRIM),
                ZIndex(1),
            ));

            p.spawn((
                Node {
                    border_radius: BorderRadius::all(Val::Px(14.0)),
                    width: Val::Px(560.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(7.0),
                    padding: UiRect::axes(Val::Px(28.0), Val::Px(24.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(glass::GLASS),
                BorderColor::all(glass::EDGE_SOFT),
                ZIndex(2),
            ))
            .with_children(|p| {
                p.spawn((
                    Text::new("MELDWORLD"),
                    TextFont { font_size: FontSize::Px(44.0), ..default() },
                    TextColor(glass::TITLE),
                ));
                p.spawn((
                    Text::new("Log in \u{2014} then muster your party in the Last City."),
                    TextFont { font_size: FontSize::Px(17.0), ..default() },
                    TextColor(glass::DIM),
                    Node { margin: UiRect::bottom(Val::Px(6.0)), ..default() },
                ));

                // Account login (real, persistent accounts): click a field and type;
                // TAB switches fields. New name → account is created on first login.
                p.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(18.0),
                    ..default()
                })
                .with_children(|row| {
                    field_box(row, "Username", JoinUserField, JoinUserText);
                    field_box(row, "Password", JoinPassField, JoinPassText);
                });
                p.spawn((
                    Text::new("Type, TAB to switch field, ENTER to log in \u{2014} first login creates your account."),
                    TextFont { font_size: FontSize::Px(13.0), ..default() },
                    TextColor(glass::DIM),
                ));
                p.spawn((
                    Text::new("username 3\u{2013}20 \u{b7} password 8+ characters"),
                    TextFont { font_size: FontSize::Px(12.0), ..default() },
                    TextColor(Color::srgba(0.72, 0.78, 0.9, 0.6)),
                ));

                // The season's Vanguard Board. The party used to be built here, but this
                // screen runs BEFORE login: it cannot know which classes the account owns,
                // so it could only offer all six and let the server clamp the answer. The
                // party is mustered in town (the Drill Yard), where the unlock set is
                // known. What belongs on a login screen is a reason to log in.
                p.spawn((
                    Text::new("The Vanguard \u{2014} deepest of the season"),
                    TextFont { font_size: FontSize::Px(16.0), ..default() },
                    TextColor(glass::TITLE),
                    Node { margin: UiRect::top(Val::Px(12.0)), ..default() },
                ));
                p.spawn((
                    Node {
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(3.0),
                        padding: UiRect::all(Val::Px(14.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(glass::EDGE_SOFT),
                    BackgroundColor(glass::GLASS_DEEP),
                ))
                .with_children(|d| {
                    d.spawn((
                        Text::new("reading the board..."),
                        JoinBoardText,
                        TextFont { font_size: FontSize::Px(14.0), ..default() },
                        TextColor(glass::TEXT),
                    ));
                });

                p.spawn((
                    StatusText,
                    Text::new(""),
                    TextFont { font_size: FontSize::Px(15.0), ..default() },
                    TextColor(glass::WARN),
                    Node { margin: UiRect::top(Val::Px(8.0)), ..default() },
                ));
            });
        });
}

/// How far the backdrop is knocked down before any UI sits on it.
const BG_TINT: Color = Color::srgb(0.62, 0.62, 0.68);

/// Ping-pong the baked frame sequence behind the login panel.
pub(crate) fn login_bg_play(
    time: Res<Time>,
    images: Res<Assets<Image>>,
    mut bg: ResMut<LoginBg>,
    mut q: Query<&mut ImageNode, With<LoginBgImage>>,
) {
    let Ok(mut img) = q.single_mut() else { return };
    if bg.frames.is_empty() {
        return;
    }
    bg.t += time.delta_secs();
    while bg.t >= 1.0 / LOGIN_BG_FPS {
        bg.t -= 1.0 / LOGIN_BG_FPS;
        if bg.forward {
            if bg.idx + 1 >= bg.frames.len() {
                bg.forward = false;
            } else {
                bg.idx += 1;
            }
        } else if bg.idx == 0 {
            bg.forward = true;
        } else {
            bg.idx -= 1;
        }
    }
    // Hold the frame on screen until its successor has actually decoded — the 120
    // loads land over the first moment or two, and swapping to a pending handle
    // draws nothing at all (a black flash) rather than an old frame.
    let frame = bg.frames[bg.idx].clone();
    if img.image != frame && images.contains(&frame) {
        img.image = frame;
    }
}

/// Cover-fit the backdrop to the window: fill it in both axes and centre the
/// overflow, so the clip never stretches to the window's aspect.
pub(crate) fn login_bg_fit(
    window: Query<&Window>,
    mut q: Query<&mut Node, With<LoginBgImage>>,
) {
    let (Ok(win), Ok(mut node)) = (window.single(), q.single_mut()) else { return };
    let (ww, wh) = (win.width(), win.height());
    let (w, h) = if ww / wh > LOGIN_BG_ASPECT {
        (ww, ww / LOGIN_BG_ASPECT)
    } else {
        (wh * LOGIN_BG_ASPECT, wh)
    };
    node.width = Val::Px(w);
    node.height = Val::Px(h);
    node.left = Val::Px((ww - w) * 0.5);
    node.top = Val::Px((wh - h) * 0.5);
}

/// Drop the frame handles on leaving the login screen so their textures are freed.
pub(crate) fn login_bg_unload(mut bg: ResMut<LoginBg>) {
    bg.frames.clear();
}

/// This frame's login keyboard, as plain data.
pub(crate) struct LoginKeys<'a> {
    pub tab: bool,
    pub backspace: bool,
    pub shift: bool,
    pub typed: &'a [KeyCode],
}

/// The longest an account name or password may be typed to.
const LOGIN_FIELD_MAX: usize = 24;

/// Apply one frame of the login keyboard to the focused field. Split out of
/// [`join_input`] so the typing rules can be tested without a window.
pub(crate) fn edit_login_field(
    focus: &mut Option<u8>,
    keys: LoginKeys,
    username: &mut String,
    password: &mut String,
) {
    // TAB is the keyboard's only way INTO the fields, so it has to work from an
    // unfocused screen too — otherwise the login is unusable without a mouse.
    if keys.tab {
        *focus = Some(1 - focus.unwrap_or(1));
        return;
    }
    let Some(f) = *focus else { return };
    let field = if f == 0 { username } else { password };
    if keys.backspace {
        field.pop();
        return;
    }
    for key in keys.typed {
        if let Some(c) = typed_char(*key, keys.shift) {
            if field.chars().count() < LOGIN_FIELD_MAX {
                field.push(c);
            }
        }
    }
}

/// Click-to-focus for the login fields. Nothing else ever sets [`LoginFocus`], so
/// without this every keystroke on the login screen is discarded.
#[allow(clippy::type_complexity)]
pub(crate) fn join_field_click(
    user: Query<&Interaction, (Changed<Interaction>, With<JoinUserField>)>,
    pass: Query<&Interaction, (Changed<Interaction>, With<JoinPassField>)>,
    mut login: ResMut<LoginFocus>,
) {
    if user.iter().any(|i| *i == Interaction::Pressed) {
        login.0 = Some(0);
    }
    if pass.iter().any(|i| *i == Interaction::Pressed) {
        login.0 = Some(1);
    }
}

/// The Vanguard board line on the login screen.
#[derive(Component)]
pub(crate) struct JoinBoardText;

/// Fill the login screen's Vanguard board once the fetch lands.
pub(crate) fn join_board_refresh(
    board: Res<VanguardBoardData>,
    mut q: Query<&mut Text, With<JoinBoardText>>,
) {
    let Ok(mut t) = q.single_mut() else { return };
    if !board.loaded {
        return;
    }
    if board.entries.is_empty() {
        **t = "No one has come back deep enough yet.".to_string();
        return;
    }
    **t = board
        .entries
        .iter()
        .take(8)
        .map(|e| format!("{:>2}.  {:<20}  {} m", e.rank, e.username, e.max_distance))
        .collect::<Vec<_>>()
        .join("\n");
}

/// Join-screen keyboard. Autoplay auto-connects as a guest. Otherwise: when a login
/// field is focused (click it), typing edits it and TAB switches fields. ENTER logs in
/// with the typed account (creating it on first use). The party is mustered in town and
/// co-op starts there too, so neither is reachable from here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn join_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    mut session: ResMut<Session>,
    mut login: ResMut<LoginFocus>,
    mut status_q: Query<&mut Text, With<StatusText>>,
) {
    // Autoplay / headless: skip the login UI, connect as a throwaway guest.
    if autoplay.0 && !session.connecting {
        session.connecting = true;
        session.username = std::env::var("MELD_NAME")
            .unwrap_or_else(|_| format!("guest{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        session.password = net::GUEST_PASSWORD.to_string();
        session.status = "connecting...".to_string();
        net.0.send(ClientCmd::Connect {
            username: session.username.clone(),
            password: session.password.clone(),
        });
    }

    if !session.connecting {
        let s = &mut *session;
        edit_login_field(
            &mut login.0,
            LoginKeys {
                tab: keys.just_pressed(KeyCode::Tab),
                backspace: keys.just_pressed(KeyCode::Backspace),
                shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
                typed: &keys.get_just_pressed().copied().collect::<Vec<_>>(),
            },
            &mut s.username,
            &mut s.password,
        );

        // ENTER = log in & play. Co-op is NOT startable here: a lobby wants a party,
        // and the party is assembled in town, so starting one from the login screen
        // means picking teammates before you have picked heroes. The city's [C] is the
        // one way in.
        if keys.just_pressed(KeyCode::Enter) {
            let user = session.username.trim().to_string();
            if user.is_empty() {
                session.status = "Enter a username to log in.".to_string();
            } else if session.password.is_empty() {
                session.status = "Enter a password.".to_string();
                login.0 = Some(1);
            } else {
                session.connecting = true;
                session.coop = false;
                session.status = "logging in...".to_string();
                let password = session.password.clone();
                net.0.send(ClientCmd::Connect { username: user, password });
            }
        }
    }

    if let Ok(mut t) = status_q.single_mut() {
        **t = session.status.clone();
    }
}



/// Render the account login fields: username as typed, password masked, a caret on
/// the focused field, and a gold border on it.
#[allow(clippy::type_complexity)]
pub(crate) fn join_login_refresh(
    time: Res<Time>,
    session: Res<Session>,
    login: Res<LoginFocus>,
    mut user_text: Query<&mut Text, (With<JoinUserText>, Without<JoinPassText>)>,
    mut pass_text: Query<&mut Text, (With<JoinPassText>, Without<JoinUserText>)>,
    mut user_border: Query<&mut BorderColor, (With<JoinUserField>, Without<JoinPassField>)>,
    mut pass_border: Query<&mut BorderColor, (With<JoinPassField>, Without<JoinUserField>)>,
) {
    let gold = glass::EDGE;
    // One caret for every typable box in the game (`glass::caret`), so a field that can be
    // typed into is recognisable as one wherever you meet it.
    let now = time.elapsed_secs();
    if let Ok(mut t) = user_text.single_mut() {
        **t = format!("{}{}", session.username, glass::caret(now, login.0 == Some(0)));
    }
    if let Ok(mut t) = pass_text.single_mut() {
        let masked: String = "\u{2022}".repeat(session.password.chars().count());
        **t = format!("{masked}{}", glass::caret(now, login.0 == Some(1)));
    }
    if let Ok(mut b) = user_border.single_mut() {
        *b = BorderColor::all(if login.0 == Some(0) { gold } else { glass::EDGE_SOFT });
    }
    if let Ok(mut b) = pass_border.single_mut() {
        *b = BorderColor::all(if login.0 == Some(1) { gold } else { glass::EDGE_SOFT });
    }
}

// ---------------------------------------------------------------- lobby ----

/// Marker for the descent screen's root, so `despawn::<DescendRoot>` clears it.
#[derive(Component)]
pub(crate) struct DescendRoot;

/// The live line — how long the wait has taken.
#[derive(Component)]
pub(crate) struct DescendStatus;

/// The pass the server says it is on, in a few words.
#[derive(Component)]
pub(crate) struct DescendPass;

/// One clause on what that pass actually does — the little detail that turns a step name
/// into something worth reading.
#[derive(Component)]
pub(crate) struct DescendDetail;

/// The filled part of the honest progress bar (a section count, never a timer).
#[derive(Component)]
pub(crate) struct DescendFill;

/// **WHAT THE SERVER SAID IT WAS DOING, LAST.** Fed by `run.generating` (see
/// `wr::Generating`), read by `render_descending`.
///
/// Every field is the server's own answer, so an empty `step` means the honest thing:
/// nothing has told us anything. That is a real case and not a bug — a **re-dive into a
/// world that already exists** generates nothing at all (the server builds one world per
/// instance), and a **restored** world is replayed from its seed in one un-narrated call. The
/// screen falls back to naming what a world is MADE of in those cases, rather than inventing
/// a pass nobody ran.
#[derive(Resource, Default)]
pub(crate) struct Descent {
    pub(crate) step: String,
    pub(crate) index: u32,
    pub(crate) total: u32,
    pub(crate) biome: Option<String>,
    pub(crate) attempt: u32,
}

/// What a pass is called, and one clause on what it is doing — the whole point of the
/// screen. Both halves are true of the code that reports them (`meld_world::GenStage`); a
/// step this does not know is named plainly rather than dressed up, because a wrong
/// explanation is worse than none.
fn pass_words(d: &Descent) -> (String, &'static str) {
    match d.step.as_str() {
        "maze" => (
            "the maze is decided".into(),
            "which walls stand, and where the passes through them are left",
        ),
        "section" => {
            let where_ = match d.biome.as_deref() {
                Some(b) if !b.is_empty() => format!("the {} is laid down", biome_words(b)),
                _ => "the ground is laid down".into(),
            };
            (where_, "its ranges raised, its rivers walked downhill, its wildlife placed")
        }
        "bend" => (
            "the corridor is bent into an arc".into(),
            "so the world fans out around the hub in every direction but west",
        ),
        "route" => (
            "the way out is walked".into(),
            "end to end, so a route through is guaranteed before you set foot on it",
        ),
        "restart" => (
            "the route did not hold".into(),
            "this world is discarded, and another is drawn from scratch",
        ),
        // A world §W5 persisted is rebuilt, not loaded: the generator runs in full and then
        // these two passes put back what players changed. Both carry their own count, so
        // they drive the bar exactly as sections do.
        "stream" => (
            "the frontier is walked back out".into(),
            "as far as anyone had reached, because a world remembers how far it got",
        ),
        "shift" => (
            "the Shifts are replayed".into(),
            "every turn of the weather this world has already lived through, in order",
        ),
        // ⚠️ **NOTHING HAS SPOKEN YET, AND THIS ARM MUST NOT SAY WHAT THE SUBTITLE SAYS.**
        // It used to read "the world is being drawn" — verbatim the static line above it, at
        // a smaller size, so the screen's whole silent state was one sentence printed twice
        // and an empty bar. That is what the wait looked like for every re-dive into a
        // persisted seed until the restore path started narrating, and it is still what the
        // first instant of any dive looks like.
        "" => (
            "the threshold is opening".into(),
            "a world already drawn is entered without a word — only a new one has passes to name",
        ),
        other => (other.to_string(), "the world is being drawn"),
    }
}

/// How full the honest bar is, 0-100.
///
/// ANY pass that carries a count drives it — sections, and a restore's streamed frontier and
/// replayed Shifts. This keyed on the literal `"section"`, which was right while that was the
/// only counted pass and would silently have left the two LONGEST passes in the game at zero.
/// The passes with no count of their own are pinned to the ends they sit at — the maze before
/// any ground, the bend and the route walk after all of it — which is a fact about the order,
/// not a guess about the clock.
///
/// Extracted from the render system purely so the rule can be held by test; a bar that is a
/// lie is the one thing `WG-12` set out not to ship.
fn descent_fill_pct(d: &Descent) -> f32 {
    if d.total > 0 {
        100.0 * d.index as f32 / d.total as f32
    } else if matches!(d.step.as_str(), "bend" | "route") {
        100.0
    } else {
        0.0
    }
}

/// "amber_wood" → "Amber Wood". `title_case` alone leaves the underscore in, and a biome
/// with one in its key is the only kind this screen ever gets wrong.
fn biome_words(key: &str) -> String {
    key.split('_')
        .map(crate::world_render::title_case)
        .collect::<Vec<_>>()
        .join(" ")
}

/// **THE WORLD IS BEING MADE, AND THE PLAYER SHOULD SEE THAT.**
///
/// Between the dive and `run.started` the server builds an entire world: the maze is decided,
/// its ranges raised, its rivers walked downhill, its guaranteed route routed. That is the most
/// expensive thing this game does. The player used to wait through it in the city, looking at a
/// one-line status string in the bottom strip — *"stepping through The Threshold…"* — which
/// does not read as work happening. It reads as a hang.
///
/// ⚠️ **THE READOUT IS HONEST, WHICH IS THE WHOLE POINT — AND NOW IT CAN AFFORD TO SAY MORE.**
/// This comment used to explain why the screen showed nothing but an elapsed clock: the client
/// had no idea how far along generation was, because the server sent nothing until it was
/// finished, and a bar that fills on a timer is a lie that gets found out the first time a
/// world takes twice as long.
///
/// The server tells us now (`run.generating` — see `Descent`), so the pass name, the clause
/// under it and the bar are all the server's own answers about work it has actually done. The
/// clock stays, because it is the one thing that is true even when nothing has spoken: a
/// re-dive into a live world generates nothing at all, and a restored world is replayed in one
/// un-narrated call.
///
/// **Reported at 3.4-4.2 s in release for the initial chain**, and several times that when the
/// route does not hold — this is not a flash the player never sees.
pub(crate) fn descending_ui(mut commands: Commands, mut descent: ResMut<Descent>) {
    // A previous dive's last pass is not this one's first. Forget it, or a re-dive opens on
    // "the way out is walked" for a world nobody is drawing.
    *descent = match crate::flags::descend_stage_flag() {
        // `MELD_DESCEND=<step>` stages one pass for a screenshot. Every COUNTED pass gets a
        // count — a section, and a restore's streamed frontier and replayed Shifts — since a
        // part-filled bar with a number beside it is the frame the layout has to survive.
        // A section gets a country too.
        Some(step) => {
            let counted = matches!(step.as_str(), "section" | "stream" | "shift");
            Descent {
                index: if counted { 3 } else { 0 },
                total: if counted { 8 } else { 0 },
                biome: (step == "section").then(|| "amber_wood".to_string()),
                attempt: if step == "restart" { 2 } else { 1 },
                step,
            }
        }
        None => Descent::default(),
    };
    commands
        .spawn((
            DescendRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(18.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.06, 1.0)),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("THE THRESHOLD"),
                TextFont { font_size: FontSize::Px(44.0), ..default() },
                TextColor(Color::srgb(0.86, 0.91, 1.0)),
            ));
            p.spawn((
                // ⚠️ Not "made FOR YOU": a world outlives its divers (CANON §W1) and a co-op
                // dive joins one somebody else opened, so the personal phrasing is false half
                // the time. It IS always being drawn, though — restoring a saved world
                // regenerates it from its seed and replays its Shift log, which is the same
                // work.
                Text::new("The world is being drawn."),
                TextFont { font_size: FontSize::Px(22.0), ..default() },
                TextColor(Color::srgb(0.72, 0.80, 0.95)),
            ));
            // The pass the server is on, and one clause on what that pass does. Not invented
            // steps: `meld_world::GenStage` reports the real ones, in the order they run.
            p.spawn((
                DescendPass,
                Text::new(""),
                TextFont { font_size: FontSize::Px(20.0), ..default() },
                TextColor(Color::srgb(0.86, 0.91, 1.0)),
            ));
            p.spawn((
                DescendDetail,
                Text::new(""),
                TextFont { font_size: FontSize::Px(15.0), ..default() },
                TextColor(Color::srgb(0.50, 0.58, 0.74)),
            ));
            // The bar. Fixed width so it never resizes the column as the fill moves, and it
            // stays in place (empty) when nothing has told us a count — a bar that appears
            // and disappears reads as a glitch, where an empty one reads as "not yet".
            p.spawn((
                Node {
                    width: Val::Px(420.0),
                    height: Val::Px(6.0),
                    margin: UiRect::top(Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.30, 0.38, 0.55, 0.35)),
            ))
            .with_children(|bar| {
                bar.spawn((
                    DescendFill,
                    Node { width: Val::Percent(0.0), height: Val::Percent(100.0), ..default() },
                    BackgroundColor(Color::srgb(0.62, 0.74, 0.98)),
                ));
            });
            p.spawn((
                DescendStatus,
                Text::new(""),
                TextFont { font_size: FontSize::Px(18.0), ..default() },
                TextColor(Color::srgb(0.78, 0.86, 1.0)),
            ));
        });
}

/// Ticks the descent readout: the pass the server is on, what that pass does, an honest bar,
/// and a moving ellipsis + elapsed seconds so the screen is visibly alive even when nothing
/// has spoken.
pub(crate) fn render_descending(
    time: Res<Time>,
    descent: Res<Descent>,
    mut started: Local<f32>,
    mut status: Query<&mut Text, With<DescendStatus>>,
    mut pass: Query<&mut Text, (With<DescendPass>, Without<DescendStatus>)>,
    mut detail: Query<
        &mut Text,
        (With<DescendDetail>, Without<DescendStatus>, Without<DescendPass>),
    >,
    mut fill: Query<&mut Node, With<DescendFill>>,
) {
    let Ok(mut t) = status.single_mut() else {
        // Not on this screen: forget the clock so the next descent starts from zero.
        *started = 0.0;
        return;
    };
    let now = time.elapsed_secs();
    if *started <= 0.0 {
        *started = now;
    }
    let secs = (now - *started).max(0.0);
    let dots = ".".repeat(1 + ((secs * 2.0) as usize % 3));
    // ⚠️ The number appears only after a couple of seconds. A counter that starts at 0.0s on a
    // world that arrives in 200 ms is noise flashing past; one that appears when the wait
    // becomes noticeable is the screen answering the question the player has just started
    // asking.
    **t = if secs < 2.0 {
        format!("stepping through{dots}")
    } else {
        format!("stepping through{dots}    {secs:.0}s")
    };

    let (name, clause) = pass_words(&descent);
    if let Ok(mut p) = pass.single_mut() {
        // The section count rides the pass line rather than the bar, because a bar with no
        // number on it cannot say whether it is stuck. An attempt past the first is named
        // too: a re-draw is otherwise indistinguishable from the first one hanging.
        let count = if descent.total > 0 {
            format!("      {} of {}", descent.index, descent.total)
        } else {
            String::new()
        };
        let again = if descent.attempt > 1 {
            format!("      attempt {}", descent.attempt)
        } else {
            String::new()
        };
        **p = format!("{name}{count}{again}");
    }
    if let Ok(mut d) = detail.single_mut() {
        **d = clause.to_string();
    }
    if let Ok(mut f) = fill.single_mut() {
        f.width = Val::Percent(descent_fill_pct(&descent));
    }
}

pub(crate) fn lobby_ui(mut commands: Commands) {
    commands
        .spawn((
            LobbyRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("CO-OP LOBBY"),
                TextFont { font_size: FontSize::Px(40.0), ..default() },
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
            ));
            p.spawn((
                LobbyText,
                Text::new(""),
                TextFont { font_size: FontSize::Px(20.0), ..default() },
                TextColor(Color::srgb(0.8, 0.88, 1.0)),
            ));
            // Tap actions. Which are visible depends on lobby state (`render_lobby`
            // toggles them): Create before you're in a lobby; Ready/Start/Leave once
            // you are. Typing/joining by CODE stays on the keyboard (text entry).
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(12.0),
                margin: UiRect::top(Val::Px(8.0)),
                ..default()
            })
            .with_children(|row| {
                for (act, label) in [
                    (LobbyAct::Create, "Create Lobby"),
                    (LobbyAct::Ready, "Ready"),
                    (LobbyAct::Start, "Start"),
                    (LobbyAct::Leave, "Leave"),
                ] {
                    lobby_button(row, act, label);
                }
            });
        });
}

/// A tap action in the co-op lobby (mirrors the keyboard, except code entry).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum LobbyAct {
    Create,
    Ready,
    Start,
    Leave,
}

/// Marks a tappable lobby button; `render_lobby` shows/hides it per lobby state.
#[derive(Component)]
pub(crate) struct LobbyButton(pub(crate) LobbyAct);

/// Spawn one lobby button (starts hidden; `render_lobby` reveals the relevant ones).
fn lobby_button(parent: &mut ChildSpawnerCommands, act: LobbyAct, label: &str) {
    parent
        .spawn((
            Button,
            LobbyButton(act),
            Node {
                border_radius: BorderRadius::all(Val::Px(8.0)),
                display: Display::None,
                padding: UiRect::axes(Val::Px(18.0), Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.5)),
                ..default()
            },
            BorderColor::all(glass::EDGE_SOFT),
            BackgroundColor(glass::GLASS),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: FontSize::Px(18.0), ..default() },
                TextColor(Color::srgb(0.9, 0.94, 1.0)),
            ));
        });
}

/// Map a just-pressed key to a lobby-code character (A–Z, 0–9).
pub(crate) fn key_to_code_char(key: KeyCode) -> Option<char> {
    use KeyCode::*;
    let c = match key {
        KeyA => 'A', KeyB => 'B', KeyC => 'C', KeyD => 'D', KeyE => 'E', KeyF => 'F',
        KeyG => 'G', KeyH => 'H', KeyI => 'I', KeyJ => 'J', KeyK => 'K', KeyL => 'L',
        KeyM => 'M', KeyN => 'N', KeyO => 'O', KeyP => 'P', KeyQ => 'Q', KeyR => 'R',
        KeyS => 'S', KeyT => 'T', KeyU => 'U', KeyV => 'V', KeyW => 'W', KeyX => 'X',
        KeyY => 'Y', KeyZ => 'Z',
        Digit0 => '0', Digit1 => '1', Digit2 => '2', Digit3 => '3', Digit4 => '4',
        Digit5 => '5', Digit6 => '6', Digit7 => '7', Digit8 => '8', Digit9 => '9',
        _ => return None,
    };
    Some(c)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lobby_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    session: Res<Session>,
    mut lobby: ResMut<LobbyData>,
    mut next: ResMut<NextState<Screen>>,
) {
    if !lobby.in_lobby {
        // ⚠️ **ESC GETS YOU OUT BEFORE YOU HAVE JOINED ANYTHING, TOO.** Escape used to be
        // handled only in the in-lobby branch below, and this one early-`return`s — so a
        // player who opened co-op by mistake and had not yet typed a code had no way back
        // at all. Reported from play: "there is no way to leave the lobby for co-op… you
        // should hit esc to leave if you didn't mean to be there."
        //
        // No `LobbyLeave` sent: there is nothing to leave yet, and telling the server you
        // left a lobby you were never in is a message that means nothing.
        if keys.just_pressed(KeyCode::Escape) {
            leave_lobby_state(&mut lobby, &mut next);
            return;
        }
        // **TWO FIELDS, BECAUSE THEY ARE OPPOSITE ACTIONS** (SC-3). A CODE joins somebody
        // else's group; a SEED names the world your own group is going to. Collapsing
        // them onto one line would mean guessing which a player meant from what they
        // typed — and an all-digit join code is a perfectly ordinary join code.
        if keys.just_pressed(KeyCode::Tab) {
            lobby.editing_seed = !lobby.editing_seed;
            return;
        }
        // Not in a lobby yet: create one, or type a code and join.
        if keys.just_pressed(KeyCode::Enter) {
            // ENTER with no code = create; with a code = join. A seed only means anything
            // on the create side: a joiner is going wherever the host already named, and
            // the server tells them where that is on `lobby.state`.
            if lobby.code_input.is_empty() {
                net.0.send(ClientCmd::LobbyCreate {
                    party: session.party.clone(),
                    seed: lobby.seed_input.parse::<u64>().ok(),
                });
            } else {
                net.0.send(ClientCmd::LobbyJoin {
                    code: lobby.code_input.clone(),
                    party: session.party.clone(),
                });
            }
            return;
        }
        if keys.just_pressed(KeyCode::Backspace) {
            if lobby.editing_seed {
                lobby.seed_input.pop();
            } else {
                lobby.code_input.pop();
            }
        }
        for key in keys.get_just_pressed() {
            if lobby.editing_seed {
                // Digits only: a seed is a number, and `u64::MAX` is 20 digits.
                if lobby.seed_input.len() < 20 {
                    if let Some(c) = key_to_code_char(*key).filter(char::is_ascii_digit) {
                        lobby.seed_input.push(c);
                    }
                }
            } else if lobby.code_input.len() < 6 {
                if let Some(c) = key_to_code_char(*key) {
                    lobby.code_input.push(c);
                }
            }
        }
        return;
    }

    // In a lobby: ready up, start (host), or leave.
    if keys.just_pressed(KeyCode::KeyR) {
        let want = !lobby.my_ready;
        lobby.my_ready = want;
        net.0.send(ClientCmd::LobbyReady { ready: want });
    }
    if keys.just_pressed(KeyCode::Enter) && lobby.host == session.player_id {
        net.0.send(ClientCmd::LobbyStart);
    }
    if keys.just_pressed(KeyCode::Escape) {
        net.0.send(ClientCmd::LobbyLeave);
        leave_lobby_state(&mut lobby, &mut next);
    }
}

/// Drop every trace of the lobby and go back to the city.
///
/// ⚠️ **`my_ready` is the one that was being left behind.** Both exits cleared `in_lobby`
/// and `code_input` and neither cleared the ready flag, so it survived leaving: join a
/// second lobby and the client believed you had already readied up, showed Ready as
/// toggled, and the next `[R]` sent `ready: false` — un-readying you in a lobby you had
/// never readied in. Shared by the key and the button so a third exit cannot forget again.
fn leave_lobby_state(lobby: &mut LobbyData, next: &mut NextState<Screen>) {
    lobby.in_lobby = false;
    lobby.code_input.clear();
    lobby.seed_input.clear();
    lobby.editing_seed = false;
    lobby.seed = None;
    lobby.my_ready = false;
    next.set(Screen::City);
}

#[allow(clippy::type_complexity)]
/// Which lobby buttons apply right now: Create before you are in one, Ready once in,
/// Start only for the host — and **LEAVE IN BOTH STATES**.
///
/// That last one is the way out of the SCREEN rather than out of a lobby, which is what a
/// player who opened co-op by mistake reaches for, before they have joined anything. It is
/// pulled out of the render loop so `a_player_can_always_leave_the_lobby_screen` can hold
/// it: the rule was a one-line arm inside a `for` over `Node`s, where nothing could reach
/// it, and it had already been wrong once.
pub(crate) fn lobby_button_visible(act: LobbyAct, in_lobby: bool, host_is_me: bool) -> bool {
    match act {
        LobbyAct::Create => !in_lobby,
        LobbyAct::Ready => in_lobby,
        LobbyAct::Leave => true,
        LobbyAct::Start => in_lobby && host_is_me,
    }
}

pub(crate) fn render_lobby(
    lobby: Res<LobbyData>,
    session: Res<Session>,
    mut q: Query<&mut Text, With<LobbyText>>,
    mut btns: Query<(&LobbyButton, &mut Node), Without<LobbyText>>,
) {
    // Reveal only the buttons that apply to the current lobby state: Create before
    // you're in a lobby; Ready/Leave once in; Start only for the host.
    let host_is_me = lobby.host == session.player_id;
    for (btn, mut node) in &mut btns {
        let show = lobby_button_visible(btn.0, lobby.in_lobby, host_is_me);
        node.display = if show { Display::Flex } else { Display::None };
    }
    let Ok(mut t) = q.single_mut() else { return };
    if !lobby.in_lobby {
        // The caret marks which field [Tab] is on, so "why is nothing typing" is never a
        // question — with two fields and one keyboard, an invisible focus is a dead key.
        let (code_caret, seed_caret) = if lobby.editing_seed { ("", "_") } else { ("_", "") };
        let world = if lobby.seed_input.is_empty() {
            "a new world".to_string()
        } else {
            format!("world {}", lobby.seed_input)
        };
        **t = format!(
            "Join code: {}{code_caret}\n     World: {}{seed_caret}   ({world})\n\n             [TAB] switch field\n\ntype a code + ENTER to join someone,\n             or ENTER with no code to create a lobby\n\n[ESC] back to the city",
            lobby.code_input, lobby.seed_input,
        );
        return;
    }
    let host_is_me = lobby.host == session.player_id;
    // The world is the SERVER's answer, never the host's own input echoed back — a
    // joiner never typed one and still has to see which place they are agreeing to go to.
    let world = match lobby.seed {
        Some(seed) => format!("World {seed}"),
        None => "World: a new one".to_string(),
    };
    let mut lines = vec![format!("Code: {}    {world}", lobby.code), String::new()];
    for (id, username, ready) in &lobby.members {
        let you = if id == &session.player_id { " (you)" } else { "" };
        let host = if id == &lobby.host { " [host]" } else { "" };
        let tag = if *ready { "READY" } else { "..." };
        lines.push(format!("  {username}{you}{host}  -  {tag}"));
    }
    lines.push(String::new());
    let all_ready = !lobby.members.is_empty() && lobby.members.iter().all(|(_, _, r)| *r);
    let start = if host_is_me {
        if all_ready { "ENTER: start the run" } else { "ENTER: start (need everyone READY)" }
    } else {
        "waiting for the host to start..."
    };
    lines.push(format!("R: toggle ready    {start}    ESC: leave"));
    **t = lines.join("\n");
}

/// Tap handler for the lobby buttons — same effects as [`lobby_input`] (except code
/// entry, which stays on the keyboard).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lobby_buttons(
    q: Query<(&Interaction, &LobbyButton), Changed<Interaction>>,
    net: NonSend<NetRes>,
    session: Res<Session>,
    mut lobby: ResMut<LobbyData>,
    mut next: ResMut<NextState<Screen>>,
) {
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match btn.0 {
            LobbyAct::Create => {
                net.0.send(ClientCmd::LobbyCreate {
                    party: session.party.clone(),
                    seed: lobby.seed_input.parse::<u64>().ok(),
                });
            }
            LobbyAct::Ready => {
                let want = !lobby.my_ready;
                lobby.my_ready = want;
                net.0.send(ClientCmd::LobbyReady { ready: want });
            }
            LobbyAct::Start => {
                if lobby.host == session.player_id {
                    net.0.send(ClientCmd::LobbyStart);
                }
            }
            LobbyAct::Leave => {
                // Only tell the server when there is something to leave.
                if lobby.in_lobby {
                    net.0.send(ClientCmd::LobbyLeave);
                }
                leave_lobby_state(&mut lobby, &mut next);
            }
        }
    }
}

// ----------------------------------------------------------------- ended ---

pub(crate) fn ended_ui(mut commands: Commands, end: Res<EndInfo>) {
    let (title, color): (String, Color) = match end.outcome.as_str() {
        "victory" => ("VICTORY - the creature is slain!".into(), Color::srgb(0.5, 0.95, 0.6)),
        "extracted" => {
            let mut msg = format!(
                "EXTRACTED - banked {} item(s) + {} chits to your Vault",
                end.banked, end.chits
            );
            if end.gear > 0 {
                msg.push_str(&format!(" - {} red-chest gear", end.gear));
            }
            (msg, Color::srgb(0.4, 0.9, 0.95))
        }
        "defeat" | "died" => {
            let msg = if end.chits > 0 {
                format!("DEFEAT - your hero has fallen. Lost {} chits.", end.chits)
            } else {
                "DEFEAT - your hero has fallen.".into()
            };
            (msg, Color::srgb(0.95, 0.4, 0.4))
        }
        // WG-4: walked back west across the border into Last City (run abandoned).
        "abandoned" => (
            "RETURNED to Last City - you slipped back west through the wall.".into(),
            Color::srgb(0.75, 0.85, 0.95),
        ),
        _ => ("The run is over.".into(), Color::srgb(0.8, 0.8, 0.8)),
    };
    commands
        .spawn((
            EndedRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(title),
                TextFont {
                    font_size: FontSize::Px(40.0),
                    ..default()
                },
                TextColor(color),
            ));
            // The wipe's bill, itemised per hero. A TPK is the largest durability charge
            // in the game and the one a player is least able to infer, since every hero
            // in the party paid it at once.
            for (hero, points, burned) in &end.worn {
                p.spawn((
                    Text::new(format!("{hero} fell: kit worn -{points} durability")),
                    TextFont {
                        font_size: FontSize::Px(18.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.75, 0.35)),
                ));
                // The ephemeral half of the bill, named. It is not repairable and not
                // recoverable, so it belongs on the death screen more than the durability
                // line does — that one is an invoice, this one is a eulogy.
                for name in burned {
                    p.spawn((
                        Text::new(format!("{name} burned - Ephemeral, gone with them")),
                        TextFont {
                            font_size: FontSize::Px(16.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.98, 0.62, 0.35)),
                    ));
                }
            }
            if !end.worn.is_empty() {
                p.spawn((
                    Text::new("Repair at the Forge before your next dive.".to_string()),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.7, 0.75, 0.85)),
                ));
            }
            p.spawn((
                Text::new("Tap a button below, or press ENTER to return / ESC to quit"),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
            // Tap equivalents of Enter / Esc, so the summary is click/tap driven too.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(14.0),
                margin: UiRect::top(Val::Px(6.0)),
                ..default()
            })
            .with_children(|row| {
                ended_button(row, EndedAct::Continue, "Return to The Last City", Color::srgb(0.35, 0.55, 0.85));
                ended_button(row, EndedAct::Quit, "Quit", Color::srgb(0.55, 0.3, 0.3));
            });
        });
}

/// A tap action on the run-summary screen (mirrors ENTER / ESC).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum EndedAct {
    Continue,
    Quit,
}

/// Marks a tappable button on the Ended screen.
#[derive(Component)]
pub(crate) struct EndedButton(pub(crate) EndedAct);

/// Spawn one Ended-screen button.
fn ended_button(parent: &mut ChildSpawnerCommands, act: EndedAct, label: &str, bg: Color) {
    parent
        .spawn((
            Button,
            EndedButton(act),
            Node {
                border_radius: BorderRadius::all(Val::Px(8.0)),
                padding: UiRect::axes(Val::Px(20.0), Val::Px(12.0)),
                border: UiRect::all(Val::Px(1.5)),
                ..default()
            },
            BorderColor::all(Color::srgb(0.5, 0.6, 0.85)),
            BackgroundColor(bg),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: FontSize::Px(18.0), ..default() },
                TextColor(Color::srgb(0.95, 0.97, 1.0)),
            ));
        });
}

pub(crate) fn ended_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut session: ResMut<Session>,
    mut next: ResMut<NextState<Screen>>,
    mut exit: MessageWriter<AppExit>,
) {
    // Return to the hub — banked loot (or, on death, your insured blue gear) is
    // waiting there. `city_ui` re-fetches the Vault and re-arms the next dive.
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        session.channeling = false;
        session.status.clear();
        next.set(Screen::City);
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

/// Tap handler for the Ended screen buttons — same effects as [`ended_input`].
pub(crate) fn ended_buttons(
    q: Query<(&Interaction, &EndedButton), Changed<Interaction>>,
    mut session: ResMut<Session>,
    mut next: ResMut<NextState<Screen>>,
    mut exit: MessageWriter<AppExit>,
) {
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match btn.0 {
            EndedAct::Continue => {
                session.channeling = false;
                session.status.clear();
                next.set(Screen::City);
            }
            EndedAct::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// **EVERY PASS THE SERVER CAN REPORT HAS WORDS ON THIS SIDE.** `pass_words` falls back
    /// to printing the bare key, so a step nobody wrote a line for does not fail — it just
    /// puts `section` on screen where a sentence should be, which is this repo's oldest
    /// failure mode (a token the client never renders). `wr::Generating::STEPS` is the one
    /// list both sides read; the server `debug_assert`s against it too.
    #[test]
    fn every_generation_pass_has_words_for_the_player() {
        for step in meld_proto::realtime::run::Generating::STEPS {
            let d = Descent { step: step.to_string(), ..Descent::default() };
            let (name, clause) = pass_words(&d);
            assert_ne!(name, step, "the `{step}` pass renders as its own wire key");
            assert!(
                name.len() > step.len() && clause.len() > 20,
                "the `{step}` pass has no readable line: {name:?} / {clause:?}"
            );
        }
    }

    /// **THE SILENT STATE MUST NOT ECHO THE LINE ABOVE IT.** Before the server has said
    /// anything the screen shows its static subtitle *and* `pass_words("")`, stacked — and
    /// those two were the same sentence at two sizes, with an empty bar under them. It read
    /// as a half-built screen rather than as a wait, and until the restore path started
    /// narrating it was the ONLY thing a re-dive into a persisted seed ever showed.
    #[test]
    fn the_wordless_state_does_not_repeat_the_subtitle() {
        let (name, clause) = pass_words(&Descent::default());
        let subtitle = "The world is being drawn.";
        let same = |a: &str| a.trim_end_matches('.').eq_ignore_ascii_case(subtitle.trim_end_matches('.'));
        assert!(!same(&name), "the silent state prints the subtitle back at the player: {name:?}");
        assert!(!same(clause), "the silent state's clause is the subtitle: {clause:?}");
    }

    /// Every pass that carries a count fills the bar, and the two a RESTORE adds are the
    /// longest ones in the game — keying the fill on the literal `"section"` left them at
    /// zero for the whole wait they exist to measure.
    #[test]
    fn every_counted_pass_can_fill_the_bar() {
        for step in ["section", "stream", "shift"] {
            let d = Descent { step: step.to_string(), index: 3, total: 8, ..Descent::default() };
            assert!(
                descent_fill_pct(&d) > 0.0,
                "the `{step}` pass carries a count and still cannot move the bar"
            );
        }
    }

    /// A section names the ground it laid, and an underscored key is the only kind the screen
    /// could get wrong — `title_case` alone leaves "amber_wood" in the sentence.
    #[test]
    fn a_section_names_the_country_it_just_made() {
        let d = Descent {
            step: "section".into(),
            index: 3,
            total: 8,
            biome: Some("amber_wood".into()),
            attempt: 1,
        };
        assert_eq!(pass_words(&d).0, "the Amber Wood is laid down");
    }

    /// Every class card's kit comes from the ONE registry, so a card can never again name
    /// an ability that was renamed or put its unlocks at the wrong level — the Explorer's
    /// card still said "Set Anchor" at Lv9 long after the registry said otherwise.
    #[test]
    fn every_class_card_reads_its_kit_from_the_registry() {
        for ci in CLASS_INFO.iter() {
            let defs = meld_proto::skills::skills_for_class(ci.key);
            assert!(!defs.is_empty(), "{} has no abilities in the registry", ci.key);
            let text = kit_text(ci);
            for def in defs.iter().take(KIT_ROWS) {
                assert!(
                    text.contains(def.name),
                    "{}'s card omits {} - the card and the registry have drifted",
                    ci.key,
                    def.name
                );
            }
        }
    }

    /// And the ability that used to be called "Set Anchor" is gone from the setting: an
    /// Anchor takes three orders to make and only an Explorer of Serin may set one, so a
    /// routine party Barrier must not borrow the name (docs/lore/factions.md).
    #[test]
    fn no_class_claims_to_set_an_anchor() {
        for ci in CLASS_INFO.iter() {
            let text = kit_text(ci);
            assert!(!text.contains("Set Anchor"), "{} still claims to set Anchors", ci.key);
        }
        assert!(
            kit_text(class_info("explorer")).contains("Stable Ground"),
            "the Explorer should offer Stable Ground instead"
        );
    }

    use super::*;

    /// **A PLAYER CAN ALWAYS LEAVE THE LOBBY SCREEN.** Leave was gated on `in_lobby`, so
    /// opening Co-op and changing your mind before creating or joining left exactly one
    /// button on screen (Create). Fixed in `#374`; held here, because the rule lived as a
    /// one-line arm inside a `for` over `Node`s where no test could reach it.
    #[test]
    fn a_player_can_always_leave_the_lobby_screen() {
        for in_lobby in [false, true] {
            for host_is_me in [false, true] {
                assert!(
                    lobby_button_visible(LobbyAct::Leave, in_lobby, host_is_me),
                    "no way out at in_lobby={in_lobby} host={host_is_me}"
                );
            }
        }
    }

    /// The rest of the rule, so widening Leave did not quietly widen everything.
    #[test]
    fn the_other_lobby_buttons_still_follow_the_state() {
        assert!(lobby_button_visible(LobbyAct::Create, false, false));
        assert!(!lobby_button_visible(LobbyAct::Create, true, false));
        assert!(!lobby_button_visible(LobbyAct::Ready, false, false));
        assert!(lobby_button_visible(LobbyAct::Ready, true, false));
        // Start is the host's alone, and only once there is a lobby to start.
        assert!(lobby_button_visible(LobbyAct::Start, true, true));
        assert!(!lobby_button_visible(LobbyAct::Start, true, false));
        assert!(!lobby_button_visible(LobbyAct::Start, false, true));
    }

    /// **LEAVING FORGETS YOU WERE READY.** `my_ready` survived both exits, so the next
    /// lobby you joined thought you had already readied up — and the next `[R]` sent
    /// `ready: false`, un-readying you in a lobby you had never readied in.
    #[test]
    fn leaving_a_lobby_clears_the_ready_flag() {
        let mut lobby = LobbyData {
            in_lobby: true,
            my_ready: true,
            code_input: "ABC123".into(),
            ..Default::default()
        };
        let mut next = NextState::<Screen>::default();
        leave_lobby_state(&mut lobby, &mut next);
        assert!(!lobby.my_ready, "a stale ready flag follows you into the next lobby");
        assert!(!lobby.in_lobby);
        assert!(lobby.code_input.is_empty());
    }

    fn keys(typed: &[KeyCode]) -> LoginKeys<'_> {
        LoginKeys { tab: false, backspace: false, shift: false, typed }
    }

    #[test]
    fn tab_reaches_the_fields_from_an_unfocused_screen() {
        let mut focus = None;
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(&mut focus, LoginKeys { tab: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(focus, Some(0), "TAB with nothing focused must land on the username");
        edit_login_field(&mut focus, LoginKeys { tab: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(focus, Some(1), "TAB again crosses to the password");
        edit_login_field(&mut focus, LoginKeys { tab: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(focus, Some(0), "and back — the two fields cycle");
    }

    #[test]
    fn typing_lands_in_the_focused_field_only() {
        let mut focus = Some(0);
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(&mut focus, keys(&[KeyCode::KeyA, KeyCode::KeyB]), &mut u, &mut p);
        assert_eq!((u.as_str(), p.as_str()), ("ab", ""));

        focus = Some(1);
        edit_login_field(&mut focus, keys(&[KeyCode::Digit7]), &mut u, &mut p);
        assert_eq!((u.as_str(), p.as_str()), ("ab", "7"));
    }

    #[test]
    fn shift_uppercases_and_backspace_deletes_one() {
        let mut focus = Some(0);
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(
            &mut focus,
            LoginKeys { shift: true, ..keys(&[KeyCode::KeyD]) },
            &mut u,
            &mut p,
        );
        edit_login_field(&mut focus, keys(&[KeyCode::KeyO]), &mut u, &mut p);
        assert_eq!(u, "Do");
        edit_login_field(&mut focus, LoginKeys { backspace: true, ..keys(&[]) }, &mut u, &mut p);
        assert_eq!(u, "D");
        assert!(p.is_empty(), "backspace must not reach across fields");
    }

    #[test]
    fn a_field_stops_growing_at_its_cap() {
        let mut focus = Some(0);
        let (mut u, mut p) = ("x".repeat(LOGIN_FIELD_MAX), String::new());
        edit_login_field(&mut focus, keys(&[KeyCode::KeyA]), &mut u, &mut p);
        assert_eq!(u.chars().count(), LOGIN_FIELD_MAX);
    }

    #[test]
    fn an_unfocused_screen_swallows_typing() {
        let mut focus = None;
        let (mut u, mut p) = (String::new(), String::new());
        edit_login_field(&mut focus, keys(&[KeyCode::KeyA]), &mut u, &mut p);
        assert!(u.is_empty() && p.is_empty());
        assert_eq!(focus, None, "which is why join_ui focuses a field up front");
    }
}
