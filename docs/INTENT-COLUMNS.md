# Intent Columns: Retrofitting Duck Crawl Combat into the Dataflow

## Overview

This document describes how to move Duck Crawl's imperative combat system (`combat.rs`, ~680 lines) into QuackEngine's formula system using **intent columns** and **cell latching**.

**Goal:** Make combat logic inspectable, moddable, and visible to the AI Game Master through table data instead of compiled Rust.

**Prerequisite:** QuackEngine v0.3 with cell latching (see `quack-engine/docs/CELL-LATCHING.md`).

---

## Architecture: Before and After

### Before (current)

```
Player action → combat.rs (imperative Rust)
  → reads stats from tables
  → computes damage/healing/effects
  → writes results via SetCell
  → world.tick() runs (formulas irrelevant to combat)
```

Combat is a parallel logic layer that uses the engine as a database.

### After (target)

```
Player action → combat.rs (reduced, ~430 lines)
  → writes INTENTS via SetCell (DeltaHP, PoisonIntent, etc.)
  → world.tick() runs formulas that process intents
  → world.clear_columns() resets intents
  → combat reads results from table state
```

Combat writes what happened. Formulas compute what it means.

---

## New Columns

### Intent Columns (ephemeral, cleared after each tick)

Add to both **Party** and **Enemies** tables:

| Column | Type | Written by | Purpose |
|--------|------|-----------|---------|
| `DeltaHP` | Custom | Attack (-dmg), Heal (+heal) | HP change this tick |
| `DeltaMana` | Custom | Spell cast (-cost) | Mana change this tick |
| `PoisonIntent` | Custom | Poison spell (duration) | Apply new poison |
| `StunIntent` | Custom | Stun spell (duration) | Apply new stun |
| `ShieldIntent` | Custom | Shield spell (duration) | Apply new shield |

All initialize to 0.0. No formulas on intent columns.

### Computed Columns (formula-driven)

| Column | Formula | Purpose |
|--------|---------|---------|
| `PoisonDmg` | `select(prev(Self.PoisonTicks) > 0, -3, 0)` | Poison damage this tick |

### Latched Columns (persist across ticks via latch)

| Column | When latched | When released |
|--------|-------------|---------------|
| `InCombat` | Combat start (set to 1.0) | Combat end (release) |
| `AggroTarget` | Attack on skeleton enemy | Combat end (release) |
| `ShieldAmount` | Shield spell cast | Combat end (release) |

---

## Formulas

### Party table formulas (per entity: warrior, mage, scout, healer)

```
{label}.HP          = clamp(prev(Self.HP) + Self.DeltaHP + Self.PoisonDmg, 0, Self.MaxHP)
{label}.Mana        = clamp(prev(Self.Mana) + Self.DeltaMana, 0, Self.MaxMana)
{label}.PoisonTicks = select(Self.PoisonIntent > 0, Self.PoisonIntent, max(prev(Self.PoisonTicks) - 1, 0))
{label}.StunTicks   = select(Self.StunIntent > 0, Self.StunIntent, max(prev(Self.StunTicks) - 1, 0))
{label}.ShieldTicks = select(Self.ShieldIntent > 0, Self.ShieldIntent, max(prev(Self.ShieldTicks) - 1, 0))
{label}.PoisonDmg   = select(prev(Self.PoisonTicks) > 0, -3, 0)
{label}.Alive        = select(Self.HP > 0, prev(Self.Alive), 0)
```

### Enemies table formulas (generated per entity in enemy_gen.rs)

Same formulas as Party. Applied programmatically after entity creation:

```rust
for row in &table.rows {
    let l = &row.label;
    table.formulas.insert(
        format!("{}.HP", l),
        "clamp(prev(Self.HP) + Self.DeltaHP + Self.PoisonDmg, 0, Self.MaxHP)".into(),
    );
    table.formulas.insert(
        format!("{}.PoisonTicks", l),
        "select(Self.PoisonIntent > 0, Self.PoisonIntent, max(prev(Self.PoisonTicks) - 1, 0))".into(),
    );
    table.formulas.insert(
        format!("{}.StunTicks", l),
        "select(Self.StunIntent > 0, Self.StunIntent, max(prev(Self.StunTicks) - 1, 0))".into(),
    );
    table.formulas.insert(
        format!("{}.ShieldTicks", l),
        "select(Self.ShieldIntent > 0, Self.ShieldIntent, max(prev(Self.ShieldTicks) - 1, 0))".into(),
    );
    table.formulas.insert(
        format!("{}.PoisonDmg", l),
        "select(prev(Self.PoisonTicks) > 0, -3, 0)".into(),
    );
    table.formulas.insert(
        format!("{}.Alive", l),
        "select(Self.HP > 0, prev(Self.Alive), 0)".into(),
    );
}
```

---

## combat.rs Changes

### Eliminated functions (~200 lines removed)

