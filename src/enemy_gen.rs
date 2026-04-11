use quack_engine::table::{Column, ColumnKind, Table};

/// Enemy type presets with base stats.
#[derive(Debug, Clone, Copy)]
pub enum EnemyType {
    Slime,
    Rat,
    Skeleton,
}

impl EnemyType {
    pub fn type_id(self) -> f64 {
        match self {
            EnemyType::Slime => 0.0,
            EnemyType::Rat => 1.0,
            EnemyType::Skeleton => 2.0,
        }
    }

    pub fn stats(self) -> EnemyStats {
        match self {
            EnemyType::Slime => EnemyStats {
                hp: 15.0, attack: 5.0, defense: 2.0, speed: 3.0, range: 1.0,
            },
            EnemyType::Rat => EnemyStats {
                hp: 8.0, attack: 3.0, defense: 1.0, speed: 8.0, range: 1.0,
            },
            EnemyType::Skeleton => EnemyStats {
                hp: 20.0, attack: 8.0, defense: 5.0, speed: 4.0, range: 1.0,
            },
        }
    }

    pub fn prefix(self) -> &'static str {
        match self {
            EnemyType::Slime => "slime",
            EnemyType::Rat => "rat",
            EnemyType::Skeleton => "skel",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnemyStats {
    pub hp: f64,
    pub attack: f64,
    pub defense: f64,
    pub speed: f64,
    pub range: f64,
}

/// Spawn definition: position + type.
pub struct EnemySpawn {
    pub x: i32,
    pub y: i32,
    pub enemy_type: EnemyType,
}

/// Build an Enemies table from a list of spawn definitions.
pub fn build_enemies_table(spawns: &[EnemySpawn]) -> Table {
    let columns = vec![
        Column { name: "PosX".into(), kind: ColumnKind::Position },
        Column { name: "PosY".into(), kind: ColumnKind::Position },
        Column { name: "HP".into(), kind: ColumnKind::Stat },
        Column { name: "MaxHP".into(), kind: ColumnKind::Stat },
        Column { name: "Attack".into(), kind: ColumnKind::Stat },
        Column { name: "Defense".into(), kind: ColumnKind::Stat },
        Column { name: "Speed".into(), kind: ColumnKind::Stat },
        Column { name: "Type".into(), kind: ColumnKind::Enum },
        Column { name: "Alive".into(), kind: ColumnKind::Flag },
        Column { name: "AggroTarget".into(), kind: ColumnKind::Index },
        Column { name: "Range".into(), kind: ColumnKind::Stat },
        Column { name: "PoisonTicks".into(), kind: ColumnKind::Status },
        Column { name: "StunTicks".into(), kind: ColumnKind::Status },
        Column { name: "ShieldTicks".into(), kind: ColumnKind::Status },
        Column { name: "ShieldAmount".into(), kind: ColumnKind::Stat },
        Column { name: "DeltaHP".into(), kind: ColumnKind::Custom },
        Column { name: "DeltaMana".into(), kind: ColumnKind::Custom },
        Column { name: "PoisonIntent".into(), kind: ColumnKind::Custom },
        Column { name: "StunIntent".into(), kind: ColumnKind::Custom },
        Column { name: "ShieldIntent".into(), kind: ColumnKind::Custom },
        Column { name: "PoisonDmg".into(), kind: ColumnKind::Custom },
    ];

    let mut table = Table::new(columns);
    let mut counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();

    for spawn in spawns {
        let prefix = spawn.enemy_type.prefix();
        let count = counts.entry(prefix).or_insert(0);
        *count += 1;
        let label = format!("{}_{}", prefix, count);
        let stats = spawn.enemy_type.stats();

        table.add_entity(label.clone(), vec![
            spawn.x as f64,   // PosX
            spawn.y as f64,   // PosY
            stats.hp,         // HP
            stats.hp,         // MaxHP
            stats.attack,     // Attack
            stats.defense,    // Defense
            stats.speed,      // Speed
            spawn.enemy_type.type_id(), // Type
            1.0,              // Alive
            0.0,              // AggroTarget
            stats.range,      // Range
            0.0,              // PoisonTicks
            0.0,              // StunTicks
            0.0,              // ShieldTicks
            0.0,              // ShieldAmount
            0.0,              // DeltaHP
            0.0,              // DeltaMana
            0.0,              // PoisonIntent
            0.0,              // StunIntent
            0.0,              // ShieldIntent
            0.0,              // PoisonDmg
        ]);

        // Add combat formulas for this entity
        table.formulas.insert(
            format!("{}.HP", label),
            "clamp(prev(Self.HP) + Self.DeltaHP + Self.PoisonDmg, 0, Self.MaxHP)".into(),
        );
        table.formulas.insert(
            format!("{}.PoisonTicks", label),
            "select(Self.PoisonIntent > 0, Self.PoisonIntent, max(prev(Self.PoisonTicks) - 1, 0))".into(),
        );
        table.formulas.insert(
            format!("{}.StunTicks", label),
            "select(Self.StunIntent > 0, Self.StunIntent, max(prev(Self.StunTicks) - 1, 0))".into(),
        );
        table.formulas.insert(
            format!("{}.ShieldTicks", label),
            "select(Self.ShieldIntent > 0, Self.ShieldIntent, max(prev(Self.ShieldTicks) - 1, 0))".into(),
        );
        table.formulas.insert(
            format!("{}.PoisonDmg", label),
            "select(prev(Self.PoisonTicks) > 0, -3, 0)".into(),
        );
        table.formulas.insert(
            format!("{}.Alive", label),
            "select(Self.HP > 0, prev(Self.Alive), 0)".into(),
        );
    }

    table
}

/// Default enemy spawns for the Great Hall.
pub fn great_hall_enemies() -> Vec<EnemySpawn> {
    vec![
        EnemySpawn { x: 4, y: 1, enemy_type: EnemyType::Rat },
        EnemySpawn { x: 3, y: 5, enemy_type: EnemyType::Slime },
        EnemySpawn { x: 5, y: 4, enemy_type: EnemyType::Skeleton },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_enemies_table() {
        let spawns = great_hall_enemies();
        let table = build_enemies_table(&spawns);
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.columns.len(), 21);
    }

    #[test]
    fn test_enemy_labels_unique() {
        let spawns = vec![
            EnemySpawn { x: 0, y: 0, enemy_type: EnemyType::Slime },
            EnemySpawn { x: 1, y: 1, enemy_type: EnemyType::Slime },
        ];
        let table = build_enemies_table(&spawns);
        assert_eq!(table.rows[0].label, "slime_1");
        assert_eq!(table.rows[1].label, "slime_2");
    }

    #[test]
    fn test_enemy_stats_correct() {
        let spawns = vec![EnemySpawn { x: 3, y: 3, enemy_type: EnemyType::Skeleton }];
        let table = build_enemies_table(&spawns);
        let hp_col = table.col_index("HP").unwrap();
        let atk_col = table.col_index("Attack").unwrap();
        assert_eq!(table.get_val(1, hp_col), 20.0);
        assert_eq!(table.get_val(1, atk_col), 8.0);
    }
}
