use meld_balance::Balance;
use meld_world::Arena;

/// **A CELL'S WET SHARE FILLS ITS LOW GROUND** (`WG-11` stage 9's ambient).
///
/// The level is the share's QUANTILE of the cell's own terrain, so water finds the hollows and
/// the shore follows the contour — the same level floods a wide ragged sheet over flat ground
/// and a pool on a slope. Slope separates a bog from a lake for free; the biome only says how
/// much.
///
/// ⚠️ Measured as a DELTA against the same seed with every share zeroed, never as an absolute:
/// a naive "how much water is in a mire cell" also counts the OCEAN those cells border, which
/// is why the raw figure for desert reads over 50% and means nothing. What the feature does is
/// the difference it makes.
#[test]
fn a_wet_cell_is_wetter_than_it_would_have_been() {
    let on = Balance::load_default().unwrap();
    let mut off = on.clone();
    off.worldgen.wet_share_mire = 0.0;
    off.worldgen.wet_share_forest = 0.0;
    off.worldgen.wet_share_field = 0.0;
    off.worldgen.wet_share_tundra = 0.0;
    let seed = 424242u64;
    let wet_of = |b: &Balance, want: &str| -> f64 {
        let mut a = Arena::generate(b, seed, false);
        let mut r = 0.0f64;
        while r < 700.0 {
            r += 60.0;
            a.ensure_frontier(b, r);
        }
        let g = a.regions();
        let (mut wet, mut n) = (0usize, 0usize);
        let step = 5.0f64;
        let mut x = -700.0f64;
        while x < 700.0 {
            let mut z = -700.0f64;
            while z < 700.0 {
                let rr = x.hypot(z);
                if (120.0..660.0).contains(&rr)
                    && a.biome_of_cell(g.cell_at(x as f32, z as f32)) == want
                {
                    n += 1;
                    if !a.on_land(x, z) {
                        wet += 1;
                    }
                }
                z += step;
            }
            x += step;
        }
        100.0 * wet as f64 / n.max(1) as f64
    };
    let (mire_off, mire_on) = (wet_of(&off, "mire"), wet_of(&on, "mire"));
    let (dry_off, dry_on) = (wet_of(&off, "desert"), wet_of(&on, "desert"));
    println!("mire {mire_off:.1}% -> {mire_on:.1}%, desert {dry_off:.1}% -> {dry_on:.1}%");
    assert!(
        mire_on > mire_off + 5.0,
        "a mire's wet share put only {:.1} points of water on it ({mire_off:.1}% to \
         {mire_on:.1}%) — the ambient this stage asks for is not there",
        mire_on - mire_off
    );
    // …and a biome whose share is zero gains nothing, or the level is not reading the share.
    assert!(
        (dry_on - dry_off).abs() < 1.0,
        "a desert has no wet share and gained {:.1} points anyway ({dry_off:.1}% to \
         {dry_on:.1}%)",
        dry_on - dry_off
    );
}
