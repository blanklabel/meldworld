//! `Db::vanguard_placement` against real Postgres — the ranking SQL, including the
//! tie-breaks, which the in-memory backend's `position()` cannot vouch for.
//!
//! The board orders `max_distance DESC, achieved_at ASC, player_id ASC`; a rank query
//! that gets the tie-break direction backwards still returns a plausible number, so it
//! needs a case where two players sit on the same distance.
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use uuid::Uuid;

#[tokio::test]
async fn placement_ranks_against_the_whole_season_and_breaks_ties_like_the_board() {
    let url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let db = meld_db::Db::connect(&url, 4).await.unwrap();

    // A season of our own, far from the live one, so the shared QA database's other
    // rows cannot change our ranks.
    let season = 900_000 + (Uuid::new_v4().as_u128() % 90_000) as i32;

    let mut ids = Vec::new();
    for i in 0..6 {
        let p = db
            .register(
                &format!("vgp_{}", &Uuid::new_v4().simple().to_string()[..10]),
                &Uuid::new_v4().to_string(),
            )
            .await
            .unwrap();
        // Exactly two players tie on 500; the rest are strictly deeper, and none of
        // them may land back on 500.
        let d = if i < 2 { 500 } else { 1000 + i * 100 };
        db.record_vanguard_distance(p.player_id, season, d)
            .await
            .unwrap();
        ids.push((p.player_id, d));
    }

    let board = db.vanguard_board(season, 100).await.unwrap();
    assert_eq!(board.len(), 6);

    // Every player's placement rank must equal their index on the board — that is the
    // whole contract, and it is what a backwards tie-break breaks.
    for (i, row) in board.iter().enumerate() {
        let (found, rank) = db
            .vanguard_placement(season, row.player_id)
            .await
            .unwrap()
            .expect("a player on the board has a placement");
        assert_eq!(
            rank,
            i as i64 + 1,
            "board position {} disagrees with placement rank {rank} (distance {})",
            i + 1,
            row.max_distance
        );
        assert_eq!(found.max_distance, row.max_distance);
    }

    // The two tied players get distinct, adjacent ranks rather than a shared one.
    let tied: Vec<i64> = {
        let mut v = Vec::new();
        for (pid, d) in &ids {
            if *d == 500 {
                v.push(db.vanguard_placement(season, *pid).await.unwrap().unwrap().1);
            }
        }
        v.sort();
        v
    };
    assert_eq!(tied.len(), 2);
    assert_eq!(tied[1] - tied[0], 1, "tied players share a rank: {tied:?}");

    // Ranked below the board's cut is still ranked.
    let deepest_cut = db.vanguard_board(season, 1).await.unwrap();
    assert_eq!(deepest_cut.len(), 1);
    let (_, last_rank) = db
        .vanguard_placement(season, board[5].player_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last_rank, 6);
}
