//! Background music, one looping track per screen. The four tracks
//! (`assets/music/*.mp3`) map to the game's contexts by name:
//! `party_screen` → Join/Lobby party building, `town` → The Last City,
//! `overworld` → the dive, `battle` → combat. A single [`update_music`] system
//! watches the [`Screen`] state and cross-fades by swapping the audio entity only
//! when the desired track actually changes (so Join→Lobby, both party music,
//! doesn't restart the track).

use bevy::audio::{PlaybackMode, Volume};
use bevy::prelude::*;

use crate::Screen;

/// Which track a screen wants. Equality drives "should we swap?" in [`update_music`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Track {
    Party,
    Town,
    Overworld,
    Battle,
}

/// The loaded looping-music handles (one per [`Track`]).
#[derive(Resource)]
pub(crate) struct MusicAssets {
    party: Handle<AudioSource>,
    town: Handle<AudioSource>,
    overworld: Handle<AudioSource>,
    battle: Handle<AudioSource>,
}

impl MusicAssets {
    fn handle(&self, t: Track) -> Handle<AudioSource> {
        match t {
            Track::Party => self.party.clone(),
            Track::Town => self.town.clone(),
            Track::Overworld => self.overworld.clone(),
            Track::Battle => self.battle.clone(),
        }
    }
}

/// The currently-playing track + its audio entity (so we can stop it on a swap).
#[derive(Resource, Default)]
pub(crate) struct NowPlaying {
    track: Option<Track>,
    entity: Option<Entity>,
}

/// Marker for the single background-music audio entity.
#[derive(Component)]
pub(crate) struct MusicEntity;

pub(crate) fn setup_music(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(MusicAssets {
        party: assets.load("music/party_screen.mp3"),
        town: assets.load("music/town.mp3"),
        overworld: assets.load("music/overworld.mp3"),
        battle: assets.load("music/battle.mp3"),
    });
    commands.init_resource::<NowPlaying>();
}

/// Pick the track for a screen. Join + Lobby (party building) share the party
/// theme; the result screen keeps the overworld theme until the loop returns home.
fn track_for(screen: &Screen) -> Track {
    match screen {
        Screen::Join | Screen::Lobby => Track::Party,
        Screen::City => Track::Town,
        Screen::Overworld | Screen::Ended => Track::Overworld,
        Screen::Battle => Track::Battle,
    }
}

/// Swap the looping track whenever the screen's desired track changes. Cheap to run
/// every frame — it only touches the world when `want != NowPlaying.track`.
pub(crate) fn update_music(
    mut commands: Commands,
    state: Res<State<Screen>>,
    assets: Option<Res<MusicAssets>>,
    mut now: ResMut<NowPlaying>,
) {
    let Some(assets) = assets else { return };
    let want = track_for(state.get());
    if now.track == Some(want) {
        return;
    }
    if let Some(e) = now.entity.take() {
        commands.entity(e).despawn();
    }
    let entity = commands
        .spawn((
            AudioPlayer(assets.handle(want)),
            PlaybackSettings {
                mode: PlaybackMode::Loop,
                volume: Volume::Linear(0.5),
                ..default()
            },
            MusicEntity,
        ))
        .id();
    now.track = Some(want);
    now.entity = Some(entity);
}
