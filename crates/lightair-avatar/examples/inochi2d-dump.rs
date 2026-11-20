fn main() {
    let path = std::env::args().nth(1).expect("path");
    let data = std::fs::read(&path).unwrap();
    let model = lightair_inochi2d::formats::inp::parse_inp(data.as_slice()).unwrap();
    println!("{}", model.puppet.meta);
    let mut ps: Vec<_> = model.puppet.params().values().collect();
    ps.sort_by_key(|p| p.name.clone());
    for p in ps {
        println!(
            "param {:?} vec2={} min={} max={} def={} bindings={}",
            p.name,
            p.is_vec2,
            p.min,
            p.max,
            p.defaults,
            p.bindings.len()
        );
    }
    for v in &model.vendors {
        println!("vendor {}", v.to_string().chars().take(6000).collect::<String>());
    }
    println!("textures {}", model.textures.len());
}
