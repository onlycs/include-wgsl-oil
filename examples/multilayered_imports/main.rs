#[include_wgsl_oil::include_wgsl_oil("examples/multilayered_imports/shaders/vertex_shader.wgsl")]
mod vertex_shader {}
#[include_wgsl_oil::include_wgsl_oil("examples/multilayered_imports/shaders/fragment_shader.wgsl")]
mod fragment_shader {}

fn main() {
    println!("Vertex source: {}", vertex_shader::SOURCE);
    println!("Fragment source: {}", fragment_shader::SOURCE);
}
