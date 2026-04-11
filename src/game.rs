use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use quack_engine::command::EngineCommand;
use quack_engine::table::{EntityId, Val};
use quack_engine::World;

use crate::combat::{
    self, CombatEvent, CombatOutcome, CombatState, CombatantTable,
};
use crate::map_gen::build_tile_lookup;

const INTENT_COLUMNS: &[&str] = &["DeltaHP", "DeltaMana", "PoisonIntent", "StunIntent", "ShieldIntent"];

pub type SharedGame = Arc<RwLock<GameState>>;

/// Direction the player is facing. Y increases southward.
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
    pub combat_state: CombatState,
}

impl GameState {
    pub fn new(world: World, grid_size: i32) -> Self {
        let tile_lookup = build_tile_lookup(world.table("Map").expect("World must have Map table"));
        GameState {
            world,
            tile_lookup,
            active_character: "warrior".into(),
            grid_size,
            combat_state: CombatState::Exploring,
        }
    }

    pub fn is_in_combat(&self) -> bool {
        matches!(self.combat_state, CombatState::InCombat(_))
    }

    /// Set a movement intent for the active character.
    /// Ignored if in combat.
    pub fn set_move_intent(&mut self, direction: &str) {
        if self.is_in_combat() {
            return;
        }

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
            let new_facing = (facing + 3) % 4;
            self.world.queue_command(EngineCommand::SetCell {
                table: "Party".into(), label: label.clone(),
                column: "Facing".into(), value: new_facing as f64,
            });
        } else if intent == INTENT_TURN_RIGHT {
            let new_facing = (facing + 1) % 4;
            self.world.queue_command(EngineCommand::SetCell {
                table: "Party".into(), label: label.clone(),
                column: "Facing".into(), value: new_facing as f64,
            });
        } else {
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

        self.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label,
            column: "MoveIntent".into(), value: INTENT_NONE,
        });
    }

    /// Check if movement from (cx, cy) by (dx, dy) is valid.
    pub fn can_move(&self, cx: i32, cy: i32, dx: i32, dy: i32) -> bool {
        let tx = cx + dx;
        let ty = cy + dy;

        if tx < 0 || tx >= self.grid_size || ty < 0 || ty >= self.grid_size {
            return false;
        }

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

        let entry_wall = match (dx, dy) {
            (0, -1) => "WallSouth",
            (1, 0) => "WallWest",
            (0, 1) => "WallNorth",
            (-1, 0) => "WallEast",
            _ => return false,
        };

        if self.get_wall(tx, ty, entry_wall) > 0.0 {
            return false;
        }

        let tile_type = self.get_tile_val(tx, ty, "TileType");
        if tile_type == 3.0 {
            return false;
        }

        true
    }

    fn get_wall(&self, x: i32, y: i32, wall_col: &str) -> Val {
        self.get_tile_val(x, y, wall_col)
    }

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

    /// Tick the world: apply pending commands, process movement,
    /// check combat triggers, run engine tick, clear intents.
    pub fn tick(&mut self) -> Result<(), String> {
        self.world.apply_commands()?;

        if !self.is_in_combat() {
            self.process_movement();
        }

        self.world.tick()?;

        // Clear intent columns after tick so they don't double-apply
        self.world.clear_columns("Party", INTENT_COLUMNS);
        self.world.clear_columns("Enemies", INTENT_COLUMNS);

        // Check combat trigger after movement
        if !self.is_in_combat() {
            let adjacent = combat::check_combat_trigger(&self.world, &self.active_character);
            if !adjacent.is_empty() {
                let ctx = combat::start_combat(&self.world, self.world.tick);
                // Latch InCombat flag on all party members
                for label in &["warrior", "mage", "scout", "healer"] {
                    self.world.queue_command(EngineCommand::LatchCell {
                        table: "Party".into(),
                        label: label.to_string(),
                        column: "InCombat".into(),
                        value: 1.0,
                    });
                }
                self.combat_state = CombatState::InCombat(ctx);
            }
        }

        Ok(())
    }

    /// Process a player combat action. Returns events from the action
    /// plus all subsequent enemy turns until the next player turn or combat end.
    pub fn process_combat_action(
        &mut self,
        action: &str,
        target: &str,
        spell: Option<&str>,
    ) -> Vec<CombatEvent> {
        let mut all_events = Vec::new();

        // Process current player's action
        let events = self.resolve_current_turn_action(action, target, spell);
        all_events.extend(events);

        // Apply commands and tick
        let _ = self.world.apply_commands();
        let _ = self.world.tick();
        self.world.clear_columns("Party", INTENT_COLUMNS);
        self.world.clear_columns("Enemies", INTENT_COLUMNS);

        // Check if combat ended
        if let Some(outcome) = combat::check_combat_end(&self.world) {
            all_events.push(self.end_combat(outcome));
            return all_events;
        }

        // Advance to next turn
        self.advance_turn();

        // Process enemy turns until we hit a player turn or combat ends
        loop {
            if !self.is_in_combat() {
                break;
            }

            let ctx = match &self.combat_state {
                CombatState::InCombat(c) => c,
                _ => break,
            };

            let combatant = &ctx.turn_order[ctx.turn_index];
            if combatant.table == CombatantTable::Party {
                break;
            }

            // Enemy turn — check status (formulas already applied poison/stun)
            let enemy_label = combatant.label.clone();
            let (effect_events, stunned) = combat::check_turn_start_status(
                &self.world, &enemy_label, CombatantTable::Enemies,
            );
            all_events.extend(effect_events);

            if !stunned {
                if let Some(outcome) = combat::check_combat_end(&self.world) {
                    all_events.push(self.end_combat(outcome));
                    return all_events;
                }

                let (action, target) = combat::select_enemy_action(&self.world, &enemy_label);
                if action == "attack" && !target.is_empty() {
                    let evt = combat::resolve_attack(
                        &mut self.world,
                        &enemy_label, CombatantTable::Enemies,
                        &target, CombatantTable::Party,
                        1.0,
                    );
                    all_events.push(evt);
                }
            }

            let _ = self.world.apply_commands();
            let _ = self.world.tick();
            self.world.clear_columns("Party", INTENT_COLUMNS);
            self.world.clear_columns("Enemies", INTENT_COLUMNS);

            if let Some(outcome) = combat::check_combat_end(&self.world) {
                all_events.push(self.end_combat(outcome));
                return all_events;
            }

            self.advance_turn();
        }

        // Store events in combat log
        if let CombatState::InCombat(ref mut ctx) = self.combat_state {
            ctx.log.extend(all_events.clone());
        }

        all_events
    }

    fn resolve_current_turn_action(
        &mut self,
        action: &str,
        target: &str,
        spell: Option<&str>,
    ) -> Vec<CombatEvent> {
        let ctx = match &self.combat_state {
            CombatState::InCombat(c) => c,
            _ => return vec![],
        };

        let combatant = &ctx.turn_order[ctx.turn_index];
        let actor_label = combatant.label.clone();
        let actor_table = combatant.table;

        // Check start-of-turn status (formulas already applied poison/stun)
        let (mut events, stunned) = combat::check_turn_start_status(
            &self.world, &actor_label, actor_table,
        );

        if stunned {
            return events;
        }

        match action {
            "attack" => {
                let evt = combat::resolve_attack(
                    &mut self.world,
                    &actor_label, actor_table,
                    target, CombatantTable::Enemies,
                    1.0,
                );
                events.push(evt);
            }
            "spell" => {
                if let Some(spell_label) = spell {
                    let spell_events = combat::resolve_spell(
                        &mut self.world,
                        &actor_label, actor_table,
                        target, CombatantTable::Enemies,
                        spell_label,
                    );
                    events.extend(spell_events);
                }
            }
            "defend" => {
                events.push(CombatEvent {
                    actor: actor_label,
                    action: "defend".into(),
                    target: None,
                    damage: None,
                    heal: None,
                    effect: Some("defending".into()),
                    killed: false,
                    message: format!("{} defends!", combatant.label),
                });
            }
            _ => {}
        }

        events
    }

    fn advance_turn(&mut self) {
        if let CombatState::InCombat(ref mut ctx) = self.combat_state {
            ctx.turn_index += 1;

            // Skip dead combatants
            while ctx.turn_index < ctx.turn_order.len() {
                let c = &ctx.turn_order[ctx.turn_index];
                let tbl = match c.table {
                    CombatantTable::Party => "Party",
                    CombatantTable::Enemies => "Enemies",
                };
                let alive = self.world.table(tbl)
                    .and_then(|t| t.entity_by_label(&c.label))
                    .and_then(|r| {
                        let ai = self.world.table(tbl)?.col_index("Alive")?;
                        Some(r.cells[ai].as_val())
                    })
                    .unwrap_or(0.0);

                if alive == 1.0 {
                    break;
                }
                ctx.turn_index += 1;
            }

            // If we've gone past the end, start new round
            if ctx.turn_index >= ctx.turn_order.len() {
                ctx.round += 1;
                ctx.turn_index = 0;
                // Recompute initiative for new round
                let new_ctx = combat::start_combat(&self.world, self.world.tick);
                ctx.turn_order = new_ctx.turn_order;
                // Skip dead combatants at start of new round
                while ctx.turn_index < ctx.turn_order.len() {
                    let c = &ctx.turn_order[ctx.turn_index];
                    let tbl = match c.table {
                        CombatantTable::Party => "Party",
                        CombatantTable::Enemies => "Enemies",
                    };
                    let alive = self.world.table(tbl)
                        .and_then(|t| t.entity_by_label(&c.label))
                        .and_then(|r| {
                            let ai = self.world.table(tbl)?.col_index("Alive")?;
                            Some(r.cells[ai].as_val())
                        })
                        .unwrap_or(0.0);
                    if alive == 1.0 { break; }
                    ctx.turn_index += 1;
                }
            }
        }
    }

    fn end_combat(&mut self, outcome: CombatOutcome) -> CombatEvent {
        // Release all combat latches and reset formula-less columns
        let latch_cols = vec!["InCombat".into(), "ShieldAmount".into()];
        for label in &["warrior", "mage", "scout", "healer"] {
            self.world.queue_command(EngineCommand::ReleaseCells {
                table: "Party".into(),
                label: label.to_string(),
                columns: latch_cols.clone(),
            });
            // InCombat has no formula — release alone won't zero it
            self.world.queue_command(EngineCommand::SetCell {
                table: "Party".into(),
                label: label.to_string(),
                column: "InCombat".into(),
                value: 0.0,
            });
        }
        // Release enemy aggro latches
        if let Some(enemies) = self.world.table("Enemies") {
            let labels: Vec<String> = enemies.rows.iter().map(|r| r.label.clone()).collect();
            for label in labels {
                self.world.queue_command(EngineCommand::ReleaseCells {
                    table: "Enemies".into(),
                    label,
                    columns: vec!["AggroTarget".into()],
                });
            }
        }
        let _ = self.world.apply_commands();

        let msg = match outcome {
            CombatOutcome::Victory => "Victory! All enemies defeated.",
            CombatOutcome::TPK => "The party has fallen...",
        };

        self.combat_state = CombatState::Exploring;

        CombatEvent {
            actor: "system".into(),
            action: "combat_end".into(),
            target: None,
            damage: None,
            heal: None,
            effect: Some(format!("{:?}", outcome)),
            killed: false,
            message: msg.into(),
        }
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

        let combat_meta = match &self.combat_state {
            CombatState::Exploring => serde_json::json!({ "active": false }),
            CombatState::InCombat(ctx) => {
                let current = ctx.turn_order.get(ctx.turn_index)
                    .map(|c| c.label.clone())
                    .unwrap_or_default();
                let order: Vec<serde_json::Value> = ctx.turn_order.iter().map(|c| {
                    serde_json::json!({
                        "label": c.label,
                        "table": format!("{:?}", c.table),
                        "initiative": c.initiative,
                    })
                }).collect();
                let log: Vec<serde_json::Value> = ctx.log.iter().rev().take(20).map(|e| {
                    serde_json::json!({ "message": e.message })
                }).collect();

                serde_json::json!({
                    "active": true,
                    "round": ctx.round,
                    "current_turn": current,
                    "turn_order": order,
                    "log": log,
                })
            }
        };

        serde_json::json!({
            "tick": self.world.tick,
            "tables": tables,
            "combat": combat_meta,
        })
    }
}

