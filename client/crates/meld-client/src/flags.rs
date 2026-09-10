//! Launch-time flags: the server URL plus the `MELD_*` dev/QA toggles.
//!
//! Every flag used to be written TWICE — an env reader for the native build and a
//! `?query=` reader for the browser — which is what made this file 336 lines for ~25
//! booleans. The wasm client is gone, so there is one reader per flag and env is the
//! only channel. A new flag is one function; if it ever needs a second channel again,
//! give it one deliberately rather than by cfg-duplicating the whole file.

/// Where the API + realtime socket live: `MELD_SERVER`, defaulting to localhost. The
/// `embedded-server` build overwrites it in `main` before this is read, so a
/// self-contained binary points at its own in-process server.
pub(crate) fn server_base() -> String {
    std::env::var("MELD_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}

/// Autopilot self-drives the loop (connect → walk → attack) for demos and
/// headless screenshots (`MELD_AUTOPLAY`). Real players use the keyboard as normal.
pub(crate) fn autoplay_flag() -> bool {
    std::env::var("MELD_AUTOPLAY").is_ok()
}
/// With autoplay, enter the maze but **idle** at the hub instead of walking east —
/// a stable overworld frame for screenshotting the world art. `MELD_IDLE`.
pub(crate) fn world_idle_flag() -> bool {
    std::env::var("MELD_IDLE").is_ok()
}

/// Connect, then **park in The Last City** (the hub city) instead of diving — a stable
/// City frame for screenshotting / iterating on the hub. Reuses the autoplay
/// connect path but gates the auto-dive. `MELD_CITY`.
pub(crate) fn city_idle_flag() -> bool {
    std::env::var("MELD_CITY").is_ok()
}

/// **Hold the descent screen** — the transition that covers world generation. It is normally
/// on screen for as long as the server takes to build a world, which on a warm machine is a
/// flash, so there is no way to look at it otherwise. `MELD_DESCEND`.
pub(crate) fn descend_preview_flag() -> bool {
    std::env::var("MELD_DESCEND").is_ok()
}

/// **Pin the descent screen to one generation PASS** — `MELD_DESCEND=section` (or `maze` /
/// `bend` / `route` / `restart`).
///
/// ⚠️ Holding the screen is not enough to look at it. The passes are real
/// (`run.generating`), so what survives on a held screen is whichever arrived LAST — the
/// route walk, with no biome and a full bar. The one frame worth judging, a section mid-chain
/// naming its own country, is unreachable without this. Same argument as `MELD_OPENING`
/// staging an ambush and `MELD_TALLY` holding a haul: a state you cannot put on screen on
/// demand cannot be developed, only guessed at.
///
/// While it is set the live messages are IGNORED rather than merged, or the pinned frame would
/// be overwritten a tick later by the real thing.
pub(crate) fn descend_stage_flag() -> Option<String> {
    let v = std::env::var("MELD_DESCEND").ok()?;
    let v = v.trim();
    meld_proto::realtime::run::Generating::STEPS
        .iter()
        .find(|s| **s == v)
        .map(|s| s.to_string())
}

/// Open the Apothecary's shelf on arrival (with `MELD_CITY`) — a stable frame for
/// screenshotting the shop without walking to the district. Native: `MELD_SHOP`.
///
pub(crate) fn shop_preview_flag() -> bool {
    std::env::var("MELD_SHOP").is_ok()
}

/// Open the Forge & Alembic's recipe book on arrival (with `MELD_CITY`) — a stable
/// frame for screenshotting the crafting panel without walking to the district.
/// `MELD_FORGE`.
pub(crate) fn forge_preview_flag() -> bool {
    std::env::var("MELD_FORGE").is_ok()
}

/// Seed a fake open HEAT on arrival (with `MELD_CITY`) so the smithing bar can be
/// screenshotted without diving, gathering and building a station first. Native:
/// `MELD_HEAT`.
pub(crate) fn heat_preview_flag() -> bool {
    std::env::var("MELD_HEAT").is_ok()
}

/// Seed a fake extraction TALLY on arrival so the haul screen (icons + counts) can be
/// screenshotted without completing a dive. `MELD_TALLY`.
pub(crate) fn tally_preview_flag() -> bool {
    std::env::var("MELD_TALLY").is_ok()
}

/// Light the Vanguard Wall on arrival (with `MELD_CITY`) — a stable frame for
/// screenshotting the seasonal board without having to walk over and press [E].
/// `MELD_WALL`.
pub(crate) fn wall_preview_flag() -> bool {
    std::env::var("MELD_WALL").is_ok()
}

/// PICK a counter row on arrival, so the detail column's description + amount + commit
/// buttons are on screen for a screenshot. The buy flow is two steps by design (a row picks,
/// the third column commits), and the second step is exactly the half a capture cannot reach
/// without a click. `MELD_PICK=<row>` / `?pick=<row>`, 0-based.
pub(crate) fn pick_preview_flag() -> Option<usize> {
    std::env::var("MELD_PICK").ok().and_then(|v| v.parse().ok())
}

/// Open the Bounty Board's hunts on arrival in Last City (AD-4) — a stable frame for
/// screenshots. Native: `MELD_HUNTS=1`.
pub(crate) fn hunts_preview_flag() -> bool {
    std::env::var("MELD_HUNTS").is_ok()
}

/// Open the **Drill Yard** on arrival (with `MELD_CITY`) — a stable frame for the party
/// screen without walking to the district and pressing [E]. `MELD_YARD=1`.
///
/// It also stands the town tour down, for the reason `autoplay_flag` does: the tour is a
/// modal card, nothing presses its button for you, and it lands squarely over the panel you
/// asked to look at.
pub(crate) fn yard_preview_flag() -> bool {
    std::env::var("MELD_YARD").is_ok()
}

/// Land with one hero's CLASS PICKER already open (`MELD_YARD_PICK=<slot>`, 0-based, with
/// `MELD_YARD`) — the picker is a click deep and a capture is one frame. Its own variable
/// rather than a value on `MELD_YARD`, because every other flag here is `=1` and
/// `MELD_YARD=1` reading as "slot 1" is a trap: it opens the picker when you asked for the
/// screen.
pub(crate) fn yard_pick_preview_flag() -> Option<usize> {
    std::env::var("MELD_YARD_PICK").ok().and_then(|v| v.trim().parse().ok()).filter(|i| *i < 4)
}

/// PIN the Drill Yard's detail column on one class (`MELD_YARD_FOCUS=<class key>`, with
/// `MELD_YARD`) instead of whatever the cursor last touched. Two captures at two classes are
/// how you prove the panel does NOT move when the description under the cursor changes —
/// which is the whole point of the fixed row height, and is unobservable in a single frame.
pub(crate) fn yard_focus_preview_flag() -> Option<String> {
    std::env::var("MELD_YARD_FOCUS").ok().filter(|s| !s.is_empty())
}

/// Preview a boss/elite sprite in The Last City plaza (with `MELD_CITY`) — a stable
/// frame for eyeballing the encounter art (`MELD_BOSS=ironmaw`). See `meld_proto::bosses` for valid ids.
pub(crate) fn boss_preview() -> Option<String> {
    std::env::var("MELD_BOSS").ok().filter(|s| !s.is_empty())
}

/// Offline render demo: no networking; scripted canned data drives the real
/// rendering so the Overworld/Battle screens can be shown without a server.
/// `MELD_DEMO`.
pub(crate) fn demo_flag() -> bool {
    std::env::var("MELD_DEMO").is_ok()
}

/// Pre-select a class without the Join screen (handy for demos/headless runs and
/// with `?autoplay`). `MELD_CLASS`.
pub(crate) fn class_flag() -> Option<String> {
    std::env::var("MELD_CLASS").ok().filter(|s| !s.is_empty())
}

/// Pre-build the whole party (comma-separated class keys) without the builder.
/// Native: `MELD_PARTY=explorer,psyker,resonant,explorer`.
pub(crate) fn party_flag() -> Option<String> {
    std::env::var("MELD_PARTY").ok().filter(|s| !s.is_empty())
}

/// Offline battle-screen mockup: jump straight into the Battle screen with canned
/// combatants and the command window open, so the subscreen can be inspected
/// without a server or walking there. `MELD_BATTLE`.
///
/// `MELD_BATTLE=coop` adds joined ally parties (the surround layout); `=skills` opens
/// the Skill page, which is the only one that draws a tooltip — an ability's prose and
/// its magnitudes are otherwise unreachable in a screenshot without driving a fight.
pub(crate) fn battle_mockup_flag() -> bool {
    std::env::var("MELD_BATTLE").is_ok()
}

/// Fire an ability's VFX on a loop in the battle mockup, so the elemental bursts and the
/// screen wash can be looked at. `MELD_FX=fire|ice|lightning|water|wind|earth|mind|poison|
/// celestial|shadow|slash|blunt|pierce|all`.
///
/// The mockup is deliberately STATIC — no server, no engine, nothing resolves — so without
/// this there is no way to see a battle effect at all except by walking a real party into a
/// real fight and hoping the creature happens to cast. `make check` never boots the app, so
/// a WGSL error in `ability_fx.wgsl` would otherwise reach `main` with every test green;
/// this is the fixture that makes the shader observable. Same argument as `MELD_TALLY`
/// holding a haul on screen.
///
/// `MELD_FX=all` cycles every element in turn, which is the one frame that proves each
/// branch of the shader compiles and draws something distinct.
pub(crate) fn battle_fx_flag() -> Option<String> {
    std::env::var("MELD_FX").ok().filter(|s| !s.is_empty())
}

/// Hold the AMBUSHED!/SURPRISE! opening card up in the battle mockup.
/// `MELD_OPENING=ambush|surprise`.
///
/// Same argument as [`battle_fx_flag`], one beat earlier in the fight: the card lives for
/// `feel.opening_ttl` (2 s, matching `[battle] open_grace_ms`) and is raised by a
/// `battle.started` the static mockup never receives, so without this it is unreachable in
/// a screenshot — and a capture is one frame at an arbitrary moment, so the mock re-raises
/// it on a cadence just under its own life rather than once at startup.
pub(crate) fn battle_opening_flag() -> Option<String> {
    std::env::var("MELD_OPENING").ok().filter(|s| !s.is_empty())
}

/// Offline mockups for the overworld overlays (`?inventory` / `?levelup`, or
/// `MELD_INVENTORY` / `MELD_LEVELUP`).
pub(crate) fn inventory_mockup_flag() -> bool {
    std::env::var("MELD_INVENTORY").is_ok()
}
/// Which tab (and, for Equip, which category picker) the `MELD_INVENTORY` mockup
/// opens on — so a gear-UI change can be screenshot-verified without a server or
/// a mouse. `MELD_INVENTORY_TAB=equip|items|status` / `?inventory_tab=`.
pub(crate) fn inventory_tab_flag() -> Option<String> {
    std::env::var("MELD_INVENTORY_TAB").ok().filter(|s| !s.is_empty())
}

/// `MELD_UNLOCK=<key>` / `?unlock=<key>` — queue that unlock's banner (and the
/// locked-roster rows) with no server, so the CL-1 presentation can be
/// screenshot-verified. Any key from `meld_proto::unlocks::UNLOCKS`.
/// `MELD_MENU=party|party.abilities|party.equipment|items|materials|map` /
/// `?menu=` — open the three-column cascade at that column with no server, so each
/// depth can be screenshot-verified.
pub(crate) fn menu_flag() -> Option<String> {
    std::env::var("MELD_MENU").ok().filter(|s| !s.is_empty())
}

/// `MELD_HERO_LEVEL=<n>` / `?hero_level=` — the level the mock roster's heroes are at.
/// Defaults to 1. The deep ability rungs land at 49 and 100, so without this the
/// mocked Abilities pane can only ever show a level-1 kit and the rows added out
/// there cannot be screenshot-verified at all.
pub(crate) fn hero_level_flag() -> i32 {
    std::env::var("MELD_HERO_LEVEL").ok().and_then(|v| v.parse().ok()).unwrap_or(1)
}

pub(crate) fn unlock_mockup_flag() -> Option<String> {
    std::env::var("MELD_UNLOCK").ok().filter(|s| !s.is_empty())
}

/// `MELD_FEEL="lunge_ttl=0.5,number_rise=70"` / `?feel=` — override any
/// [`crate::BattleFeel`] knob at launch, so the combat feel can be dialed in against a
/// running fight instead of a recompile per guess.
pub(crate) fn feel_flag() -> Option<String> {
    std::env::var("MELD_FEEL").ok().filter(|s| !s.is_empty())
}

pub(crate) fn world_feel_flag() -> Option<String> {
    std::env::var("MELD_WORLD_FEEL").ok().filter(|s| !s.is_empty())
}

pub(crate) fn levelup_mockup_flag() -> bool {
    std::env::var("MELD_LEVELUP").is_ok()
}
/// Offline mockup for the animated "LEVEL UP!" stat screen (`?levelup_anim` /
/// `MELD_LEVELUP_ANIM`) — seeds a canned level-up so the sequence can be
/// screenshotted without a battle.
pub(crate) fn levelup_anim_mockup_flag() -> bool {
    std::env::var("MELD_LEVELUP_ANIM").is_ok()
}
