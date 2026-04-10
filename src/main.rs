mod combat;
mod enemy_gen;
mod game;
mod map_gen;
mod server;
mod visibility;

use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use quack_engine::table::{Table, TableFile};
use quack_engine::World;

use crate::enemy_gen::{build_enemies_table, great_hall_enemies};
use crate::game::GameState;
use crate::map_gen::generate_great_hall;
use crate::server::{AppState, create_router};
use crate::visibility::build_visibility_table;

const GRID_SIZE: i32 = 6;
const SIGHT_RANGE: i32 = 3;

#[tokio::main]
async fn main() {
    eprintln!("DuckCrawl v0.2 — QuackEngine dungeon crawler");

    // 1. Generate Map table
    let (map_tf, _) = generate_great_hall();
    let map_table = Table::from_file(map_tf);
    eprintln!("  Map: {} tiles", map_table.rows.len());

    // 2. Load Party table
    let party_path = find_data_file("party.quack.json");
    let party_json = std::fs::read_to_string(&party_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", party_path, e));
    let party_tf: TableFile = serde_json::from_str(&party_json)
        .unwrap_or_else(|e| panic!("Failed to parse party.quack.json: {}", e));
    let party_table = Table::from_file(party_tf);
    eprintln!("  Party: {} members, {} columns", party_table.rows.len(), party_table.columns.len());

    // 3. Build Visibility table
    let visibility_table = build_visibility_table(GRID_SIZE, "warrior", "Party", SIGHT_RANGE);
    eprintln!("  Visibility: {} tiles", visibility_table.rows.len());

    // 4. Generate Enemies table
    let enemy_spawns = great_hall_enemies();
    let enemies_table = build_enemies_table(&enemy_spawns);
    eprintln!("  Enemies: {} spawned", enemies_table.rows.len());

    // 5. Load Spells table
    let spells_path = find_data_file("spells.quack.json");
    let spells_json = std::fs::read_to_string(&spells_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", spells_path, e));
    let spells_tf: TableFile = serde_json::from_str(&spells_json)
        .unwrap_or_else(|e| panic!("Failed to parse spells.quack.json: {}", e));
    let spells_table = Table::from_file(spells_tf);
    eprintln!("  Spells: {} available", spells_table.rows.len());

    // 6. Assemble World
    let mut world = World::new();
    world.add_table("Map".into(), map_table);
    world.add_table("Party".into(), party_table);
    world.add_table("Enemies".into(), enemies_table);
    world.add_table("Spells".into(), spells_table);
    world.add_table("Visibility".into(), visibility_table);
    world.tick_order = vec![
        "Map".into(),
        "Party".into(),
        "Enemies".into(),
        "Spells".into(),
        "Visibility".into(),
    ];

    // Initial tick
    world.tick().unwrap();
    eprintln!("  Initial tick complete (tick={})", world.tick);

    // 7. Create GameState
    let game_state = GameState::new(world, GRID_SIZE);
    let shared_game = Arc::new(RwLock::new(game_state));

    // 8. Broadcast channel
    let (tick_tx, _) = broadcast::channel::<String>(64);

    // 9. Start server
    let state = AppState {
        game: shared_game,
        tick_tx,
    };

    let static_dir = find_static_dir();
    let app = create_router(state, &static_dir);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".into());
    let addr = format!("0.0.0.0:{}", port);
    eprintln!("DuckCrawl listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn find_data_file(filename: &str) -> String {
    let candidates = [
        format!("data/{}", filename),
        format!("../data/{}", filename),
        format!("{}/data/{}", env!("CARGO_MANIFEST_DIR"), filename),
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return c.clone();
        }
    }
    candidates[0].clone()
}

fn find_static_dir() -> String {
    let candidates = ["static", "../static"];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return c.to_string();
        }
    }
    "static".to_string()
}
