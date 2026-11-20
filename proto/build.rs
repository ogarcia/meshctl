// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Óscar García Amor <ogarcia@connectical.com>

use std::path::Path;

/// Where the vendored Meshtastic protobufs live, relative to this crate.
const PROTO_ROOT: &str = "protobufs";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(PROTO_ROOT);

    // Every file in the directory rather than a list kept by hand: updating the
    // protobufs is then copying the new ones in, with nothing here to remember.
    let mut files: Vec<_> = std::fs::read_dir(root.join("meshtastic"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "proto"))
        .collect();
    files.sort();

    let mut compiler = protox::Compiler::new([root])?;
    compiler.include_imports(true).open_files(&files)?;

    // The descriptors themselves, for reading settings by their description:
    // labels, units, limits and which fields are secret. See `descriptors()`.
    // Encoded by protox rather than through prost's types, which would drop
    // the field options' extensions: the very metadata wanted here.
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(
        out.join("descriptors.bin"),
        compiler.encode_file_descriptor_set(),
    )?;

    // Enums stay as i32 on the wire types, which is what lets a value a newer
    // firmware sends, and these files do not know yet, arrive intact.
    prost_build::Config::new().compile_fds(compiler.file_descriptor_set())?;

    println!("cargo:rerun-if-changed={PROTO_ROOT}");
    Ok(())
}
