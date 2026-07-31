//! `dungeon-preview [out_dir]` — write `<name>.svg` for every embedded authored
//! dungeon (default out dir: the current directory). Handy for eyeballing the
//! content pool: `cargo run -p meld-dungeon-viz --bin dungeon-preview -- /tmp/dungeons`.

fn main() -> std::io::Result<()> {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    std::fs::create_dir_all(&out_dir)?;
    for d in meld_dungeon_content::all() {
        let path = format!("{out_dir}/{}.svg", d.name);
        std::fs::write(&path, meld_dungeon_viz::to_svg(d))?;
        println!("wrote {path}");
    }
    Ok(())
}
