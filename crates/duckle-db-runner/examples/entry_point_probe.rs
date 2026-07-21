use duckle_db_runner::cutover::{configured_entry_point_class, EntryPointClass};

fn main() {
    let value = match configured_entry_point_class() {
        EntryPointClass::Production => "production",
        EntryPointClass::ReleaseCi => "release-ci",
        EntryPointClass::Test => "test",
        EntryPointClass::Compatibility => "compatibility",
    };
    println!("{value}");
}
