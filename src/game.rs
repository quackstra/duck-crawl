use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use quack_engine::command::EngineCommand;
use quack_engine::table::{EntityId, Val};
use quack_engine::World;

use crate::map_gen::build_tile_lookup;

pub type SharedGame = Arc<RwLock<GameState>>;

/// Direction the player is facing. Y increases southward.
/// 0=North (toward Y=0), 1=East, 2=South, 3=West
const FACING_NORTH: i32 = 0;
const FACING_EAST: i32 = 1;
const FACING_SOUTH: i32 = 2;
const FACING_WEST: i32 = 3;

/// Movement intents (stored in MoveIntent column)
const INTENT_NONE: f64 = 0.0;
const INTENT_FORWARD: f64 = 1.0;
const INTENT_BACK: f64 = 2.0;
const INTENT_LEFT: f64 = 3.0;
const INTENT_RIGHT: f64 = 4.0;
const INTENT_TURN_LEFT: f64 = 5.0;
const INTENT_TURN_RIGHT: f64 = 6.0;

pub struct GameState {
    pub world: World,
    pub tile_lookup: HashMap<(i32, i32), EntityId>,
    pub active_character: String,
    pub grid_size: i32,
}

impl GameState {
    pub fn new(world: World, grid_size: i32) -> Self {
        let tile_lookup = build_tile_lookup(world.table("Map").expect("World must have Map table"));
        GameState {
            world,
            tile_lookup,
            active_character: "warrior".into(),
            grid_size,
        }
    }

