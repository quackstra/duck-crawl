use quack_engine::World;

fn main() {
    println!("DuckCrawl — QuackEngine v0.2 dungeon crawler");
    println!("Engine ready: World supports {} features", "multi-table");

    // Verify engine link works
    let world = World::new();
    println!("World created with {} tables", world.table_names().len());
}
