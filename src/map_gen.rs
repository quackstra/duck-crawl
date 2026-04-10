use std::collections::HashMap;
use quack_engine::table::{Column, ColumnKind, EntityFileRow, TableFile, EntityId};

/// Wall flags for a tile.
#[derive(Debug, Clone, Default)]
pub struct TileWalls {
    pub north: bool,
    pub east: bool,
    pub south: bool,
    pub west: bool,
}

/// Generate the "Great Hall" room — a 6×6 grid with perimeter walls,
/// two 2×2 pillar blocks, and a door on the east wall.
///
/// Returns a TableFile and a tile lookup map: (x, y) -> entity_id.
pub fn generate_great_hall() -> (TableFile, HashMap<(i32, i32), EntityId>) {
    let columns = vec![
        Column { name: "X".into(), kind: ColumnKind::Position },
        Column { name: "Y".into(), kind: ColumnKind::Position },
        Column { name: "WallNorth".into(), kind: ColumnKind::Flag },
        Column { name: "WallEast".into(), kind: ColumnKind::Flag },
        Column { name: "WallSouth".into(), kind: ColumnKind::Flag },
        Column { name: "WallWest".into(), kind: ColumnKind::Flag },
        Column { name: "TileType".into(), kind: ColumnKind::Enum },
        Column { name: "HasDoor".into(), kind: ColumnKind::Flag },
        Column { name: "DoorOpen".into(), kind: ColumnKind::Flag },
        Column { name: "SpawnPoint".into(), kind: ColumnKind::Flag },
    ];

    // Column indices
    const X: usize = 0;
    const Y: usize = 1;
    const WALL_N: usize = 2;
    const WALL_E: usize = 3;
    const WALL_S: usize = 4;
    const WALL_W: usize = 5;
    const TILE_TYPE: usize = 6;
    const HAS_DOOR: usize = 7;
    const DOOR_OPEN: usize = 8;
    const SPAWN: usize = 9;

    let size = 6;

    // Pillar blocks: (1,1)-(2,2) and (3,3)-(4,4)
    let pillars: Vec<(i32, i32)> = vec![(1, 1), (2, 1), (1, 2), (2, 2), (3, 3), (4, 3), (3, 4), (4, 4)];

    // Build a wall grid — each tile has its own wall flags
    let mut walls: HashMap<(i32, i32), TileWalls> = HashMap::new();
    for y in 0..size {
        for x in 0..size {
            let mut w = TileWalls::default();
            // Perimeter walls
            if y == 0 { w.north = true; }
            if y == size - 1 { w.south = true; }
            if x == 0 { w.west = true; }
            if x == size - 1 { w.east = true; }
            walls.insert((x, y), w);
        }
    }

    // Add pillar walls — each pillar tile gets walls on faces adjacent to non-pillar tiles
    for &(px, py) in &pillars {
        let w = walls.get_mut(&(px, py)).unwrap();
        // North face: wall if (px, py-1) is not a pillar
        if py == 0 || !pillars.contains(&(px, py - 1)) { w.north = true; }
        if py == size - 1 || !pillars.contains(&(px, py + 1)) { w.south = true; }
        if px == 0 || !pillars.contains(&(px - 1, py)) { w.west = true; }
        if px == size - 1 || !pillars.contains(&(px + 1, py)) { w.east = true; }
    }

    // Ensure bilateral wall consistency: if tile A has a wall toward B, B has wall toward A
    let coords: Vec<(i32, i32)> = (0..size)
        .flat_map(|y| (0..size).map(move |x| (x, y)))
        .collect();

    for &(x, y) in &coords {
        // Check east neighbor
        if x + 1 < size {
            let a_east = walls[&(x, y)].east;
            let b_west = walls[&(x + 1, y)].west;
            if a_east || b_west {
                walls.get_mut(&(x, y)).unwrap().east = true;
                walls.get_mut(&(x + 1, y)).unwrap().west = true;
            }
        }
        // Check south neighbor
        if y + 1 < size {
            let a_south = walls[&(x, y)].south;
            let b_north = walls[&(x, y + 1)].north;
            if a_south || b_north {
                walls.get_mut(&(x, y)).unwrap().south = true;
                walls.get_mut(&(x, y + 1)).unwrap().north = true;
            }
        }
    }

    // Door at east wall of (5, 3) — remove east wall
    walls.get_mut(&(5, 3)).unwrap().east = false;

    // Build entities
    let mut entities = Vec::new();
    let mut tile_lookup = HashMap::new();

    for y in 0..size {
        for x in 0..size {
            let id = (y * size + x + 1) as u64;
            let label = format!("tile_{}_{}", x, y);
            let w = &walls[&(x, y)];

            let is_pillar = pillars.contains(&(x, y));
            let tile_type = if is_pillar { 3.0 } else { 0.0 }; // 3=solid/pit, 0=floor

            // Spawn points: top-left 2x2 that aren't pillars
            let spawn = if x <= 1 && y <= 1 && !is_pillar { 1.0 } else { 0.0 };

            // Door tile
            let has_door = if x == 5 && y == 3 { 1.0 } else { 0.0 };
            let door_open = has_door; // starts open

            let mut cells = vec![0.0_f64; 10];
            cells[X] = x as f64;
            cells[Y] = y as f64;
            cells[WALL_N] = if w.north { 1.0 } else { 0.0 };
            cells[WALL_E] = if w.east { 1.0 } else { 0.0 };
            cells[WALL_S] = if w.south { 1.0 } else { 0.0 };
            cells[WALL_W] = if w.west { 1.0 } else { 0.0 };
            cells[TILE_TYPE] = tile_type;
            cells[HAS_DOOR] = has_door;
            cells[DOOR_OPEN] = door_open;
            cells[SPAWN] = spawn;

            let cell_files: Vec<quack_engine::table::CellFile> =
                cells.iter().map(|&v| quack_engine::table::CellFile::Direct(v)).collect();

            entities.push(EntityFileRow {
                id,
                label,
                cells: cell_files,
            });

            tile_lookup.insert((x as i32, y as i32), id);
        }
    }

    let tf = TableFile {
        version: "0.1".into(),
        columns,
        entities,
        formulas: HashMap::new(),
    };

    (tf, tile_lookup)
}

