use std::io;

use bevy_api_gen_lib::{generate_api_for_crates, Args};
use clap::Parser;

pub fn main() -> Result<(), io::Error> {
    let args = Args::parse();
    generate_api_for_crates(&args)
}