    /// Set a movement intent for the active character.
    pub fn set_move_intent(&mut self, direction: &str) {
        let intent = match direction {
            "forward" => INTENT_FORWARD,
            "back" => INTENT_BACK,
            "left" => INTENT_LEFT,
            "right" => INTENT_RIGHT,
            "turn_left" => INTENT_TURN_LEFT,
            "turn_right" => INTENT_TURN_RIGHT,
            _ => return,
        };

        self.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(),
            label: self.active_character.clone(),
            column: "MoveIntent".into(),
            value: intent,
        });
    }

    /// Process movement intents before the engine tick.
    /// Reads the active character's MoveIntent, validates against walls,
    /// and queues position/facing updates.
    pub fn process_movement(&mut self) {
        let party = self.world.table("Party").expect("Party table");
        let row = match party.entity_by_label(&self.active_character) {
            Some(r) => r,
            None => return,
        };

        let pos_x_col = party.col_index("PosX").unwrap();
        let pos_y_col = party.col_index("PosY").unwrap();
        let facing_col = party.col_index("Facing").unwrap();
        let intent_col = party.col_index("MoveIntent").unwrap();

        let cx = row.cells[pos_x_col].as_val() as i32;
        let cy = row.cells[pos_y_col].as_val() as i32;
        let facing = row.cells[facing_col].as_val() as i32;
        let intent = row.cells[intent_col].as_val();

        if intent == INTENT_NONE {
            return;
        }

        let label = self.active_character.clone();

        if intent == INTENT_TURN_LEFT {
            let new_facing = (facing + 3) % 4; // CCW
            self.world.queue_command(EngineCommand::SetCell {
                table: "Party".into(), label: label.clone(),
                column: "Facing".into(), value: new_facing as f64,
            });
        } else if intent == INTENT_TURN_RIGHT {
            let new_facing = (facing + 1) % 4; // CW
            self.world.queue_command(EngineCommand::SetCell {
                table: "Party".into(), label: label.clone(),
                column: "Facing".into(), value: new_facing as f64,
            });
        } else {
            // Compute movement delta based on facing + intent
            let (dx, dy) = movement_delta(facing, intent as i32);
            let tx = cx + dx;
            let ty = cy + dy;

            if self.can_move(cx, cy, dx, dy) {
                self.world.queue_command(EngineCommand::SetCell {
                    table: "Party".into(), label: label.clone(),
                    column: "PosX".into(), value: tx as f64,
                });
                self.world.queue_command(EngineCommand::SetCell {
                    table: "Party".into(), label: label.clone(),
                    column: "PosY".into(), value: ty as f64,
                });
            }
        }

        // Clear intent
        self.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label,
            column: "MoveIntent".into(), value: INTENT_NONE,
        });
    }

    /// Check if movement from (cx, cy) by (dx, dy) is valid.
    pub fn can_move(&self, cx: i32, cy: i32, dx: i32, dy: i32) -> bool {
        let tx = cx + dx;
        let ty = cy + dy;

        // Bounds check
        if tx < 0 || tx >= self.grid_size || ty < 0 || ty >= self.grid_size {
            return false;
        }

        // Check current tile's exit wall
        let exit_wall = match (dx, dy) {
            (0, -1) => "WallNorth",
            (1, 0) => "WallEast",
            (0, 1) => "WallSouth",
            (-1, 0) => "WallWest",
            _ => return false,
        };

        if self.get_wall(cx, cy, exit_wall) > 0.0 {
            return false;
        }

        // Check target tile's entry wall
        let entry_wall = match (dx, dy) {
            (0, -1) => "WallSouth",  // entering from south
            (1, 0) => "WallWest",    // entering from west
            (0, 1) => "WallNorth",   // entering from north
            (-1, 0) => "WallEast",   // entering from east
            _ => return false,
        };

        if self.get_wall(tx, ty, entry_wall) > 0.0 {
            return false;
        }

        // Check tile type — can't walk into solid/pillar tiles (type 3)
        let tile_type = self.get_tile_val(tx, ty, "TileType");
        if tile_type == 3.0 {
            return false;
        }

        true
    }

    /// Read a wall flag from the Map table.
    fn get_wall(&self, x: i32, y: i32, wall_col: &str) -> Val {
        self.get_tile_val(x, y, wall_col)
    }

    /// Read any value from a Map tile at (x, y).
    fn get_tile_val(&self, x: i32, y: i32, col_name: &str) -> Val {
        let map = match self.world.table("Map") {
            Some(t) => t,
            None => return 0.0,
        };
        let col_idx = match map.col_index(col_name) {
            Some(i) => i,
            None => return 0.0,
        };
        let entity_id = match self.tile_lookup.get(&(x, y)) {
            Some(&id) => id,
            None => return 0.0,
        };
        map.get_val(entity_id, col_idx)
    }

    /// Tick the world: apply pending commands (including move intent),
    /// process movement, then run engine tick.
    pub fn tick(&mut self) -> Result<(), String> {
        // Apply queued commands first (e.g., SetCell for MoveIntent)
        self.world.apply_commands()?;
        // Now process movement — reads intent from table, queues position changes
        self.process_movement();
        // Engine tick applies position changes and evaluates all formulas
        self.world.tick()?;
        Ok(())
    }

    /// Serialize all table state for WebSocket broadcast.
    pub fn snapshot(&self) -> serde_json::Value {
        let mut tables = serde_json::Map::new();

        for table_name in &self.world.tick_order {
            let table = match self.world.table(table_name) {
                Some(t) => t,
                None => continue,
            };

            let mut entities = serde_json::Map::new();
            for row in &table.rows {
                if !row.alive { continue; }
                let mut cols = serde_json::Map::new();
                for (i, col) in table.columns.iter().enumerate() {
                    if i < row.cells.len() {
                        cols.insert(col.name.clone(), serde_json::json!(row.cells[i].as_val()));
                    }
                }
                entities.insert(row.label.clone(), serde_json::Value::Object(cols));
            }
            tables.insert(table_name.clone(), serde_json::Value::Object(entities));
        }

        serde_json::json!({
            "tick": self.world.tick,
            "tables": tables,
        })
    }
}

