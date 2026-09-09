/// **A BIOME MUST SAY HOW IT MAZES, OR IT WEARS A FALLBACK AND READS AS NOWHERE.**
///
/// `AGENTS.md`: *"each biome mazes with a different primitive … which one a biome uses is what
/// makes it feel like a place."* Five of the eleven biomes had no entry in the density table at
/// all — including `seraphic_oubliette`, the exclusive band the whole walk out is pointed at —
/// so they silently drew `maze_obstacle_mult`. Measured, `amber_wood` (a FALL WOOD, with its own
/// amber trees authored and its own art) drew **11.8 props per 1000 u² against open grassland's
/// 12.5**: an autumn forest thinner than a meadow, which is exactly how it was reported from
/// play — *"the amount of trees in the fall biome doesn't really feel like a forest."*
///
/// A fallback is the right thing for an unknown key and the wrong thing for a shipped biome, and
/// nothing could tell the two apart. This reads the source, the way the shader-mirror tests do,
/// because the property is "somebody made a decision here" and that is not observable from the
/// function's output — a biome deliberately set to the fallback VALUE is fine; a biome that
/// never got asked about is not.
#[test]
fn every_biome_says_how_it_mazes() {
    let src = include_str!("../src/lib.rs");
    for (what, needle) in [
        ("scatters", "fn biome_obstacle_mult("),
        ("threads", "fn biome_minimaze_chance("),
    ] {
        let body = src
            .split_once(needle)
            .unwrap_or_else(|| panic!("meld-world defines `{needle}`"))
            .1
            .split_once("\n}")
            .expect("…and closes it")
            .0;
        for biome in meld_proto::regions::BIOMES {
            assert!(
                body.contains(&format!("\"{biome}\"")),
                "`{biome}` never says how densely it {what} — it falls through to the generic \
                 arm, which is how a fall WOOD shipped thinner than open grassland. Give it a \
                 line, even if the number matches the fallback: the decision is the point."
            );
        }
    }
}