#### `apply_turn_start_effects()` (lines 472-538) — DELETE ENTIRELY

Poison damage, stun decrement, shield decrement are all handled by formulas now.

**Before:**
```rust
// Imperative poison damage
if poison > 0.0 {
    let new_hp = (hp - 3.0).max(0.0);
    world.queue_command(SetCell { table, label, column: "HP", value: new_hp });
    world.queue_command(SetCell { table, label, column: "PoisonTicks", value: poison - 1.0 });
    if new_hp <= 0.0 {
        world.queue_command(SetCell { table, label, column: "Alive", value: 0.0 });
    }
}
```

**After:** Formulas handle it automatically on tick(). No code needed.

#### HP mutation in `resolve_attack()` (lines 195-223) — SIMPLIFY

**Before:**
```rust
let new_hp = (target_hp - damage).max(0.0);
world.queue_command(SetCell { column: "HP", value: new_hp });
if new_hp <= 0.0 {
    world.queue_command(SetCell { column: "Alive", value: 0.0 });
}
```

**After:**
```rust
// Write damage intent — formula handles HP calculation and Alive flag
world.queue_command(EngineCommand::SetCell {
    table: target_tbl_name.into(),
    label: target_label.into(),
    column: "DeltaHP".into(),
    value: -damage,
});
```

Kill detection changes — see "Kill Detection" section below.

#### `resolve_spell()` effect branches (lines 322-414) — SIMPLIFY

**Before (poison):**
```rust
world.queue_command(SetCell { column: "PoisonTicks", value: effect_dur });
```

**After:**
```rust
world.queue_command(EngineCommand::SetCell {
    table: target_tbl.into(), label: t_label.clone(),
    column: "PoisonIntent".into(), value: effect_dur,
});
```

Same pattern for stun → `StunIntent`, shield → `ShieldIntent`.

**Before (heal):**
```rust
let new_hp = (hp + heal_amount).min(max_hp);
world.queue_command(SetCell { column: "HP", value: new_hp });
```

**After:**
```rust
world.queue_command(EngineCommand::SetCell {
    table: target_tbl.into(), label: t_label.clone(),
    column: "DeltaHP".into(), value: heal_amount,
});
```

**Before (mana):**
```rust
world.queue_command(SetCell { column: "Mana", value: caster_mana - mana_cost });
```

**After:**
```rust
world.queue_command(EngineCommand::SetCell {
    table: caster_tbl.into(), label: caster_label.into(),
    column: "DeltaMana".into(), value: -mana_cost,
});
```

### Functions that stay imperative (~430 lines)

| Function | Why it stays | Lines |
|----------|-------------|-------|
| `compute_damage()` | Pure math — computes the DeltaHP value | ~15 |
| `resolve_attack()` | Orchestration — compute damage, write DeltaHP, generate CombatEvent | ~50 (reduced from ~83) |
| `resolve_spell()` | Dispatch by spell type, write intents, generate events | ~100 (reduced from ~160) |
| `select_enemy_action()` | AI decision logic — reads state, picks target | ~50 |
| `start_combat()` / turn management | Stateful — turn order, round tracking | ~100 |
| `check_combat_trigger()` | Proximity check (reads positions) | ~40 |
| `check_combat_end()` | Reads Alive counts | ~15 |
| CombatEvent generation | Display/logging concern | ~60 |

---

## game.rs Changes

### After tick(), clear intent columns

```rust
// In GameState::tick() or process_combat_action(), after world.tick():
let intent_cols = &["DeltaHP", "DeltaMana", "PoisonIntent", "StunIntent", "ShieldIntent"];
self.world.clear_columns("Party", intent_cols);
self.world.clear_columns("Enemies", intent_cols);
```

### On combat start, latch persistent flags

```rust
// In the combat trigger handler:
for member in &party_labels {
    self.world.queue_command(EngineCommand::LatchCell {
        table: "Party".into(), label: member.clone(),
        column: "InCombat".into(), value: 1.0,
    });
}
```

### On combat end, release all latches

```rust
fn end_combat(&mut self) {
    let latch_cols = vec!["InCombat".into(), "AggroTarget".into(), "ShieldAmount".into()];
    for member in &party_labels {
        self.world.queue_command(EngineCommand::ReleaseCells {
            table: "Party".into(), label: member.clone(),
            columns: latch_cols.clone(),
        });
    }
    for enemy in &enemy_labels {
        self.world.queue_command(EngineCommand::ReleaseCells {
            table: "Enemies".into(), label: enemy.clone(),
            columns: vec!["AggroTarget".into()],
        });
    }
}
```

---

## Kill Detection

### The timing change

Currently, `resolve_attack()` checks `new_hp <= 0.0` immediately and sets `killed = true` for the CombatEvent. With the intent pattern, HP changes during `tick()`, not during `resolve_attack()`.

### Solution: predict the kill inline