/// Compute (dx, dy) for a movement intent given the player's facing direction.
/// Y increases southward.
fn movement_delta(facing: i32, intent: i32) -> (i32, i32) {
    // Forward direction per facing
    let (fx, fy) = match facing {
        FACING_NORTH => (0, -1),
        FACING_EAST => (1, 0),
        FACING_SOUTH => (0, 1),
        FACING_WEST => (-1, 0),
        _ => (0, 0),
    };

    match intent {
        1 => (fx, fy),                   // forward
        2 => (-fx, -fy),                 // back
        3 => (fy, -fx),                  // strafe left
        4 => (-fy, fx),                  // strafe right
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_gen::generate_great_hall;
    use crate::visibility::build_visibility_table;
    use quack_engine::table::Table;

    fn make_test_world() -> World {
        let (map_tf, _) = generate_great_hall();
        let party_json = include_str!("../data/party.quack.json");
        let party_tf: quack_engine::table::TableFile = serde_json::from_str(party_json).unwrap();
        let visibility = build_visibility_table(6, "warrior", "Party", 3);

        let mut world = World::new();
        world.add_table("Map".into(), Table::from_file(map_tf));
        world.add_table("Party".into(), Table::from_file(party_tf));
        world.add_table("Visibility".into(), visibility);
        world.tick_order = vec!["Map".into(), "Party".into(), "Visibility".into()];
        world
    }

    fn make_game() -> GameState {
        let world = make_test_world();
        GameState::new(world, 6)
    }

    #[test]
    fn test_initial_position() {
        let game = make_game();
        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let px = party.col_index("PosX").unwrap();
        let py = party.col_index("PosY").unwrap();
        assert_eq!(warrior.cells[px].as_val(), 0.0);
        assert_eq!(warrior.cells[py].as_val(), 0.0);
    }

    #[test]
    fn test_movement_forward_east() {
        let mut game = make_game();
        // Warrior starts at (0,0) facing east
        game.set_move_intent("forward");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let px = party.col_index("PosX").unwrap();
        let py = party.col_index("PosY").unwrap();
        // Can't move east from (0,0) — pillar at (1,1) but tile (1,0) should be accessible
        // Actually (1,0) is not a pillar. Check walls.
        // (0,0) has WallEast? No — only perimeter walls on (0,0) are N and W.
        // So moving east from (0,0) to (1,0) should work.
        assert_eq!(warrior.cells[px].as_val(), 1.0);
        assert_eq!(warrior.cells[py].as_val(), 0.0);
    }

    #[test]
    fn test_movement_blocked_by_north_wall() {
        let mut game = make_game();
        // Warrior at (0,0) facing north — wall on north edge
        // First turn to face north
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "Facing".into(), value: 0.0,
        });
        game.world.apply_commands().unwrap();

        game.set_move_intent("forward");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let px = party.col_index("PosX").unwrap();
        let py = party.col_index("PosY").unwrap();
        // Should not have moved — blocked by perimeter wall
        assert_eq!(warrior.cells[px].as_val(), 0.0);
        assert_eq!(warrior.cells[py].as_val(), 0.0);
    }

    #[test]
    fn test_movement_blocked_by_bounds() {
        let mut game = make_game();
        // Set warrior facing west at (0,0)
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "Facing".into(), value: FACING_WEST as f64,
        });
        game.world.apply_commands().unwrap();

        game.set_move_intent("forward");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let px = party.col_index("PosX").unwrap();
        assert_eq!(warrior.cells[px].as_val(), 0.0);
    }

    #[test]
    fn test_turn_left() {
        let mut game = make_game();
        // Warrior starts facing east (1)
        game.set_move_intent("turn_left");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let fc = party.col_index("Facing").unwrap();
        assert_eq!(warrior.cells[fc].as_val(), FACING_NORTH as f64);
    }

    #[test]
    fn test_turn_right() {
        let mut game = make_game();
        // Warrior starts facing east (1)
        game.set_move_intent("turn_right");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let fc = party.col_index("Facing").unwrap();
        assert_eq!(warrior.cells[fc].as_val(), FACING_SOUTH as f64);
    }

    #[test]
    fn test_movement_clears_intent() {
        let mut game = make_game();
        game.set_move_intent("forward");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let ic = party.col_index("MoveIntent").unwrap();
        assert_eq!(warrior.cells[ic].as_val(), INTENT_NONE);
    }

    #[test]
    fn test_movement_blocked_by_pillar() {
        let mut game = make_game();
        // Move warrior to (0,1) first, then try to go east to (1,1) which is a pillar
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PosY".into(), value: 1.0,
        });
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "Facing".into(), value: FACING_EAST as f64,
        });
        game.world.apply_commands().unwrap();

        game.set_move_intent("forward");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let px = party.col_index("PosX").unwrap();
        // Should be blocked — (1,1) is a pillar (TileType=3) with walls
        assert_eq!(warrior.cells[px].as_val(), 0.0);
    }

    #[test]
    fn test_visibility_near_start() {
        let mut game = make_game();
        game.tick().unwrap();

        let vis = game.world.table("Visibility").unwrap();
        let vis_col = vis.col_index("Visible").unwrap();

        // Warrior at (0,0) — vis_0_0 should be visible (distance 0)
        let v00 = vis.entity_by_label("vis_0_0").unwrap();
        assert_eq!(v00.cells[vis_col].as_val(), 1.0);

        // vis_5_5 should NOT be visible (distance ~7)
        let v55 = vis.entity_by_label("vis_5_5").unwrap();
        assert_eq!(v55.cells[vis_col].as_val(), 0.0);
    }

    #[test]
    fn test_discovered_persists() {
        let mut game = make_game();
        // Tick 1: warrior at (0,0), vis_0_0 becomes Visible
        game.tick().unwrap();
        // Tick 2: Discovered latches from prev(Visible)=1
        game.tick().unwrap();

        let vis = game.world.table("Visibility").unwrap();
        let disc_col = vis.col_index("Discovered").unwrap();
        let v00 = vis.entity_by_label("vis_0_0").unwrap();
        assert_eq!(v00.cells[disc_col].as_val(), 1.0);

        // Move warrior far away
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PosX".into(), value: 5.0,
        });
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PosY".into(), value: 5.0,
        });

        game.tick().unwrap();

        // vis_0_0 should still be Discovered even if no longer Visible
        let vis = game.world.table("Visibility").unwrap();
        let v00 = vis.entity_by_label("vis_0_0").unwrap();
        let vis_col = vis.col_index("Visible").unwrap();
        let disc_col = vis.col_index("Discovered").unwrap();
        assert_eq!(v00.cells[vis_col].as_val(), 0.0); // no longer visible
        assert_eq!(v00.cells[disc_col].as_val(), 1.0); // still discovered
    }

    #[test]
    fn test_find_where_resolves_tile() {
        let mut game = make_game();
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let tile_id_col = party.col_index("CurrentTileId").unwrap();

        // Warrior at (0,0) — tile_0_0 has entity ID 1
        let tile_id = warrior.cells[tile_id_col].as_val();
        assert_eq!(tile_id, 1.0);
    }

    #[test]
    fn test_multi_step_traversal() {
        let mut game = make_game();
        // Walk warrior east across the top row: (0,0) -> (1,0) -> (2,0) -> (3,0)
        for expected_x in 1..=3 {
            game.set_move_intent("forward");
            game.tick().unwrap();

            let party = game.world.table("Party").unwrap();
            let warrior = party.entity_by_label("warrior").unwrap();
            let px = party.col_index("PosX").unwrap();
            assert_eq!(warrior.cells[px].as_val(), expected_x as f64,
                "Expected PosX={} after step", expected_x);
        }
    }

    #[test]
    fn test_movement_delta_table() {
        // Forward while facing each direction
        assert_eq!(movement_delta(FACING_NORTH, 1), (0, -1));
        assert_eq!(movement_delta(FACING_EAST, 1), (1, 0));
        assert_eq!(movement_delta(FACING_SOUTH, 1), (0, 1));
        assert_eq!(movement_delta(FACING_WEST, 1), (-1, 0));

        // Back is opposite
        assert_eq!(movement_delta(FACING_NORTH, 2), (0, 1));
        assert_eq!(movement_delta(FACING_EAST, 2), (-1, 0));

        // Strafe left while facing north goes west
        assert_eq!(movement_delta(FACING_NORTH, 3), (-1, 0));
        // Strafe right while facing north goes east
        assert_eq!(movement_delta(FACING_NORTH, 4), (1, 0));
    }

    #[test]
    fn test_snapshot_has_all_tables() {
        let game = make_game();
        let snap = game.snapshot();
        let tables = snap["tables"].as_object().unwrap();
        assert!(tables.contains_key("Map"));
        assert!(tables.contains_key("Party"));
        assert!(tables.contains_key("Visibility"));
    }
}
