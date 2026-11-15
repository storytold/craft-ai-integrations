//! Developer tool, not shipped: asks the pinned burn-onnx to write the Rust architecture (and the
//! weights it would convert) for one ONNX file. `tools/burn/generate_sam21.py` runs it, patches and
//! splits the result; nobody runs this by hand or on a user's computer.

use std::process::ExitCode;

/// Writes `<name>.rs` and `<name>.bpk` for the ONNX file in the first argument into the folder in
/// the second.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, out_dir] = args.as_slice() else {
        eprintln!("usage: generate_architecture <model.onnx> <out-folder>");
        return ExitCode::from(2);
    };
    burn_onnx::ModelGen::new()
        .input(input)
        .out_dir(out_dir)
        .run_from_cli();
    ExitCode::SUCCESS
}
