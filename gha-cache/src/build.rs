fn main() {
  let proto_source_files = [
    "./proto/results/api/v1/cache.proto",
    "./proto/results/entities/v1/cachemetadata.proto",
    "./proto/results/entities/v1/cachescope.proto",
  ];

  for entry in &proto_source_files {
      println!("cargo:rerun-if-changed={}", entry);
  }

  prost_build::Config::new()
      .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
      .service_generator(twirp_build::service_generator())
      .compile_protos(&proto_source_files, &["./proto"])
      .expect("error compiling protos");
}
