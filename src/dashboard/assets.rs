use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend"]
pub struct Asset;
