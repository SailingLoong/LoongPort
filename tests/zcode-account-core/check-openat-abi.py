#!/usr/bin/env python3
"""Compile the product openat expression with 16/32-bit mode_t; never run it."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
source = (root / "src-tauri/src/zcode_file_lock.rs").read_text()
owner = source.split("fn open_owner(", 1)[1]
expression = "let fd = unsafe {" + owner.split("let fd = unsafe {", 1)[1].split("};", 1)[0] + "};"
assert expression.count("libc::openat(") == 1

with tempfile.TemporaryDirectory(prefix="zcode-openat-abi-") as temporary:
    directory = Path(temporary)
    for mode_type in ("u16", "u32"):
        fixture = directory / f"mode_{mode_type}.rs"
        fixture.write_text(
            "use std::os::fd::AsRawFd;\n"
            "#[allow(dead_code, non_camel_case_types)] mod libc {\n"
            f"    pub type mode_t = {mode_type};\n"
            "    pub type c_uint = std::ffi::c_uint;\n"
            "    unsafe extern \"C\" { pub fn openat(fd: i32, path: *const std::ffi::c_char, flags: i32, ...) -> i32; }\n"
            "}\n"
            "pub fn compile_only(directory_file: &std::fs::File, name: &std::ffi::CStr, flags: i32) -> i32 {\n"
            + expression + "\nfd\n}\n"
        )
        subprocess.run(
            ["rustc", "--edition=2021", "--crate-type=lib", "--emit=metadata",
             str(fixture), "-o", str(directory / f"mode_{mode_type}.rmeta")],
            check=True,
        )
        print(f"product openat expression: mode_t={mode_type}: PASS")
