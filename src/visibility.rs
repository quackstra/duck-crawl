use quack_engine::table::{Column, ColumnKind, Table};

/// Build a visibility table for a 6×6 grid, tracking one character's line of sight.
/// Each tile gets a Visible formula (distance-based) and Discovered (sticky latch).
pub fn build_visibility_table(
    grid_size: i32,
    character_label: &str,
    party_table_name: &str,
    sight_range: i32,
) -> Table {
    let columns = vec![
        Column { name: "TileX".into(), kind: ColumnKind::Position },
        Column { name: "TileY".into(), kind: ColumnKind::Position },
        Column { name: "Discovered".into(), kind: ColumnKind::Flag },
        Column { name: "Visible".into(), kind: ColumnKind::Flag },
    ];

    let mut table = Table::new(columns);

    for y in 0..grid_size {
        for x in 0..grid_size {
            let label = format!("vis_{}_{}", x, y);
            table.add_entity(label.clone(), vec![x as f64, y as f64, 0.0, 0.0]);

            // Visible = 1 if within sight_range of character
            let visible_formula = format!(
                "select(lte(dist({}, {}, {}.PosX.{}, {}.PosY.{}), {}), 1, 0)",
                x, y, party_table_name, character_label,
                party_table_name, character_label, sight_range
            );
            table.formulas.insert(
                format!("{}.Visible", label),
                visible_formula,
            );

            // Discovered = 1 if ever seen (sticky latch: once discovered, always discovered)
            let discovered_formula = "select(eq(prev(Self.Discovered), 1), 1, Self.Visible)".to_string();
            table.formulas.insert(
                format!("{}.Discovered", label),
                discovered_formula,
            );
        }
    }

    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visibility_table_size() {
        let table = build_visibility_table(6, "warrior", "Party", 3);
        assert_eq!(table.rows.len(), 36);
        assert_eq!(table.formulas.len(), 72); // 36 Visible + 36 Discovered
    }

    #[test]
    fn test_visibility_formulas_reference_party() {
        let table = build_visibility_table(6, "warrior", "Party", 3);
        let formula = table.formulas.get("vis_2_3.Visible").unwrap();
        assert!(formula.contains("Party.PosX.warrior"));
        assert!(formula.contains("Party.PosY.warrior"));
        assert!(formula.contains("dist(2, 3,"));
    }

    #[test]
    fn test_visibility_discovered_uses_prev() {
        let table = build_visibility_table(6, "warrior", "Party", 3);
        let formula = table.formulas.get("vis_0_0.Discovered").unwrap();
        assert!(formula.contains("prev(Self.Discovered)"));
    }
}
