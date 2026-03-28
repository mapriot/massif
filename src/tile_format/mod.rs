pub mod png;
pub mod webp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Webp,
    Png,
}