/// Build a tile lookup from an existing Map table.
pub fn build_tile_lookup(table: &quack_engine::table::Table) -> HashMap<(i32, i32), EntityId> {
    let x_col = table.col_index("X").expect("Map table must have X column");
    let y_col = table.col_index("Y").expect("Map table must have Y column");

    let mut lookup = HashMap::new();
    for row in &table.rows {
        if !row.alive { continue; }
        let x = row.cells[x_col].as_val() as i32;
        let y = row.cells[y_col].as_val() as i32;
        lookup.insert((x, y), row.id);
    }
    lookup
}

/// Validate bilateral wall consistency in a Map table.
pub fn validate_walls(table: &quack_engine::table::Table) -> Result<(), Vec<String>> {
    let lookup = build_tile_lookup(table);
    let wn = table.col_index("WallNorth").unwrap();
    let we = table.col_index("WallEast").unwrap();
    let ws = table.col_index("WallSouth").unwrap();
    let ww = table.col_index("WallWest").unwrap();

    let mut errors = Vec::new();

    for (&(x, y), &id) in &lookup {
        let row = table.entity_by_id(id).unwrap();

        // Check east neighbor
        if let Some(&neighbor_id) = lookup.get(&(x + 1, y)) {
            let neighbor = table.entity_by_id(neighbor_id).unwrap();
            let a_east = row.cells[we].as_val();
            let b_west = neighbor.cells[ww].as_val();
            if a_east != b_west {
                errors.push(format!(
                    "Wall mismatch: tile({},{}).WallEast={} but tile({},{}).WallWest={}",
                    x, y, a_east, x + 1, y, b_west
                ));
            }
        }

        // Check south neighbor
        if let Some(&neighbor_id) = lookup.get(&(x, y + 1)) {
            let neighbor = table.entity_by_id(neighbor_id).unwrap();
            let a_south = row.cells[ws].as_val();
            let b_north = neighbor.cells[wn].as_val();
            if a_south != b_north {
                errors.push(format!(
                    "Wall mismatch: tile({},{}).WallSouth={} but tile({},{}).WallNorth={}",
                    x, y, a_south, x, y + 1, b_north
                ));
            }
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quack_engine::table::Table;

    #[test]
    fn test_great_hall_has_36_tiles() {
        let (tf, lookup) = generate_great_hall();
        assert_eq!(tf.entities.len(), 36);
        assert_eq!(lookup.len(), 36);
    }

    #[test]
    fn test_great_hall_coordinates_complete() {
        let (_, lookup) = generate_great_hall();
        for y in 0..6 {
            for x in 0..6 {
                assert!(lookup.contains_key(&(x, y)), "Missing tile ({}, {})", x, y);
            }
        }
    }

    #[test]
    fn test_great_hall_perimeter_walls() {
        let (tf, _) = generate_great_hall();
        let table = Table::from_file(tf);
        let wn = table.col_index("WallNorth").unwrap();
        let ws = table.col_index("WallSouth").unwrap();
        let we = table.col_index("WallEast").unwrap();
        let ww = table.col_index("WallWest").unwrap();

        for row in &table.rows {
            let x = row.cells[0].as_val() as i32;
            let y = row.cells[1].as_val() as i32;

            if y == 0 {
                assert_eq!(row.cells[wn].as_val(), 1.0, "tile({},{}) should have north wall", x, y);
            }
            if y == 5 {
                assert_eq!(row.cells[ws].as_val(), 1.0, "tile({},{}) should have south wall", x, y);
            }
            if x == 0 {
                assert_eq!(row.cells[ww].as_val(), 1.0, "tile({},{}) should have west wall", x, y);
            }
            // East wall at x=5 except the door tile (5,3)
            if x == 5 && y != 3 {
                assert_eq!(row.cells[we].as_val(), 1.0, "tile({},{}) should have east wall", x, y);
            }
        }
    }

    #[test]
    fn test_great_hall_bilateral_consistency() {
        let (tf, _) = generate_great_hall();
        let table = Table::from_file(tf);
        validate_walls(&table).expect("Wall validation should pass");
    }

    #[test]
    fn test_great_hall_spawn_points() {
        let (tf, _) = generate_great_hall();
        let table = Table::from_file(tf);
        let spawn_col = table.col_index("SpawnPoint").unwrap();

        // (0,0) should be a spawn point (not a pillar)
        let t00 = table.entity_by_label("tile_0_0").unwrap();
        assert_eq!(t00.cells[spawn_col].as_val(), 1.0);

        // (1,1) is a pillar — should NOT be a spawn
        let t11 = table.entity_by_label("tile_1_1").unwrap();
        assert_eq!(t11.cells[spawn_col].as_val(), 0.0);
    }

    #[test]
    fn test_great_hall_door() {
        let (tf, _) = generate_great_hall();
        let table = Table::from_file(tf);
        let door_col = table.col_index("HasDoor").unwrap();
        let open_col = table.col_index("DoorOpen").unwrap();

        let t53 = table.entity_by_label("tile_5_3").unwrap();
        assert_eq!(t53.cells[door_col].as_val(), 1.0);
        assert_eq!(t53.cells[open_col].as_val(), 1.0);
    }
}