```rust
// In resolve_attack, after computing damage:
let current_hp = get_stat(world, target_tbl_name, target_label, "HP");
let poison_dmg = if get_stat(world, target_tbl_name, target_label, "PoisonTicks") > 0.0 { 3.0 } else { 0.0 };
let expected_hp = (current_hp - damage - poison_dmg).max(0.0);
let killed = expected_hp <= 0.0;

// Write intent
world.queue_command(EngineCommand::SetCell {
    table: target_tbl_name.into(), label: target_label.into(),
    column: "DeltaHP".into(), value: -damage,
});

// CombatEvent uses predicted `killed` flag
```

This prediction matches what the HP formula will compute: `clamp(prev(HP) + DeltaHP + PoisonDmg, 0, MaxHP)`.

### Alternative: post-tick diff

After `world.tick()`, compare Alive values before/after:

```rust
let was_alive = prev_alive_snapshot.get(target_label);
let is_alive = get_stat(world, target_tbl_name, target_label, "Alive");
let killed = was_alive == 1.0 && is_alive == 0.0;
```

This is cleaner but requires restructuring the event generation to happen after the tick rather than inline with the action.

---

## Data File Changes

### party.quack.json

Add columns:
```json
{ "name": "DeltaHP", "kind": "Custom" },
{ "name": "DeltaMana", "kind": "Custom" },
{ "name": "PoisonIntent", "kind": "Custom" },
{ "name": "StunIntent", "kind": "Custom" },
{ "name": "ShieldIntent", "kind": "Custom" },
{ "name": "PoisonDmg", "kind": "Custom" }
```

Add formulas (for each of warrior, mage, scout, healer):
```json
"formulas": {
    "warrior.HP": "clamp(prev(Self.HP) + Self.DeltaHP + Self.PoisonDmg, 0, Self.MaxHP)",
    "warrior.Mana": "clamp(prev(Self.Mana) + Self.DeltaMana, 0, Self.MaxMana)",
    "warrior.PoisonTicks": "select(Self.PoisonIntent > 0, Self.PoisonIntent, max(prev(Self.PoisonTicks) - 1, 0))",
    "warrior.StunTicks": "select(Self.StunIntent > 0, Self.StunIntent, max(prev(Self.StunTicks) - 1, 0))",
    "warrior.ShieldTicks": "select(Self.ShieldIntent > 0, Self.ShieldIntent, max(prev(Self.ShieldTicks) - 1, 0))",
    "warrior.PoisonDmg": "select(prev(Self.PoisonTicks) > 0, -3, 0)",
    "warrior.Alive": "select(Self.HP > 0, prev(Self.Alive), 0)"
}
```

### enemy_gen.rs

Add the same 6 columns to the enemy table builder. Add formulas programmatically (see Formulas section above).

---

## Testing Plan

### Regression: all 42 existing tests must pass

The intent columns + formulas are additive. Existing combat tests that use direct SetCell on HP/Mana/etc. will auto-latch (since those columns now have formulas), preserving the imperative write behavior.

### New tests

| Test | What it validates |
|------|-------------------|
| `test_intent_damage_through_formula` | Write DeltaHP=-10, tick, assert HP decreased by 10 |
| `test_poison_formula_decrement` | Set PoisonIntent=3, tick 4x, assert PoisonTicks counts down 3→2→1→0 |
| `test_poison_damage_in_hp_formula` | With PoisonTicks>0, tick, assert HP drops by 3 via PoisonDmg formula |
| `test_alive_formula_on_lethal` | Set DeltaHP to lethal amount, tick, assert Alive=0 |
| `test_intent_clear_no_double_apply` | Write DeltaHP=-5, tick, clear intents, tick again, assert HP only dropped once |
| `test_heal_intent_caps_at_max` | DeltaHP=+999 on damaged entity, assert HP capped at MaxHP |
| `test_stun_intent_overrides_countdown` | Mid-countdown PoisonTicks=1, write PoisonIntent=3, assert reset to 3 |
| `test_multiple_intents_same_tick` | DeltaHP from attack + PoisonDmg from formula, assert both apply |
| `test_combat_end_releases_latches` | Latch InCombat, ReleaseCells, tick, assert InCombat reverts to formula/default |

---

## Implementation Order

1. Update `quack-engine` dependency in `duck-crawl/Cargo.toml` to use cell-latching branch/version
2. Add intent columns + PoisonDmg to `party.quack.json` and `enemy_gen.rs`
3. Add formulas to both tables
4. Add `clear_columns` call to `game.rs` after tick
5. Refactor `resolve_attack` to write DeltaHP instead of HP
6. Refactor `resolve_spell` to write intent columns
7. Delete `apply_turn_start_effects` entirely
8. Update kill detection to use prediction or post-tick diff
9. Add latch/release for InCombat, AggroTarget, ShieldAmount
10. Run all tests — existing 42 + new formula-combat tests
11. Browser playtest: verify combat damage, spells, poison, stun, shield all work
