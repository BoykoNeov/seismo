//! Every WGSL shader parses and validates with naga (wgpu's shader compiler), with no GPU, so a
//! shader error fails `cargo test` (and CI) instead of only showing when the app starts.

use wgpu::naga;

const SHADERS: [(&str, &str); 4] = [
    ("crt", include_str!("../../../assets/shaders/crt.wgsl")),
    ("lines", include_str!("../../../assets/shaders/lines.wgsl")),
    ("map", include_str!("../../../assets/shaders/map.wgsl")),
    (
        "slice_disc",
        include_str!("../../../assets/shaders/slice_disc.wgsl"),
    ),
];

#[test]
fn every_shader_parses_and_validates() {
    for (name, source) in SHADERS {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}.wgsl: {}", e.emit_to_string(source)));
    }
}

#[test]
fn every_shader_file_is_checked() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders");
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .expect("shaders folder")
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            Some(name.strip_suffix(".wgsl")?.to_owned())
        })
        .collect();
    on_disk.sort();
    let checked: Vec<&str> = SHADERS.iter().map(|(n, _)| *n).collect();
    assert_eq!(on_disk, checked, "add new shaders to SHADERS");
}
