#[include_wgsl_oil::include_wgsl_oil("examples/simple_multifile_shader/vertex_shader.wgsl")]
mod vertex_shader {}
#[include_wgsl_oil::include_wgsl_oil("examples/simple_multifile_shader/fragment_shader.wgsl")]
mod fragment_shader {}

fn main() {
    println!("Vertex source: {}", vertex_shader::SOURCE);
    println!("Fragment source: {}", fragment_shader::SOURCE);
}