/// Compute (dx, dy) for a movement intent given the player's facing direction.
fn movement_delta(facing: i32, intent: i32) -> (i32, i32) {
    let (fx, fy) = match facing {
        FACING_NORTH => (0, -1),
        FACING_EAST => (1, 0),
        FACING_SOUTH => (0, 1),
        FACING_WEST => (-1, 0),
        _ => (0, 0),
    };

    match intent {
        1 => (fx, fy),
        2 => (-fx, -fy),
        3 => (fy, -fx),
        4 => (-fy, fx),
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enemy_gen::{build_enemies_table, EnemySpawn, EnemyType};
    use crate::map_gen::generate_great_hall;
    use crate::visibility::build_visibility_table;
    use quack_engine::table::{Table, TableFile};

    fn make_test_world() -> World {
        let (map_tf, _) = generate_great_hall();
        let party_json = include_str!("../data/party.quack.json");
        let party_tf: TableFile = serde_json::from_str(party_json).unwrap();
        let visibility = build_visibility_table(6, "warrior", "Party", 3);

        let mut world = World::new();
        world.add_table("Map".into(), Table::from_file(map_tf));
        world.add_table("Party".into(), Table::from_file(party_tf));
        world.add_table("Visibility".into(), visibility);
        world.tick_order = vec!["Map".into(), "Party".into(), "Visibility".into()];
        world
    }

    fn make_test_world_with_enemies() -> World {
        let (map_tf, _) = generate_great_hall();
        let party_json = include_str!("../data/party.quack.json");
        let party_tf: TableFile = serde_json::from_str(party_json).unwrap();
        let visibility = build_visibility_table(6, "warrior", "Party", 3);
        let enemies = build_enemies_table(&[
            EnemySpawn { x: 1, y: 0, enemy_type: EnemyType::Slime },
        ]);
        let spells_json = include_str!("../data/spells.quack.json");
        let spells_tf: TableFile = serde_json::from_str(spells_json).unwrap();

        let mut world = World::new();
        world.add_table("Map".into(), Table::from_file(map_tf));
        world.add_table("Party".into(), Table::from_file(party_tf));
        world.add_table("Visibility".into(), visibility);
        world.add_table("Enemies".into(), enemies);
        world.add_table("Spells".into(), Table::from_file(spells_tf));
        world.tick_order = vec!["Map".into(), "Party".into(), "Enemies".into(), "Spells".into(), "Visibility".into()];
        world
    }

    fn make_game() -> GameState {
        let world = make_test_world();
        GameState::new(world, 6)
    }

    fn make_game_with_enemies() -> GameState {
        let world = make_test_world_with_enemies();
        GameState::new(world, 6)
    }

    // --- Movement tests (unchanged) ---

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
        game.set_move_intent("forward");
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let px = party.col_index("PosX").unwrap();
        let py = party.col_index("PosY").unwrap();
        assert_eq!(warrior.cells[px].as_val(), 1.0);
        assert_eq!(warrior.cells[py].as_val(), 0.0);
    }

    #[test]
    fn test_movement_blocked_by_north_wall() {
        let mut game = make_game();
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
        assert_eq!(warrior.cells[px].as_val(), 0.0);
        assert_eq!(warrior.cells[py].as_val(), 0.0);
    }

    #[test]
    fn test_movement_blocked_by_bounds() {
        let mut game = make_game();
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
        assert_eq!(warrior.cells[px].as_val(), 0.0);
    }

    #[test]
    fn test_visibility_near_start() {
        let mut game = make_game();
        game.tick().unwrap();

        let vis = game.world.table("Visibility").unwrap();
        let vis_col = vis.col_index("Visible").unwrap();

        let v00 = vis.entity_by_label("vis_0_0").unwrap();
        assert_eq!(v00.cells[vis_col].as_val(), 1.0);

        let v55 = vis.entity_by_label("vis_5_5").unwrap();
        assert_eq!(v55.cells[vis_col].as_val(), 0.0);
    }

    #[test]
    fn test_discovered_persists() {
        let mut game = make_game();
        game.tick().unwrap();
        game.tick().unwrap();

        let vis = game.world.table("Visibility").unwrap();
        let disc_col = vis.col_index("Discovered").unwrap();
        let v00 = vis.entity_by_label("vis_0_0").unwrap();
        assert_eq!(v00.cells[disc_col].as_val(), 1.0);

        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PosX".into(), value: 5.0,
        });
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PosY".into(), value: 5.0,
        });

        game.tick().unwrap();

        let vis = game.world.table("Visibility").unwrap();
        let v00 = vis.entity_by_label("vis_0_0").unwrap();
        let vis_col = vis.col_index("Visible").unwrap();
        let disc_col = vis.col_index("Discovered").unwrap();
        assert_eq!(v00.cells[vis_col].as_val(), 0.0);
        assert_eq!(v00.cells[disc_col].as_val(), 1.0);
    }

    #[test]
    fn test_find_where_resolves_tile() {
        let mut game = make_game();
        game.tick().unwrap();

        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let tile_id_col = party.col_index("CurrentTileId").unwrap();
        let tile_id = warrior.cells[tile_id_col].as_val();
        assert_eq!(tile_id, 1.0);
    }

    #[test]
    fn test_multi_step_traversal() {
        let mut game = make_game();
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
        assert_eq!(movement_delta(FACING_NORTH, 1), (0, -1));
        assert_eq!(movement_delta(FACING_EAST, 1), (1, 0));
        assert_eq!(movement_delta(FACING_SOUTH, 1), (0, 1));
        assert_eq!(movement_delta(FACING_WEST, 1), (-1, 0));
        assert_eq!(movement_delta(FACING_NORTH, 2), (0, 1));
        assert_eq!(movement_delta(FACING_EAST, 2), (-1, 0));
        assert_eq!(movement_delta(FACING_NORTH, 3), (-1, 0));
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

    // --- Combat tests ---

    #[test]
    fn test_combat_triggers_on_adjacent_enemy() {
        let mut game = make_game_with_enemies();
        // Warrior at (0,0), slime at (1,0) — adjacent
        game.tick().unwrap();
        assert!(game.is_in_combat(), "Combat should trigger when enemy is adjacent");
    }

    #[test]
    fn test_combat_does_not_trigger_when_far() {
        let mut game = make_game_with_enemies();
        // Move slime far away first
        game.world.queue_command(EngineCommand::SetCell {
            table: "Enemies".into(), label: "slime_1".into(),
            column: "PosX".into(), value: 5.0,
        });
        game.world.queue_command(EngineCommand::SetCell {
            table: "Enemies".into(), label: "slime_1".into(),
            column: "PosY".into(), value: 5.0,
        });
        game.world.apply_commands().unwrap();
        game.tick().unwrap();
        assert!(!game.is_in_combat());
    }

    #[test]
    fn test_movement_blocked_in_combat() {
        let mut game = make_game_with_enemies();
        game.tick().unwrap(); // triggers combat
        assert!(game.is_in_combat());

        // Try to move — should be ignored
        game.set_move_intent("forward");
        let party = game.world.table("Party").unwrap();
        let warrior = party.entity_by_label("warrior").unwrap();
        let px = party.col_index("PosX").unwrap();
        assert_eq!(warrior.cells[px].as_val(), 0.0);
    }

    #[test]
    fn test_combat_attack_deals_damage() {
        let mut game = make_game_with_enemies();
        game.tick().unwrap(); // triggers combat

        let events = game.process_combat_action("attack", "slime_1", None);
        assert!(!events.is_empty());

        // Slime should have lost HP
        let enemies = game.world.table("Enemies").unwrap();
        let slime = enemies.entity_by_label("slime_1").unwrap();
        let hp_col = enemies.col_index("HP").unwrap();
        assert!(slime.cells[hp_col].as_val() < 15.0, "Slime should have taken damage");
    }

    #[test]
    fn test_combat_full_cycle_to_victory() {
        let mut game = make_game_with_enemies();
        game.tick().unwrap();
        assert!(game.is_in_combat());

        // Attack repeatedly until combat ends
        for _ in 0..20 {
            if !game.is_in_combat() { break; }
            let _events = game.process_combat_action("attack", "slime_1", None);
        }

        // Slime has 15 HP, warrior does 12 atk - 2 def = 10 damage per hit
        // Should be dead in 2 hits
        assert!(!game.is_in_combat(), "Combat should have ended");
    }

    #[test]
    fn test_snapshot_includes_combat_metadata() {
        let mut game = make_game_with_enemies();
        game.tick().unwrap();
        let snap = game.snapshot();
        assert_eq!(snap["combat"]["active"], true);
        assert!(snap["combat"]["turn_order"].is_array());
    }

    #[test]
    fn test_snapshot_includes_enemies() {
        let game = make_game_with_enemies();
        let snap = game.snapshot();
        let tables = snap["tables"].as_object().unwrap();
        assert!(tables.contains_key("Enemies"));
    }

    #[test]
    fn test_spell_fireball_aoe() {
        let mut game = make_game_with_enemies();
        game.tick().unwrap();
        assert!(game.is_in_combat());

        // Switch active character to mage for spell test
        // For now just test that the action processes without panic
        let events = game.process_combat_action("spell", "slime_1", Some("fireball"));
        // Mage may or may not be the current turn actor, but it shouldn't crash
        assert!(!events.is_empty());
    }

    // --- Formula-combat tests (intent column system) ---

    #[test]
    fn test_intent_damage_through_formula() {
        // Write DeltaHP=-10 on warrior, tick, assert HP decreased by 10
        let mut game = make_game_with_enemies();
        let hp_before = get_party_stat(&game.world, "warrior", "HP");
        assert_eq!(hp_before, 50.0);

        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "DeltaHP".into(), value: -10.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();

        let hp_after = get_party_stat(&game.world, "warrior", "HP");
        assert_eq!(hp_after, 40.0, "HP formula should apply DeltaHP=-10");
    }

    #[test]
    fn test_poison_formula_decrement() {
        // Set PoisonIntent=3, tick, assert PoisonTicks=3, then tick 3 more times
        // and assert countdown 2→1→0
        let mut game = make_game_with_enemies();

        // Apply poison intent
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PoisonIntent".into(), value: 3.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();
        game.world.clear_columns("Party", INTENT_COLUMNS);

        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 3.0);

        // Tick 1: 3→2
        game.world.tick().unwrap();
        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 2.0);

        // Tick 2: 2→1
        game.world.tick().unwrap();
        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 1.0);

        // Tick 3: 1→0
        game.world.tick().unwrap();
        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 0.0);
    }

    #[test]
    fn test_poison_damage_in_hp_formula() {
        // With PoisonTicks>0, tick, assert HP drops by 3 via PoisonDmg formula.
        // Note: HP formula (col 5) evaluates before PoisonDmg (col 26) due to
        // column-index ordering. So HP reads the *previous tick's* PoisonDmg.
        // This means PoisonDmg=-3 computed in tick N is consumed by HP in tick N+1.
        let mut game = make_game_with_enemies();

        // Tick 0: Set PoisonIntent=3 → PoisonTicks=3, PoisonDmg=0 (prev PT was 0)
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PoisonIntent".into(), value: 3.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();
        game.world.clear_columns("Party", INTENT_COLUMNS);
        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 3.0);

        // Tick 1: PoisonDmg=-3 (prev PT=3>0), but HP reads prev PoisonDmg=0
        game.world.tick().unwrap();
        assert_eq!(get_party_stat(&game.world, "warrior", "HP"), 50.0,
            "HP unaffected — PoisonDmg lag tick");

        // Tick 2: HP now reads prev PoisonDmg=-3 → HP drops by 3
        game.world.tick().unwrap();
        let hp_after_tick2 = get_party_stat(&game.world, "warrior", "HP");
        assert_eq!(hp_after_tick2, 47.0,
            "Poison should deal 3 damage via PoisonDmg formula (1-tick propagation delay)");
    }

    #[test]
    fn test_alive_formula_on_lethal() {
        // Set DeltaHP to lethal amount, tick, assert Alive=0
        let mut game = make_game_with_enemies();
        assert_eq!(get_party_stat(&game.world, "warrior", "Alive"), 1.0);

        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "DeltaHP".into(), value: -999.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();

        assert_eq!(get_party_stat(&game.world, "warrior", "HP"), 0.0, "HP should be 0");
        assert_eq!(get_party_stat(&game.world, "warrior", "Alive"), 0.0,
            "Alive formula should set to 0 when HP=0");
    }

    #[test]
    fn test_intent_clear_no_double_apply() {
        // Write DeltaHP=-5, tick, clear intents, tick again, assert HP only dropped once
        let mut game = make_game_with_enemies();
        let hp_before = get_party_stat(&game.world, "warrior", "HP");

        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "DeltaHP".into(), value: -5.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();
        game.world.clear_columns("Party", INTENT_COLUMNS);

        let hp_after_first = get_party_stat(&game.world, "warrior", "HP");
        assert_eq!(hp_after_first, hp_before - 5.0);

        // Second tick with cleared intents — DeltaHP=0 now
        game.world.tick().unwrap();
        let hp_after_second = get_party_stat(&game.world, "warrior", "HP");
        assert_eq!(hp_after_second, hp_after_first,
            "HP should not change on second tick after intents cleared");
    }

    #[test]
    fn test_heal_intent_caps_at_max() {
        // Damage first, then DeltaHP=+999, assert HP capped at MaxHP
        let mut game = make_game_with_enemies();

        // Deal 20 damage
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "DeltaHP".into(), value: -20.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();
        game.world.clear_columns("Party", INTENT_COLUMNS);
        assert_eq!(get_party_stat(&game.world, "warrior", "HP"), 30.0);

        // Heal for 999
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "DeltaHP".into(), value: 999.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();

        assert_eq!(get_party_stat(&game.world, "warrior", "HP"), 50.0,
            "HP should be capped at MaxHP=50");
    }

    #[test]
    fn test_stun_intent_overrides_countdown() {
        // Set PoisonTicks counting down, then write PoisonIntent=3 mid-countdown
        let mut game = make_game_with_enemies();

        // Apply PoisonIntent=2 first
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PoisonIntent".into(), value: 2.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();
        game.world.clear_columns("Party", INTENT_COLUMNS);
        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 2.0);

        // Tick once to decrement: 2→1
        game.world.tick().unwrap();
        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 1.0);

        // Now override with PoisonIntent=3 — should reset to 3
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PoisonIntent".into(), value: 3.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();
        assert_eq!(get_party_stat(&game.world, "warrior", "PoisonTicks"), 3.0,
            "PoisonIntent should override countdown");
    }

    #[test]
    fn test_multiple_intents_same_tick() {
        // DeltaHP from attack + PoisonDmg from formula, assert both apply.
        // Due to column-index ordering, HP reads prev(PoisonDmg). So we need
        // one propagation tick after PoisonDmg first computes to -3.
        let mut game = make_game_with_enemies();

        // Tick 0: Set PoisonIntent=3 → PoisonTicks=3, PoisonDmg=0
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "PoisonIntent".into(), value: 3.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();
        game.world.clear_columns("Party", INTENT_COLUMNS);

        // Tick 1: PoisonDmg=-3 computed (prev PT=3>0), HP reads prev PoisonDmg=0
        game.world.tick().unwrap();
        let hp_before = get_party_stat(&game.world, "warrior", "HP");
        assert_eq!(hp_before, 50.0);

        // Tick 2: Write DeltaHP=-10. HP reads prev(PoisonDmg)=-3 from tick 1.
        // HP = clamp(50 + (-10) + (-3), 0, 50) = 37
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "DeltaHP".into(), value: -10.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();

        let hp_after = get_party_stat(&game.world, "warrior", "HP");
        assert_eq!(hp_after, 37.0,
            "Both DeltaHP and PoisonDmg should apply in the same tick");
    }

    #[test]
    fn test_combat_end_releases_latches() {
        // Latch InCombat=1, then release and SetCell to 0 (mimicking end_combat).
        // Verifies that after release, the cell is writable again (not stuck latched).
        let mut game = make_game_with_enemies();

        // Latch InCombat=1 on warrior
        game.world.queue_command(EngineCommand::LatchCell {
            table: "Party".into(), label: "warrior".into(),
            column: "InCombat".into(), value: 1.0,
        });
        game.world.apply_commands().unwrap();
        assert_eq!(get_party_stat(&game.world, "warrior", "InCombat"), 1.0);

        // Release the latch + set back to 0 (InCombat has no formula, needs explicit reset)
        game.world.queue_command(EngineCommand::ReleaseCells {
            table: "Party".into(), label: "warrior".into(),
            columns: vec!["InCombat".into()],
        });
        game.world.queue_command(EngineCommand::SetCell {
            table: "Party".into(), label: "warrior".into(),
            column: "InCombat".into(), value: 0.0,
        });
        game.world.apply_commands().unwrap();
        game.world.tick().unwrap();

        assert_eq!(get_party_stat(&game.world, "warrior", "InCombat"), 0.0,
            "InCombat should be 0 after latch release + explicit reset");
    }

    fn get_party_stat(world: &World, label: &str, col: &str) -> f64 {
        let table = world.table("Party").unwrap();
        let ci = table.col_index(col).unwrap();
        let row = table.entity_by_label(label).unwrap();
        row.cells[ci].as_val()
    }
}
