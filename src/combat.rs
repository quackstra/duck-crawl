use quack_engine::command::EngineCommand;
use quack_engine::table::{EntityId, Val};
use quack_engine::World;
use serde::Serialize;

/// Combat state machine.
#[derive(Debug, Clone)]
pub enum CombatState {
    Exploring,
    InCombat(CombatContext),
}

/// Active combat context.
#[derive(Debug, Clone)]
pub struct CombatContext {
    pub round: u32,
    pub turn_index: usize,
    pub turn_order: Vec<Combatant>,
    pub engaged_enemies: Vec<String>,
    pub log: Vec<CombatEvent>,
    #[allow(dead_code)]
    pub waiting_for_player: bool,
}

/// A participant in combat.
#[derive(Debug, Clone, Serialize)]
pub struct Combatant {
    pub label: String,
    pub table: CombatantTable,
    pub initiative: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum CombatantTable {
    Party,
    Enemies,
}

/// A single combat event for the log.
#[derive(Debug, Clone, Serialize)]
pub struct CombatEvent {
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub damage: Option<f64>,
    pub heal: Option<f64>,
    pub effect: Option<String>,
    pub killed: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CombatOutcome {
    Victory,
    TPK,
}

/// Check if any alive enemy is adjacent (Manhattan distance <= 1) to the warrior.
/// Returns labels of adjacent enemies.
pub fn check_combat_trigger(world: &World, active_char: &str) -> Vec<String> {
    let party = match world.table("Party") {
        Some(t) => t,
        None => return vec![],
    };
    let enemies = match world.table("Enemies") {
        Some(t) => t,
        None => return vec![],
    };

    let warrior = match party.entity_by_label(active_char) {
        Some(r) => r,
        None => return vec![],
    };

    let px_col = party.col_index("PosX").unwrap();
    let py_col = party.col_index("PosY").unwrap();
    let wx = warrior.cells[px_col].as_val() as i32;
    let wy = warrior.cells[py_col].as_val() as i32;

    let ex_col = enemies.col_index("PosX").unwrap();
    let ey_col = enemies.col_index("PosY").unwrap();
    let alive_col = enemies.col_index("Alive").unwrap();

    let mut adjacent = Vec::new();
    for row in &enemies.rows {
        if !row.alive || row.cells[alive_col].as_val() != 1.0 {
            continue;
        }
        let ex = row.cells[ex_col].as_val() as i32;
        let ey = row.cells[ey_col].as_val() as i32;
        let dist = (wx - ex).abs() + (wy - ey).abs();
        if dist <= 1 {
            adjacent.push(row.label.clone());
        }
    }
    adjacent
}

/// Start combat: build turn order from all alive party + only the engaged enemies.
pub fn start_combat(world: &World, tick: u64, engaged_enemies: &[String]) -> CombatContext {
    let mut combatants = Vec::new();

    // Add party members
    if let Some(party) = world.table("Party") {
        let speed_col = party.col_index("Speed").unwrap();
        let alive_col = party.col_index("Alive").unwrap();
        for row in &party.rows {
            if !row.alive || row.cells[alive_col].as_val() != 1.0 { continue; }
            let speed = row.cells[speed_col].as_val();
            combatants.push(Combatant {
                label: row.label.clone(),
                table: CombatantTable::Party,
                initiative: compute_initiative(speed, row.id, tick),
            });
        }
    }

    // Add only engaged enemies (the ones that triggered this encounter)
    if let Some(enemies) = world.table("Enemies") {
        let speed_col = enemies.col_index("Speed").unwrap();
        let alive_col = enemies.col_index("Alive").unwrap();
        for row in &enemies.rows {
            if !row.alive || row.cells[alive_col].as_val() != 1.0 { continue; }
            if !engaged_enemies.contains(&row.label) { continue; }
            let speed = row.cells[speed_col].as_val();
            combatants.push(Combatant {
                label: row.label.clone(),
                table: CombatantTable::Enemies,
                initiative: compute_initiative(speed, row.id, tick),
            });
        }
    }

    // Sort by initiative descending (highest goes first)
    combatants.sort_by(|a, b| b.initiative.partial_cmp(&a.initiative).unwrap());

    CombatContext {
        round: 1,
        turn_index: 0,
        turn_order: combatants,
        engaged_enemies: engaged_enemies.to_vec(),
        log: vec![CombatEvent {
            actor: "system".into(),
            action: "combat_start".into(),
            target: None,
            damage: None,
            heal: None,
            effect: None,
            killed: false,
            message: "Combat begins!".into(),
        }],
        waiting_for_player: false,
    }
}

/// Deterministic initiative: speed + hash-based offset [0, 1).
pub fn compute_initiative(speed: f64, entity_id: EntityId, tick: u64) -> f64 {
    let hash = entity_id.wrapping_mul(2654435761).wrapping_add(tick.wrapping_mul(40503));
    let offset = (hash % 1000) as f64 / 1000.0;
    speed + offset
}

/// Compute damage from an attack.
pub fn compute_damage(
    attacker_attack: f64,
    damage_mult: f64,
    target_defense: f64,
    is_defending: bool,
    shield_amount: f64,
    shield_ticks: f64,
) -> f64 {
    let raw = attacker_attack * damage_mult;
    let def_divisor = if is_defending { 2.0 } else { 1.0 };
    let shield = if shield_ticks > 0.0 { shield_amount } else { 0.0 };
    (raw - (target_defense / def_divisor) - shield).max(1.0)
}

/// Resolve an attack action. Returns the combat event and queues SetCell commands.
pub fn resolve_attack(
    world: &mut World,
    actor_label: &str,
    actor_table: CombatantTable,
    target_label: &str,
    target_table: CombatantTable,
    damage_mult: f64,
) -> CombatEvent {
    let actor_tbl_name = table_name(actor_table);
    let target_tbl_name = table_name(target_table);

    let atk = get_stat(world, actor_tbl_name, actor_label, "Attack");
    let def = get_stat(world, target_tbl_name, target_label, "Defense");
    let shield_ticks = get_stat(world, target_tbl_name, target_label, "ShieldTicks");
    let shield_amount = get_stat(world, target_tbl_name, target_label, "ShieldAmount");
    let target_hp = get_stat(world, target_tbl_name, target_label, "HP");

    // TODO: is_defending could be tracked but for now always false
    let damage = compute_damage(atk, damage_mult, def, false, shield_amount, shield_ticks);
    let new_hp = (target_hp - damage).max(0.0);
    let killed = new_hp <= 0.0;

    world.queue_command(EngineCommand::SetCell {
        table: target_tbl_name.into(),
        label: target_label.into(),
        column: "HP".into(),
        value: new_hp,
    });

    if killed {
        world.queue_command(EngineCommand::SetCell {
            table: target_tbl_name.into(),
            label: target_label.into(),
            column: "Alive".into(),
            value: 0.0,
        });

        // Track kills for party members
        if actor_table == CombatantTable::Party {
            let kills = get_stat(world, actor_tbl_name, actor_label, "TotalKills");
            world.queue_command(EngineCommand::SetCell {
                table: actor_tbl_name.into(),
                label: actor_label.into(),
                column: "TotalKills".into(),
                value: kills + 1.0,
            });
        }
    }

    // Set aggro on skeleton-type enemies
    if target_table == CombatantTable::Enemies {
        let etype = get_stat(world, target_tbl_name, target_label, "Type");
        if etype == 2.0 {
            // Skeleton: set aggro to attacker's entity ID
            if let Some(party) = world.table("Party") {
                if let Some(row) = party.entity_by_label(actor_label) {
                    world.queue_command(EngineCommand::SetCell {
                        table: target_tbl_name.into(),
                        label: target_label.into(),
                        column: "AggroTarget".into(),
                        value: row.id as f64,
                    });
                }
            }
        }
    }

    let msg = if killed {
        format!("{} attacks {} for {:.0} damage — killed!", actor_label, target_label, damage)
    } else {
        format!("{} attacks {} for {:.0} damage (HP: {:.0})", actor_label, target_label, damage, new_hp)
    };

    CombatEvent {
        actor: actor_label.into(),
        action: "attack".into(),
        target: Some(target_label.into()),
        damage: Some(damage),
        heal: None,
        effect: None,
        killed,
        message: msg,
    }
}

/// Resolve a spell cast. Returns the event(s) and queues commands.
pub fn resolve_spell(
    world: &mut World,
    caster_label: &str,
    caster_table: CombatantTable,
    target_label: &str,
    target_table: CombatantTable,
    spell_label: &str,
) -> Vec<CombatEvent> {
    let caster_tbl = table_name(caster_table);

    // Extract all spell data upfront to avoid borrow conflicts
    let spell_data = {
        let spells = match world.table("Spells") {
            Some(t) => t,
            None => return vec![CombatEvent::system("No Spells table found")],
        };
        let spell = match spells.entity_by_label(spell_label) {
            Some(r) => r,
            None => return vec![CombatEvent::system(&format!("Unknown spell: {}", spell_label))],
        };
        (
            col_val(spells, spell, "ManaCost"),
            col_val(spells, spell, "EffectType") as i32,
            col_val(spells, spell, "EffectDuration"),
            col_val(spells, spell, "EffectMagnitude"),
            col_val(spells, spell, "DamageMult"),
            col_val(spells, spell, "HealAmount"),
            col_val(spells, spell, "AOE") == 1.0,
        )
    };

    let (mana_cost, effect_type, effect_dur, effect_mag, damage_mult, heal_amount, is_aoe) = spell_data;
    let mut events = Vec::new();
    let caster_mana = get_stat(world, caster_tbl, caster_label, "Mana");

    if caster_mana < mana_cost {
        return vec![CombatEvent {
            actor: caster_label.into(),
            action: "spell_fail".into(),
            target: None, damage: None, heal: None, effect: None, killed: false,
            message: format!("{} lacks mana for {} ({:.0}/{:.0})", caster_label, spell_label, caster_mana, mana_cost),
        }];
    }

    // Deduct mana
    world.queue_command(EngineCommand::SetCell {
        table: caster_tbl.into(),
        label: caster_label.into(),
        column: "Mana".into(),
        value: caster_mana - mana_cost,
    });

    let targets = if is_aoe {
        // Get all alive enemies
        get_alive_labels(world, table_name(target_table))
    } else {
        vec![target_label.to_string()]
    };

    for t_label in &targets {
        match effect_type {
            0 => {
                // Damage spell
                let evt = resolve_attack(world, caster_label, caster_table, t_label, target_table, damage_mult);
                let mut evt = evt;
                evt.action = format!("spell:{}", spell_label);
                events.push(evt);
            }
            1 => {
                // Heal
                let target_tbl = table_name(target_table);
                let hp = get_stat(world, target_tbl, t_label, "HP");
                let max_hp = get_stat(world, target_tbl, t_label, "MaxHP");
                let new_hp = (hp + heal_amount).min(max_hp);
                world.queue_command(EngineCommand::SetCell {
                    table: target_tbl.into(),
                    label: t_label.clone(),
                    column: "HP".into(),
                    value: new_hp,
                });
                events.push(CombatEvent {
                    actor: caster_label.into(),
                    action: format!("spell:{}", spell_label),
                    target: Some(t_label.clone()),
                    damage: None,
                    heal: Some(heal_amount),
                    effect: Some("heal".into()),
                    killed: false,
                    message: format!("{} heals {} for {:.0} HP ({:.0})", caster_label, t_label, heal_amount, new_hp),
                });
            }
            2 => {
                // Poison
                let target_tbl = table_name(target_table);
                world.queue_command(EngineCommand::SetCell {
                    table: target_tbl.into(),
                    label: t_label.clone(),
                    column: "PoisonTicks".into(),
                    value: effect_dur,
                });
                events.push(CombatEvent {
                    actor: caster_label.into(),
                    action: format!("spell:{}", spell_label),
                    target: Some(t_label.clone()),
                    damage: None, heal: None,
                    effect: Some(format!("poison:{:.0}", effect_dur)),
                    killed: false,
                    message: format!("{} poisons {} for {:.0} ticks", caster_label, t_label, effect_dur),
                });
            }
            3 => {
                // Stun
                let target_tbl = table_name(target_table);
                world.queue_command(EngineCommand::SetCell {
                    table: target_tbl.into(),
                    label: t_label.clone(),
                    column: "StunTicks".into(),
                    value: effect_dur,
                });
                events.push(CombatEvent {
                    actor: caster_label.into(),
                    action: format!("spell:{}", spell_label),
                    target: Some(t_label.clone()),
                    damage: None, heal: None,
                    effect: Some(format!("stun:{:.0}", effect_dur)),
                    killed: false,
                    message: format!("{} stuns {} for {:.0} ticks", caster_label, t_label, effect_dur),
                });
            }
            4 => {
                // Shield (self-target)
                world.queue_command(EngineCommand::SetCell {
                    table: caster_tbl.into(),
                    label: caster_label.into(),
                    column: "ShieldTicks".into(),
                    value: effect_dur,
                });
                world.queue_command(EngineCommand::SetCell {
                    table: caster_tbl.into(),
                    label: caster_label.into(),
                    column: "ShieldAmount".into(),
                    value: effect_mag,
                });
                events.push(CombatEvent {
                    actor: caster_label.into(),
                    action: format!("spell:{}", spell_label),
                    target: Some(caster_label.into()),
                    damage: None, heal: None,
                    effect: Some(format!("shield:{:.0}", effect_dur)),
                    killed: false,
                    message: format!("{} raises a shield for {:.0} ticks", caster_label, effect_dur),
                });
            }
            _ => {}
        }
    }

    events
}

/// Enemy AI: select action and target.
pub fn select_enemy_action(world: &World, enemy_label: &str) -> (String, String) {
    let enemies = world.table("Enemies").unwrap();
    let enemy = enemies.entity_by_label(enemy_label).unwrap();
    let etype = col_val(enemies, enemy, "Type") as i32;

    let party = world.table("Party").unwrap();
    let alive_members = get_alive_party_members(world);

    if alive_members.is_empty() {
        return ("none".into(), "".into());
    }

    let ex = col_val(enemies, enemy, "PosX") as i32;
    let ey = col_val(enemies, enemy, "PosY") as i32;

    match etype {
        0 => {
            // Slime: target nearest party member
            let target = nearest_party_member(party, &alive_members, ex, ey);
            ("attack".into(), target)
        }
        1 => {
            // Rat: target lowest HP party member
            let target = lowest_hp_party_member(party, &alive_members);
            ("attack".into(), target)
        }
        2 => {
            // Skeleton: target aggro holder, or nearest
            let aggro = col_val(enemies, enemy, "AggroTarget") as u64;
            if aggro > 0 {
                if let Some(row) = party.entity_by_id(aggro) {
                    let alive_col = party.col_index("Alive").unwrap();
                    if row.alive && row.cells[alive_col].as_val() == 1.0 {
                        return ("attack".into(), row.label.clone());
                    }
                }
            }
            let target = nearest_party_member(party, &alive_members, ex, ey);
            ("attack".into(), target)
        }
        _ => {
            let target = nearest_party_member(party, &alive_members, ex, ey);
            ("attack".into(), target)
        }
    }
}

/// Apply start-of-turn effects: poison damage, stun check, decrement counters.
/// Returns events and whether the entity is stunned (skip turn).
pub fn apply_turn_start_effects(world: &mut World, label: &str, table: CombatantTable) -> (Vec<CombatEvent>, bool) {
    let tbl_name = table_name(table);
    let mut events = Vec::new();

    let poison = get_stat(world, tbl_name, label, "PoisonTicks");
    let stun = get_stat(world, tbl_name, label, "StunTicks");

    // Poison damage
    if poison > 0.0 {
        let hp = get_stat(world, tbl_name, label, "HP");
        let dmg = 3.0; // flat poison damage per tick
        let new_hp = (hp - dmg).max(0.0);
        world.queue_command(EngineCommand::SetCell {
            table: tbl_name.into(), label: label.into(),
            column: "HP".into(), value: new_hp,
        });
        world.queue_command(EngineCommand::SetCell {
            table: tbl_name.into(), label: label.into(),
            column: "PoisonTicks".into(), value: poison - 1.0,
        });
        let killed = new_hp <= 0.0;
        if killed {
            world.queue_command(EngineCommand::SetCell {
                table: tbl_name.into(), label: label.into(),
                column: "Alive".into(), value: 0.0,
            });
        }
        events.push(CombatEvent {
            actor: "poison".into(),
            action: "dot".into(),
            target: Some(label.into()),
            damage: Some(dmg),
            heal: None,
            effect: Some(format!("poison:{:.0}", poison - 1.0)),
            killed,
            message: format!("{} takes {:.0} poison damage (HP: {:.0})", label, dmg, new_hp),
        });
    }

    // Decrement stun
    let stunned = stun > 0.0;
    if stunned {
        world.queue_command(EngineCommand::SetCell {
            table: tbl_name.into(), label: label.into(),
            column: "StunTicks".into(), value: stun - 1.0,
        });
        events.push(CombatEvent {
            actor: label.into(),
            action: "stunned".into(),
            target: None, damage: None, heal: None,
            effect: Some(format!("stun:{:.0}", stun - 1.0)),
            killed: false,
            message: format!("{} is stunned!", label),
        });
    }

    // Decrement shield
    let shield = get_stat(world, tbl_name, label, "ShieldTicks");
    if shield > 0.0 {
        world.queue_command(EngineCommand::SetCell {
            table: tbl_name.into(), label: label.into(),
            column: "ShieldTicks".into(), value: shield - 1.0,
        });
    }

    (events, stunned)
}

/// Check if combat should end. Only checks engaged enemies, not all enemies on the map.
pub fn check_combat_end(world: &World, engaged_enemies: &[String]) -> Option<CombatOutcome> {
    let party_alive = count_alive(world, "Party");
    if party_alive == 0 {
        return Some(CombatOutcome::TPK);
    }

    // Victory when all engaged enemies are dead
    let engaged_alive = count_alive_by_labels(world, "Enemies", engaged_enemies);
    if engaged_alive == 0 {
        return Some(CombatOutcome::Victory);
    }

    None
}

// --- Helpers ---

fn table_name(t: CombatantTable) -> &'static str {
    match t {
        CombatantTable::Party => "Party",
        CombatantTable::Enemies => "Enemies",
    }
}

fn get_stat(world: &World, tbl: &str, label: &str, col: &str) -> Val {
    let table = match world.table(tbl) {
        Some(t) => t,
        None => return 0.0,
    };
    let ci = match table.col_index(col) {
        Some(i) => i,
        None => return 0.0,
    };
    let row = match table.entity_by_label(label) {
        Some(r) => r,
        None => return 0.0,
    };
    row.cells[ci].as_val()
}

fn col_val(table: &quack_engine::table::Table, row: &quack_engine::table::EntityRow, col: &str) -> Val {
    let ci = match table.col_index(col) {
        Some(i) => i,
        None => return 0.0,
    };
    row.cells[ci].as_val()
}

fn count_alive(world: &World, tbl: &str) -> usize {
    let table = match world.table(tbl) {
        Some(t) => t,
        None => return 0,
    };
    let alive_col = match table.col_index("Alive") {
        Some(i) => i,
        None => return 0,
    };
    table.rows.iter().filter(|r| r.alive && r.cells[alive_col].as_val() == 1.0).count()
}

fn count_alive_by_labels(world: &World, tbl: &str, labels: &[String]) -> usize {
    let table = match world.table(tbl) {
        Some(t) => t,
        None => return 0,
    };
    let alive_col = match table.col_index("Alive") {
        Some(i) => i,
        None => return 0,
    };
    table.rows.iter()
        .filter(|r| r.alive && r.cells[alive_col].as_val() == 1.0 && labels.contains(&r.label))
        .count()
}

fn get_alive_labels(world: &World, tbl: &str) -> Vec<String> {
    let table = match world.table(tbl) {
        Some(t) => t,
        None => return vec![],
    };
    let alive_col = match table.col_index("Alive") {
        Some(i) => i,
        None => return vec![],
    };
    table.rows.iter()
        .filter(|r| r.alive && r.cells[alive_col].as_val() == 1.0)
        .map(|r| r.label.clone())
        .collect()
}

fn get_alive_party_members(world: &World) -> Vec<String> {
    get_alive_labels(world, "Party")
}

fn nearest_party_member(
    party: &quack_engine::table::Table,
    alive: &[String],
    ex: i32, ey: i32,
) -> String {
    let px_col = party.col_index("PosX").unwrap();
    let py_col = party.col_index("PosY").unwrap();

    let mut best = String::new();
    let mut best_dist = i32::MAX;

    for label in alive {
        if let Some(row) = party.entity_by_label(label) {
            let mx = row.cells[px_col].as_val() as i32;
            let my = row.cells[py_col].as_val() as i32;
            let d = (ex - mx).abs() + (ey - my).abs();
            if d < best_dist {
                best_dist = d;
                best = label.clone();
            }
        }
    }
    best
}

fn lowest_hp_party_member(
    party: &quack_engine::table::Table,
    alive: &[String],
) -> String {
    let hp_col = party.col_index("HP").unwrap();

    let mut best = String::new();
    let mut best_hp = f64::MAX;

    for label in alive {
        if let Some(row) = party.entity_by_label(label) {
            let hp = row.cells[hp_col].as_val();
            if hp < best_hp {
                best_hp = hp;
                best = label.clone();
            }
        }
    }
    best
}

impl CombatEvent {
    pub fn system(msg: &str) -> Self {
        CombatEvent {
            actor: "system".into(),
            action: "system".into(),
            target: None,
            damage: None,
            heal: None,
            effect: None,
            killed: false,
            message: msg.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_damage_basic() {
        // 12 attack * 1.0 mult - 5 defense = 7
        assert_eq!(compute_damage(12.0, 1.0, 5.0, false, 0.0, 0.0), 7.0);
    }

    #[test]
    fn test_compute_damage_minimum_one() {
        // 3 attack - 10 defense would be negative, floors at 1
        assert_eq!(compute_damage(3.0, 1.0, 10.0, false, 0.0, 0.0), 1.0);
    }

    #[test]
    fn test_compute_damage_defending() {
        // 12 attack - (10 defense / 2) = 12 - 5 = 7
        assert_eq!(compute_damage(12.0, 1.0, 10.0, true, 0.0, 0.0), 7.0);
    }

    #[test]
    fn test_compute_damage_shield() {
        // 12 attack - 5 defense - 3 shield = 4
        assert_eq!(compute_damage(12.0, 1.0, 5.0, false, 3.0, 2.0), 4.0);
    }

    #[test]
    fn test_compute_damage_shield_expired() {
        // Shield with 0 ticks doesn't apply
        assert_eq!(compute_damage(12.0, 1.0, 5.0, false, 3.0, 0.0), 7.0);
    }

    #[test]
    fn test_compute_damage_spell_multiplier() {
        // 8 attack * 3.0 mult - 5 defense = 19
        assert_eq!(compute_damage(8.0, 3.0, 5.0, false, 0.0, 0.0), 19.0);
    }

    #[test]
    fn test_initiative_deterministic() {
        let a = compute_initiative(5.0, 1, 10);
        let b = compute_initiative(5.0, 1, 10);
        assert_eq!(a, b);
    }

    #[test]
    fn test_initiative_speed_matters() {
        let fast = compute_initiative(12.0, 1, 10);
        let slow = compute_initiative(3.0, 2, 10);
        assert!(fast > slow, "Faster entity should have higher initiative");
    }
}
