#[include_wgsl_oil::include_wgsl_oil("examples/item_imports/main.wgsl")]
mod main_shader {}

fn main() {
    println!("Main source: {}", main_shader::SOURCE);
}
